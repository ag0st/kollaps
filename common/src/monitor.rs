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
        // use native endianness
        let octets = self.addr.to_ne_bytes();
        write!(f, "{:^3}.{:^3}.{:^3}.{:^3}", octets[3], octets[2], octets[1], octets[0])
    }
}

impl From<&str> for SocketAddr {
    fn from(value: &str) -> Self {
        let v = value.split('.').map(|v| u8::from_str(v).unwrap()).collect::<Vec<u8>>();
        if v.len() != 4 {
            panic!("malformed ip");
        }
        let mut bytes = [0u8; 4];
        bytes[0] = v[0];
        bytes[1] = v[1];
        bytes[2] = v[2];
        bytes[3] = v[3];

        // use native endianness
        let addr = u32::from_ne_bytes(bytes);
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
        let octets = self.addr.to_ne_bytes();
        IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]))
    }
}


impl Deref for SocketAddr {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.addr
    }
}

