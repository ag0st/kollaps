extern crate core;

mod runner_config;
mod error;
mod subnet;
mod tc_message;
mod reporter_config;
mod cluster_node;
mod topology_message;

use std::net::{IpAddr, Ipv4Addr};
use bytes::{BufMut, Bytes, BytesMut};
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
pub use tc_message::{EmulMessage, FlowConf, TCConf, EmulationEvent, EventAction};

// Exporting topology messages for the pubapi and the client
pub use topology_message::{OManagerMessage, TopologyRejectReason};

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

pub trait ToBytesSerialize: Serialize + for<'a> Deserialize<'a> {
    fn serialize_to_bytes(&self) -> Bytes where Self: Sized {
        let bytes = serialize(self);
        let mut buf = BytesMut::new();
        buf.put_slice(bytes.as_slice());
        Bytes::from(buf)
    }
    fn from_bytes(buf: BytesMut) -> Result<Self> where Self: Sized {
        let in_string = String::from_utf8(buf[..].to_owned()).unwrap();
        deserialize::<Self>(&*in_string)
    }
}

pub trait ToU32IpAddr {
    fn to_u32(&self) -> u32;
}

pub trait AsV4 {
    fn as_v4(&self) -> Result<&Ipv4Addr>;
}

impl AsV4 for IpAddr {
    fn as_v4(&self) -> Result<&Ipv4Addr> {
        match self {
            IpAddr::V4(a) => Ok(a),
            IpAddr::V6(_) => {
                Err(Error::new("AsV4 Ipaddr", ErrorKind::InvalidData, "You cannot convert an IpV6 in IpV4"))
            }
        }
    }
}

impl ToU32IpAddr for Ipv4Addr {
    fn to_u32(&self) -> u32 {
        // a.octets gives us big endian value like [127, 0, 0, 1]
        u32::from_be_bytes(self.octets())
    }
}
