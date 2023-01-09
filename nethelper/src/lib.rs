use std::borrow::BorrowMut;
use std::collections::HashSet;
use std::future::Future;
use common::{Error, ErrorKind, ErrorProducer, Result};
use std::marker::PhantomData;
use std::net::{SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};
use tokio::sync::oneshot;

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
}

impl ToSocketAddr for (&str, u16) {}

impl ToSocketAddr for &str {}

impl ToSocketAddr for SocketAddr {}

impl ToSocketAddr for String {}


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


// -------------------------------------------------------------------------------------------------
//                                    DEFINING PROTOCOLS

//****************** UDP ***************

pub struct UDP {}

#[async_trait]
impl<T: Sendable, H: Handler<T> + 'static> Protocol<T, H, UDPBinding<T, H>> for UDP {
    async fn bind_addr<'a>(addr: impl ToSocketAddr, handler: Option<H>) -> Result<UDPBinding<T, H>> where
        T: 'a,
        H: 'a
    {
        let addr = addr.to_socket_addr()?;
        let socket = UdpSocket::bind(addr).await?;
        let binding = UDPBinding {
            handler,
            socket: Arc::new(socket),
            ignore_list: HashSet::new(),
            unit_type: PhantomData,
        };
        Ok(binding)
    }
}

pub struct UDPBinding<T: Sendable, H: Handler<T>> {
    handler: Option<H>,
    socket: Arc<UdpSocket>,
    ignore_list: HashSet<SocketAddr>,
    unit_type: PhantomData<T>,
}

impl<T: Sendable, H: Handler<T> + 'static> UDPBinding<T, H> {
    pub async fn broadcast(&mut self, data: T, dest_port: u16) -> Result<()> {
        self.socket.set_broadcast(true)?;
        assert_eq!(self.socket.broadcast()?, true);
        self.send_to(data, (BROADCAST_ADDR, dest_port)).await
    }

    pub fn ignore(&mut self, val: impl ToSocketAddr) {
        self.ignore_list.insert(val.to_socket_addr().unwrap());
    }

    pub fn stop_ignoring(&mut self, val: impl ToSocketAddr) {
        self.ignore_list.remove(&val.to_socket_addr().unwrap());
    }

    fn err_producer() -> ErrorProducer {
        Error::producer("udp binding")
    }

    async fn send_to(socket: Arc<UdpSocket>, to_send: T, addr: impl ToSocketAddr) -> Result<()> {
        let buf = prepare_data_to_send(to_send);
        let addr = addr.to_socket_addr()?;
        let size = socket.send_to(buf.as_ref(), addr).await?;
        if size < buf.len() {
            Err(Self::err_producer().create(ErrorKind::BadWrite, "not all the bytes where sent"))
        } else {
            Ok(())
        }
    }
    async fn receive_and_handle_from(socket: Arc<UdpSocket>, mut handler: impl Handler<T>, ignore: HashSet<SocketAddr>) -> Result<()> {
        let mut buf = vec![0u8; 2];
        // todo: do not work on Windows, need to set 65536 buffer size to not loose packet
        let (_, addr) = socket.peek_from(&mut buf).await
            .expect("cannot retrieve the size of the payload for UDP");

        // take the size
        let size_to_read = get_size(buf) + CONTENT_LENGTH_LENGTH;

        let mut buf = vec![0u8; size_to_read];

        let (_, source) = socket.recv_from(&mut buf).await.unwrap();

        // do not care about the first elements containing the content length.
        // use sub-slicing instead of removing (more efficient)
        let buf = &buf[CONTENT_LENGTH_LENGTH..];

        let bytes_mut = BytesMut::from(buf);

        // Ignore the packet if it comes from one in the list.
        if !ignore.contains(&addr) {
            if let Some(x) = handler.handle(bytes_mut).await {
                Self::send_to(socket, x, source).await
                    .expect("Cannot send back udp Info");
            }
        }

        Ok(())
    }
}


