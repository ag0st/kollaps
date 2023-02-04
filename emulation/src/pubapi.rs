use std::collections::HashMap;

use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use uuid::Uuid;

use cgraph::{CGraph, CGraphUpdate};
use common::{ClusterNodeInfo, EmulationEvent, Error, ErrorKind, OManagerMessage, Result, TopologyRejectReason};
use netgraph::Network;
use nethelper::{ALL_ADDR, DefaultHandler, MessageWrapper, NoHandler, ProtoBinding, Protocol, TCP, TCPBinding, UDP};

use crate::data::{EManagerMessage, Emulation, Node};
use crate::orchestrator::Orchestrator;
use crate::xmlgraphparser::parse_topology;

pub struct OManager {
    my_info_emanager: ClusterNodeInfo,
    my_info_omanager: ClusterNodeInfo,
    local_sender: Sender<MessageWrapper<EManagerMessage>>,
    orchestrator: Orchestrator,
    cgraph_update_receiver: Receiver<CGraphUpdate>,
    emulations_and_their_nodes: HashMap<Uuid, HashMap<ClusterNodeInfo, bool>>,
}

impl OManager {
    pub async fn build_and_start(my_info: ClusterNodeInfo, local_sender: Sender<MessageWrapper<EManagerMessage>>,
                                 cgraph_update_receiver: Receiver<CGraphUpdate>, omanager_port: u16,
                                 emanager_port: u16, original_cgraph: CGraph<ClusterNodeInfo>, max_load_per_node: usize)
                                 -> Result<()> {
        let mut my_info_emanager = my_info.clone();
        my_info_emanager.port = emanager_port;

        let mut my_info_omanager = my_info.clone();
        my_info_omanager.port = omanager_port;

        let manager = OManager {
            my_info_emanager,
            my_info_omanager,
            local_sender,
            orchestrator: Orchestrator::new(original_cgraph, emanager_port, max_load_per_node),
            cgraph_update_receiver,
            emulations_and_their_nodes: HashMap::new(),
        };
        manager.accept_topology().await
    }


    async fn accept_topology(mut self) -> Result<()> {
        let (sender, mut receiver) = mpsc::channel(10);
        // Create the handler
        let handler = DefaultHandler::<OManagerMessage>::new(sender.clone());

        // listen on TCP
        let mut udp_binding = UDP::bind_addr((ALL_ADDR, self.my_info_omanager.port), Some(handler.clone())).await.unwrap();
        let mut binding = TCP::bind_addr((ALL_ADDR, self.my_info_omanager.port), Some(handler)).await.unwrap();
        binding.listen().unwrap();
        udp_binding.listen().unwrap();

        loop {
            tokio::select! {
            Some(message) = receiver.recv() => {
                self.handle_experiment_message(message).await;
            }
            Some(update) = self.cgraph_update_receiver.recv() => {
                match update {
                    CGraphUpdate::New(graph) => {
                            let affected_uuids = self.orchestrator.update_with_cgraph(graph).unwrap();
                            self.abort_emulations(affected_uuids).await;
                    }
                    CGraphUpdate::Remove(node) => {
                            let affected_uuid = self.orchestrator.remove_cluster_node(node).unwrap();
                            self.abort_emulations(affected_uuid).await;
                    },
                }
            }
            }
        }
    }

    async fn abort_emulations(&mut self, uuids: Vec<Uuid>) {
        for uuid in uuids {
            let mess = EManagerMessage::ExperimentStop(uuid.to_string());
            for (node, _) in self.emulations_and_their_nodes.get(&uuid).unwrap() {
                // Send an abort for this
                let m = mess.clone();
                if self.my_info_emanager.eq(node) {
                    self.local_sender.send(MessageWrapper { message: m, sender: None }).await
                        .expect(&*format!("[WARNING]: Wanted to abort emulation {} on local machine, error", uuid.to_string()));
                } else {
                    let mut binding: TCPBinding<EManagerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                    let n = node.clone();
                    if let Err(_) = binding.send_to(m, n).await {
                        println!("[WARNING]: Wanted to abort emulation {} on node {}, node unreachable", uuid.to_string(), node);
                    }
                }
            }
            self.emulations_and_their_nodes.remove_entry(&uuid);
        }
    }

