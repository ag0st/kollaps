use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomPinned;
use std::net::IpAddr;
use std::ops::Add;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::sleep_until;
use uuid::Uuid;

use common::{ClusterNodeInfo, EmulationEvent, EmulBeginTime, EmulMessage, Error, ErrorKind, EventAction, FlowConf, Result, TCConf};
use dockhelper::DockerHelper;
use netgraph::Network;
use nethelper::{Handler, NoHandler, ProtoBinding, Protocol, TCP, TCPBinding, Unix, UnixBinding};

use crate::bwsync::{remove_flow, update_flows};
use crate::data::{Application, ApplicationKind, AppStatus, ContainerConfig, ControllerMessage, Emulation, Flow, Node};
use crate::scheduler::schedule;

#[derive(Clone)]
struct FlowHandler {
    tc_messages_sender: Sender<EmulMessage>,
}

#[async_trait]
impl Handler<EmulMessage> for FlowHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<EmulMessage> {
        let mess = EmulMessage::from_bytes(bytes).unwrap();
        self.tc_messages_sender.send(mess).await.unwrap();
        None
    }
}

/// EmulCore is an emulation core. It is spawn when the node receive an emulation
/// to execute from the leader. Its responsibilities are to assure that the all attributed
/// application are running an monitored. At changes it communicate with the others EmulCore
/// dispatched across the cluster. It is also responsible to follow the dynamic events associated
/// with their associated nodes.
pub struct EmulCore {
    // By copying the applications in multiple places, we use a bit more memory but we achieve
    // way faster searches across the structures
    // There is no problem of cloning an app regarding mutability. An app itself do not allow
    // to be mutable. It doesn't publish its internals and no permissions is given via calls.
    is_leader: bool,
    leader: ClusterNodeInfo,
    graph: Network<Node>,
    events: Option<Vec<EmulationEvent>>,
    my_apps: HashMap<Node, (UnixBinding<EmulMessage, NoHandler>, AppStatus)>,
    other_hosts: HashSet<ClusterNodeInfo>,
    ip_to_app: HashMap<IpAddr, Node>,
    myself: ClusterNodeInfo,
    flow_socket: Option<UnixBinding<EmulMessage, FlowHandler>>,
    flow_receiver: Option<Receiver<EmulMessage>>,
    flow_sender: Option<Sender<EmulMessage>>,
    emul_id: Uuid,
    reporters: Vec<Child>,
    emul_id_to_channel: Arc<Mutex<HashMap<Uuid, Sender<EmulMessage>>>>,
    _marker: PhantomPinned,
}

impl EmulCore {
    pub fn new(emul: Emulation, myself: ClusterNodeInfo, emul_id_to_channel: Arc<Mutex<HashMap<Uuid, Sender<EmulMessage>>>>) -> EmulCore {
        // First thing is to parse the emulation to gather all the information I need:
        let (ip_to_app, other_hosts) = parse_emulation(&emul, &myself);
        let is_leader = emul.am_i_leader(&myself);
        EmulCore {
            is_leader,
            leader: emul.leader(),
            emul_id: emul.uuid(),
            graph: emul.graph,
            events: Some(emul.events),
            my_apps: HashMap::new(),
            other_hosts,
            ip_to_app,
            myself,
            flow_socket: None,
            flow_receiver: None,
            flow_sender: None,
            reporters: Vec::new(),
            emul_id_to_channel,
            _marker: PhantomPinned,
        }
    }

