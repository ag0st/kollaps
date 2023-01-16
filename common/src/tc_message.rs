use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::time::Duration;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use crate::{deserialize, Error, ErrorKind, Result, serialize, ToBytesSerialize};


#[derive(Serialize, Deserialize, Debug)]
pub struct TCConf {
    pub dest: IpAddr,
    pub bandwidth_kbitps: Option<u32>,
    pub latency_and_jitter: Option<(f32, f32)>,
    pub drop: Option<f32>,
}

impl TCConf {
    pub fn default(dest: IpAddr) -> TCConf {
        TCConf {
            dest,
            bandwidth_kbitps: None,
            latency_and_jitter: None,
            drop: None,
        }
    }
    
    pub fn bandwidth_kbs(&mut self, bw: u32) -> &mut Self {
        self.bandwidth_kbitps = Some(bw);
        self
    }

    pub fn latency_jitter(&mut self, lat: f32, jitter: f32) -> &mut Self {
        self.latency_and_jitter = Some((lat, jitter));
        self
    }

    pub fn drop(&mut self, drop: f32) -> &mut Self {
        self.drop = Some(drop);
        self
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FlowConf {
    pub src: IpAddr,
    pub dest: IpAddr,
    pub throughput: Option<u32>,
}

impl FlowConf {
    pub fn build(src: IpAddr, dest: IpAddr, throughput: Option<u32>) -> FlowConf {
        FlowConf {
            src,
            dest,
            throughput,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum TCMessage {
    FlowUpdate(FlowConf),
    TCInit(TCConf),
    TCUpdate(TCConf),
    TCDisconnect,
    TCReconnect,
    TCTeardown,
    SocketReady,
    Event(EmulationEvent)
}


/// EmulationEvent is a programmed dynamic event of the emulation. It is for now, coupled to an application.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EmulationEvent {
    pub app_uuid: String,
    pub time: Duration,
    pub action: EventAction,
}

impl PartialOrd for EmulationEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.time.partial_cmp(&other.time)
    }
}

/// Represents possible actions of a dynamic event happening to the emulation. It is linked, for now,
/// to an application via EmulationEvent struct.
#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
pub enum EventAction {
    Join,
    Quit,
    Crash,
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
            0x0001 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let fl_conf = deserialize::<FlowConf>(data)?;
                Ok(TCMessage::FlowUpdate(fl_conf))
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
            0x0009 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let event = deserialize::<EmulationEvent>(data)?;
                Ok(TCMessage::Event(event))
            }
            _ => Err(Error::new("opcode to tc_message", ErrorKind::OpcodeNotRecognized, &*format!("cannot decrypt {opcode}.")))
        }
    }

    pub fn event_2_opcode(com: &TCMessage) -> u16 {
        match com {
            TCMessage::FlowUpdate(_) => 0x0001,
            TCMessage::TCInit(_) => 0x0003,
            TCMessage::TCUpdate(_) => 0x0004,
            TCMessage::TCDisconnect => 0x0005,
            TCMessage::TCReconnect => 0x0006,
            TCMessage::TCTeardown => 0x0007,
            TCMessage::SocketReady => 0x0008,
            TCMessage::Event(_) => 0x0009,
        }
    }
}

impl ToBytesSerialize for TCMessage {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_u16(TCMessage::event_2_opcode(self));
        match self {
            TCMessage::FlowUpdate(a) => buf.put_slice(serialize(a).as_slice()),
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
            TCMessage::FlowUpdate(_) => write!(f, "FlowUpdate"),
            TCMessage::TCInit(_) => write!(f, "TCInit"),
            TCMessage::TCUpdate(_) => write!(f, "TCUpdate"),
            TCMessage::TCDisconnect => write!(f, "TCDisconnect"),
            TCMessage::TCReconnect => write!(f, "TCReconnect"),
            TCMessage::TCTeardown => write!(f, "TCTeardown"),
            TCMessage::SocketReady => write!(f, "SocketReady"),
            TCMessage::Event(_) => write!(f, "EmulationEvent"),
        }
    }
}


