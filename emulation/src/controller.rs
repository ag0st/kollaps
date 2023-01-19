use std::collections::HashMap;
use std::net::IpAddr;
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
    dock: DockerHelper,
}

impl Controller {
    pub async fn build(config: RunnerConfig) -> Result<Controller> {
        // Parse my ip address
        let my_ip = match IpAddr::from_str(&*config.ip_address) {
            Ok(ip) => ip,
            Err(e) => return Err(Error::wrap("CMANAGER CTRL", ErrorKind::NotASocketAddr, "Cannot parse the IP address", e))
        };
        let gateway = IpAddr::from_str(&*config.gateway).unwrap();
        let subnet = Subnet::from(&*config.subnet);
        let dock = DockerHelper::init(&*config.interface, subnet, &*gateway.to_string()).await.unwrap();
        let myself = ClusterNodeInfo::new(my_ip, config.emulation_event_port);
        Ok(Controller {
            config,
            myself,
            emul_id_to_channel: Arc::new(Default::default()),
            dock,
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
                    self.start_emulation(emul, self.dock.clone(), leader).await.unwrap();
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulationStart(leader, emul), sender: None } => {
                    // coming directly from me because I am the leader
                    self.start_emulation(emul, self.dock.clone(), leader).await.unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulCoreInterchange(id, mess), sender: Some(sender) } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    if let Some(mess_sender) = self.emul_id_to_channel.lock().await.get(&id) {
                        mess_sender.send(mess).await.unwrap();
                    }
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulationStop(id), sender: Some(sender) } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    if let Some(mess_sender) = self.emul_id_to_channel.lock().await.get(&id) {
                        mess_sender.send(EmulMessage::EmulAbort).await.unwrap();
                    }
                    sender.send(None).unwrap();
                }
                MessageWrapper { message: ControllerMessage::EmulationStop(id), sender: None } => {
                    let id = Uuid::parse_str(&*id).unwrap();
                    if let Some(mess_sender) = self.emul_id_to_channel.lock().await.get(&id) {
                        mess_sender.send(EmulMessage::EmulAbort).await.unwrap();
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn start_emulation(&mut self, emul: Emulation, dock: DockerHelper, leader: ClusterNodeInfo) -> Result<()> {
        // Clone for the emulation for moving
        let me = self.myself.clone();
        let reporter_path = self.config.reporter_exec_path.clone();
        let emul_id_to_chan = self.emul_id_to_channel.clone();
        let emul_id = emul.uuid();
        tokio::spawn(async move {
            if let Err(e) = async {
                let mut emulcore = Box::pin(EmulCore::new(emul, me, emul_id_to_chan));
                let d = dock.clone();
                emulcore.as_mut().start_emulation(d, reporter_path).await?;
                // Now, the emulcores must synchronize between themself
                emulcore.as_mut().synchronize(Duration::from_secs(2)).await?;
                emulcore.as_mut().flow_loop(dock).await?;
                Ok::<_, Error>(())
            }.await {
                // An error occurred, we need to notify the leader
                let mut bind: TCPBinding<TopologyMessage, NoHandler> = TCP::bind(None).await.unwrap();
                bind.send_to(TopologyMessage::Abort(emul_id.to_string()), leader).await.unwrap();
            }
        });
        Ok(())
    }
}

