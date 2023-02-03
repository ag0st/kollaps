use std::fmt::{Display, Formatter};
use serde::{Deserialize, Serialize};
use crate::{ClusterNodeInfo, ToBytesSerialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum OManagerMessage {
    NewTopology(String),
    Accepted,
    Rejected(TopologyRejectReason),
    Abort(String),
    CleanStop((String, ClusterNodeInfo, usize)),
    EmulationReady((String, ClusterNodeInfo)),
}

impl Display for OManagerMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            OManagerMessage::NewTopology(_) => write!(f, "New topology request"),
            OManagerMessage::Accepted => write!(f, "Topology Accepted"),
            OManagerMessage::Rejected(reason) => write!(f, "Your topology submission has been rejected. Reason: {}", reason),
            OManagerMessage::Abort(id) => write!(f, "Emulation {} aborted", id),
            OManagerMessage::CleanStop((id, node, app)) => write!(f, "App {} on node {} of experiment {} finished", app, node, id),
            OManagerMessage::EmulationReady((id, node)) => write!(f, "Emulation {} ready on node {}", id, node),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum TopologyRejectReason {
    TooMuch,
    NoDeploymentFound,
    BadFile(String)
}

impl Display for TopologyRejectReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyRejectReason::TooMuch => write!(f, "Already too much topology is running, please try later"),
            TopologyRejectReason::NoDeploymentFound => write!(f, "No deployment suitable for with the actual cluster for your topology"),
            TopologyRejectReason::BadFile(e) => write!(f, "Bad file: {}", e),
        }
    }
}

impl ToBytesSerialize for OManagerMessage {}