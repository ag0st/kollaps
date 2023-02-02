use std::collections::HashMap;
use std::net::IpAddr;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};
use tokio::sync::mpsc::{Receiver, Sender};
use uuid::Uuid;

use cgraph::CGraphUpdate;
use common::{ClusterNodeInfo, EmulMessage, Error, ErrorKind, Result, RunnerConfig, Subnet, TopologyMessage};
use dockhelper::DockerHelper;
use nethelper::{ALL_ADDR, DefaultHandler, MessageWrapper, NoHandler, ProtoBinding, Protocol, TCP, TCPBinding};

use crate::data::{ControllerMessage, Emulation};
use crate::emulcore::EmulCore;
use crate::pubapi::OrchestrationManager;

pub struct Controller {
    config: RunnerConfig,
    myself: ClusterNodeInfo,
    /// Every emulations will add themself and retire themself from this.
    emul_id_to_channel: Arc<Mutex<HashMap<Uuid, Sender<EmulMessage>>>>,
    emul_id_to_emulcore: Arc<Mutex<HashMap<Uuid, Pin<Box<EmulCore>>>>>,
    dock: DockerHelper,
    cluster_leader: Option<ClusterNodeInfo>,
}

impl Controller {
    pub async fn build(config: RunnerConfig) -> Result<Controller> {
        // Parse my ip address
        let my_ip = match IpAddr::from_str(&*config.ip_address) {
            Ok(ip) => ip,
            Err(e) => return Err(Error::wrap("EMUL CTRL", ErrorKind::NotASocketAddr, "Cannot parse the IP address", e))
        };
        let gateway = IpAddr::from_str(&*config.gateway).unwrap();
        let subnet = Subnet::from(&*config.subnet);
        let dock = DockerHelper::init(&*config.interface, subnet, &*gateway.to_string()).await.unwrap();
        let myself = ClusterNodeInfo::new(my_ip, config.emulation_event_port);
        Ok(Controller {
            config,
            myself,
            emul_id_to_channel: Arc::new(Default::default()),
            emul_id_to_emulcore: Arc::new(Default::default()),
            dock,
            cluster_leader: None,
        })
    }

    pub async fn start(&mut self, mut cgraph_update_receiver: Option<Receiver<CGraphUpdate>>) -> Result<()> {
        // Create the TCP binding for new incoming messages
        let (sender, mut receiver) = mpsc::channel(1000);
        let handler = DefaultHandler::<ControllerMessage>::new(sender.clone());
        let mut tcp_binding = TCP::bind_addr((ALL_ADDR, self.config.emulation_event_port), Some(handler.clone())).await.unwrap();
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
            let controllers_port = self.config.emulation_event_port;
            let connections_port = self.config.topology_submission_port;
            self.cluster_leader = Some(me.clone());
            tokio::spawn(async move {
                OrchestrationManager::build_and_start(
                    me,
                    ls,
                    cgraph_update_receiver.unwrap(),
                    connections_port,
                    controllers_port,
                    cgraph).await
            });
        }

