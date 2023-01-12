use std::collections::HashMap;
use std::path::Path;
use std::ptr;
use futures::StreamExt;

use redbpf::load::{Loaded, Loader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

use common::{Error, ErrorKind, ErrorProducer, Result};
use common::{message, SocketAddr};

pub struct UsageAnalyzer {
    loaded: Option<Loaded>,
    usage: HashMap<u32, u32>,
    stream: Option<UnixStream>,
    err_pro: ErrorProducer,
}

impl UsageAnalyzer {
    pub async fn build(socket_addr: &str, interface: &str) -> Result<UsageAnalyzer> {
        let err_pro = Error::producer("usage analyzer");

        // First, connect to the Socket
        // let socket_path = Path::new(socket_addr);
        // let mut stream = match UnixStream::connect(&socket_path).await {
        //     Ok(stream) => stream,
        //     Err(e) => return Err(err_pro.wrap(ErrorKind::ConnectionInterrupted, "cannot attach to unix socket", e))
        // };

        // Then load the ebpf file
        let loaded = Self::load_ebpf(interface).await?;

        Ok(UsageAnalyzer {
            loaded: Some(loaded),
            usage: HashMap::new(),
            stream: None,
            err_pro,
        })
    }

    pub async fn start(&mut self) {
        // create message passing communication between the ebpf event reader and the main thread
        let (sender, mut receiver) = mpsc::channel(4096);

        // launch tokio task to read the events coming from the ebpf program
        // need to take the loaded ebpf to pass it to the thread
        let mut loaded = self.loaded.take().unwrap();
        tokio::spawn(async move {
            while let Some((name, events)) = loaded.events.next().await {
                match name.as_str() {
                    "perf_events" => {
                        for event in events {
                            let message = unsafe { ptr::read(event.as_ptr() as *const message) };
                            if let Err(e) =  sender.send((message.dst, message.bytes)).await {
                                println!("[eBPF EVENT READER]: Quitting...");
                                break
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        // main loop: Reading changes, calculate connection bandwidth changes and notifying the socket
        // on changes
        while let Some((ip, bytes)) = receiver.recv().await {
            println!("DEST: {} \t BYTES: {}", SocketAddr::new(ip), bytes)
        }
    }

    async fn load_ebpf(interface: &str) -> Result<Loaded> {
        let mut raw_fds = Vec::new();
        let mut loaded = Loader::load(probe_code()).expect("error loading BPF program");
        //insert socket filter
        let mut attach_counts = 0;
        for sf in loaded.socket_filters_mut() {
            match sf.attach_socket_filter(interface) {
                Ok(sock_raw_fd) => {
                    attach_counts = attach_counts + 1;
                    raw_fds.push(sock_raw_fd);
                }
                Err(e) =>
                    return Err(Error::new(
                        "load ebpf",
                        ErrorKind::CommandFailed,
                        &*format!("cannot attach interface {} to the eBPF probe.: {:?}", interface, e),
                    ))
            }
        }
        if attach_counts == 0 {
            Err(Error::new("load ebpf", ErrorKind::CommandFailed, "no network interface attached to the eBPF program"))
        } else {
            Ok(loaded)
        }
    }
}

//retrieve.elf file
fn probe_code() -> &'static [u8] {
    include_bytes!("usage.elf")
}
