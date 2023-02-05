use std::cmp::{min};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::marker::PhantomPinned;
use std::net::IpAddr;
use std::ops::{Add, Mul};
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

use common::{ClusterNodeInfo, EmulationEvent, EmulBeginTime, EmulMessage, Error, ErrorKind, EventAction, FlowConf, Result, TCConf, ToBytesSerialize, OManagerMessage, AsV4, ToU32IpAddr};
use dockhelper::DockerHelper;
use netgraph::Network;
use nethelper::{ALL_ADDR, DefaultHandler, Handler, MessageWrapper, NoHandler, ProtoBinding, Protocol, TCP, TCPBinding, Unix, UnixBinding};

use crate::bwsync::{remove_flow, update_flows};
use crate::data::{Application, ApplicationKind, AppStatus, ContainerConfig, EManagerMessage, Emulation, Flow, Node};
use crate::scheduler::schedule;

const SYNC_PORT: u16 = 9090;

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
    sync_binding: Option<(TCPBinding<EmulMessage, DefaultHandler<EmulMessage>>, Receiver<MessageWrapper<EmulMessage>>)>,
    cluster_leader: ClusterNodeInfo,
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
    dock: DockerHelper,
    _marker: PhantomPinned,
}

impl Drop for EmulCore {
    fn drop(&mut self) {
        // We were dropped, we need to destroy the emulation. For this, we will kill all the apps if they are managed
        // and kill the reporters.
        // We need to go in async mode for this.
        let mut my_apps = self.my_apps.drain().collect::<HashMap<Node, (UnixBinding<EmulMessage, NoHandler>, AppStatus)>>();
        let dock = self.dock.clone();
        let emul_id_to_channel = self.emul_id_to_channel.clone();
        let emul_id = self.emul_id.clone();
        tokio::spawn(async move {
            // Remove this emulation from the list of emulations
            emul_id_to_channel.lock().await.remove_entry(&emul_id);
            // Clean all managed apps and reporters
            for (node, (tc_socket, status)) in my_apps.iter_mut() {
                match status {
                    AppStatus::NotInit | AppStatus::Running | AppStatus::Stopped => {
                        let _ = tc_socket.send(EmulMessage::TCTeardown).await;
                        // kill the container app if exists, and if we control the life cycle
                        let n = node.clone();
                        let d = dock.clone();
                        let _ = stop_app_if_necessary(n, d).await;
                    }
                    AppStatus::Crashed => {} // Already crashed
                }
            }
        });
    }
}


impl EmulCore {
    pub fn new(emul: Emulation, myself: ClusterNodeInfo, emul_id_to_channel: Arc<Mutex<HashMap<Uuid, Sender<EmulMessage>>>>, cluster_leader: ClusterNodeInfo, dock: DockerHelper) -> EmulCore {
        // First thing is to parse the emulation to gather all the information I need:
        let (ip_to_app, other_hosts) = parse_emulation(&emul, &myself);
        EmulCore {
            sync_binding: None,
            cluster_leader,
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
            dock,
            _marker: PhantomPinned,
        }
    }

