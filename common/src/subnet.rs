use std::fmt::{Display, Formatter};
use std::net::Ipv4Addr;
use std::str::FromStr;

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