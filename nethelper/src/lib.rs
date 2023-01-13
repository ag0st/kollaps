use std::borrow::BorrowMut;
use std::collections::HashSet;
use std::future::Future;
use common::{Error, ErrorKind, ErrorProducer, Result};
use std::marker::PhantomData;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket, UnixListener, UnixStream};
use tokio::sync::oneshot;

mod unix;
mod udp;
mod tcp;

pub use unix::{UnixBinding, Unix};
pub use udp::{UDP, UDPBinding};
pub use tcp::{TCP, TCPBinding};

const CONTENT_LENGTH_LENGTH: usize = 2;
const BROADCAST_ADDR: &str = "255.255.255.255";
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:0";

// ------------------------------------------------------------------------------------------------
//                     DEFINING TO SOCKET TRAIT FOR EASY IP/PORT USE


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
    if path.exists() {
        Some(PathBuf::from(path))
    } else {
        None
    }
}


// ------------------------------------------------------------------------------------------------
//                           DEFINING TRAITS FOR HANDLERS


pub trait ToBytesSerialize {
    fn serialize(&self) -> Bytes;
}

pub type Responder<T> = Arc<dyn Fn(BytesMut) -> Pin<Box<dyn Future<Output=Option<T>> + Send>> + Send + Sync>;
pub type ResponderOnce<T> = Box<dyn FnOnce(BytesMut) -> Pin<Box<dyn Future<Output=Option<T>> + Send>> + Send + Sync>;

pub fn handler_box<'a, T: 'a + ToBytesSerialize, E: 'a>(f: impl Fn(BytesMut) -> E + Sync + Send + 'static) -> Responder<T>
    where E: Future<Output=Option<T>> + Send + 'static + Sync {
    Arc::new(move |n| Box::pin(f(n)))
}

pub fn handler_once_box<'a, T: 'a + ToBytesSerialize, E: 'a>(f: impl FnOnce(BytesMut) -> E + Sync + Send + 'static) -> ResponderOnce<T>
    where E: Future<Output=Option<T>> + Send + 'static + Sync {
    Box::new(move |n| Box::pin(f(n)))
}

#[async_trait]
pub trait Handler<T: ToBytesSerialize>: Clone + Send + Sync {
    async fn handle(&mut self, bytes: BytesMut) -> Option<T>;
}

#[async_trait]
impl<T: ToBytesSerialize> Handler<T> for Responder<T> {
    async fn handle(&mut self, bytes: BytesMut) -> Option<T> {
        self(bytes).await
    }
}

// ------------------------------------------------------------------------------------------------
//                           DEFINING TRAITS ALIASES

pub trait Sendable: 'static + ToBytesSerialize + Send {}

impl<T: 'static + ToBytesSerialize + Send> Sendable for T {}

// ------------------------------------------------------------------------------------------------
//                            DEFINING TRAITS FOR PROTOCOLS

#[async_trait]
pub trait ProtoBinding<T: Sendable, H: Handler<T>> {
    fn set_handler(&mut self, handler: H);
    fn listen(&mut self) -> Result<()>;
    async fn receive(&mut self) -> Result<()>;
    async fn send_to(&mut self, to_send: T, addr: impl ToSocketAddr) -> Result<()>;
    async fn send(&mut self, to_send: T) -> Result<()>;
}

#[async_trait]
pub trait Protocol<T: Sendable, H: Handler<T>, B: ProtoBinding<T, H> + Send> {
    async fn bind_addr<'a>(addr: impl ToSocketAddr, handler: Option<H>) -> Result<B>
        where
            B: 'a,
            T: 'a,
            H: 'a;
    async fn bind<'a>(handler: Option<H>) -> Result<B>
        where
            B: 'a,
            T: 'a,
            H: 'a {
        Self::bind_addr(DEFAULT_BIND_ADDR, handler).await
    }
}


// ------------------------------------------------------------------------------------------------
//                                  UTILITY FUNCTIONS

/// Transform a vector of byte defining in binary a number (a size) and transform it into a usize.
/// Used to transform the first bytes of an incoming communication into a size regarding the protocol.
/// i.e. get the content-length parameter registered at the beginning of a network message.
fn get_size(buf: Vec<u8>) -> usize {
    // push the first byte as u16 and shift on the left by 8 (xxxxxxxx00000000)
    // then take the second byte as u16 (00000000yyyyyyyy)
    // Finally, OR between the two => (xxxxxxxxyyyyyyyy)
    assert_eq!(buf.len(), CONTENT_LENGTH_LENGTH);
    let mut size: usize = 0;
    for i in 0..CONTENT_LENGTH_LENGTH {
        size = size | ((buf[i] as usize) << (8 * (CONTENT_LENGTH_LENGTH - 1 - i) as usize));
    }
    size
}


pub fn prepare_data_to_send<T: ToBytesSerialize>(data: T) -> BytesMut {
    let data = data.serialize();
    // create a buffer to store what we have to send. First we put the content-length (protocol)
    // and then we push the data given to us by the Handler.
    let capacity = CONTENT_LENGTH_LENGTH + data.len();
    let mut buf = BytesMut::with_capacity(capacity);
    // first, push the content length
    buf.put_u16(data.len() as u16);
    // then push the data
    buf.put_slice(data.to_vec().as_ref());
    buf
}