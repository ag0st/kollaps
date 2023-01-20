use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, BytesMut};

use common::{Result, ToBytesSerialize, ToSocketAddr};
pub use tcp::{TCP, TCPBinding};
pub use udp::{UDP, UDPBinding};
pub use unix::{Unix, UnixBinding};

mod unix;
mod udp;
mod tcp;

const CONTENT_LENGTH_LENGTH: usize = 2;
const BROADCAST_ADDR: &str = "255.255.255.255";
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:0";
pub const ALL_ADDR: &str = "0.0.0.0";


// ------------------------------------------------------------------------------------------------
//                           DEFINING TRAITS FOR HANDLERS


pub type Responder<T> = Arc<dyn Fn(BytesMut) -> Pin<Box<dyn Future<Output=Option<T>> + Send>> + Send + Sync>;
pub type ResponderOnce<T> = Box<dyn FnOnce(BytesMut) -> Pin<Box<dyn Future<Output=Option<T>> + Send>> + Send + Sync>;

#[derive(Clone)]
/// NoHandler represents a type that never handle its incoming messages.
/// Useful when creating binding that never read but only send.
pub struct NoHandler;

pub fn handler_box<'a, T: 'a + ToBytesSerialize, E: 'a>(f: impl Fn(BytesMut) -> E + Sync + Send + 'static) -> Responder<T>
    where E: Future<Output=Option<T>> + Send + 'static + Sync {
    Arc::new(move |n| Box::pin(f(n)))
}

pub fn handler_once_box<'a, T: 'a + ToBytesSerialize, E: 'a>(f: impl FnOnce(BytesMut) -> E + Sync + Send + 'static) -> ResponderOnce<T>
    where E: Future<Output=Option<T>> + Send + 'static + Sync {
    Box::new(move |n| Box::pin(f(n)))
}


pub struct DefaultHandler<T: Debug> {
    sender: tokio::sync::mpsc::Sender<MessageWrapper<T>>,
}

impl<T: Debug + Send> DefaultHandler<T> {
    pub fn new(sender: tokio::sync::mpsc::Sender<MessageWrapper<T>>) -> Self {
        DefaultHandler { sender }
    }
}

impl<T: ToBytesSerialize + Debug + Send> Clone for DefaultHandler<T> {
    fn clone(&self) -> Self {
        DefaultHandler {
            sender: self.sender.clone()
        }
    }
}

#[async_trait]
impl<T: ToBytesSerialize + Debug + Send> Handler<T> for DefaultHandler<T> {
    async fn handle(&mut self, bytes: BytesMut) -> Option<T> {
        let message = T::from_bytes(bytes).unwrap();
        // create the channel to get the response from the controller
        let (tx, rx) = tokio::sync::oneshot::channel();
        // prepare the event to send to the controller:
        let message = MessageWrapper { message, sender: Some(tx) };
        self.sender.send(message).await.unwrap();
        // wait on the response from the controller
        match rx.await {
            Err(_) => {
                eprintln!("Default handler for panic on waiting on oneshot receiver");
                None
            }
            Ok(v) => v
        }
    }
}

#[derive(Debug)]
pub struct MessageWrapper<T: Debug> {
    pub message: T,
    pub sender: Option<tokio::sync::oneshot::Sender<Option<T>>>,
}

impl<T: Display + Debug> Display for MessageWrapper<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
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

#[async_trait]
impl<T: ToBytesSerialize> Handler<T> for NoHandler {
    async fn handle(&mut self, _: BytesMut) -> Option<T> {
        None
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
    let data = data.serialize_to_bytes();
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