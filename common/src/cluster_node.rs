use std::borrow::Borrow;
use std::fmt::{Display, Formatter};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::vec;
use bytes::{BufMut, Bytes, BytesMut};
use crate::{serialize, ToBytesSerialize, ToSocketAddr};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
pub struct ClusterNodeInfo {
    pub ip_addr: String,
    pub port: u16,
}

impl Display for ClusterNodeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}]", self.ip_addr, self.port)
    }
}

impl Eq for ClusterNodeInfo {}

impl PartialEq for ClusterNodeInfo {
    fn eq(&self, other: &Self) -> bool {
        self.ip_addr.eq_ignore_ascii_case(other.ip_addr.borrow())
            && self.port.eq(other.port.borrow())
    }
}

impl ToSocketAddrs for ClusterNodeInfo {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        (self.ip_addr.clone(), self.port.clone()).to_socket_addrs()
    }
}

impl ToSocketAddr for ClusterNodeInfo {
    fn as_unix_socket_address(&self) -> Option<PathBuf> {
        None
    }
}

impl ClusterNodeInfo {
    pub fn new(addr: impl ToSocketAddr) -> ClusterNodeInfo {
        let sa = addr.to_socket_addr().expect("cannot get the socket addr");
        ClusterNodeInfo { ip_addr: sa.ip().to_string(), port: sa.port() }
    }
}

impl ToBytesSerialize for ClusterNodeInfo {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_slice(serialize(self).as_slice());
        Bytes::from(buf)
    }
}