        // Now wait on incoming requests
        while let Some(message) = receiver.recv().await {
            match message {
                MessageWrapper { message: ControllerMessage::EmulationStart(leader, emul), sender: Some(sender) } => {
                    // We can store the cluster leader for next requests. The Emulation Start is the first message that we must receive.
                    if let None = self.cluster_leader {
                        self.cluster_leader = Some(leader);
                    }
                    self.create_emulation(emul, self.dock.clone(), self.cluster_leader.as_ref().unwrap().clone()).await.unwrap();
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulationStart(_, emul), sender: None } => {
                    // If we receive this message, we already know the leader, it is me! Mario!

                    // coming directly from me because I am the leader
                    self.create_emulation(emul, self.dock.clone(), self.cluster_leader.as_ref().unwrap().clone()).await.unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulCoreInterchange(id, mess), sender: Some(sender) } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // It is possible that our emulcore for this experiment is already finished. So it may not be in the emul_id_to_channel anymore
                    if let Some(mess_sender) = self.emul_id_to_channel.lock().await.get(&id) {
                        mess_sender.send(mess).await.unwrap();
                    }
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulationStop(id), sender: Some(sender) } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // It is possible that our emulcore for this experiment is already finished. So it may not be in the emul_id_to_channel anymore
                    self.emul_id_to_emulcore.lock().await.remove_entry(&id);
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulationStop(id), sender: None } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // It is possible that our emulcore for this experiment is already finished. So it may not be in the emul_id_to_channel anymore
                    self.emul_id_to_emulcore.lock().await.remove_entry(&id);
                }
                MessageWrapper { message: ControllerMessage::ExperimentReady(id), sender: Some(sender) } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // We can safely get the registered cluster leader from our attribute, this kind of message always comes after we receive a
                    // EmulationStart where we store the leader.
                    let leader = self.cluster_leader.as_ref().unwrap().clone();
                    self.start_emulation(id, self.dock.clone(), leader).await.unwrap();
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::ExperimentReady(id), sender: None } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    // If we receive this message via direct send over our receive channel, we are the leader
                    let leader = self.cluster_leader.as_ref().unwrap().clone();
                    self.start_emulation(id, self.dock.clone(), leader).await.unwrap();
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn start_emulation(&self, uuid: Uuid, dock: DockerHelper, cluster_leader: ClusterNodeInfo) -> Result<()> {
        let emul_id_to_emulcore = self.emul_id_to_emulcore.clone();
        tokio::spawn(async move {
            if let Err(error) = async {
                // take ownership of the emulcore
                let mut emulcore = emul_id_to_emulcore.lock().await.remove(&uuid).unwrap();
                // Now, the emulcores must synchronize between themself
                emulcore.as_mut().synchronize(Duration::from_secs(5)).await?;
                emulcore.as_mut().flow_loop(dock).await?;

                Ok::<_, Error>(())
            }.await {
                // An error occurred, we need to notify the leader
                eprintln!("Error occurred when starting the emulation: {}", error);
                let mut bind: TCPBinding<TopologyMessage, NoHandler> = TCP::bind(None).await.unwrap();
                bind.send_to(TopologyMessage::Abort(uuid.to_string()), cluster_leader).await.unwrap();
            }
        });
        Ok(())
    }

    async fn create_emulation(&mut self, emul: Emulation, dock: DockerHelper, cluster_leader: ClusterNodeInfo) -> Result<()> {
        // Clone for the emulation for moving
        let me = self.myself.clone();
        let reporter_path = self.config.reporter_exec_path.clone();
        let emul_id_to_chan = self.emul_id_to_channel.clone();
        let emul_id = emul.uuid();
        let c_l = cluster_leader.clone();
        let emul_id_to_emulcore = self.emul_id_to_emulcore.clone();
        tokio::spawn(async move {
            if let Err(error) = async {
                let mut emulcore = Box::pin(EmulCore::new(emul, me.clone(), emul_id_to_chan, c_l.clone(), dock.clone()));
                emulcore.as_mut().init_emulation(dock, reporter_path).await?;
                // Now we can tell the Kollaps cluster leader that we are ready and store the emulation core.
                emul_id_to_emulcore.lock().await.insert(emul_id.clone(), emulcore);
                TCP::bind(Some(NoHandler)).await?.send_to(TopologyMessage::EmulationReady((emul_id.to_string(), me)), c_l).await?;
                Ok::<_, Error>(())
            }.await {
                // An error occurred, we need to notify the leader
                eprintln!("Error occured when instatiating the emulation: {}", error);
                let mut bind: TCPBinding<TopologyMessage, NoHandler> = TCP::bind(None).await.unwrap();
                bind.send_to(TopologyMessage::Abort(emul_id.to_string()), cluster_leader).await.unwrap();
            }
        });
        Ok(())
    }
}

