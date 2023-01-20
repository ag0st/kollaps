use std::{fs, ptr};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::ops::Add;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::BytesMut;
use futures::StreamExt;
use lockfree::map::Map;
use redbpf::load::{Loaded, Loader};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use common::{FlowConf, ReporterConfig, EmulMessage, ToU32IpAddr, ToBytesSerialize};
use common::EmulMessage::{FlowUpdate};
use error::{Error, Result};
use nethelper::{Handler, ProtoBinding, Protocol, Responder, Unix, UnixBinding};

use monitor::usage::Message;
use monitor::usage::MonitorIpAddr;
use crate::tc_manager::TCManager;

pub mod error;
mod tc_manager;
mod data;

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
impl Handler<EmulMessage> for TCHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<EmulMessage> {
        if let Ok(mess) = EmulMessage::from_bytes(bytes) {
            match mess {
                EmulMessage::TCInit(conf) => {
                    let dest_u32 = conf.dest.to_u32().unwrap().to_be();
                    self.manager.lock().ok()?.init(dest_u32)
                },
                EmulMessage::TCUpdate(conf) => {
                    let dest_u32 = conf.dest.to_u32().unwrap().to_be();
                    let manager = self.manager.lock().ok()?;
                    if self.initialized_path.lock().ok()?.contains(&dest_u32) {
                        if let Some(bw) = conf.bandwidth_kbitps {
                            manager.change_bandwidth(dest_u32, bw);
                        }
                        if let Some((lat, jit)) = conf.latency_and_jitter {
                            manager.change_latency(dest_u32, lat, jit);
                        }
                        if let Some(drop) = conf.drop {
                            manager.change_loss(dest_u32, drop);
                        }
                    } else {
                        manager
                            .initialize_path(
                                dest_u32,
                                conf.bandwidth_kbitps.unwrap(),
                                conf.latency_and_jitter.unwrap().0,
                                conf.latency_and_jitter.unwrap().1,
                                conf.drop.unwrap());
                        self.initialized_path.lock().ok()?.insert(dest_u32);
                    }
                }
                EmulMessage::TCDisconnect => self.manager.lock().ok()?.disconnect(),
                EmulMessage::TCReconnect => self.manager.lock().ok()?.reconnect(),
                EmulMessage::TCTeardown => {
                    // need to destroy the socket
                    self.clean_sender.send(()).await.unwrap();
                    self.manager.lock().ok()?.tear_down();
                }
                _ => { eprintln!("[REPORTER]: Received a FLOW message via tc_socket. Issue is happening") }
            }
        } else {
            eprintln!("[REPORTER]: Received unrecognized message from the Emulation")
        }
        None
    }
}

struct Cleaner {
    receiver: mpsc::Receiver<()>,
    listener: UnixBinding<EmulMessage, TCHandler>,
    ip: IpAddr,
}

impl Cleaner {
    fn build(receiver: mpsc::Receiver<()>, listener: UnixBinding<EmulMessage, TCHandler>, ip: IpAddr) -> Cleaner {
        Cleaner {
            receiver,
            listener,
            ip,
        }
    }
    async fn wait_to_clean(mut self) {
        self.receiver.recv().await;
        println!("[REPORTER: {}] - Stopping and cleaning...", self.ip.to_string());
        // no really need to to it but it is just to show what we are doing.
        drop(self.listener)
    }
}

pub struct UsageAnalyzer {
    /// This field is an option because it allow to be taken by the listener process.
    usage: Arc<Map<u32, (u32, Instant)>>,
    config: ReporterConfig,
    my_ip: IpAddr,
}