#[async_trait]
impl<T: Sendable, H: Handler<T> + 'static> ProtoBinding<T, H> for UDPBinding<T, H> {
    fn set_handler(&mut self, handler: H) {
        self.handler = Some(handler);
    }

    fn listen(&mut self) -> Result<()> {
        let handler = self.handler.as_ref()
            .expect("No handler set, please add a handler to begin to listen")
            .clone();
        let socket = self.socket.clone();
        let handler = handler.clone();
        let ignore_list = self.ignore_list.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = Self::receive_and_handle_from(socket.clone(), handler.clone(), ignore_list.clone()).await {
                    let e = UDPBinding::<T, H>::err_producer().wrap(ErrorKind::ConnectionInterrupted, "UDP Connection interrupted", e);
                    eprintln!("{}", e);
                    break;
                }
            }
        });
        Ok(())
    }

    async fn receive(&mut self) -> Result<()> {
        let handler = self.handler.as_ref()
            .expect("No handler set, please add a handler to receive")
            .clone();
        Self::receive_and_handle_from(self.socket.clone(), handler.clone(), self.ignore_list.clone()).await
    }

    async fn send_to(&mut self, to_send: T, addr: impl ToSocketAddr) -> Result<()> {
        Self::send_to(self.socket.clone(), to_send, addr).await
    }
    async fn send(&mut self, to_send: T) -> Result<()> {
        let to_send = prepare_data_to_send(to_send);
        if let Err(e) = self.socket.send(to_send.as_ref()).await {
            Err(e.into())
        } else {
            Ok(())
        }
    }
}


//****************** TCP ***************


pub struct TCP {}

#[async_trait]
impl<T: Sendable, H: Handler<T> + 'static> Protocol<T, H, TCPBinding<T, H>> for TCP {
    async fn bind_addr<'a>(addr: impl ToSocketAddr, handler: Option<H>) -> Result<TCPBinding<T, H>> where T: 'a, H: 'a {
        let socket = TcpSocket::new_v4()?;
        // On platforms with Berkeley-derived sockets, this allows to quickly
        // rebind a socket, without needing to wait for the OS to clean up the
        // previous one.
        //
        // On Windows, this allows rebinding sockets which are actively in use,
        // which allows “socket hijacking”, so we explicitly don't set it here.
        // https://docs.microsoft.com/en-us/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse
        socket.set_reuseaddr(true)?;

        let addr = addr.to_socket_addr()?;
        socket.bind(addr)?;

        let binding = TCPBinding { mode: TCPBindingMode::None, handler, socket: Some(socket), close_channel: None, unit_type: PhantomData };

        Ok(binding)
    }
}

enum TCPBindingMode {
    None,
    CONNECTED(TcpStream),
    LISTENING(Arc<TcpListener>),
}

pub struct TCPBinding<T: Sendable, H: Handler<T>> {
    mode: TCPBindingMode,
    handler: Option<H>,
    socket: Option<TcpSocket>,
    close_channel: Option<oneshot::Sender<&'static str>>,
    unit_type: PhantomData<T>,
}

impl<T: Sendable, H: Handler<T>> TCPBinding<T, H> {
    pub async fn connect(&mut self, addr: impl ToSocketAddr) -> Result<()> {
        let stream = match self.mode {
            TCPBindingMode::None => {
                self.socket.take().expect("no socket found in TCP Binding... weird")
                    .connect(addr.to_socket_addr()?).await?
            }
            _ => { return Err(Self::err_producer().create(ErrorKind::BadMode, "Binding already in a mode")); }
        };
        self.mode = TCPBindingMode::CONNECTED(stream);
        Ok(())
    }

