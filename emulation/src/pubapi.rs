use std::collections::HashMap;


use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc};
use uuid::Uuid;
use cgraph::{CGraph, CGraphUpdate};
use common::{ClusterNodeInfo, TopologyMessage, TopologyRejectReason, Result, Error, ErrorKind, EmulationEvent};
use netgraph::Network;
use nethelper::{ALL_ADDR, DefaultHandler, MessageWrapper, NoHandler, ProtoBinding, Protocol, TCP, TCPBinding};
use crate::data::{ControllerMessage, Emulation, Node};
use crate::orchestrator::Orchestrator;
use crate::xmlgraphparser::parse_topology;


pub struct OrchestrationManager {
    my_info_controller: ClusterNodeInfo,
    my_info_leader: ClusterNodeInfo,
    local_sender: Sender<MessageWrapper<ControllerMessage>>,
    orchestrator: Orchestrator,
    cgraph_update_receiver: Receiver<CGraphUpdate>,
    emulations_and_their_nodes: HashMap<Uuid, Vec<ClusterNodeInfo>>,
}

impl OrchestrationManager {
    pub async fn build_and_start(my_info: ClusterNodeInfo, local_sender: Sender<MessageWrapper<ControllerMessage>>,
                                 cgraph_update_receiver: Receiver<CGraphUpdate>, connection_port: u16,
                                 controllers_port: u16, original_cgraph: CGraph<ClusterNodeInfo>)
                                 -> Result<()> {
        let mut my_info_controller = my_info.clone();
        my_info_controller.port = controllers_port;

        let mut my_info_leader = my_info.clone();
        my_info_leader.port = connection_port;

        let manager = OrchestrationManager {
            my_info_controller,
            my_info_leader,
            local_sender,
            orchestrator: Orchestrator::new(original_cgraph, controllers_port),
            cgraph_update_receiver,
            emulations_and_their_nodes: HashMap::new(),
        };
        manager.accept_topology().await
    }


    async fn accept_topology(mut self) -> Result<()> {
        let (sender, mut receiver) = mpsc::channel(10);
        // Create the handler
        let handler = DefaultHandler::<TopologyMessage>::new(sender);

        // listen on TCP
        let mut binding = TCP::bind_addr((ALL_ADDR, self.my_info_leader.port), Some(handler)).await.unwrap();
        binding.listen().unwrap();

        loop {
            tokio::select! {
            Some(message) = receiver.recv() => {
                self.handle_user_message(message).await;
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
            let mess = ControllerMessage::EmulationStop(uuid.to_string());
            for node in self.emulations_and_their_nodes.get(&uuid).unwrap() {
                // Send an abort for this
                let m = mess.clone();
                if self.my_info_controller.eq(node) {
                    self.local_sender.send(MessageWrapper { message: m, sender: None }).await
                        .expect(&*format!("[WARNING]: Wanted to abort emulation {} on local machine, error", uuid.to_string()));
                } else {
                    let mut binding: TCPBinding<ControllerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                    let n = node.clone();
                    binding.send_to(m, n).await
                        .expect(&*format!("[WARNING]: Wanted to abort emulation {} on node {}, node unreachable", uuid.to_string(), node));
                }
            }
        }
    }

    async fn handle_user_message(&mut self, message: MessageWrapper<TopologyMessage>) {
        match message {
            MessageWrapper { message: TopologyMessage::NewTopology(top), sender: Some(sender) } => {
                let uuid = Uuid::new_v4();
                match parse_topology(top, uuid.clone()) {
                    Ok((network, events)) => {
                        match self.orchestrator.new_emulation(network, uuid.clone()) {
                            Ok(possible_net) => {
                                match possible_net {
                                    None => sender.send(Some(TopologyMessage::Rejected(TopologyRejectReason::NoDeploymentFound))).unwrap(),
                                    Some((network, cluster_nodes_affected)) => {
                                        let id = uuid.clone();
                                        match self.send_start_emulation(id, network, cluster_nodes_affected, events).await {
                                            Ok(_) => sender.send(Some(TopologyMessage::Accepted)).unwrap(),
                                            Err(_) => sender.send(Some(TopologyMessage::Rejected(TopologyRejectReason::NoDeploymentFound))).unwrap()
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[NEW_TOPOLOGY]: Error when finding an dispatch for the topology: {}", e);
                                sender.send(Some(TopologyMessage::Rejected(TopologyRejectReason::NoDeploymentFound))).unwrap();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[NEW TOPOLOGY]: Cannot parse {}", e);
                        sender.send(Some(TopologyMessage::Rejected(TopologyRejectReason::BadFile(e.to_string())))).unwrap();
                    }
                }
            }
            MessageWrapper { message: TopologyMessage::Abort(id), sender: Some(sender) } => {
                // We receive an abort, we need to stop this emulation
                let id = Uuid::parse_str(&*id).unwrap();
                let uuids = vec![id];
                self.abort_emulations(uuids).await;
            }
            MessageWrapper { message: _message, sender: Some(sender) } => {
                let _ = sender.send(Some(TopologyMessage::Rejected(TopologyRejectReason::BadFile("You need to send file via NewTopology enumeration".to_string()))));
            }
            _ => eprintln!("[ACCEPT TOPOLOGY]: Unrecognized message")
        }
    }

    // Cannot ask for references because it is in an await.
    async fn send_start_emulation(&self, uuid: Uuid, network: Network<Node>, nodes: Vec<ClusterNodeInfo>, events: Vec<EmulationEvent>) -> Result<()> {
        // We found a topology! let's create a new one
        // Choose a leader randomly (first we find)
        let leader = nodes[0].clone();
        let emul = Emulation::build(uuid, &network, &events, leader);
        // Create the message for the different cluster nodes
        // Send my info leader for them to contact me if there is something bad happening with an emulation
        let mess = ControllerMessage::EmulationStart(self.my_info_leader.clone(), emul.clone());
        // Send the topology to all concerned cluster nodes
        for i in 0..nodes.len() {
            // If affected to me, directly send it through the local_sender
            let m = mess.clone();
            if let Err(_) = if nodes[i] == self.my_info_controller {
                if let Err(e) = self.local_sender.send(MessageWrapper { message: m, sender: None }).await {
                    return Err(Error::wrap("send start emulation", ErrorKind::CommandFailed, "cannot send new topology to local node", e));
                }
                Ok(())
            } else {
                let mut binding: TCPBinding<ControllerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                let n = nodes[i].clone();
                binding.send_to(m, n)
                    .await
            } {
                // If we cannot send to one, we need to abort all the others to which we already sent
                for j in 0..i {
                    let mut binding: TCPBinding<ControllerMessage, NoHandler> = TCP::bind(None).await.unwrap();
                    let n = nodes[j].clone();
                    let id = emul.uuid().to_string();
                    let _ = binding.send_to(ControllerMessage::EmulationStop(id), n)
                        .await;
                }
                return Err(Error::new("send start emulation", ErrorKind::CommandFailed, "Error when sending emulation to everyone. Destination not reached."));
            }
        }
        Ok(())
    }
}