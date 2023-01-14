use std::fmt::{Display, Formatter};
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