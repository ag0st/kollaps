use std::{fs, ptr};
use std::collections::{HashMap, HashSet};
use std::ops::Add;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::BytesMut;
use futures::StreamExt;
use lockfree::map::Map;
use redbpf::load::{Loaded, Loader};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use common::{FlowConf, Message, ReporterConfig, SocketAddr, TCMessage};
use common::TCMessage::{FlowNew, FlowRemove, FlowUpdate};
use error::{Error, Result};
use nethelper::{Handler, ProtoBinding, Protocol, Responder, Unix, UnixBinding};

use crate::tc_manager::TCManager;

pub mod error;
mod tc_manager;

#[derive(Clone)]
/// TCHandler is used to receive events message from the emulation module in the main application
/// and apply Traffic Control changes to the network namespace it is running in.
struct TCHandler {
    // protect the manager with a mutex, normally, this will not be used by multiple listener,
    // so no need of mutex. It is for Rust, like this he is sure for the compilation. When only
    // on thread access the mutex, the overhead is not that dramatically enormous.
    // todo: remove the mutex and use a RefCell configuration instead.
    manager: Arc<Mutex<TCManager>>,
    initialized_path: Arc<Mutex<HashSet<u32>>>,
    clean_sender: mpsc::Sender<()>,
}

impl TCHandler {
    fn new(clean_sender: mpsc::Sender<()>) -> TCHandler {
        TCHandler {
            manager: Arc::new(Mutex::new(TCManager::new())),
            initialized_path: Arc::new(Mutex::new(HashSet::new())),
            clean_sender,
        }
    }
}

#[async_trait]
impl Handler<TCMessage> for TCHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<TCMessage> {
        if let Ok(mess) = TCMessage::from_bytes(bytes) {
            println!("Received a TC Message: {}", mess);
            match mess {
                TCMessage::TCInit(conf) => self.manager.lock().ok()?.init(conf.dest),
                TCMessage::TCUpdate(conf) => {
                    let manager = self.manager.lock().ok()?;
                    if self.initialized_path.lock().ok()?.contains(&conf.dest) {
                        if let Some(bw) = conf.bandwidth {
                            manager.change_bandwidth(conf.dest, bw);
                        }
                        if let Some((lat, jit)) = conf.latency_and_jitter {
                            manager.change_latency(conf.dest, lat, jit);
                        }
                        if let Some(drop) = conf.drop {
                            manager.change_loss(conf.dest, drop);
                        }
                    } else {
                        self.manager.lock().ok()?
                            .initialize_path(
                                conf.dest,
                                conf.bandwidth.unwrap(),
                                conf.latency_and_jitter.unwrap().0,
                                conf.latency_and_jitter.unwrap().1,
                                conf.drop.unwrap());
                        self.initialized_path.lock().ok()?.insert(conf.dest);
                    }
                }
                TCMessage::TCDisconnect => self.manager.lock().ok()?.disconnect(),
                TCMessage::TCReconnect => self.manager.lock().ok()?.reconnect(),
                TCMessage::TCTeardown => {
                    // need to destroy the socket
                    self.clean_sender.send(()).await.unwrap();
                    self.manager.lock().ok()?.tear_down();
                }
                _ => { eprintln!("received flow message, it must not happens, ignoring") }
            }
        } else {
            eprintln!("Bad message received from the emulation process")
        }
        None
    }
}

struct Cleaner {
    receiver: mpsc::Receiver<()>,
    listener: UnixBinding<TCMessage, TCHandler>,
}

impl Cleaner {
    fn build(receiver: mpsc::Receiver<()>, listener: UnixBinding<TCMessage, TCHandler>) -> Cleaner {
        Cleaner {
            receiver,
            listener,
        }
    }
    async fn wait_to_clean(mut self) {
        println!("Received cleaning command");
        self.receiver.recv().await;
        drop(self.listener)
    }
}

pub struct UsageAnalyzer {
    /// This field is an option because it allow to be taken by the listener process.
    usage: Arc<Map<u32, (u32, Instant)>>,
    config: ReporterConfig,
}