    /// Start the emulation, launch all necessary application and reporters and start to listen
    /// For incoming messages.
    /// When the emulation has everything set, it will enters itself into the queue
    pub async fn start_emulation(self: Pin<&mut Self>, dock: DockerHelper, reporter_path: String) -> Result<()> {
        let this = unsafe { self.get_unchecked_mut() };

        // First, creating the socket, its handler and the incoming flow message channel.
        let (tc_messages_sender, tc_messages_receiver) = mpsc::channel(1000);
        let flow_handler = FlowHandler { tc_messages_sender: tc_messages_sender.clone() };
        let flow_socket_name = generate_flow_socket(&this.emul_id)?;
        let mut flow_socket_binding: UnixBinding<EmulMessage, FlowHandler> = Unix::bind_addr(flow_socket_name.clone(), Some(flow_handler.clone())).await?;
        flow_socket_binding.listen()?;
        this.flow_socket = Some(flow_socket_binding);

        // Now we launch all the reporters for the applications it needs to emulate

        // We need to create a RefCell reference to the receiver as we are going to pass to
        // an async function named "waiting_on_response" that will wait on the reporter response telling us
        // that he is ready. We do not create mutex as it is not necessary, we are not
        // using the reference here, only in the async function, but rust doesn't want to let us.
        let receiver_refcell = Arc::new(Mutex::new(Some(tc_messages_receiver)));

        // this temporary hashmap is used to store the socket path of each app returned by the method
        // launch_reporter_for_app for then, after looping, being able to connect to each of these
        // sockets.
        let mut reporters_sockets = HashMap::new();

        // Todo: This implementation requires that each app is running in its own namespace for now and have his own ip addr.
        // If you want to modify this, you need to find a way for a reporter to detect which app is
        // receiving / sending. This may be quite difficult. A reporter receives a "src"
        // when it is started corresponding of the app ip addr. Then it is used by the EmulCore to know
        // which app launch which flow.
        // A way may be instead of qualifying an app by it's ip, is to add a port and let the monitor application (eBPF) check only
        // certain port, but it then limit to the use of TCP and UDP.
        // Then you can redefine the Application structure to have instead of ip_addr the (ip_addr, port) combo and give this
        // combo to the reporter.

        for node in this.graph.vertices().iter()
            .filter(|node| node.is_app() && node.as_app().runs_on(&this.myself)) {
            let (proc, tc_socket_name) = launch_reporter_for_app(this.emul_id, node.as_app(), &dock, flow_socket_name.clone(), node.id(), reporter_path.clone()).await?;
            // store the process, when I drop, all the child process will drop and be killed.
            this.reporters.push(proc);
            // Keep the socket path
            reporters_sockets.insert(node.clone(), tc_socket_name);

            // waiting response logic: ignore every message and take care only of the SocketReady.
            // ignoring message is not a big deal as we will begin to consider flows after everything
            // is initialized
            let waiting_on_response = async {
                loop {
                    if let EmulMessage::SocketReady = receiver_refcell.lock().await
                        .as_mut().unwrap().recv().await.unwrap() {
                        break;
                    }
                }
            };

            // Wait on the reporter to be ready with a timeout
            if let Err(_) = tokio::time::timeout(Duration::from_secs(5), waiting_on_response).await {
                return Err(Error::new("emulcore start", ErrorKind::CommandFailed, "The reporter didn't send ready in the defined timeout"));
            }
        }

        // Now it is time to connect to each of the TC sockets
        for (node, socket) in reporters_sockets.into_iter() {
            // As the reporter is started, we will connect to its socket
            let mut binding: UnixBinding<EmulMessage, NoHandler> = Unix::bind_addr(socket, None).await?;
            binding.connect().await?;
            // Send the TC Init
            let conf = TCConf::default(node.as_app().ip_addr());
            binding.send(EmulMessage::TCInit(conf)).await?;
            // insert everything into my_apps
            this.my_apps.insert(node, (binding, AppStatus::NotInit));
        }

        // Everything is ready, we can now begin the main loop!
        // just before, add ourself into the emul_id_to_channel, means that we are ready to rock!
        // Add itself to the emulation list
        this.emul_id_to_channel.lock().await.insert(this.emul_id.clone(), tc_messages_sender.clone());

        // take back the ownership of the receiver
        let tc_messages_receiver = receiver_refcell.lock().await.take().unwrap();
        this.flow_receiver = Some(tc_messages_receiver);
        this.flow_sender = Some(tc_messages_sender);
        Ok(())
    }


