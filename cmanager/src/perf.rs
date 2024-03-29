use common::{ClusterNodeInfo, Error, ErrorKind, Result, ToBytesSerialize, ToSocketAddr};
use std::sync::Arc;

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot};

use crate::data::{Event};
use nethelper::{handler_once_box, ProtoBinding, Protocol, Responder, TCP, TCPBinding};

pub struct PerfCtrl {
    iperf3_info: ClusterNodeInfo,
    test_mutex: Arc<Mutex<bool>>,
    test_duration: u8,
}

impl PerfCtrl {
    pub async fn new(iperf3_info: ClusterNodeInfo, test_duration: u8) -> PerfCtrl {
        PerfCtrl {
            iperf3_info,
            test_mutex: Arc::new(Mutex::new(false)),
            test_duration,
        }
    }

    pub async fn launch_server(&mut self) -> Result<()> {
        // Get the lock to be sure we are the only one making a test
        let _ = self.test_mutex.lock().await;

        // Spawn the child process (start the server)
        Command::new("iperf3")
            .arg(format!("-p {}", self.iperf3_info.port)) // on port given by the server
            .arg("-s") // Server mode
            .arg("-1") // one time test
            .spawn()?;

        Ok(())
    }

    pub async fn launch_test(&mut self, addr: impl ToSocketAddr) -> Result<usize> {
        // Get the lock to be sure we are the only one making a test
        let _ = self.test_mutex.lock().await;

        let mut binding: TCPBinding<Event, Responder<Event>> = TCP::bind(None).await?;

        // connect to the other host
        binding.connect(addr).await?;

        // get my address and his
        let (_, target) = binding.peers_addr()?;

        // Prepare the request for starting the test
        let client_perf_event = Event::PClient;

        // Send the request to the other guy
        binding.send(client_perf_event).await?;

        // Wait on the receive, when received, create a new tokio task that will execute the test,
        // and wait on the test to finish via channel.
        let (tx, rx) = oneshot::channel::<f64>();

        // copy duration to be used into the closure
        let test_duration = self.test_duration.clone();

        // receive only one response
        binding.receive_once(handler_once_box(move |buf| async move {
            let event = Event::from_bytes(buf).unwrap();
            let other = match event {
                Event::PServer(info) => info,
                _ => {
                    println!("Expected PServer response, got another thing");
                    return None;
                }
            };
            tokio::spawn(async move {
                match async {
                    let command = Command::new("iperf3")
                        .arg(format!("-p {}", other.port)) // on port given by the server
                        .arg("-J") // Json format output
                        .arg("-c")// client mode
                        .arg(target.ip().to_string())// target
                        .arg(format!("-t {}", test_duration))
                        .output();
                    let output = command.await?;
                    let output = String::from_utf8_lossy(&output.stdout).to_string();
                    let output = output.replace("\t", "").replace("\n", "");
                    let result: Value = serde_json::from_str(&*output)?;
                    let sender_bps = result["end"]["sum_sent"]["bits_per_second"]
                        .as_f64()
                        .ok_or(Error::new("perf", ErrorKind::InvalidData, "cannot take the bits/sec result"))?;
                    let receiver_bps = result["end"]["sum_received"]["bits_per_second"]
                        .as_f64()
                        .ok_or(Error::new("perf", ErrorKind::InvalidData, "cannot take the bits/sec result"))?;

                    let bandwidth = f64::min(sender_bps, receiver_bps);
                    Ok::<f64, Error>(bandwidth)
                }.await {
                    Ok(bandwidth) => tx.send(bandwidth).unwrap(),
                    Err(e) => {
                        eprintln!("[PERF]: Error being the client: {}", e);
                        drop(tx);
                    }
                }
            });
            None
        })).await?;

        let res = match rx.await {
            Ok(bps) => {
                // transform from bits per second to Mb/sec
                let kbps = (bps as u64) >> 10;
                Ok(kbps as usize)
            }
            Err(e) => Err(Error::wrap("perf", ErrorKind::CommandFailed, "error launching the perf test", e))
        };
        // The mutex is dropped, ready for next test
        res
    }
}