impl UsageAnalyzer {
    pub async fn build(config: &ReporterConfig) -> Result<UsageAnalyzer> {
        // parse the ip addr
        let my_ip = IpAddr::from_str(&*config.ip.clone()).unwrap();
        Ok(UsageAnalyzer {
            usage: Arc::new(Map::new()),
            config: config.clone(),
            my_ip
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        // create cleaner channel
        let (sender, receiver) = mpsc::channel(1);

        // Connecting to the socket for receiving traffic control commands from the main app.
        let mut tc_command_binding =
            Unix::bind_addr(self.config.tc_socket.clone(), Some(TCHandler::new(sender)))
                .await.unwrap();
        tc_command_binding.listen().unwrap();

        // Create the cleaner, it will hold the socket and drop it (deleting it) when receiving Teardown
        // event through the tc_command_socket.
        let cleaner = Cleaner::build(receiver, tc_command_binding, self.my_ip);
        // launch the cleaner in another thread, it will just wait
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

        // Connect to the socket where we will give flow update.
        let mut flow_socket_stream =
            Unix::bind_addr(self.config.flow_socket.clone(), None).await.unwrap();
        flow_socket_stream.connect().await.unwrap();

        // Send a message that our socket is ready for connection
        flow_socket_stream.send(EmulMessage::SocketReady).await.unwrap();

        self.listen_ebpf().await?;
        self.check_flows(flow_socket_stream).await?;
        Ok(())
    }

    /// Load the ebpf file and attach it to the interface given.
    /// If there is a problem with loading OR no interface has been attached, it results in an Error.
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

    /// Listen on the ebpf program and insert new value into the
    /// self.usage map.
    async fn listen_ebpf(&self) -> Result<()> {
        let usage = self.usage.clone();

        // launch tokio task to read the events coming from the ebpf program
        let network_int = self.config.network_interface.clone();
        let my_ip = self.my_ip.clone();
        tokio::spawn(async move {
            let mut loaded = Self::load_ebpf(&*network_int).await.unwrap();
            while let Some((name, events)) = loaded.events.next().await {
                match name.as_str() {
                    "perf_events" => {
                        for event in events {
                            let message = unsafe { ptr::read(event.as_ptr() as *const Message) };
                            // put the update in the map
                            // Check if it is me, if yes ignore
                            let dst = to_ip_addr_u32(message.dst.clone());
                            if dst == my_ip {
                                continue
                            }
                            usage.insert(message.dst, (message.throughput, Instant::now()));
                        }
                    }
                    _ => {}
                }
            }
        });
        Ok(())
    }

    /// The check flow method is the main loop of the program.
    /// It reads the value into the self.usage and updates the main program via FlowSocket.
    async fn check_flows(&self, mut stream: UnixBinding<EmulMessage, Responder<EmulMessage>>) -> Result<()> {
        println!("[REPORTER: {}] is ready, listening for flows changes", self.my_ip);

        // used to store the old values of the flows
        let mut old_values: HashMap<u32, u32> = HashMap::new();
        // The interval of going through the self.usage map and check for the flow.
        let interval_check = Duration::from_millis(self.config.flow_control_interval);
        // Duration after which a flow that has not been updated will be considered as no more.
        let kill_flow_duration = Duration::from_millis(self.config.kill_flow_duration_ms);
        // Main loop
        loop {
            // Store the entries to remove in another vector. It prevents from deadlock with the map.
            let mut to_remove = Vec::new();
            // Going through the usage.
            for entry in self.usage.iter() {
                if entry.val().0 <= self.config.ignore_threshold {
                    continue;
                }
                if entry.val().1.elapsed() > kill_flow_duration {
                    let dest = MonitorIpAddr::new(entry.key().clone());
                    old_values.remove(entry.key());
                    // Add it into the to_remove and remove after to not create dead lock
                    to_remove.push(entry.key().clone());
                    // Send the information to the emulation
                    let dest = to_ip_addr(dest);
                    stream.send(FlowUpdate(FlowConf::build(self.my_ip, dest, None))).await.unwrap();
                    continue;
                } else { // updated recently, check if update regarding old value or a new flow
                    let dest = MonitorIpAddr::new(entry.key().clone());
                    let mut percentage_variation = 100.0;
                    if old_values.contains_key(entry.key()) { // This is not a new value! check if it enters in our tolerance
                        // Check the percentage variation, only care if the variation stay within the 5%
                        let old = old_values.get(entry.key()).unwrap().clone() as f32;
                        let new = entry.val().0 as f32;
                        percentage_variation = (old - new).abs() / old * 100f32;
                    }
                    // Check if it is an enough good variation to notify the other app,
                    // new flows will have by default 100% variation
                    if percentage_variation >= self.config.percentage_variation {
                        old_values.insert(entry.key().clone(), entry.val().0.clone());
                        // Send the information to the emulation
                        let dest = to_ip_addr(dest);
                        stream.send(FlowUpdate(FlowConf::build(self.my_ip, dest, Some(entry.val().0)))).await.unwrap();
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

/// retrieve the usage.elf file and return the bytes of the file.
fn probe_code() -> &'static [u8] {
    include_bytes!("usage.elf")
}

fn to_ip_addr(mia: MonitorIpAddr) -> IpAddr {
    to_ip_addr_u32(mia.addr)
}


fn to_ip_addr_u32(mia: u32) -> IpAddr {
    let octets = mia.to_be_bytes();
    IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]))
}