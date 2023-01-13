use ::core::fmt;
use ::core::mem::transmute;
use std::net::{IpAddr, Ipv4Addr};
use std::ops::Deref;
use std::str::FromStr;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Message {
    pub dst: u32,
    pub throughput: u32,
}

#[repr(C)]
#[derive(Serialize, Deserialize, Debug)]
pub struct SocketAddr {
    pub addr: u32,
}

impl fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, self.to_string())
    }
}

impl From<&str> for SocketAddr {
    fn from(value: &str) -> Self {
        let v = value.split('.').map(|v| u8::from_str(v).unwrap()).collect::<Vec<u8>>();
        if v.len() != 4 {
            panic!("malformed ip");
        }
        let mut addr: u32 = 0;
        addr = addr | ((v[3] as u32) << 24);
        addr = addr | ((v[2] as u32) << 16);
        addr = addr | ((v[1] as u32) << 8);
        addr = addr | ((v[0] as u32) << 0);
        SocketAddr { addr }
    }
}

impl ToString for SocketAddr {
    fn to_string(&self) -> String {
        // Transmut is too much unsafe! It depends on the endianness of the machine
        let mut octets = [u8; 4];
        octets[3] = (self.addr >> 24) as u8;
        octets[2] = (self.addr >> 16) as u8;
        octets[1] = (self.addr >> 8) as u8;
        octets[0] = (self.addr >> 0) as u8;

        format!("{:^3}.{:^3}.{:^3}.{:^3}", octets[3], octets[2], octets[1], octets[0])
    }
}

impl SocketAddr {
    pub fn new(addr: u32) -> Self {
        SocketAddr {
            addr,
        }
    }
}

impl Deref for SocketAddr {
    type Target = IpAddr;

    fn deref(&self) -> &Self::Target {
        &IpAddr::V4(Ipv4Addr::from(self.addr))
    }
}

