use std::borrow::BorrowMut;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use common::{Error, ErrorKind, ErrorProducer, Result};

use crate::Handler;
use crate::ProtoBinding;
use crate::Protocol;
use crate::Sendable;
use crate::ToSocketAddr;

pub struct Unix {}

#[async_trait]
impl<T: Sendable, H: Handler<T> + 'static> Protocol<T, H, UnixBinding<T, H>> for Unix {
    async fn bind_addr<'a>(addr: impl ToSocketAddr, handler: Option<H>) -> Result<UnixBinding<T, H>> where T: 'a, H: 'a {
        match addr.as_unix_socket_address() {
            None => Err(Error::new("nethelper unix socket", ErrorKind::InvalidData, "The address given cannot be converted in unix socket")),
            Some(path) => {
                Ok(UnixBinding {
                    path,
                    mode: UnixBindingMode::None,
                    handler,
                    close_channel: None,
                    unit_type: PhantomData,
                })
            }
        }
    }
    async fn bind<'a>(_handler: Option<H>) -> Result<UnixBinding<T, H>> where T: 'a, H: 'a {
        Err(Error::new("nethelper unix socket", ErrorKind::Unsupported, "You cannot bind unix socket without giving the path"))
    }
}

enum UnixBindingMode {
    None,
    Connected(UnixStream),
    Listening(Arc<UnixListener>),
}

pub struct UnixBinding<T: Sendable, H: Handler<T>> {
    path: PathBuf,
    mode: UnixBindingMode,
    handler: Option<H>,
    close_channel: Option<oneshot::Sender<&'static str>>,
    unit_type: PhantomData<T>,
}

impl<T: Sendable, H: Handler<T>> UnixBinding<T, H> {
    pub async fn connect(&mut self) -> Result<()> {
        let stream = match self.mode {
            UnixBindingMode::None => {
                UnixStream::connect(self.path.clone()).await?
            }
            _ => { return Err(Self::err_producer().create(ErrorKind::BadMode, "Binding already in a mode")); }
        };
        self.mode = UnixBindingMode::Connected(stream);
        Ok(())
    }

    fn err_producer() -> ErrorProducer {
        Error::producer("tcp binding")
    }

    fn get_stream(&mut self) -> Result<&mut UnixStream> {
        if let UnixBindingMode::Connected(stream) = self.mode.borrow_mut() {
            Ok(stream)
        } else {
            Err(Self::err_producer().create(ErrorKind::BadMode, "Binding not in good mode"))
        }
    }

    async fn read_and_handle_from_stream(stream: &mut UnixStream, mut handler: impl Handler<T>) -> Result<()> {
        let b = Self::read_from_stream(stream).await?;
        if let Some(x) = handler.handle(b).await {
            if let Err(e) = Self::write_to_stream(stream, x).await {
                return Err(e);
            }
        }
        Ok(())
    }

    async fn read_from_stream(stream: &mut UnixStream) -> Result<BytesMut> {
        // read the message length
        let content_length = stream.read_u16().await?;
        // create a buffer to store the whole remaining of the message
        let mut buf = vec![0u8; content_length as usize];
        stream.read_exact(&mut buf).await.unwrap();
        Ok(BytesMut::from(buf.as_slice()))
    }

    async fn write_to_stream(stream: &mut UnixStream, x: T) -> Result<()> {
        let data = x.serialize();
        stream.write_u16(data.len() as u16).await.unwrap();
        stream.write_all(data.as_ref()).await.or_else(|e| {
            Err(Self::err_producer().wrap(ErrorKind::BadWrite, "Cannot write on the Unix Socket connection", e))
        })
    }
}

impl<T: Sendable, H: Handler<T>> Drop for UnixBinding<T, H> {
    fn drop(&mut self) {
        if let UnixBindingMode::Listening(_) = self.mode {
            self.close_channel.take().unwrap().send("stopping accept loop").unwrap();
            // delete the file
            if self.path.exists() {
                // There's no way to return a useful error here
                if let Err(e) = std::fs::remove_file(&self.path) {
                    eprintln!("Cannot delete the socket because of: {}", e)
                }
            }
        }
        // nothing to do for the other modes
    }
}

#[async_trait]
impl<T: Sendable, H: Handler<T> + 'static> ProtoBinding<T, H> for UnixBinding<T, H> {
    fn set_handler(&mut self, handler: H) {
        self.handler = Some(handler)
    }

    fn listen(&mut self) -> Result<()> {
        let handler = self.handler.as_ref()
            .expect("cannot start listening if there is no handler")
            .clone();

        let listener = match self.mode {
            UnixBindingMode::None => {
                UnixListener::bind(&self.path.clone())?
            }
            _ => { return Err(Self::err_producer().create(ErrorKind::BadMode, "Binding already in a mode")); }
        };
        let listener = Arc::new(listener);
        let (tx, rx) = oneshot::channel();
        self.close_channel = Some(tx);

        self.mode = UnixBindingMode::Listening(listener.clone());
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
                                                println!("connection closed by the peer");
                                                break
                                            }
                                        }
                                        Ok::<_, Error>(())
                                    } => {}
                                    val = receiver => {
                                        println!("Stop reading on UnixStream: {:?}", val);
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
                        println!("Stop listening for new Unix connections: {:?}", val);
                        for a in chans {
                            if let Err(_) = a.send("Stop listening on Unix Stream") {
                                println!("Connection already closed");
                            }
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

    async fn send_to(&mut self, to_send: T, _: impl ToSocketAddr) -> Result<()> {
        if let UnixBindingMode::None = self.mode {
            self.connect().await?;
        }
        self.send(to_send).await
    }

    async fn send(&mut self, to_send: T) -> Result<()> {
        let stream = self.get_stream()?;
        Self::write_to_stream(stream, to_send).await
    }
}