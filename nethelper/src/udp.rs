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
use crate::{BROADCAST_ADDR, CONTENT_LENGTH_LENGTH, get_size, Handler, prepare_data_to_send, ProtoBinding, Protocol, Sendable, ToSocketAddr};


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
