mod runner_config;
mod error;
mod subnet;
mod tc_message;
mod reporter_config;
mod cluster_node;

use std::net::IpAddr;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
// re-exporting the configs objects
pub use runner_config::RunnerConfig;
pub use reporter_config::ReporterConfig;

// Exporting error handling
pub use error::Error;
pub use error::ErrorKind;
pub use error::Result;
pub use error::ErrorProducer;

// Exporting subnet and other structures
pub use subnet::{Subnet, IpMask, ToSocketAddr};

// Exporting tc messages for communication between main app en reporter
pub use tc_message::{TCMessage, FlowConf, TCConf, EmulationEvent, EventAction};

// exporting the ClusterNodeInfo
pub use cluster_node::ClusterNodeInfo;

// Utils functions
pub fn deserialize<'a, T: Deserialize<'a>>(data: &'a str) -> Result<T> {
    Ok(serde_json::from_str::<T>(data)?)
}

pub fn serialize<'a, T: Serialize>(data: &T) -> Vec<u8> {
    let json = serde_json::to_string(data).unwrap();
    json.as_bytes().to_vec()
}

pub trait ToBytesSerialize {
    fn serialize(&self) -> Bytes;
}

pub trait ToU32IpAddr {
    fn to_u32(&self) -> Result<u32>;
}

impl ToU32IpAddr for IpAddr {
    fn to_u32(&self) -> Result<u32> {
        match self {
            IpAddr::V4(a) => Ok(u32::from_ne_bytes(a.octets())),
            IpAddr::V6(_) =>
                Err(Error::new("common", ErrorKind::NotASocketAddr, "cannot transform an IpV6 into a u32"))
        }
    }
}
