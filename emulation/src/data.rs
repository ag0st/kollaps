use std::borrow::Borrow;
use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use common::{ClusterNodeInfo, EmulationEvent, EmulMessage, Error, ErrorKind, ToBytesSerialize};
use netgraph::Network;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ApplicationKind {
    // todo: not implemented yet
    BareMetal,
    /// Container define an application that must be run in a container with image + command.
    Container(ContainerConfig),
}

/// Represent a container configuration, can be used to express one already existing
/// by leaving the image_command empty or one that must be started by setting the image_command.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerConfig {
    name: String,
    id: Option<String>,
    image_command: Option<(String, Option<String>)>,
}


pub enum AppStatus {
    NotInit,
    Running,
    Stopped,
    Crashed,
}

impl ContainerConfig {
    pub fn new(name: String, id: Option<String>, image_command: Option<(String, Option<String>)>) -> ContainerConfig {
        // check if image command not empty, that the image is a non empty String, or else it will crash.
        if let Some((image, comm)) = image_command.borrow() {
            if image.is_empty() {
                panic!("[CONTAINER CONFIG]: You must specify a container image, or use None");
            }
            if let Some(com) = comm {
                if com.is_empty() {
                    panic!("[CONTAINER CONFIG]: You must specify a container command, or use None");
                }
            }
        }
        if name.is_empty() {
            panic!("[CONTAINER CONFIG]: You must give a name to the container");
        }
        ContainerConfig {
            name,
            id,
            image_command,
        }
    }

    pub fn image(&self) -> Option<String> {
        match self.image_command.borrow() {
            None => None,
            Some((im, _)) => Some(im.clone())
        }
    }

    pub fn command(&self) -> Option<String> {
        match self.image_command.borrow() {
            None => None,
            Some((_, com)) => com.clone()
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[allow(dead_code)]
    pub fn id(&self) -> Option<String> {
        self.id.clone()
    }
}


/// Represent an application being part of the emulation.
/// It is notably used by the topology processor as vertex in the netgraph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Application {
    ip_app: Option<IpAddr>,
    host: Option<ClusterNodeInfo>,
    name: String,
    kind: ApplicationKind,
}

impl Application {
    pub fn build(ip_app: Option<IpAddr>, host: Option<ClusterNodeInfo>, name: String, kind: ApplicationKind) -> Application {
        Application {
            ip_app,
            host: host.clone(),
            name,
            kind,
        }
    }

    pub fn runs_on(&self, node: &ClusterNodeInfo) -> bool {
        self.host.as_ref().unwrap().eq(node)
    }

    pub fn host(&self) -> ClusterNodeInfo {
        self.host.as_ref().unwrap().clone()
    }

    pub fn kind(&self) -> ApplicationKind {
        self.kind.clone()
    }

    pub fn ip_addr(&self) -> IpAddr {
        self.ip_app.unwrap().clone()
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn set_host(&mut self, host: ClusterNodeInfo) {
        self.host = Some(host);
    }
}


impl Display for Application {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[APP {}] IP {} \t Host {} \t", self.name, self.ip_app.unwrap(), self.host.as_ref().unwrap())
    }
}


#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Node {
    App(u32, Application),
    Bridge(u32),
}

impl Node {
    pub fn is_app(&self) -> bool {
        match self {
            Node::App(_, _) => true,
            Node::Bridge(_) => false
        }
    }
    pub fn as_app(&self) -> &Application {
        match self {
            Node::App(_, app) => app,
            Node::Bridge(_) => panic!("Trying to get the app from a bridge")
        }
    }

    pub fn as_app_mut(&mut self) -> &mut Application {
        match self {
            Node::App(_, app) => app,
            Node::Bridge(_) => panic!("Trying to get the app from a bridge")
        }
    }

    pub fn is_same_by_id(&self, id: u32) -> bool {
        match self {
            Node::App(my_id, _) => id.eq(my_id),
            Node::Bridge(my_id) => id.eq(my_id)
        }
    }
    pub fn id(&self) -> u32 {
        match self {
            Node::App(id, _) => id.clone(),
            Node::Bridge(id) => id.clone()
        }
    }
}

