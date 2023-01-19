use std::fmt::{Display, Formatter};
use serde::{Deserialize, Serialize};
use crate::ToBytesSerialize;

#[derive(Serialize, Deserialize, Debug)]
pub enum TopologyMessage {
    NewTopology(String),
    Accepted,
    Rejected(TopologyRejectReason),
}

impl Display for TopologyMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyMessage::NewTopology(_) => write!(f, "New topology request"),
            TopologyMessage::Accepted => write!(f, "Topology Accepted"),
            TopologyMessage::Rejected(reason) => write!(f, "Your topology submission has been rejected. Reason: {}", reason),
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

impl ToBytesSerialize for TopologyMessage {}