    /// Start the emulation, launch all necessary application and reporters and start to listen
    /// For incoming messages.
    /// When the emulation has everything set, it will enters itself into the queue
    pub async fn init_emulation(self: Pin<&mut Self>, dock: DockerHelper, reporter_path: String) -> Result<()> {
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

        // Create the binding for the synchronization
        let (tx, rx) = mpsc::channel(10);
        let handler: DefaultHandler<EmulMessage> = DefaultHandler::new(tx);
        let mut binding = TCP::bind_addr((ALL_ADDR, SYNC_PORT), Some(handler)).await?;
        binding.listen()?;
        this.sync_binding = Some((binding, rx));

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
                EmulMessage::FlowUpdate(flow_conf) => {
                    // Todo: to support multiple app with same ip, change here.
                    // Get the concerned application based on their ip addr
                    // For now, only consider flow inside the emulation
                    if let (Some(src), Some(dest)) = (this.ip_to_app.get(&flow_conf.src), this.ip_to_app.get(&flow_conf.dest)) {
                        println!("Flow updated between {} and {} : {:?}", src, dest, flow_conf.throughput);
                        // Get the defined properties (by the topology) of the path between the source of the flow and the destination.
                        let (max_bandwidth, drop, latency_jitter) = this.graph.properties_between(src, dest)
                            .expect(&*format!("New flow between applications that do not have a connection!?: {} -> {}", src, dest));

                        // We can calculate the target bandwidth to be the min between allowed and the asked bandwidth
                        // If the flowConf.throughput == None, it means that the flow ended and so, the target bandwidth = 0
                        let used_bandwidth = match flow_conf.throughput {
                            None => 0,
                            Some(bw) => {
                                min(bw, max_bandwidth)
                            }
                        };

                        // Build the internal representation of the flow based on what we received.
                        // The comparison between flows and hash are based on Src and Dest, this is what matters.
                        let mut new_flow = Flow::build(src, dest, max_bandwidth, used_bandwidth);

                        // To keep a trace if the flow has been updated or removed. This is important
                        // at the end to know what Traffic Control config we have to send to the reporter
                        // if the source of the concerned flow runs on our machine
                        let mut flow_deletion = false;

                        if new_flow.used_bandwidth == 0 { // we are in deactivation
                            if active_flows.contains(&new_flow) { // no need to remove if not exists
                                (active_flows, bw_graph) = remove_flow(active_flows, &mut new_flow, &this.graph);
                            }
                            flow_deletion = true;
                        } else { // we need to add / update a new flow
                            (active_flows, bw_graph) = update_flows(active_flows, &mut new_flow, &this.graph, bw_graph);
                        }
                        // After executing modifications on the active flows, all the TC configurations
                        // of the impacted apps must be redefined. First, we send the flow to the other
                        // to begin directly the same process.
                        if src.as_app().runs_on(&this.myself) {
                            // If the flow was deleted, we need to set back the original configuration for app.
                            if flow_deletion {
                                let tc_conf = TCConf {
                                    dest: dest.as_app().ip_addr(),
                                    bandwidth_kbitps: Some(max_bandwidth),
                                    latency_and_jitter: Some(latency_jitter),
                                    drop: Some(drop),
                                };
                                this.my_apps.get_mut(src).unwrap()
                                    .0.send(EmulMessage::TCUpdate(tc_conf)).await?;
                            }

                            broadcast_flow(&flow_conf, &this.other_hosts, this.emul_id.clone()).await?;
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
                                (active_flows, bw_graph) = Self::remove_all_flows_from(active_flows, bw_graph, &node, &this.graph, &this.other_hosts, this.emul_id.clone()).await?;
                                // Send the disconnect
                                tc_socket.send(EmulMessage::TCDisconnect).await?;
                                *status = AppStatus::Stopped;
                            }
                            EventAction::Crash => {
                                (active_flows, bw_graph) = Self::remove_all_flows_from(active_flows, bw_graph, &node, &this.graph, &this.other_hosts, this.emul_id.clone()).await?;
                                tc_socket.send(EmulMessage::TCDisconnect).await?;
                                // kill the container app if exists, and if we control the life cycle
                                stop_app_if_necessary(node.clone(), dock.clone()).await?;
                                *status = AppStatus::Crashed;
                                // Send to the orchestrator that we finished our emulation for this app
                                let mut bind: TCPBinding<OManagerMessage, NoHandler> = TCP::bind(None).await?;
                                bind.send_to(OManagerMessage::CleanStop((this.emul_id.to_string(), this.myself.clone(), node.as_app().index())), this.cluster_leader.clone()).await?;
                            }
                        }
                    }
                }
                _ => {}
            }

            // Do the updates of the flows at the end of each iteration.
            // If flows got removed or we don't know, we update anyway.
            for flow in &active_flows {
                if flow.source.as_app().runs_on(&this.myself) {
                    let mut tc_conf = TCConf::default(flow.destination.as_app().ip_addr());
                    tc_conf.bandwidth_kbs(flow.authorized_bandwidth);
                    this.my_apps.get_mut(flow.source).unwrap()
                        .0.send(EmulMessage::TCUpdate(tc_conf)).await?;
                    println!("[EmulCore] BW UPDATE : {} -> {} : {}", flow.source.as_app().name(), flow.destination.as_app().name(), flow.authorized_bandwidth)
                }
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
        // remove myself from the channels
        this.emul_id_to_channel.lock().await.remove_entry(&this.emul_id);
        Ok(())
    }

    pub async fn synchronize(self: Pin<&mut Self>) -> Result<()> {
        let this = unsafe { self.get_unchecked_mut() };
        // Using a decentralized synchronization. each node send to the other.
        // We need to create the order. Get the other hosts.
        let mut all: Vec<&IpAddr> = this.other_hosts.iter().map(|c| &c.ip_addr).collect();
        all.push(&this.myself.ip_addr);

        // If I am alone, do nothing of this
        if all.len() == 1 {
            return Ok(());
        }

        all.sort();
        let mut my_position = 0;
        for ip in &all {
            if this.myself.ip_addr.eq(*ip) {
                break;
            }
            my_position += 1;
        }

        // moving to immutable
        let my_position = my_position;
        let iam_leader = my_position == 0;
        let next_host = ClusterNodeInfo { ip_addr: *all[my_position + 1 % all.len()], port: SYNC_PORT };
        // Take the ownership of the synchronization binding and the receiver, at the end, they will be dropped.
        let (_binding, mut receiver) = this.sync_binding.take().unwrap();

        println!("[EmulCore {}] : Synchronization, my next host: {}", this.emul_id, next_host);

        // If a connection crash, it will be reported to the EManager that will send an abort
        let mut next_conn = TCP::bind(Some(NoHandler)).await?;
        next_conn.connect(next_host).await?;

        if iam_leader {
            println!("[EmulCore {}] : I am the leader of the emulation, start Synchro", this.emul_id);
            // I need to send the first element
            let mut time = Instant::now();
            let mut begin_time = time.add(Duration::from_secs(5));
            println!("[EmulCore {}] : Synchro : first proposed time: {:?}", this.emul_id, begin_time);
            // Open the connexion to the next
            next_conn.send(EmulMessage::EmulStart(EmulBeginTime { time: begin_time })).await?;
            // Now, as leader, we need to wait on receiving our time again
            while let Some(message) = receiver.recv().await {
                match message {
                    MessageWrapper { message: EmulMessage::EmulStart(begin), sender: Some(sender) } => {
                        // Check if it remains enough time to make a full circle
                        let round_duration = time.elapsed();
                        let next_round_time = Instant::now().add(round_duration.add(Duration::from_secs(1)));
                        println!("[EmulCore {}] : Synchro : first round in: {:?}", this.emul_id, round_duration);
                        if begin.time > next_round_time {
                            // Enough time
                            println!("[EmulCore {}] : Synchro : enough time for validation", this.emul_id);
                            next_conn.send(EmulMessage::EmulStartOk).await?;
                        } else {
                            // Send a new proposed time with more margin.
                            time = Instant::now();
                            begin_time = time.add(round_duration.mul(2).add(Duration::from_secs(2)));
                            println!("[EmulCore {}] : Synchro : adaptation of time, new starting time: {:?}", this.emul_id, begin_time);
                            next_conn.send(EmulMessage::EmulStart(EmulBeginTime{time: begin_time})).await?;
                        }
                        let _ = sender.send(None);
                    },
                    MessageWrapper {message: EmulMessage::EmulStartOk, sender: Some(sender)} => {
                        // I received an EmulStart. I need to check my last received begin time if it is still valid.
                        if begin_time > Instant::now() {
                            // Ok, sleep until this time
                            println!("[EmulCore {}] : Synchro : Leader finish synchro, OK", this.emul_id);
                            sleep_until(tokio::time::Instant::from(begin_time)).await;
                        } else {
                            // Abort the emulation by crashing
                            return Err(Error::new("emulation synchronization", ErrorKind::CommandFailed, "Leader of synchronization failed because validation token received after begin time."));
                        }
                        let _ = sender.send(None);
                        // Quit the reading message loop
                        break;
                    }
                    _ => {
                        eprintln!("[EmulCore {}] : Leader received bad message, waiting to receive begin time.", this.emul_id);
                        return Err(Error::new("emulation synchronization", ErrorKind::CommandFailed, "Leader received bad message during sync, waiting on begin time."));
                    }
                }
            }
        } else {
            // Initialization for the checker to be happy, but will never use because we need to
            // receive an EmulStartOk start before EmulStartOk
            let mut begin_time = Instant::now();
            while let Some(message) = receiver.recv().await {
                match message {
                    MessageWrapper { message: EmulMessage::EmulStart(begin), sender: Some(sender) } => {
                        // Just register it and pass it to the next
                        begin_time = begin.time;
                        println!("[EmulCore {}] : Synchro : Received begin time, passing to next: {:?}", this.emul_id, begin_time);
                        next_conn.send(EmulMessage::EmulStart(EmulBeginTime{time: begin_time})).await?;
                        let _ = sender.send(None);
                    },
                    MessageWrapper {message: EmulMessage::EmulStartOk, sender: Some(sender)} => {
                        // I received an EmulStartOk. I need to check my last received begin time if it is still valid.
                        if begin_time > Instant::now() {
                            // Send it to the next
                            println!("[EmulCore {}] : Synchro : Confirmation Ok, passing to next", this.emul_id);
                            next_conn.send(EmulMessage::EmulStartOk).await?;
                            // Ok, sleep until this time
                            sleep_until(tokio::time::Instant::from(begin_time)).await;
                        } else {
                            // Abort the emulation by crashing
                            return Err(Error::new("emulation synchronization", ErrorKind::CommandFailed, "Synchronization failed because validation token received after begin time."));
                        }
                        let _ = sender.send(None);
                        break;
                    }
                    _ => {
                        eprintln!("[EmulCore {}] : Leader received bad message, waiting to receive begin time.", this.emul_id);
                        return Err(Error::new("emulation synchronization", ErrorKind::CommandFailed, "Leader received bad message during sync, waiting on begin time."));
                    }
                }
            }
        }
        Ok(())
    }

    async fn remove_all_flows_from<'a>(mut active_flows: HashSet<Flow<'a, Node>>, mut bw_graph: Network<Node>, node: &Node, graph: &Network<Node>, other_hosts: &HashSet<ClusterNodeInfo>, emul_id: Uuid) -> Result<(HashSet<Flow<'a, Node>>, Network<Node>)> {
        let to_remove = active_flows.iter()
            .filter(|f| f.source == node)
            .map(|flow| (FlowConf {
                src: flow.source.as_app().ip_addr(),
                dest: flow.destination.as_app().ip_addr(),
                throughput: None,
            }, flow.clone()))
            .collect::<Vec<(FlowConf, Flow<Node>)>>();

        for (fc, mut f) in to_remove {
            // delete the flow from the active flows
            if active_flows.contains(&f) { // no need to remove if not exists
                (active_flows, bw_graph) = remove_flow(active_flows, &mut f, graph);
            }
            // Tell the others that this flow does not exists anymore
            broadcast_flow(&fc, other_hosts, emul_id).await?;
        }

        Ok((active_flows, bw_graph))
    }
}


