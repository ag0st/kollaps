use std::borrow::Borrow;
use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use common::{ClusterNodeInfo, EmulationEvent, Error, ErrorKind};
use netgraph::Network;

#[derive(Clone, Serialize, Deserialize)]
pub enum ApplicationKind {
    // todo: not implemented yet
    BareMetal,
    /// Container define an application that must be run in a container with image + command.
    Container(ContainerConfig),
}

/// Represent a container configuration, can be used to express one already existing
/// by leaving the image_command empty or one that must be started by setting the image_command.
#[derive(Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    name: String,
    id: Option<String>,
    image_command: Option<(String, Option<String>)>,
}


pub enum AppStatus {
    NotInit,
    Running,
    Stopped,
    Crashed
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

    pub fn id(&self) -> Option<String> {
        self.id.clone()
    }
}


/// Represent an application being part of the emulation.
/// It is notably used by the topology processor as vertex in the netgraph.
#[derive(Clone, Serialize, Deserialize)]
pub struct Application {
    ip_app: IpAddr,
    host: ClusterNodeInfo,
    // using String as uuid is not Serializable/Deserializable, but the control is made everywhere
    uuid: String,
    name: String,
    kind: ApplicationKind,
}

impl Application {
    pub fn build(ip_app: IpAddr, host: &ClusterNodeInfo, uuid: Uuid, name: String, kind: ApplicationKind) -> Application {
        Application {
            ip_app,
            host: host.clone(),
            uuid: uuid.to_string(),
            name,
            kind,
        }
    }

    /// creates a fake application to be then used as key in HashMap or HashSet. As the Hash is
    /// made on the uuid for an application, it is useful to be able to produce a fake application with
    /// a defined uuid to use it then has key/entry in these kinds of structures.
    pub fn fake(uuid: Uuid) -> Application {
        let fake_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        Application {
            ip_app: fake_ip,
            host: ClusterNodeInfo::new(fake_ip, 0),
            uuid: uuid.to_string(),
            name: "".to_string(),
            kind: ApplicationKind::BareMetal,
        }
    }

    pub fn runs_on(&self, node: &ClusterNodeInfo) -> bool {
        self.host.eq(node)
    }

    pub fn host(&self) -> ClusterNodeInfo {
        self.host.clone()
    }

    pub fn uuid(&self) -> Uuid {
        parse_uuid_or_crash(self.uuid.clone())
    }

    pub fn kind(&self) -> ApplicationKind {
        self.kind.clone()
    }

    pub fn ip_addr(&self) -> IpAddr {
        self.ip_app.clone()
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }
}


impl Display for Application {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[APP {}] Name {} \t App IP {} \t Host IP {} \t", self.uuid, self.name, self.host, self.ip_app)
    }
}

impl PartialEq for Application {
    fn eq(&self, other: &Self) -> bool {
        self.uuid.eq(&other.uuid)
    }
}

impl Eq for Application {}

impl Hash for Application {
    /// Hash is been made on the id
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state)
    }
}




/// Represent an emulation
#[derive(Serialize, Deserialize)]
pub struct Emulation {
    uuid: String,
    pub graph: Network<Application>,
    pub events: Vec<EmulationEvent>,
}

impl Emulation {
    pub fn build(uuid: Uuid, graph: &Network<Application>, events: &Vec<EmulationEvent>) -> Emulation {
        let events_clone = events.iter().map(|e| e.clone()).collect();
        Emulation {
            uuid: uuid.to_string(),
            graph: graph.clone(),
            events: events_clone,
        }
    }

    pub fn uuid(&self) -> Uuid {
        parse_uuid_or_crash(self.uuid.clone())
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