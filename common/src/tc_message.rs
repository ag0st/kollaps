use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::time::{Duration, Instant};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use crate::{deserialize, Error, ErrorKind, Result, serialize, ToBytesSerialize};


#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EmulBeginTime {
    #[serde(with = "serde_millis")]
    pub time: Instant,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EmulMessage {
    FlowUpdate(FlowConf),
    TCInit(TCConf),
    TCUpdate(TCConf),
    TCDisconnect,
    TCReconnect,
    TCTeardown,
    SocketReady,
    EmulStart(EmulBeginTime),
    Event(EmulationEvent),
}


/// EmulationEvent is a programmed dynamic event of the emulation. It is for now, coupled to an application.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EmulationEvent {
    pub app_id: u32,
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


impl EmulMessage {
    pub fn opcode_2_event(opcode: u16, data: Option<String>) -> Result<EmulMessage> {
        match opcode {
            0x0001 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let fl_conf = deserialize::<FlowConf>(data)?;
                Ok(EmulMessage::FlowUpdate(fl_conf))
            }

            0x0003 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let tc_conf = deserialize::<TCConf>(data)?;
                Ok(EmulMessage::TCInit(tc_conf))
            }
            0x0004 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let tc_conf = deserialize::<TCConf>(data)?;
                Ok(EmulMessage::TCUpdate(tc_conf))
            }
            0x0005 => Ok(EmulMessage::TCDisconnect),
            0x0006 => Ok(EmulMessage::TCReconnect),
            0x0007 => Ok(EmulMessage::TCTeardown),
            0x0008 => Ok(EmulMessage::SocketReady),
            0x0009 if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let event = deserialize::<EmulationEvent>(data)?;
                Ok(EmulMessage::Event(event))
            }
            0x000A if data.is_some() => {
                let data = data.as_deref().unwrap_or("");
                let time = deserialize::<EmulBeginTime>(data)?;
                Ok(EmulMessage::EmulStart(time))
            }
            _ => Err(Error::new("opcode to tc_message", ErrorKind::OpcodeNotRecognized, &*format!("cannot decrypt {opcode}.")))
        }
    }

    pub fn event_2_opcode(com: &EmulMessage) -> u16 {
        match com {
            EmulMessage::FlowUpdate(_) => 0x0001,
            EmulMessage::TCInit(_) => 0x0003,
            EmulMessage::TCUpdate(_) => 0x0004,
            EmulMessage::TCDisconnect => 0x0005,
            EmulMessage::TCReconnect => 0x0006,
            EmulMessage::TCTeardown => 0x0007,
            EmulMessage::SocketReady => 0x0008,
            EmulMessage::Event(_) => 0x0009,
            EmulMessage::EmulStart(_) => 0x000A,
        }
    }
}

impl ToBytesSerialize for EmulMessage {
    fn serialize_to_bytes(&self) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_u16(EmulMessage::event_2_opcode(self));
        match self {
            EmulMessage::FlowUpdate(a) => buf.put_slice(serialize(a).as_slice()),
            EmulMessage::TCInit(a) => buf.put_slice(serialize(a).as_slice()),
            EmulMessage::TCUpdate(a) => buf.put_slice(serialize(a).as_slice()),
            _ => {}
        };
        Bytes::from(buf)
    }

    fn from_bytes(mut buf: BytesMut) -> Result<EmulMessage> {
        // takes the two first bytes for the opcode
        let opcode: u16 = buf.get_u16();
        // Get the payload if it exists
        let payload = if buf.has_remaining() {
            Some(String::from_utf8(buf[..].to_owned()).unwrap())
        } else {
            None
        };
        // transform everything to return an EmulMessage
        EmulMessage::opcode_2_event(opcode, payload)
    }
}

impl Display for EmulMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EmulMessage::FlowUpdate(_) => write!(f, "FlowUpdate"),
            EmulMessage::TCInit(_) => write!(f, "TCInit"),
            EmulMessage::TCUpdate(_) => write!(f, "TCUpdate"),
            EmulMessage::TCDisconnect => write!(f, "TCDisconnect"),
            EmulMessage::TCReconnect => write!(f, "TCReconnect"),
            EmulMessage::TCTeardown => write!(f, "TCTeardown"),
            EmulMessage::SocketReady => write!(f, "SocketReady"),
            EmulMessage::EmulStart(_) => write!(f, "EmulationReady"),
            EmulMessage::Event(_) => write!(f, "EmulationEvent"),
        }
    }
}