    pub async fn receive_once(&mut self, handler_once: ResponderOnce<T>) -> Result<()> {
        let stream = self.get_stream()?;
        let b = Self::read_from_stream(stream).await?;
        if let Some(x) = handler_once(b).await {
            if let Err(e) = Self::write_to_stream(stream, x).await {
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn peers_addr(&mut self) -> Result<(SocketAddr, SocketAddr)> {
        let stream = self.get_stream()?;
        let local = stream.local_addr()?;
        let peer = stream.peer_addr()?;
        Ok((local, peer))
    }

    fn err_producer() -> ErrorProducer {
        Error::producer("tcp binding")
    }

    fn get_stream(&mut self) -> Result<&mut TcpStream> {
        if let TCPBindingMode::CONNECTED(stream) = self.mode.borrow_mut() {
            Ok(stream)
        } else {
            Err(Self::err_producer().create(ErrorKind::BadMode, "Binding not in good mode"))
        }
    }

    async fn read_and_handle_from_stream(stream: &mut TcpStream, mut handler: impl Handler<T>) -> Result<()> {
        let b = Self::read_from_stream(stream).await?;
        if let Some(x) = handler.handle(b).await {
            if let Err(e) = Self::write_to_stream(stream, x).await {
                return Err(e);
            }
        }
        Ok(())
    }

    async fn read_from_stream(s: &mut TcpStream) -> Result<BytesMut> {
        // read the message length
        let mut buf = vec![0u8; CONTENT_LENGTH_LENGTH];
        let written_size = s.read(&mut buf).await?;
        if written_size == 0 {
            return Err(Self::err_producer().create(ErrorKind::UnexpectedEof, "End Of File, stop stream"));
        }
        //
        let size = get_size(buf);
        // create a buffer to store the whole remaining of the message
        let mut buf = BytesMut::with_capacity(size as usize);

        let mut bytes_to_read = size as usize;
        // continue reading until everything is read. It can happens that we cannot take
        // everything in one shot because we are limited.
        while bytes_to_read > 0 {
            let mut handle = s.borrow_mut().take(bytes_to_read as u64);
            let bytes_read = handle.read_buf(&mut buf).await?;
            bytes_to_read -= bytes_read;
        }
        Ok(buf)
    }

    async fn write_to_stream(stream: &mut TcpStream, x: T) -> Result<()> {
        let buf = prepare_data_to_send(x);
        stream.write_all(buf.as_ref()).await.or_else(|e| {
            Err(Self::err_producer().wrap(ErrorKind::BadWrite, "Cannot write on the TCP connection", e))
        })
    }
}

impl<T: Sendable, H: Handler<T>> Drop for TCPBinding<T, H> {
    fn drop(&mut self) {
        if let TCPBindingMode::LISTENING(_) = self.mode {
            self.close_channel.take().unwrap().send("stopping accept loop").unwrap()
        }
        // nothing to do for the other modes
    }
}

#[async_trait]
impl<T: Sendable, H: Handler<T> + 'static> ProtoBinding<T, H> for TCPBinding<T, H> {
    fn set_handler(&mut self, handler: H) {
        self.handler = Some(handler);
    }

    fn listen(&mut self) -> Result<()> {
        let handler = self.handler.as_ref()
            .expect("cannot start listening if there is no handler")
            .clone();

        let listener = match self.mode {
            TCPBindingMode::None => {
                self.socket.take().expect("no socket found in TCP Binding... weird").listen(1024)?
            }
            _ => { return Err(Self::err_producer().create(ErrorKind::BadMode, "Binding already in a mode")); }
        };
        let listener = Arc::new(listener);
        let (tx, rx) = oneshot::channel();
        self.close_channel = Some(tx);

        self.mode = TCPBindingMode::LISTENING(listener.clone());
        tokio::spawn(async move {
            // create a vector to store the oneshot channel to stop the stream readers
            let mut chans = Vec::<oneshot::Sender<&str>>::new();

            tokio::select! {
                    _ = async {
                        loop {
                            let handler = handler.clone();
                            let (mut stream, _) = listener.accept().await?;
                            // println!("new TCP connection incoming with peer: {}", stream.peer_addr().unwrap());
                            let (sender, receiver) = oneshot::channel();
                            chans.push(sender);
                            tokio::spawn(async move {
                                tokio::select! {
                                    _ = async {
                                        loop {
                                            if let Err(_) = Self::read_and_handle_from_stream(&mut stream, handler.clone()).await {
                                                println!("connection closed by the peer: {}", stream.peer_addr().unwrap());
                                                break
                                            }
                                        }
                                        Ok::<_, Error>(())
                                    } => {}
                                    val = receiver => {
                                        println!("Stop reading on TCPStream: {:?}", val);
                                    }
                                }
                            });
                        }
                    // The element bellow is only here to help the
                    // Rust type checker to determine the type
                    #[allow(unreachable_code)]
                    Ok::<_, Error>(())
                    } => {}
                    val = rx => {
                        println!("Stop listening for new TCP connections: {:?}", val);
                        for a in chans {
                            a.send("Stop listening on TCP Stream").unwrap();
                        }
                    }
                }
        });
        Ok(())
    }

    async fn receive(&mut self) -> Result<()> {
        let handler = self.handler.as_ref()
            .expect("No handler set, please add a handler to receive")
            .clone();
        let stream = self.get_stream()?;
        Self::read_and_handle_from_stream(stream, handler.clone()).await
    }

    async fn send_to(&mut self, to_send: T, addr: impl ToSocketAddr) -> Result<()> {
        if let TCPBindingMode::None = self.mode {
            self.connect(addr).await?;
        }
        self.send(to_send).await
    }

    async fn send(&mut self, to_send: T) -> Result<()> {
        let stream = self.get_stream()?;
        Self::write_to_stream(stream, to_send).await
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
