use std::collections::HashMap;
use std::path::Path;
use std::{io, ptr};
use std::io::ErrorKind;
use std::sync::{Arc};
use std::thread::sleep;
use std::time::Duration;
use futures::StreamExt;
use lockfree::map::Map;

use redbpf::load::{Loaded, Loader};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixStream};

use tokio::time::Instant;

use error::{Error, Result};
use common::{Message, SocketAddr};
use smart_socket::SmartSocketListener;

pub mod error;
mod tc_manager;
mod smart_socket;

pub struct UsageAnalyzer {
    loaded: Option<Loaded>,
    usage: Arc<Map<u32, (u32, Instant)>>,
    stream: Option<UnixStream>,
    percentage_var: f32,
    ignore_threshold: u32,
    kill_flow_duration: Duration,
}

impl UsageAnalyzer {
    pub async fn build(socket_addr: &'static str, interface: &str) -> Result<UsageAnalyzer> {

        // First, connect to the Socket
        let socket_path = Path::new(socket_addr);
        let b = SmartSocketListener::bind(&socket_path)?;
        //let mut stream = UnixStream::connect(&socket_path).await?;

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = b.accept().await {
                tokio::spawn(async move {
                    'main_loop : loop {
                        match stream.read_u16().await {
                            Ok(content_length) => {
                                let mut buf = vec![0u8; content_length as usize];
                                stream.read_exact(&mut buf).await.unwrap();
                                let s = String::from_utf8(buf).unwrap();
                                println!("He said: {}", s);
                            }
                            Err(e) =>  match e.kind() {
                                ErrorKind::UnexpectedEof => {
                                    println!("Connection closed by the other guy");
                                    break 'main_loop
                                }
                                _ => {
                                    eprintln!("An error appeared while reading on the unix socket.");
                                    break 'main_loop
                                }
                            }
                        }
                    }
                });
            }
        });

        for i in 0..5 {
            let p = socket_path.clone();
            tokio::spawn(async move {
                let mut stream = UnixStream::connect(p).await.unwrap();
                for j in 0..5 {
                    let text = format!("I am {} - {}", i, j);
                    stream.write_u16(text.as_bytes().len() as u16).await.unwrap();
                    stream.write_all(text.as_bytes()).await.unwrap();
                    sleep(Duration::from_secs(1));
                }
            });
        }

        // Then load the ebpf file
        let loaded = Self::load_ebpf(interface).await?;

        Ok(UsageAnalyzer {
            loaded: Some(loaded),
            usage: Arc::new(Map::new()),
            stream: None,
            percentage_var: 5f32,
            ignore_threshold: 5,
            kill_flow_duration: Duration::from_secs(2),
        })
    }

    pub async fn start(&mut self) {
        // launch tokio task to read the events coming from the ebpf program
        // need to take the loaded ebpf to pass it to the thread
        let mut loaded = self.loaded.take().unwrap();
        let mut usage = self.usage.clone();
        tokio::spawn(async move {
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
        let mut old_values: HashMap<u32, u32> = HashMap::new();
        loop {
            let mut to_remove = Vec::new();
            for entry in self.usage.iter() {
                if entry.val().0 <= self.ignore_threshold {
                    continue;
                }
                if entry.val().1.elapsed() > self.kill_flow_duration {
                    println!("TERMINATION OF FLOW: {}", SocketAddr::new(entry.key().clone()));
                    old_values.remove(entry.key());
                    // Add it into the to_remove and remove after to not create dead lock
                    to_remove.push(entry.key().clone());
                    continue;
                } else { // updated recently, check if update regarding old value or a new flow
                    if !old_values.contains_key(entry.key()) {
                        println!("NEW FLOW: \t {} \t THROUGHPUT {}", SocketAddr::new(entry.key().clone()), entry.val().0);
                        old_values.insert(*entry.key(), entry.val().0);
                    } else { // This is not a new value! check if it enters in our tolerance
                        // Check the percentage variation, only care if the variation stay within the 5%
                        let old = old_values.get(entry.key()).unwrap().clone() as f32;
                        let new = entry.val().0 as f32;

                        let percentage_variation = (old - new).abs() / old * 100f32;
                        if percentage_variation > self.percentage_var {
                            println!("UPDATE FLOW: \t {} \t OLD-NEW: {}-{}", SocketAddr::new(entry.key().clone()), old, new);
                            old_values.insert(entry.key().clone(), entry.val().0.clone());
                        }
                    }
                }
            }
            for re in to_remove.into_iter() {
                self.usage.remove(&re);
            }
        }
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
}

//retrieve.elf file
fn probe_code() -> &'static [u8] {
    include_bytes!("usage.elf")
}