    /// Flow loop is the main loop of the emulation, It waits for event coming to its receiver
    /// and act in function. These events can come from different locations, like the Controller
    /// that receive an update for our emulation, the FlowHandler which receive updates from the
    /// reporters and the Scheduler that will send events when needed.
    pub async fn flow_loop(self: Pin<&mut Self>, dock: DockerHelper) -> Result<()> {
        let this = unsafe { self.get_unchecked_mut() };

        // Now, all reporters are correctly connected and we are bind to all of these reporter
        // We can launch the event scheduler
        schedule(this.flow_sender.as_ref().unwrap().clone(), this.events.take().unwrap());

        let mut receiver = this.flow_receiver.take().unwrap();
        // Used to store the active flows.
        // Node_id (destination) : Bandwidth
        let mut active_flows: HashSet<Flow<Node>> = HashSet::new();

        // This is the bandwidth graph. It represent the remaining bandwidth on the network.
        let mut bw_graph = this.graph.clone();

        // Start listening on events
        while let Some(event) = receiver.recv().await {
            match event {
                EmulMessage::EmulAbort => {
                    // We received an abort, we need to destroy the emulation. For this, we will kill all the apps if they are managed
                    // and kill the reporters.
                    for (node, (tc_socket, status)) in this.my_apps.iter_mut() {
                        match status {
                            AppStatus::NotInit | AppStatus::Running | AppStatus::Stopped => {
                                tc_socket.send(EmulMessage::TCTeardown).await?;
                                // kill the container app if exists, and if we control the life cycle
                                let n = node.clone();
                                let d = dock.clone();
                                stop_app_if_necessary(n, d).await.unwrap()
                            }
                            AppStatus::Crashed => {} // Already crashed
                        }
                        *status = AppStatus::Crashed;
                    }
                    // Finish, the check at the end of the loop will exit if everything is in crashed status.
                }
                EmulMessage::FlowUpdate(flow_conf) => {
                    // Todo: to support multiple app with same ip, change here.
                    // Get the concerned application based on their ip addr
                    // For now, only consider flow inside the emulation
                    if let (Some(src), Some(dest)) = (this.ip_to_app.get(&flow_conf.src), this.ip_to_app.get(&flow_conf.dest)) {
                        // Get the defined properties (by the topology) of the path between the source of the flow and the destination.
                        let (max_bandwidth, drop, latency_jitter) = this.graph.properties_between(src, dest)
                            .expect("New flow between applications that do not have a connection!?");

                        // We can calculate the target bandwidth to be the min between allowed and the asked bandwidth
                        // If the flowConf.throughput == None, it means that the flow ended and so, the target bandwidth = 0
                        let target_bandwidth = match flow_conf.throughput {
                            None => 0,
                            Some(bw) => {
                                min(bw, max_bandwidth)
                            }
                        };

                        // Build the internal representation of the flow based on what we received.
                        // The comparison between flows and hash are based on Src and Dest, this is what matters.
                        let new_flow = Flow::build(src, dest, 0, target_bandwidth);

                        // To keep a trace if the flow has been updated or removed. This is important
                        // at the end to know what Traffic Control config we have to send to the reporter
                        // if the source of the concerned flow runs on our machine
                        let mut updated_flow = true;

                        if new_flow.target_bandwidth == 0 { // we are in deactivation
                            if active_flows.contains(&new_flow) { // no need to remove if not exists
                                (active_flows, bw_graph) = remove_flow(active_flows, &new_flow, &this.graph);
                            }
                            updated_flow = false;
                        } else { // we need to add / update a new flow
                            (active_flows, bw_graph) = update_flows(active_flows, &new_flow, &this.graph, bw_graph);
                        }
                        // If I am the source of the flow, push it to the others
                        if src.as_app().runs_on(&this.myself) {
                            // We will have to update the tc regarding if it was updated or removed
                            let tc_conf = if updated_flow {
                                // If updated, adjust the bandwidth
                                let flow = active_flows.get(&new_flow).unwrap();
                                let mut conf = TCConf::default(dest.as_app().ip_addr());
                                conf.bandwidth_kbs(flow.bandwidth);
                                conf
                            } else {
                                // If removed, put back the original value
                                TCConf {
                                    dest: dest.as_app().ip_addr(),
                                    bandwidth_kbitps: Some(max_bandwidth),
                                    latency_and_jitter: Some(latency_jitter),
                                    drop: Some(drop),
                                }
                            };
                            // Update the TC Configuration of the App
                            this.my_apps.get_mut(src).unwrap()
                                .0.send(EmulMessage::TCUpdate(tc_conf)).await.unwrap();


                            // Finally as it runs on our machine, update the others
                            broadcast_flow(&flow_conf, &this.other_hosts, this.emul_id.clone()).await;
                        }
                    }
                }

                EmulMessage::Event(event) => {
                    // check if we care about this event, it may not be designated for us

                    if let Some((node, (tc_socket, status))) =
                        this.my_apps.iter_mut().filter(|(node, _)| node.is_same_by_id(event.app_id)).last() {
                        match event.action {
                            EventAction::Join => {
                                // If the status is "NotInit" it means that we have to initialize
                                // All the path from this, else it means that we must reconnect it if it was stopped
                                match status {
                                    AppStatus::NotInit => {
                                        // Join can mean rejoin or first join.
                                        for other_node in this.graph.vertices().iter().filter(|n| n.is_app()) {
                                            if other_node.eq(&node) { continue; }
                                            // get the property and initialize the TC
                                            let properties = this.graph.properties_between(&node, &other_node);
                                            if let Some((bw, drop, lat_jitter)) = properties {
                                                let tc_conf = TCConf {
                                                    dest: other_node.as_app().ip_addr(),
                                                    bandwidth_kbitps: Some(bw),
                                                    latency_and_jitter: Some(lat_jitter),
                                                    drop: Some(drop),
                                                };
                                                tc_socket.send(EmulMessage::TCUpdate(tc_conf)).await?;
                                            }
                                        }
                                    }
                                    AppStatus::Stopped => {
                                        tc_socket.send(EmulMessage::TCReconnect).await?;
                                    }
                                    _ => {} // ignoring if already running or crashed
                                }
                                // update the status in this case
                                if let AppStatus::NotInit | AppStatus::Stopped = status {
                                    *status = AppStatus::Running;
                                }
                            }
                            EventAction::Quit => {
                                // remove all flows associated with him
                                (active_flows, bw_graph) = Self::remove_all_flows_from(active_flows, bw_graph, &node, &this.graph, &this.other_hosts, this.emul_id.clone()).await;
                                // Send the disconnect
                                tc_socket.send(EmulMessage::TCDisconnect).await?;
                                *status = AppStatus::Stopped;
                            }
                            EventAction::Crash => {
                                (active_flows, bw_graph) = Self::remove_all_flows_from(active_flows, bw_graph, &node, &this.graph, &this.other_hosts, this.emul_id.clone()).await;
                                tc_socket.send(EmulMessage::TCDisconnect).await?;
                                // kill the container app if exists, and if we control the life cycle
                                stop_app_if_necessary(node.clone(), dock.clone()).await.unwrap();
                                *status = AppStatus::Crashed
                            }
                        }
                    }
                }
                _ => {}
            }
            // Check if it still is some application that are not crashed. If everyone is crashed,
            // We can stop the emulation
            let mut stop = true;
            for (_, (_, status)) in &this.my_apps {
                if let AppStatus::NotInit | AppStatus::Stopped | AppStatus::Running = status {
                    stop = false;
                    break;
                }
            }
            if stop {
                // We can safely exit, everything is crashed, so everything must be deleted and cleaned
                break;
            }
        }
        Ok(())
    }

