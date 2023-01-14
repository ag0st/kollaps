use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use crate::{deserialize, Error, ErrorKind, Result, serialize, SocketAddr, ToBytesSerialize};


#[derive(Serialize, Deserialize, Debug)]
pub struct TCConf {
    pub dest: u32,
    pub bandwidth: Option<u32>,
    pub latency_and_jitter: Option<(f32, f32)>,
    pub drop: Option<f32>,
}


#[derive(Serialize, Deserialize, Debug)]
pub struct FlowConf {
    pub src: u32,
    pub dest: SocketAddr,
    pub throughput: Option<u32>,
}

impl FlowConf {
    pub fn build(src: u32, dest: SocketAddr, throughput: Option<u32>) -> FlowConf {
        FlowConf {
            src,
            dest,
            throughput,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Container {
    pub image: String,
    pub ip: SocketAddr,
    pub name: String,
    pub pid: u32,
}

// Hash on the IP as it must be unique
impl Hash for Container {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ip.addr.hash(state)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum TCMessage {
    FlowNew(FlowConf),
    FlowUpdate(FlowConf),
    FlowRemove(FlowConf),
    TCInit(TCConf),
    TCUpdate(TCConf),
    TCDisconnect,
    TCReconnect,
    TCTeardown,
    SocketReady,
}

impl TCMessage {
    pub fn from_bytes(mut buf: BytesMut) -> Result<TCMessage> {
        // takes the two first bytes for the opcode
        let opcode: u16 = buf.get_u16();
        // Get the payload if it exists
        let payload = if buf.has_remaining() {
            Some(String::from_utf8(buf[..].to_owned()).unwrap())
        } else {
            None
        };
        // transform everything to return an TCMessage
        TCMessage::opcode_2_event(opcode, payload)
    }

    pub fn opcode_2_event(opcode: u16, data: Option<String>) -> Result<TCMessage> {
        match opcode {
            0x0000 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let fl_conf = deserialize::<FlowConf>(data)?;
                Ok(TCMessage::FlowNew(fl_conf))
            }
            0x0001 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let fl_conf = deserialize::<FlowConf>(data)?;
                Ok(TCMessage::FlowUpdate(fl_conf))
            }
            0x0002 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let fl_conf = deserialize::<FlowConf>(data)?;
                Ok(TCMessage::FlowRemove(fl_conf))
            }
            0x0003 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let tc_conf = deserialize::<TCConf>(data)?;
                Ok(TCMessage::TCInit(tc_conf))
            }
            0x0004 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let tc_conf = deserialize::<TCConf>(data)?;
                Ok(TCMessage::TCUpdate(tc_conf))
            }
            0x0005 => Ok(TCMessage::TCDisconnect),
            0x0006 => Ok(TCMessage::TCReconnect),
            0x0007 => Ok(TCMessage::TCTeardown),
            0x0008 => Ok(TCMessage::SocketReady),
            _ => Err(Error::new("opcode to tc_message", ErrorKind::OpcodeNotRecognized, &*format!("cannot decrypt {opcode}.")))
        }
    }

    pub fn event_2_opcode(com: &TCMessage) -> u16 {
        match com {
            TCMessage::FlowNew(_) => 0x0000,
            TCMessage::FlowUpdate(_) => 0x0001,
            TCMessage::FlowRemove(_) => 0x0002,
            TCMessage::TCInit(_) => 0x0003,
            TCMessage::TCUpdate(_) => 0x0004,
            TCMessage::TCDisconnect => 0x0005,
            TCMessage::TCReconnect => 0x0006,
            TCMessage::TCTeardown => 0x0007,
            TCMessage::SocketReady => 0x0008,
        }
    }
}

impl ToBytesSerialize for TCMessage {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_u16(TCMessage::event_2_opcode(self));
        match self {
            TCMessage::FlowNew(a) => buf.put_slice(serialize(a).as_slice()),
            TCMessage::FlowUpdate(a) => buf.put_slice(serialize(a).as_slice()),
            TCMessage::FlowRemove(a) => buf.put_slice(serialize(a).as_slice()),
            TCMessage::TCInit(a) => buf.put_slice(serialize(a).as_slice()),
            TCMessage::TCUpdate(a) => buf.put_slice(serialize(a).as_slice()),
            _ => {}
        };
        Bytes::from(buf)
    }
}

impl Display for TCMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TCMessage::FlowNew(_) => write!(f, "FlowNew"),
            TCMessage::FlowUpdate(_) => write!(f, "FlowUpdate"),
            TCMessage::FlowRemove(_) => write!(f, "FlowRemove"),
            TCMessage::TCInit(_) => write!(f, "TCInit"),
            TCMessage::TCUpdate(_) => write!(f, "TCUpdate"),
            TCMessage::TCDisconnect => write!(f, "TCDisconnect"),
            TCMessage::TCReconnect => write!(f, "TCReconnect"),
            TCMessage::TCTeardown => write!(f, "TCTeardown"),
            TCMessage::SocketReady => write!(f, "SocketReady"),
        }
    }
}


