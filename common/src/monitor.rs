
use ::core::fmt;
use ::core::mem::transmute;

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Message {
    pub dst: u32,
    pub throughput: u32,
}

#[repr(C)]
pub struct SocketAddr {
    pub addr: u32,
}

impl fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let octets: [u8; 4] = unsafe { transmute::<u32, [u8; 4]>(self.addr) };

        write!(
            f,
            "{:^3}.{:^3}.{:^3}.{:^3}",
            octets[3], octets[2], octets[1], octets[0]
        )
    }
}

impl SocketAddr {
    pub fn new(addr: u32) -> Self {
        SocketAddr {
            addr,
        }
    }
}