    pub async fn synchronize(self: Pin<&mut Self>, start_in_future: Duration) -> Result<()> {
        let this = unsafe { self.get_unchecked_mut() };
        // If I am the leader, I have to synchronize with other before continuing.
        // For this, I will wait to receive a message from all the others,
        // Then I will send them back the response and everybody continue. This is simple synchro
        // That works only in "good" environment. Todo: Change with something like NTP or Berkley Algorithm

        if this.is_leader {
            // wait on other responses
            let mut number_of_events_remaining = this.other_hosts.len();
            if number_of_events_remaining > 0 {
                if let Some(event) = this.flow_receiver.as_mut().unwrap().recv().await {
                    match event {
                        EmulMessage::EmulStart(_) => number_of_events_remaining -= 1,
                        _ =>{
                            return Err(Error::new("emulation synchronization", ErrorKind::CommandFailed, "received another event that EmulStart, there is an issue"));
                        }
                    }
                    if number_of_events_remaining == 0 {
                        // Everybody is ready to rock, send start
                        let begin_time = broadcast_ready(&this.other_hosts, this.emul_id.clone(), start_in_future).await;
                        sleep_until(tokio::time::Instant::from(begin_time)).await;
                    }
                }
            }
        } else {
            // Send to the leader that we are ready and wait for its response
            let mut bind:TCPBinding<ControllerMessage, NoHandler> = TCP::bind(None).await?;
            let wrapper = ControllerMessage::EmulCoreInterchange(this.emul_id.to_string(), EmulMessage::EmulStart(EmulBeginTime{time: Instant::now()}));
            bind.send_to(wrapper, this.leader.clone()).await?;
            // Now wait on time synchro
            if let Some(event) = this.flow_receiver.as_mut().unwrap().recv().await {
                match event {
                    EmulMessage::EmulStart(begin_time) => sleep_until(tokio::time::Instant::from(begin_time.time)).await,
                    _ =>{
                        return Err(Error::new("emulation synchronization", ErrorKind::CommandFailed, "received another event that EmulStart, there is an issue"));
                    }
                }
            }
        }
        Ok(())
    }

