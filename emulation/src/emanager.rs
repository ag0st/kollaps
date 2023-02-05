use std::collections::HashMap;
use std::net::IpAddr;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tokio::sync::mpsc::{Receiver, Sender};
use uuid::Uuid;

use cgraph::CGraphUpdate;
use common::{ClusterNodeInfo, EmulMessage, Error, ErrorKind, Result, RunnerConfig, Subnet, OManagerMessage};
use dockhelper::DockerHelper;
use nethelper::{ALL_ADDR, DefaultHandler, MessageWrapper, NoHandler, ProtoBinding, Protocol, TCP, TCPBinding};

use crate::data::{EManagerMessage, Emulation};
use crate::emulcore::EmulCore;
use crate::pubapi::OManager;

pub struct EManager {
    config: RunnerConfig,
    myself: ClusterNodeInfo,
    /// Every emulations will add themself and retire themself from this.
    emul_id_to_channel: Arc<Mutex<HashMap<Uuid, Sender<EmulMessage>>>>,
    emul_id_abort_chan: HashMap<Uuid, Sender<bool>>,
    emul_id_to_emulcore: Arc<Mutex<HashMap<Uuid, Pin<Box<EmulCore>>>>>,
    dock: DockerHelper,
    cluster_omanager: Option<ClusterNodeInfo>,
}

impl EManager {
    pub async fn build(config: RunnerConfig) -> Result<EManager> {
        // Parse my ip address
        let my_ip = match IpAddr::from_str(&*config.ip_address) {
            Ok(ip) => ip,
            Err(e) => return Err(Error::wrap("EMUL CTRL", ErrorKind::NotASocketAddr, "Cannot parse the IP address", e))
        };
        let gateway = IpAddr::from_str(&*config.gateway).unwrap();
        let subnet = Subnet::from(&*config.subnet);
        let dock = DockerHelper::init(&*config.interface, subnet, &*gateway.to_string()).await.unwrap();
        let myself = ClusterNodeInfo::new(my_ip, config.emanager_port);
        Ok(EManager {
            config,
            myself,
            emul_id_to_channel: Arc::new(Default::default()),
            emul_id_to_emulcore: Arc::new(Default::default()),
            emul_id_abort_chan: HashMap::new(),
            dock,
            cluster_omanager: None,
        })
    }

