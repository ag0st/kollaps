use std::borrow::Borrow;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::vec;

use serde::{Deserialize, Serialize};

use crate::{ToBytesSerialize, ToSocketAddr};

#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
pub struct ClusterNodeInfo {
    pub ip_addr: IpAddr,
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
        self.ip_addr.eq(other.ip_addr.borrow())
            && self.port.eq(other.port.borrow())
    }
}

impl ToSocketAddrs for ClusterNodeInfo {
    type Iter = vec::IntoIter<std::net::SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        (self.ip_addr.to_string(), self.port.clone()).to_socket_addrs()
    }
}

impl ToSocketAddr for ClusterNodeInfo {
    fn as_unix_socket_address(&self) -> Option<PathBuf> {
        None
    }
}

impl ClusterNodeInfo {
    pub fn from(addr: impl ToSocketAddr) -> ClusterNodeInfo {
        let sa = addr.to_socket_addr().expect("cannot get the socket addr");
        ClusterNodeInfo { ip_addr: sa.ip(), port: sa.port() }
    }

    pub fn new(addr: IpAddr, port: u16) -> ClusterNodeInfo {
        ClusterNodeInfo { ip_addr: addr, port }
    }
}

impl ToBytesSerialize for ClusterNodeInfo {}