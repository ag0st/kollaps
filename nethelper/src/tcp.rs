use std::borrow::BorrowMut;
use common::{Error, ErrorKind, ErrorProducer, Result, ToSocketAddr};
use std::marker::PhantomData;
use std::net::{SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::oneshot;
use crate::{CONTENT_LENGTH_LENGTH, get_size, Handler, prepare_data_to_send, ProtoBinding, Protocol, ResponderOnce, Sendable};


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
                self.socket.take()
                    .ok_or(Self::err_producer().create(ErrorKind::NoResource, "[TCPBinding] No socket found in TCP Binding... weird"))?
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
            self.close_channel.take().unwrap().send("[TCPListener]: Closing Socket").unwrap()
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
            .ok_or(Self::err_producer().create(ErrorKind::NoResource, "[TCPBinding] : Cannot start listening without handler"))?
            .clone();

        let listener = match self.mode {
            TCPBindingMode::None => {
                self.socket.take()
                    .ok_or(Self::err_producer().create(ErrorKind::NoResource, "[TCPBinding] No socket found in TCP Binding... weird"))?
                    .listen(1024)?
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
                            let (sender, receiver) = oneshot::channel();
                            chans.push(sender);
                            tokio::spawn(async move {
                                tokio::select! {
                                    _ = async {
                                        loop {
                                            if let Err(_) = Self::read_and_handle_from_stream(&mut stream, handler.clone()).await {
                                                println!("[TCPListener]: Connection closed by the peer: {}", stream.peer_addr()?);
                                                break
                                            }
                                        }
                                        Ok::<_, Error>(())
                                    } => {}
                                    val = receiver => {
                                        println!("[TCPListener]: Stop reading on TCPStream: {:?}", val);
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
                        println!("[TCPListener] Stop listening for new TCP connections: {:?}", val);
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
            .ok_or(Self::err_producer().create(ErrorKind::BadMode, "[TCPBinding]: Not in good mode"))?
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


