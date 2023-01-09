use std::vec;
use std::borrow::Borrow;
use std::fmt::{Debug, Display, Formatter};
use std::net::{SocketAddr, ToSocketAddrs};
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

use cgraph::CGraph;
use common::{Error, ErrorKind, Result};
use nethelper::{ToBytesSerialize, ToSocketAddr};

#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
pub struct NodeInfo {
    pub ip_addr: String,
    pub port: u16,
}

impl Display for NodeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}]", self.ip_addr, self.port)
    }
}

impl Eq for NodeInfo {}

impl PartialEq for NodeInfo {
    fn eq(&self, other: &Self) -> bool {
        self.ip_addr.eq_ignore_ascii_case(other.ip_addr.borrow())
            && self.port.eq(other.port.borrow())
    }
}

impl ToSocketAddrs for NodeInfo {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        (self.ip_addr.clone(), self.port.clone()).to_socket_addrs()
    }
}

impl ToSocketAddr for NodeInfo {}

impl NodeInfo {
    pub fn new(addr: impl ToSocketAddr) -> NodeInfo {
        let sa = addr.to_socket_addr().expect("cannot get the socket addr");
        NodeInfo { ip_addr: sa.ip().to_string(), port: sa.port() }
    }
}

impl ToBytesSerialize for NodeInfo {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_slice(serialize(self).as_slice());
        Bytes::from(buf)
    }
}

// New Type pattern for CGraph:
// https://doc.rust-lang.org/book/ch19-03-advanced-traits.html#using-the-newtype-pattern-to-implement-external-traits-on-external-types
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WCGraph(CGraph<NodeInfo>);

impl ToBytesSerialize for WCGraph {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_slice(serialize(&self).as_slice());
        Bytes::from(buf)
    }
}

impl Deref for WCGraph {
    type Target = CGraph<NodeInfo>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for WCGraph {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl WCGraph {
    pub fn new() -> WCGraph {
        WCGraph(CGraph::new())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum CJQResponseKind {
    Accepted((NodeInfo, String)),
    Wait(Duration),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Event {
    CGraphUpdate(WCGraph),
    CGraphGet,
    CGraphSend(WCGraph),
    CJQResponse(CJQResponseKind),
    CJQRequest(NodeInfo),
    CJQRetry,
    CJQAbort,
    CHeartbeat((NodeInfo, String)),
    CHeartbeatCheck,
    CHeartbeatReset,
    NodeFailure(NodeInfo),
    PClient,
    PServer(NodeInfo),
}

impl Event {
    pub fn from_bytes(mut buf: BytesMut) -> Result<Event> {
        // takes the two first bytes for the opcode
        let opcode: u16 = buf.get_u16();
        // Get the payload if it exists
        let payload = if buf.has_remaining() {
            Some(String::from_utf8(buf[..].to_owned()).unwrap())
        } else {
            None
        };
        // transform everything to return an Event
        Event::opcode_2_event(opcode, payload)
    }

    pub fn opcode_2_event(opcode: u16, data: Option<String>) -> Result<Event> {
        match opcode {
            0x0000 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let cgraph = deserialize::<WCGraph>(data)?;
                Ok(Event::CGraphUpdate(cgraph))
            }
            0x0001 => Ok(Event::CGraphGet),
            0x0002 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let response_kind = deserialize::<CJQResponseKind>(data)?;
                Ok(Event::CJQResponse(response_kind))
            }
            0x0003 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<NodeInfo>(data)?;
                Ok(Event::CJQRequest(node_info))
            }
            0x0004 => Ok(Event::CJQRetry),
            0x0005 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<(NodeInfo, String)>(data)?;
                Ok(Event::CHeartbeat(node_info))
            }
            0x0006 => Ok(Event::PClient),
            0x0007 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<NodeInfo>(data)?;
                Ok(Event::PServer(node_info))
            }
            0x0009 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let cgraph = deserialize::<WCGraph>(data)?;
                Ok(Event::CGraphSend(cgraph))
            }
            0x000A if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<NodeInfo>(data)?;
                Ok(Event::NodeFailure(node_info))
            }
            0x000B => Ok(Event::CHeartbeatCheck),
            0x000C => Ok(Event::CHeartbeatReset),
            0x000D => Ok(Event::CJQAbort),
            _ => Err(Error::new("opcode to event", ErrorKind::OpcodeNotRecognized, &*format!("cannot decrypt {opcode}.")))
        }
    }

    pub fn event_2_opcode(com: &Event) -> u16 {
        match com {
            Event::CGraphUpdate(_) => 0x0000,
            Event::CGraphGet => 0x0001,
            Event::CJQResponse(_) => 0x0002,
            Event::CJQRequest(_) => 0x0003,
            Event::CJQRetry => 0x0004,
            Event::CHeartbeat(_) => 0x0005,
            Event::PClient => 0x0006,
            Event::PServer(_) => 0x0007,
            Event::CGraphSend(_) => 0x0009,
            Event::NodeFailure(_) => 0x000A,
            Event::CHeartbeatCheck => 0x000B,
            Event::CHeartbeatReset => 0x000C,
            Event::CJQAbort => 0x000D,
        }
    }
}

impl ToBytesSerialize for Event {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_u16(Event::event_2_opcode(self));
        match self {
            Event::CGraphUpdate(a) => buf.put_slice(serialize(a).as_slice()),
            Event::CJQResponse(a) => buf.put_slice(serialize(a).as_slice()),
            Event::CJQRequest(a) => buf.put_slice(serialize(a).as_slice()),
            Event::PServer(a) => buf.put_slice(serialize(a).as_slice()),
            Event::CGraphSend(a) => buf.put_slice(serialize(a).as_slice()),
            Event::NodeFailure(a) => buf.put_slice(serialize(a).as_slice()),
            Event::CHeartbeat(a) => buf.put_slice(serialize(a).as_slice()),
            _ => {}
        };
        Bytes::from(buf)
    }
}

impl Display for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::CGraphUpdate(_) => write!(f, "CGraphUpdate"),
            Event::CGraphGet => write!(f, "CGraphGet"),
            Event::CGraphSend(_) => write!(f, "CGraphSend"),
            Event::CJQResponse(_) => write!(f, "CJQResponse"),
            Event::CJQRequest(_) => write!(f, "CJQRequest"),
            Event::CJQRetry => write!(f, "CJQRetry"),
            Event::CJQAbort => write!(f, "CJQAbort"),
            Event::CHeartbeat((_, uuid)) => write!(f, "CHeartbeat({})", uuid),
            Event::NodeFailure(_) => write!(f, "NodeFailure"),
            Event::PClient => write!(f, "PClient"),
            Event::PServer(_) => write!(f, "PServer"),
            Event::CHeartbeatCheck => write!(f, "CHeartbeatCheck"),
            Event::CHeartbeatReset => write!(f, "CHeartbeatReset"),
        }
    }
}


fn deserialize<'a, T: Deserialize<'a>>(data: &'a str) -> Result<T> {
    Ok(serde_json::from_str::<T>(data)?)
}

fn serialize<'a, T: Serialize>(data: &T) -> Vec<u8> {
    let json = serde_json::to_string(data).unwrap();
    json.as_bytes().to_vec()
}