async fn broadcast_ready(destination: &HashSet<ClusterNodeInfo>, emul_id: Uuid, future: Duration) -> Result<Instant> {
    let begin_time = Instant::now().add(future);
    let emul_begin_time = EmulBeginTime { time: begin_time };
    for host in destination {
        // Here when sending the information to the others, wrap our message with our uuid,
        // like this, when the controller of those host will receive the message, they will be
        // able to redirect the message to the good emulation core.
        let mut bind: TCPBinding<EManagerMessage, NoHandler> = TCP::bind(None).await?;
        let wrapper = EManagerMessage::EmulCoreInterchange((emul_id.to_string(), EmulMessage::EmulStart(emul_begin_time.clone())));
        bind.send_to(wrapper, host.clone()).await?
    }
    Ok(begin_time)
}

async fn broadcast_flow(flow: &FlowConf, destination: &HashSet<ClusterNodeInfo>, emul_id: Uuid) -> Result<()> {
    for host in destination {
        // Here when sending the information to the others, wrap our message with our uuid,
        // like this, when the controller of those host will receive the message, they will be
        // able to redirect the message to the good emulation core.
        let mut bind: TCPBinding<EManagerMessage, NoHandler> = TCP::bind(None).await?;
        let wrapper = EManagerMessage::EmulCoreInterchange((emul_id.to_string(), EmulMessage::FlowUpdate(flow.clone())));
        bind.send_to(wrapper, host.clone()).await?
    }
    Ok(())
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
                    start_container_and_get_pid(config.clone(), dock, app.ip_addr()).await?
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
        .spawn()?;

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
                    dock.stop_container(name).await?;
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