    async fn handle_experiment_message(&mut self, message: MessageWrapper<OManagerMessage>) {
        match message {
            MessageWrapper { message: OManagerMessage::NewTopology(top), sender: Some(sender) } => {
                println!("[OManager] : New Topology");
                let uuid = Uuid::new_v4();
                match parse_topology(top, uuid.clone()) {
                    Ok((network, events)) => {
                        match self.orchestrator.new_emulation(network, uuid.clone()) {
                            Ok(possible_net) => {
                                match possible_net {
                                    None => {
                                        println!("[OManager] : Topology Rejected: {}", TopologyRejectReason::NoDeploymentFound);
                                        sender.send(Some(OManagerMessage::Rejected(TopologyRejectReason::NoDeploymentFound))).unwrap();
                                    },
                                    Some((network, cluster_nodes_affected)) => {
                                        let id = uuid.clone();
                                        // Save the emulation
                                        let nodes_ready_map = (&cluster_nodes_affected).iter().fold(HashMap::new(), |mut acc, n| {
                                            acc.insert(n.clone(), false);
                                            acc
                                        });
                                        self.emulations_and_their_nodes.insert(id, nodes_ready_map);
                                        match self.send_new_emulation(id, network, cluster_nodes_affected, events).await {
                                            Ok(_) => {
                                                println!("[OManager] : Topology Accepted");
                                                sender.send(Some(OManagerMessage::Accepted)).unwrap();
                                            }
                                            Err(_) => {
                                                println!("[OManager] : Error sending new topology to EManagers");
                                                sender.send(Some(OManagerMessage::Rejected(TopologyRejectReason::NoDeploymentFound))).unwrap()
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[OManager]: Error when finding a dispatch for the topology: {}", e);
                                sender.send(Some(OManagerMessage::Rejected(TopologyRejectReason::NoDeploymentFound))).unwrap();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[OManager]: Cannot parse {}", e);
                        sender.send(Some(OManagerMessage::Rejected(TopologyRejectReason::BadFile(e.to_string())))).unwrap();
                    }
                }
            }
            MessageWrapper { message: OManagerMessage::Abort(id), sender: Some(sender) } => {
                // We receive an abort, we need to stop this emulation
                let id = Uuid::parse_str(&*id).unwrap();
                let uuids = vec![id];
                self.abort_emulations(uuids).await;
                sender.send(None).unwrap();
            }
            MessageWrapper { message: OManagerMessage::CleanStop((id, node, app_index)), sender: Some(sender) } => {
                // We receive an abort, we need to stop this emulation
                let id = Uuid::parse_str(&*id).unwrap();
                if let Err(e) = self.orchestrator.stop_app_emulation_on(&id, &node, app_index) {
                    eprintln!("[WARNING] Cannot clean stop the emulation {} on node {} with app_index: {}. Reason: {}", id.to_string(), node, app_index, e);
                }
                sender.send(None).unwrap();
            }
            MessageWrapper { message: OManagerMessage::EmulationReady((id, node)), sender: Some(sender) } => {
                // We receive an abort, we need to stop this emulation
                let id = Uuid::parse_str(&*id).unwrap();
                *self.emulations_and_their_nodes.get_mut(&id).unwrap().get_mut(&node).unwrap() = true;
                // Check if everybody for this emulation is ready, if yes, send start signal
                let mut ready = true;
                for (_, r) in self.emulations_and_their_nodes.get(&id).unwrap() {
                    if !r {
                        ready = false;
                        break;
                    }
                }
                if ready {
                    // Send to all that the emulation is ready
                    let mess = EManagerMessage::ExperimentReady(id.to_string());

                    for (node, _) in self.emulations_and_their_nodes.get(&id).unwrap() {
                        let m = mess.clone();
                        if self.my_info_emanager.eq(node) {
                            self.local_sender.send(MessageWrapper { message: m, sender: None }).await
                                .expect(&*format!("[WARNING]: Wanted to say emulation ready {} on local machine, error", id.to_string()));
                        } else {
                            let mut binding: TCPBinding<EManagerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                            let n = node.clone();
                            if let Err(_) = binding.send_to(m, n).await {
                                // If a host is not reachable, we need to abort directly this emulation
                                println!("[WARNING]: Wanted to say emulation ready {} on node {}, node unreachable", id.to_string(), node);
                                self.abort_emulations(vec![id]).await;
                                break;
                            }
                        }
                    }
                }
                sender.send(None).unwrap();
            }
            MessageWrapper { message: _message, sender: Some(sender) } => {
                let _ = sender.send(Some(OManagerMessage::Rejected(TopologyRejectReason::BadFile("You need to send file via NewTopology enumeration".to_string()))));
            }
            _ => eprintln!("[ACCEPT TOPOLOGY]: Unrecognized message")
        }
    }

    // Cannot ask for references because it is in an await.
    async fn send_new_emulation(&self, uuid: Uuid, network: Network<Node>, nodes: Vec<ClusterNodeInfo>, events: Vec<EmulationEvent>) -> Result<()> {
        // We found a topology! let's create a new one
        // Choose a leader randomly (first we find)
        let leader = nodes[0].clone();
        println!("[OManager] : Sending new Emulation to these nodes: {:?}", nodes);
        let emul = Emulation::build(uuid, &network, &events, leader);
        // Create the message for the different cluster nodes
        // Send my info leader for them to contact me if there is something bad happening with an emulation
        let mess = EManagerMessage::ExperimentNew((self.my_info_omanager.clone(), emul.clone()));
        // Send the topology to all concerned cluster nodes
        for i in 0..nodes.len() {
            // If affected to me, directly send it through the local_sender
            let m = mess.clone();
            if let Err(_) = if nodes[i] == self.my_info_emanager {
                if let Err(e) = self.local_sender.send(MessageWrapper { message: m, sender: None }).await {
                    return Err(Error::wrap("send start emulation", ErrorKind::CommandFailed, "cannot send new topology to local node", e));
                }
                Ok(())
            } else {
                let n = nodes[i].clone();
                println!("[OManager]: Sending new Emulation to {}", n);
                let mut binding: TCPBinding<EManagerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                binding.send_to(m, n).await
            } {
                // If we cannot send to one, we need to abort all the others to which we already sent
                for j in 0..i {
                    let mut binding: TCPBinding<EManagerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                    let n = nodes[j].clone();
                    let id = emul.uuid().to_string();
                    let _ = binding.send_to(EManagerMessage::ExperimentStop(id), n)
                        .await;
                }
                return Err(Error::new("send start emulation", ErrorKind::CommandFailed, "Error when sending emulation to everyone. Destination not reached."));
            }
        }
        Ok(())
    }
}