impl UsageAnalyzer {
    pub async fn build(config: &ReporterConfig) -> Result<UsageAnalyzer> {
        Ok(UsageAnalyzer {
            usage: Arc::new(Map::new()),
            config: config.clone(),
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        // create cleaner channel
        let (sender, receiver) = mpsc::channel(1);

        // First, connect to the Sockets for sending flow update and receiving traffic control commands
        //      1. Receive traffic control commands
        println!("Reporter {} binding on socket", self.config.id);
        let mut binding = Unix::bind_addr(self.config.tc_socket.clone(), Some(TCHandler::new(sender))).await.unwrap();
        binding.listen().unwrap();

        // Create the cleaner
        let cleaner = Cleaner::build(receiver, binding);
        // launch the cleaner in another thread
        tokio::spawn(async move { cleaner.wait_to_clean().await });

        // We need to change the socket permission, because we are currently in root mode.
        // If we do not change the socket permission, the other app cannot connect to it.
        let tc_socket_path = Path::new(&self.config.tc_socket);
        // Wait for the socket to exists, it can take time, the function in the libc do not open socket
        // immediately and there is tokio overhead maybe.
        while !tc_socket_path.exists() {}
        let mut perms = fs::metadata(tc_socket_path)?.permissions();
        perms.set_mode(0o777); // We do not really care, it is a socket
        fs::set_permissions(tc_socket_path, perms).unwrap();

        //      2. Send flow updates
        println!("Reporter {} connecting to tc socket", self.config.id);
        let mut stream = Unix::bind_addr(self.config.flow_socket.clone(), None).await.unwrap();
        stream.connect().await.unwrap();

        // Send a message that our socket is ready for connection
        println!("Reporter {} sending socket ready", self.config.id);
        stream.send(TCMessage::SocketReady).await.unwrap();

        self.listen_ebpf().await?;
        self.check_flows(stream).await?;
        Ok(())
    }

    async fn load_ebpf(interface: &str) -> Result<Loaded> {
        let mut raw_fds = Vec::new();
        let mut loaded = Loader::load(probe_code()).expect("error loading BPF program");
        //insert socket filter
        let mut attach_counts = 0;
        for sf in loaded.socket_filters_mut() {
            let sock_raw_fd = sf.attach_socket_filter(interface)?;
            attach_counts = attach_counts + 1;
            raw_fds.push(sock_raw_fd);
        }
        if attach_counts == 0 {
            Err(Error::NoIntAttached)
        } else {
            Ok(loaded)
        }
    }

    async fn listen_ebpf(&self) -> Result<()> {
        // launch tokio task to read the events coming from the ebpf program
        // need to take the loaded ebpf to pass it to the thread
        // Then load the ebpf file
        let usage = self.usage.clone();

        let network_int = self.config.network_interface.clone();
        println!("Listening on eBPF program");
        tokio::spawn(async move {

            let mut loaded = Self::load_ebpf(&*network_int).await.unwrap();
            while let Some((name, events)) = loaded.events.next().await {
                match name.as_str() {
                    "perf_events" => {
                        for event in events {
                            let message = unsafe { ptr::read(event.as_ptr() as *const Message) };
                            // put the update in the map
                            usage.insert(message.dst, (message.throughput, Instant::now()));
                        }
                    }
                    _ => {}
                }
            }
        });
        Ok(())
    }

    async fn check_flows(&self, mut stream: UnixBinding<TCMessage, Responder<TCMessage>>) -> Result<()> {
        let mut old_values: HashMap<u32, u32> = HashMap::new();
        let interval_check = Duration::from_millis(self.config.flow_control_interval);
        let kill_flow_duration = Duration::from_millis(self.config.kill_flow_duration_ms);
        loop {
            let mut to_remove = Vec::new();
            for entry in self.usage.iter() {
                if entry.val().0 <= self.config.ignore_threshold {
                    continue;
                }
                if entry.val().1.elapsed() > kill_flow_duration {
                    let dest = SocketAddr::new(entry.key().clone());
                    //println!("TERMINATION OF FLOW: {}", dest);
                    old_values.remove(entry.key());
                    // Add it into the to_remove and remove after to not create dead lock
                    to_remove.push(entry.key().clone());
                    // Send the information to the emulation
                    stream.send(FlowRemove(FlowConf::build(self.config.id, dest, None))).await.unwrap();
                    continue;
                } else { // updated recently, check if update regarding old value or a new flow
                    if !old_values.contains_key(entry.key()) {
                        let dest = SocketAddr::new(entry.key().clone());
                        //println!("NEW FLOW: \t {} \t THROUGHPUT {}", dest, entry.val().0);
                        old_values.insert(*entry.key(), entry.val().0);
                        // Send the information to the emulation
                        stream.send(FlowNew(FlowConf::build(self.config.id, dest, Some(entry.val().0)))).await.unwrap();
                    } else { // This is not a new value! check if it enters in our tolerance
                        // Check the percentage variation, only care if the variation stay within the 5%
                        let old = old_values.get(entry.key()).unwrap().clone() as f32;
                        let new = entry.val().0 as f32;

                        let percentage_variation = (old - new).abs() / old * 100f32;
                        if percentage_variation > self.config.percentage_variation {
                            let dest = SocketAddr::new(entry.key().clone());
                            //println!("UPDATE FLOW: \t {} \t OLD-NEW: {}-{}", dest, old, new);
                            old_values.insert(entry.key().clone(), entry.val().0.clone());
                            // Send the information to the emulation
                            stream.send(FlowUpdate(FlowConf::build(self.config.id, dest, Some(entry.val().0)))).await.unwrap();
                        }
                    }
                }
            }
            for re in to_remove.into_iter() {
                self.usage.remove(&re);
            }
            // Sleep a bit or we are going to use all the cpu processing
            sleep_until(Instant::now().add(interval_check)).await
        }
    }
}

//retrieve.elf file
fn probe_code() -> &'static [u8] {
    include_bytes!("usage.elf")
}