    async fn remove_all_flows_from<'a>(mut active_flows: HashSet<Flow<'a, Node>>, mut bw_graph: Network<Node>, node: &Node, graph: &Network<Node>, other_hosts: &HashSet<ClusterNodeInfo>, emul_id: Uuid) -> (HashSet<Flow<'a, Node>>, Network<Node>) {
        let to_remove = active_flows.iter()
            .filter(|f| f.source == node)
            .map(|flow| (FlowConf {
                src: flow.source.as_app().ip_addr(),
                dest: flow.destination.as_app().ip_addr(),
                throughput: None,
            }, flow.clone()))
            .collect::<Vec<(FlowConf, Flow<Node>)>>();

        for (fc, f) in to_remove {
            // delete the flow from the active flows
            if active_flows.contains(&f) { // no need to remove if not exists
                (active_flows, bw_graph) = remove_flow(active_flows, &f, graph);
            }
            // Tell the others that this flow does not exists anymore
            broadcast_flow(&fc, other_hosts, emul_id).await;
        }

        (active_flows, bw_graph)
    }
}




async fn broadcast_ready(destination: &HashSet<ClusterNodeInfo>, emul_id: Uuid, future: Duration) -> Instant {
    let begin_time = Instant::now().add(future);
    let emul_begin_time = EmulBeginTime { time: begin_time };
    for host in destination {
        // Here when sending the information to the others, wrap our message with our uuid,
        // like this, when the controller of those host will receive the message, they will be
        // able to redirect the message to the good emulation core.
        let mut bind: TCPBinding<ControllerMessage, NoHandler> = TCP::bind(None).await.unwrap();
        let wrapper = ControllerMessage::EmulCoreInterchange(emul_id.to_string(), EmulMessage::EmulStart(emul_begin_time.clone()));
        bind.send_to(wrapper, host.clone()).await.unwrap()
    }
    begin_time
}

async fn broadcast_flow(flow: &FlowConf, destination: &HashSet<ClusterNodeInfo>, emul_id: Uuid) {
    for host in destination {
        // Here when sending the information to the others, wrap our message with our uuid,
        // like this, when the controller of those host will receive the message, they will be
        // able to redirect the message to the good emulation core.
        let mut bind: TCPBinding<ControllerMessage, NoHandler> = TCP::bind(None).await.unwrap();
        let wrapper = ControllerMessage::EmulCoreInterchange(emul_id.to_string(), EmulMessage::FlowUpdate(flow.clone()));
        bind.send_to(wrapper, host.clone()).await.unwrap()
    }
}