    pub async fn start(&mut self, mut cgraph_update_receiver: Option<Receiver<CGraphUpdate>>) -> Result<()> {
        // Create the TCP binding for new incoming messages
        let (sender, mut receiver) = mpsc::channel(1000);
        let handler = DefaultHandler::<EManagerMessage>::new(sender.clone());
        let mut tcp_binding = TCP::bind_addr((ALL_ADDR, self.config.emanager_port), Some(handler.clone())).await.unwrap();
        tcp_binding.listen().unwrap();

        // Start the acceptance of topology if I am the leader
        if self.config.leader {
            // wait for receiving the first cgraph
            let cgraph;
            // wait for the first CGraph to appear
            if let Some(CGraphUpdate::New(graph)) = cgraph_update_receiver.as_mut().unwrap().recv().await {
                cgraph = graph
            } else {
                panic!("Expected a CGraph update with the newly created CGraph")
            }

            // clones for moving into the tokio task
            let me = self.myself.clone();
            let ls = sender.clone();
            let emanagers_port = self.config.emanager_port;
            let omanager_port = self.config.omanager_port;
            let max_load = self.config.max_load_per_node;
            tokio::spawn(async move {
                OManager::build_and_start(
                    me,
                    ls,
                    cgraph_update_receiver.unwrap(),
                    omanager_port,
                    emanagers_port,
                    cgraph,
                    max_load,
                ).await
            });
        }

        // Now wait on incoming requests
        while let Some(message) = receiver.recv().await {
            match message {
                MessageWrapper { message: EManagerMessage::ExperimentNew((leader, emul)), sender: is_sender } => {
                    println!("[EManager] : Received a new experiment from local node");
                    // If we receive this message, we already know the leader, it is me! Mario!
                    if let None = self.cluster_omanager {
                        self.cluster_omanager = Some(leader);
                    }
                    // coming directly from me because I am the leader
                    self.create_emulation(emul, self.dock.clone(), self.cluster_omanager.as_ref().unwrap().clone()).await.unwrap();
                    if let Some(sender) = is_sender {
                        let _ = sender.send(None);
                    }
                }
                MessageWrapper { message: EManagerMessage::EmulCoreInterchange((id, mess)), sender: Some(sender) } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // It is possible that our emulcore for this experiment is already finished. So it may not be in the emul_id_to_channel anymore
                    if let Some(mess_sender) = self.emul_id_to_channel.lock().await.get(&id) {
                        mess_sender.send(mess).await.unwrap();
                    }
                    sender.send(None).unwrap();
                }

                MessageWrapper { message: EManagerMessage::ExperimentStop(id), sender: is_sender } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // It is possible that our emulcore for this experiment is already finished. So it may not be in the emul_id_to_channel anymore
                    if let Some(abort) = self.emul_id_abort_chan.get(&id) {
                        // If we succeed, it means that it has been stopped, else, it is already stopped / crashed
                        let _ = abort.send(true).await;
                    }
                    self.emul_id_abort_chan.remove_entry(&id);
                    if let Some(sender) = is_sender {
                        let _ = sender.send(None);
                    }
                }
                MessageWrapper { message: EManagerMessage::ExperimentReady(id), sender: is_sender } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // If we receive this message via direct send over our receive channel, we are the leader
                    let leader = self.cluster_omanager.as_ref().unwrap().clone();
                    // Create a channel to abort the emulation
                    let (tx, rx) = mpsc::channel(5);
                    // save the sender
                    self.emul_id_abort_chan.insert(id.clone(), tx);
                    self.start_emulation(id, self.dock.clone(), leader, rx).await.unwrap();
                    if let Some(sender) = is_sender {
                        let _ = sender.send(None);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn start_emulation(&self, uuid: Uuid, dock: DockerHelper, cluster_leader: ClusterNodeInfo, mut abort_channel: Receiver<bool>) -> Result<()> {
        let emul_id_to_emulcore = self.emul_id_to_emulcore.clone();
        tokio::spawn(async move {
            if let Err(error) = async {
                // take ownership of the emulcore
                let mut emulcore = emul_id_to_emulcore.lock().await.remove(&uuid).unwrap();
                tokio::select! {
                    res = async {
                        emulcore.as_mut().synchronize().await?;
                        emulcore.as_mut().flow_loop(dock).await?;
                        Ok::<_, Error>(())
                    } => {
                        res?
                    }
                    Some(_) = abort_channel.recv() => {
                        println!("[EManager] Emulation aborted")
                    }
                }
                Ok::<_, Error>(())
            }.await {
                // An error occurred, we need to notify the leader
                eprintln!("Error occurred when starting the emulation: {}", error);
                let mut bind: TCPBinding<OManagerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                bind.send_to(OManagerMessage::Abort(uuid.to_string()), cluster_leader).await.unwrap();
            }
        });
        Ok(())
    }

    async fn create_emulation(&mut self, emul: Emulation, dock: DockerHelper, cluster_omanager: ClusterNodeInfo) -> Result<()> {
        // Clone for the emulation for moving
        let me = self.myself.clone();
        let reporter_path = self.config.reporter_exec_path.clone();
        let emul_id_to_chan = self.emul_id_to_channel.clone();
        let emul_id = emul.uuid();
        let c_l = cluster_omanager.clone();
        let emul_id_to_emulcore = self.emul_id_to_emulcore.clone();
        tokio::spawn(async move {
            if let Err(error) = async {
                let mut emulcore = Box::pin(EmulCore::new(emul, me.clone(), emul_id_to_chan, c_l.clone(), dock.clone()));
                emulcore.as_mut().init_emulation(dock, reporter_path).await?;
                // Now we can tell the Kollaps cluster leader that we are ready and store the emulation core.
                emul_id_to_emulcore.lock().await.insert(emul_id.clone(), emulcore);
                println!("[EManager] : EmulCore {} is ready. Sending Ready to OManager: {}", emul_id, c_l);
                TCP::bind(Some(NoHandler)).await?.send_to(OManagerMessage::EmulationReady((emul_id.to_string(), me)), c_l).await?;
                Ok::<_, Error>(())
            }.await {
                // An error occurred, we need to notify the leader
                eprintln!("Error occured when instatiating the emulation: {}", error);
                let mut bind: TCPBinding<OManagerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                bind.send_to(OManagerMessage::Abort(emul_id.to_string()), cluster_omanager).await.unwrap();
            }
        });
        Ok(())
    }
}

