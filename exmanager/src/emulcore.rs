use std::borrow::{Borrow, BorrowMut};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use common::{ClusterNodeInfo, Error, ErrorKind, Result, TCConf, TCMessage, ToU32IpAddr};
use netgraph::Network;
use nethelper::{Handler, NoHandler, ProtoBinding, Protocol, Responder, Unix, UnixBinding};
use async_trait::async_trait;
use bytes::BytesMut;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, oneshot};
use dockhelper::DockerHelper;
use crate::data::{Application, ApplicationKind, ContainerConfig, Emulation};


#[derive(Clone)]
struct FlowHandler {
    sender: mpsc::Sender<TCMessage>,
}

#[async_trait]
impl Handler<TCMessage> for FlowHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<TCMessage> {
        let mess = TCMessage::from_bytes(bytes).unwrap();
        self.sender.send(mess).await.unwrap();
        None
    }
}

/// EmulCore is an emulation core. It is spawn when the node receive an emulation
/// to execute from the leader. Its responsibilities are to assure that the all attributed
/// application are running an monitored. At changes it communicate with the others EmulCore
/// dispatched across the cluster. It is also responsible to follow the dynamic events associated
/// with their associated nodes.
pub struct EmulCore {
    graph: Network<Application>,
    bw_graph: Network<Application>,
    my_apps: HashMap<Application, Option<UnixBinding<TCMessage, NoHandler>>>,
    other_hosts: HashSet<ClusterNodeInfo>,
    myself: ClusterNodeInfo,
    flow_socket: Option<UnixBinding<TCMessage, FlowHandler>>,
    emul_id: uuid::Uuid,
    reporters: Vec<Child>,
}

impl EmulCore {
    pub fn new(emul: Emulation, myself: &ClusterNodeInfo) -> EmulCore {
        // First thing is to parse the emulation to gather all the information I need:
        let (bw_graph, my_apps, other_hosts) = parse_emulation(&emul, &myself);
        EmulCore {
            graph: emul.graph,
            bw_graph,
            my_apps,
            other_hosts,
            myself: myself.clone(),
            flow_socket: None,
            emul_id: emul.uuid(),
            reporters: Vec::new(),
        }
    }