impl Display for Node {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Node::App(_, app) => write!(f, "{}", app),
            Node::Bridge(id) => write!(f, "[BRIDGE {}]", id)
        }
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Node::App(id_1, _), Node::App(id_2, _)) => id_1 == id_2,
            (Node::Bridge(id_1), Node::Bridge(id_2)) => id_1 == id_2,
            (_, _) => false
        }
    }
}

impl Eq for Node {}

impl Hash for Node {
    /// Hash is been made on the id
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Node::App(id, _) => id.hash(state),
            Node::Bridge(id) => id.hash(state)
        }
    }
}

/// Represent an emulation
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Emulation {
    leader: ClusterNodeInfo,
    uuid: String,
    pub graph: Network<Node>,
    pub events: Vec<EmulationEvent>,
}

impl Emulation {
    pub fn build(uuid: Uuid, graph: &Network<Node>, events: &Vec<EmulationEvent>, leader: ClusterNodeInfo) -> Emulation {
        let events_clone = events.iter().map(|e| e.clone()).collect();
        Emulation {
            leader,
            uuid: uuid.to_string(),
            graph: graph.clone(),
            events: events_clone,
        }
    }

    pub fn uuid(&self) -> Uuid {
        parse_uuid_or_crash(self.uuid.clone())
    }

    pub fn am_i_leader(&self, myself: &ClusterNodeInfo) -> bool {
        self.leader.eq(myself)
    }

    pub fn leader(&self) -> ClusterNodeInfo {
        self.leader.clone()
    }
}


fn parse_uuid_or_crash(uuid: String) -> Uuid {
    match Uuid::parse_str(&*uuid) {
        Ok(id) => id,
        Err(e) => {
            let e = Error::wrap("application", ErrorKind::InvalidData, "cannot convert the id in String into a uuid", e);
            panic!("{}", e)
        }
    }
}

/// A Flow represents data going from the source to the destination.
/// The bandwidth is the actual bandwidth of the flow and the target bandwidth is what the
/// actual flow is trying to achieve. The target bandwidth is defined at the beginning of the emulation
/// and do not change in the time contrary to the bandwidth, which changes regarding the actual use
/// of the emulated network.
#[derive(Eq, Clone, Copy)]
pub struct Flow<'a, T: netgraph::Vertex> {
    pub source: &'a T,
    pub destination: &'a T,
    pub bandwidth: u32,
    pub target_bandwidth: u32,
}

impl<'a, T: netgraph::Vertex> PartialEq for Flow<'a, T> {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.destination == other.destination
    }
}

impl<'a, T: netgraph::Vertex> Hash for Flow<'a, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.source.clone(), self.destination.clone()).hash(state)
    }
}

impl<'a, T: netgraph::Vertex> Display for Flow<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{} -> {} \t Bandwidth: {} \t / Target Bandwidth: {}]", self.source, self.destination, self.bandwidth, self.target_bandwidth)
    }
}

impl<'a, T: netgraph::Vertex> Debug for Flow<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self, f)
    }
}

impl<'a, T: netgraph::Vertex> Flow<'a, T> {
    pub fn build(source: &'a T, destination: &'a T, bandwidth: u32, target_bandwidth: u32) -> Flow<'a, T> {
        Flow { source, destination, bandwidth, target_bandwidth }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ControllerMessage {
    EmulCoreInterchange(String, EmulMessage),
    EmulationStart(ClusterNodeInfo, Emulation),
    EmulationStop(String),
}

impl Display for ControllerMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ControllerMessage::EmulCoreInterchange(id, mess) => write!(f, "EmulInterchange for {}: {}", id, mess),
            ControllerMessage::EmulationStart(ClusterNodeInfo, emulation) => write!(f, "EmulationStart of {}", emulation.uuid),
            ControllerMessage::EmulationStop(uuid) => write!(f, "EmulationStop of {}", uuid),
        }
    }
}

impl ToBytesSerialize for ControllerMessage {}
