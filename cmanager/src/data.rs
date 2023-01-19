use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

use cgraph::CGraph;
use common::{ClusterNodeInfo, deserialize, Error, ErrorKind, Result, serialize, ToBytesSerialize};

// New Type pattern for CGraph:
// https://doc.rust-lang.org/book/ch19-03-advanced-traits.html#using-the-newtype-pattern-to-implement-external-traits-on-external-types
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WCGraph(CGraph<ClusterNodeInfo>);

impl ToBytesSerialize for WCGraph {}

impl Deref for WCGraph {
    type Target = CGraph<ClusterNodeInfo>;

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

    pub fn graph(&self) -> CGraph<ClusterNodeInfo> {
        self.0.clone()
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum CJQResponseKind {
    Accepted((ClusterNodeInfo, String)),
    Wait(Duration),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Event {
    CGraphUpdate(WCGraph),
    CGraphGet,
    CGraphSend(WCGraph),
    CJQResponse(CJQResponseKind),
    CJQRequest(ClusterNodeInfo),
    CJQRetry,
    CJQAbort,
    CHeartbeat((ClusterNodeInfo, String)),
    CHeartbeatCheck,
    CHeartbeatReset,
    NodeFailure(ClusterNodeInfo),
    PClient,
    PServer(ClusterNodeInfo),
}

impl Event {
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
                let node_info = deserialize::<ClusterNodeInfo>(data)?;
                Ok(Event::CJQRequest(node_info))
            }
            0x0004 => Ok(Event::CJQRetry),
            0x0005 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<(ClusterNodeInfo, String)>(data)?;
                Ok(Event::CHeartbeat(node_info))
            }
            0x0006 => Ok(Event::PClient),
            0x0007 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<ClusterNodeInfo>(data)?;
                Ok(Event::PServer(node_info))
            }
            0x0009 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let cgraph = deserialize::<WCGraph>(data)?;
                Ok(Event::CGraphSend(cgraph))
            }
            0x000A if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let node_info = deserialize::<ClusterNodeInfo>(data)?;
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
    fn serialize_to_bytes(&self) -> Bytes {
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

    fn from_bytes(mut buf: BytesMut) -> Result<Event> {
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