    pub async fn start_emulation(mut self, dock: &DockerHelper, working: oneshot::Sender<()>) -> Result<()> {
        // First, creating the socket, its handler and the incoming flow message channel.
        let (sender, mut receiver) = mpsc::channel(1000);
        let flow_handler = FlowHandler { sender };
        let flow_socket_name = generate_flow_socket(&self.emul_id)?;
        let mut flow_socket_binding: UnixBinding<TCMessage, FlowHandler> = Unix::bind_addr(flow_socket_name.clone(), Some(flow_handler)).await?;
        flow_socket_binding.listen()?;
        self.flow_socket = Some(flow_socket_binding);

        // Now we launch all the reporters for the applications it needs to emulate

        // We need to create a Arc references to the receiver as we are going to pass to
        // an async function named "waiting_on_response" that will wait on the reporter response telling us
        // that he is ready. We do not create mutex as it is not necessary, we are not
        // using the reference here, only in the async function, but rust doesn't want to let us.
        let receiver_arc = Arc::new(Mutex::new(receiver));

        let mut reporter_id = 0;
        // this temporary hashmap is used to store the socket path of each app returned by the method
        // launch_reporter_for_app for then, after looping, being able to connect to each of these
        // sockets.
        let mut reporters_sockets = HashMap::new();
        for (app, _) in self.my_apps.borrow() {
            let (proc, tc_socket_name) = launch_reporter_for_app(self.emul_id, app, dock, flow_socket_name.clone(), reporter_id).await?;
            // store the process, when I drop, all the child process will drop and be killed.
            self.reporters.push(proc);
            // Keep the socket path
            reporters_sockets.insert(app.clone(), tc_socket_name);

            // waiting response logic: ignore every message and take care only of the SocketReady.
            // ignoring message is not a big deal as we will begin to consider flows after every thing
            // is initialized
            let r = receiver_arc.clone();
            let waiting_on_response = async move {
                loop {
                    if let TCMessage::SocketReady = r.lock().await.recv().await.unwrap() {
                        break
                    }
                }
            };

            // Wait on the reporter to be ready with a timeout
            if let Err(_) = tokio::time::timeout(Duration::from_secs(5), waiting_on_response).await {
                return Err(Error::new("emulcore start", ErrorKind::CommandFailed, "The reporter didn't send ready in the defined timeout"))
            }

            reporter_id = reporter_id + 1;
        }

        // Now it is time to connect to each of the TC sockets
        for (app, socket) in reporters_sockets {
            // As the reporter is started, we will connect to its socket
            let mut binding: UnixBinding<TCMessage, NoHandler> = Unix::bind_addr(socket, None).await?;
            binding.connect().await?;
            // Send the TC Init
            let conf = TCConf::default(app.ip_addr());
            binding.send(TCMessage::TCInit(conf));
            self.my_apps.insert(app, Some(binding));
        }

        // Now, all reporters are correctly connected and we are bind to all of these reporter
        // We can launch the event scheduler




        todo!()
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
async fn launch_reporter_for_app(emul_id: uuid::Uuid, app: &Application, dock: &DockerHelper, flow_socket_name: String, reporter_id: u32) -> Result<(Child, String)> {
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
    println!("[EMULCORE {}]: Starting reporter for app: {} : {}", emul_id, app.uuid(), app.name());
    // Start the reporter with a generated tc_socket
    let tc_socket_name = generate_tc_socket(&emul_id, &app.uuid())?;

    let child = Command::new("sudo").arg("-S") // must start with super user to enter another pid namespace
        .arg("nsenter")
        .arg("-t").arg(format!("{pid}"))
        .arg("-n")
        .arg("/home/agost/workspace/MSc/development/kollaps/target/release/reporter")
        .arg("--flow-socket").arg(flow_socket_name)
        .arg("--tc-socket").arg(tc_socket_name.clone())
        .arg("--id").arg(format!("{}", reporter_id))
        .kill_on_drop(true)
        .spawn().expect(&*format!("Cannot launch reporter for app {}", app.uuid()));

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

/// Parse emulation allow to take in entry an emulation and then to extract essential information
/// from it. It is not necessary as every information is already in the emulation but it creating
/// separate structure for it allows for simpler interaction and less algorithms runs.
fn parse_emulation(emul: &Emulation, myself: &ClusterNodeInfo) -> (Network<Application>, HashMap<Application, Option<UnixBinding<TCMessage, NoHandler>>>, HashSet<ClusterNodeInfo>) {
    // Get all the application that are assigned to the node we are running.
    let my_apps = emul.graph.vertices().iter().filter(|app| app.runs_on(myself))
        .fold(HashMap::new(), |mut acc, app| {
            if let Some(_) = acc.insert(app.clone(), None) {
                panic!("Two times the same app")
            };
            acc
        });
    let other_host = emul.graph.vertices().iter()
        .filter(|app| !app.runs_on(myself))
        .fold(HashSet::new(), |mut acc, app| {
            acc.insert(app.host());
            acc
        });
    (emul.graph.clone(), my_apps, other_host)
}

/// Returns the Flow Socket name of this emulation. Can be used only one time per emulation.
pub fn generate_flow_socket(emul_id: &uuid::Uuid) -> Result<String> {
    let name = format!("/tmp/kollaps_{}_flows.sock", emul_id);
    verify_socket_exists(name)
}

fn generate_tc_socket(emul_id: &uuid::Uuid, app_id: &uuid::Uuid) -> Result<String> {
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

// 1. For each assigned application,

// The target bandwidth is calculated when detecting a new flow, we then assigned to the flow the max
// possible found in the graph.