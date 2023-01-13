use ::core::fmt;
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
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct SocketAddr {
    pub addr: u32,
}

impl fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Transmut is too much unsafe! It depends on the endianness of the machine
        let mut octets = [0u8; 4];
        octets[3] = (self.addr >> 24) as u8;
        octets[2] = (self.addr >> 16) as u8;
        octets[1] = (self.addr >> 8) as u8;
        octets[0] = (self.addr >> 0) as u8;

        write!(f, "{:^3}.{:^3}.{:^3}.{:^3}", octets[3], octets[2], octets[1], octets[0])
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

impl SocketAddr {
    pub fn new(addr: u32) -> Self {
        SocketAddr {
            addr,
        }
    }
    pub fn to_ip_addr(&self) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(self.addr))
    }
}


impl Deref for SocketAddr {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.addr
    }
}

