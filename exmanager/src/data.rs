use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::time::Duration;
use serde::{Deserialize, Serialize};
use common::{ClusterNodeInfo, SocketAddr};

#[derive(Clone, Serialize, Deserialize)]
enum ApplicationKind {
    // todo: not implemented yet
    BareMetal,
    /// Container define an application that must be run in a container with image + command.
    Container(ContainerConfig)
}

/// Represent a container configuration, can be used to express one already existing
/// by leaving the image_command empty or one that must be started by setting the image_command.
#[derive(Clone, Serialize, Deserialize)]
struct ContainerConfig {
    name: String,
    id: Option<String>,
    image_command: Option<(String, String)>
}


/// Represent an application being part of the emulation.
/// It is notably used by the topology processor as vertex in the netgraph.
#[derive(Clone, Serialize, Deserialize)]
struct Application {
    ip_app: SocketAddr,
    host: ClusterNodeInfo,
    id: u32,
    name: String,
    kind: ApplicationKind
}


impl Display for Application {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[APP {}] Name {} \t App IP {} \t Host IP {} \t", self.id, self.name, self.host, self.ip_app)
    }
}

impl PartialEq for Application {
    fn eq(&self, other: &Self) -> bool {
        self.id.eq(&other.id)
    }
}

impl Eq for Application {}

impl Hash for Application {
    /// Hash is been made on the id
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

/// Represents possible actions of a dynamic event happening to the emulation. It is linked, for now,
/// to an application via EmulationEvent struct.
#[derive(Serialize, Deserialize)]
enum EventAction {
    Join,
    Quit,
    Crash,
}

/// EmulationEvent is a programmed dynamic event of the emulation. It is for now, coupled to an application.
#[derive(Serialize, Deserialize)]
struct EmulationEvent {
    app: Application,
    time: Duration,
    action: EventAction
}

/// Represent an emulation
#[derive(Serialize, Deserialize)]
struct Emulation {
    uuid: String,
    graph: netgraph::Network<Application>,
    events: Vec<EmulationEvent>,
}


/// A Flow represents data going from the source to the destination.
/// The bandwidth is the actual bandwidth of the flow and the target bandwidth is what the
/// actual flow is trying to achieve. The target bandwidth is defined at the beginning of the emulation
/// and do not change in the time contrary to the bandwidth, which changes regarding the actual use
/// of the emulated network.
#[derive(Eq, Clone, Copy)]
pub struct Flow<T: netgraph::Vertex> {
    pub source: T,
    pub destination: T,
    bandwidth: usize,
    pub target_bandwidth: usize,
}

impl<T: netgraph::Vertex> PartialEq for Flow<T> {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.destination == other.destination
    }
}

impl<T: netgraph::Vertex> Hash for Flow<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.source.clone(), self.destination.clone()).hash(state)
    }
}

impl<T: netgraph::Vertex> Display for Flow<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{} -> {} \t Bandwidth: {} \t / Target Bandwidth: {}]", self.source, self.destination, self.bandwidth, self.target_bandwidth)
    }
}

impl<T: netgraph::Vertex> Debug for Flow<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self, f)
    }
}

impl<T: netgraph::Vertex> Flow<T> {
    pub fn build(source: T, destination: T, bandwidth: usize, target_bandwidth: usize) -> Flow<T> {
        Flow { source, destination, bandwidth, target_bandwidth }
    }
}