/// Starts a reporter process for the application regarding the application kind.
/// If the application is a Container and the (image, command) combo is given, it will consider that
/// we must launch it.
/// !! Warning !! the bellow functionality is not yet supported!
/// If not, in case of BareMetal or Container without (image, command) combo, it tries to connect to them.
///
/// The flow_socket_name is the socket path to the flow socket used by the EmulCore to get the
/// flows update from the different reporters.
/// ----------------------------
/// |IT MUST BE ALREADY CREATED | or the reporter will crash trying to connect to it.
/// ----------------------------
///
/// Returned is a child handler to the Reporter process. The reporter will, when it is ready,
/// send a TCMessage::SocketReady message via the flow socket.
/// You can wait to receive one before continuing to be sure it runs the well.
/// Also, the process is started with kill_on_drop, so if you drop the child process, it will kill it.
async fn launch_reporter_for_app(emul_id: Uuid, app: &Application, dock: &DockerHelper, flow_socket_name: String, id: u32, reporter_path: String) -> Result<(Child, String)> {
    let pid = match app.kind() {
        ApplicationKind::BareMetal => panic!("BareMetal not supported yet!"),
        ApplicationKind::Container(config) => {
            match config.image() {
                None => panic!("Connecting to an existing container is not supported yet!"),
                Some(_) => {
                    start_container_and_get_pid(config.clone(), dock, app.ip_addr()).await.unwrap()
                }
            }
        }
    };

    // Now we are sure the application is running and we have its PID, we can now launch the reporter
    // and attach it to this pid.
    println!("[EMULCORE {}]: Starting reporter for app: {} : {}", emul_id, id, app.name());
    // Start the reporter with a generated tc_socket
    let tc_socket_name = generate_tc_socket(&emul_id, id)?;

    let child = Command::new("sudo").arg("-S") // must start with super user to enter another pid namespace
        .arg("nsenter")
        .arg("-t").arg(format!("{pid}"))
        .arg("-n")
        .arg(reporter_path)
        .arg("--flow-socket").arg(flow_socket_name)
        .arg("--tc-socket").arg(tc_socket_name.clone())
        .arg("--ip").arg(format!("{}", app.ip_addr()))
        .kill_on_drop(true)
        .spawn().expect(&*format!("Cannot launch reporter for app {}", id));

    Ok((child, tc_socket_name))
}

async fn start_container_and_get_pid(container: ContainerConfig, dock: &DockerHelper, app_ip: IpAddr) -> Result<u32> {
    // Get container info
    let name = &*container.name();
    let image = &*container.image().unwrap();
    let command = container.command();
    let command = command.as_deref();

    // Start the container
    dock.launch_container(image, name, app_ip, command).await?;
    // get the pid of the container
    let pid = dock.get_pid(name).await?;
    Ok(pid)
}

async fn stop_app_if_necessary(node: Node, dock: DockerHelper) -> Result<()> {
    // kill the container app if exists, and if we control the life cycle
    match node.as_app().kind() {
        ApplicationKind::BareMetal => {} // do nothing
        ApplicationKind::Container(config) => {
            match config.image() {
                None => {} // do nothing
                Some(_) => {
                    // We kill the container, because we started it
                    let name = &*config.name();
                    dock.stop_container(name).await.unwrap();
                }
            }
        }
    }
    Ok(())
}


/// Parse emulation allow to take in entry an emulation and then to extract essential information
/// from it. It is not necessary as every information is already in the emulation but it creating
/// separate structure for it allows for simpler interaction and less algorithms runs.
fn parse_emulation(emul: &Emulation, myself: &ClusterNodeInfo) -> (HashMap<IpAddr, Node>, HashSet<ClusterNodeInfo>) {
    let other_host = emul.graph.vertices().iter()
        .filter(|node| node.is_app()).map(|n| n.as_app())
        .filter(|app| !app.runs_on(myself))
        .fold(HashSet::new(), |mut acc, app| {
            acc.insert(app.host());
            acc
        });
    let ip_to_map = emul.graph.vertices().iter()
        .filter(|node| node.is_app())
        .fold(HashMap::new(), |mut acc, node| {
            // no need to check two time the same app, already done for my_apps
            acc.insert(node.as_app().ip_addr(), node.clone());
            acc
        });

    (ip_to_map, other_host)
}

/// Returns the Flow Socket name of this emulation. Can be used only one time per emulation.
pub fn generate_flow_socket(emul_id: &uuid::Uuid) -> Result<String> {
    let name = format!("/tmp/kollaps_{}_flows.sock", emul_id);
    verify_socket_exists(name)
}

fn generate_tc_socket(emul_id: &Uuid, app_id: u32) -> Result<String> {
    let name = format!("/tmp/kollaps_{}_{}_tc.sock", emul_id, app_id);
    verify_socket_exists(name)
}

fn verify_socket_exists(name: String) -> Result<String> {
    if !std::path::Path::new(&*name).exists() {
        Ok(name)
    } else {
        Err(Error::new("emulcore", ErrorKind::AlreadyExists, &*format!("The socket {} already exists", name)))
    }
}