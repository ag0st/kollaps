use std::fmt::{Display, Formatter};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use crate::{Error, ErrorKind, Result};

pub trait ToSocketAddr: ToSocketAddrs + Send + Sync {
    fn to_socket_addr(&self) -> Result<SocketAddr> {
        let producer = Error::producer("socket address");
        let mut addrs = match self.to_socket_addrs() {
            Ok(iter) => iter,
            Err(err) => {
                return Err(producer.wrap(ErrorKind::NotASocketAddr, "Cannot convert into a Socket Address", err));
            }
        };
        if let Some(addr) = addrs.next() {
            let addr = addr as SocketAddr;
            // take the first
            Ok(addr)
        } else {
            Err(producer.create(ErrorKind::NotASocketAddr, "No address found"))
        }
    }
    fn as_unix_socket_address(&self) -> Option<PathBuf>;
}

impl ToSocketAddr for (&str, u16) {
    fn as_unix_socket_address(&self) -> Option<PathBuf> {
        None
    }
}

impl ToSocketAddr for &str {
    fn as_unix_socket_address(&self) -> Option<PathBuf> {
        check_is_socket(self.clone())
    }
}

impl ToSocketAddr for SocketAddr {
    fn as_unix_socket_address(&self) -> Option<PathBuf> {
        None
    }
}

impl ToSocketAddr for String {
    fn as_unix_socket_address(&self) -> Option<PathBuf> {
        check_is_socket(self.as_str().clone())
    }
}

fn check_is_socket(val: &str) -> Option<PathBuf> {
    let path = Path::new(val);

    if path.is_absolute() {
        if path.extension().unwrap() == "sock" {
            return Some(PathBuf::from(path));
        }
        eprintln!("Cannot parse the socket path, need .sock extension");
    } else {
        eprintln!("Cannot parse the socket path, need absolute path!");
    }
    None
}



#[derive(Clone)]
pub struct IpMask {
    bytes: [u8; 4],
}

impl Display for IpMask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let str = self.bytes.iter()
            .map(|bytes| bytes.to_string())
            .collect::<Vec<String>>()
            .join(".").to_string();
        write!(f, "{}", str)
    }
}

impl IpMask {
    pub fn to_cidr(&self) -> u8 {
        let mut zeros_count = 0;
        'external: for b in self.bytes.iter().rev() {
            let b = b.clone();
            let mut bit_count = 0;
            for _ in 0..8 {
                if (b >> bit_count) & 1 == 0 {
                    zeros_count = zeros_count + 1;
                    bit_count = bit_count + 1;
                } else {
                    break 'external;
                }
            }
        }
        32 - zeros_count
    }
}

impl From<[u8; 4]> for IpMask {
    fn from(value: [u8; 4]) -> Self {
        IpMask { bytes: value }
    }
}

impl From<String> for IpMask {
    fn from(value: String) -> Self {
        IpMask::from(&*value)
    }
}

impl From<&str> for IpMask {
    fn from(value: &str) -> Self {
        let split: Vec<_> = value.split(".").collect();
        if split.len() != 4 {
            panic!("Cannot convert the string {} into a mask", value);
        }
        let bytes_vec: Vec<_> = split.iter()
            .map(|s| u8::from_str(s).unwrap())
            .collect();
        IpMask { bytes: vec_to_array(bytes_vec) }
    }
}

#[derive(Clone)]
pub struct Subnet {
    ip: Ipv4Addr,
    mask: IpMask,
}

impl From<&str> for Subnet {
    fn from(value: &str) -> Self {
        // split the value at /
        let both = value.split("/").collect::<Vec<&str>>();
        assert_eq!(both.len(), 2);
        Subnet::from((both[0], u8::from_str(both[1]).unwrap()))
    }
}

impl From<(&str, u8)> for Subnet {
    fn from(value: (&str, u8)) -> Self {
        if value.1 > 32 {
            panic!("Cannot parse a mask > 32, CIDR format!")
        }
        let ip = Ipv4Addr::from_str(value.0).unwrap();
        let mut mask = [0 as u8; 4];
        let mut mask_index= 0;
        let c = 0b1000_0000;
        for i in 0..value.1 {
            if i != 0 && i % 8 == 0 {
                mask_index = mask_index + 1;
            }
            mask[mask_index] = mask[mask_index] | (c >> (i % 8));
        }
        Subnet {
            ip,
            mask: IpMask::from(mask)
        }
    }
}

impl Display for Subnet {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.ip, self.mask.to_cidr())
    }
}


fn vec_to_array<T, const N: usize>(v: Vec<T>) -> [T; N] {
    v.try_into()
        .unwrap_or_else(|v: Vec<T>| panic!("Expected a Vec of length {} but it was {}", N, v.len()))
}