use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use common::{Error, ErrorKind, Result};
use std::time::Duration;

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

use common::RunnerConfig;
use crate::data::{CJQResponseKind, Event, NodeInfo, WCGraph};
use nethelper::{Handler, handler_once_box, ProtoBinding, Protocol, Responder, TCP, TCPBinding, UDP, UDPBinding};
use crate::perf::PerfCtrl;


#[derive(Clone)]
struct EventHandler {
    sender: mpsc::Sender<EventMessage>,
}

#[async_trait]
impl Handler<Event> for EventHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<Event> {
        let event = Event::from_bytes(bytes).unwrap();
        // create the channel to get the response from the controller
        let (tx, rx) = oneshot::channel();
        // prepare the event to send to the controller:
        let message = EventMessage { event, sender: Some(tx) };
        self.sender.send(message).await.unwrap();
        // wait on the response from the controller
        rx.await.unwrap()
    }
}

impl EventHandler {
    pub fn new(sender: mpsc::Sender<EventMessage>) -> EventHandler {
        EventHandler { sender }
    }
}

#[derive(Clone)]
struct HeartBeatHandler {
    sender: mpsc::Sender<NodeInfo>,
}

#[async_trait]
impl Handler<Event> for HeartBeatHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<Event> {
        if let Ok(Event::CHeartbeat((info, _))) = Event::from_bytes(bytes) {
            if let Err(_) = self.sender.send(info).await {
                println!("[HEARTBEAT CHECK]: Cannot send receive heartbeat, it must have been aborted");
            }
        } else {
            println!("Expected CHeartbeat, go another event");
        }
        None
    }
}

impl HeartBeatHandler {
    pub fn new(sender: mpsc::Sender<NodeInfo>) -> HeartBeatHandler {
        HeartBeatHandler { sender }
    }
}

#[derive(Debug)]
struct EventMessage {
    event: Event,
    sender: Option<oneshot::Sender<Option<Event>>>,
}

impl Display for EventMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.event)
    }
}


pub struct Ctrl {
    events_receiver: mpsc::Receiver<EventMessage>,
    event_sender: mpsc::Sender<EventMessage>,
    cgraph: WCGraph,
    my_info: NodeInfo,
    my_speed: usize,
    is_leader: bool,
    adding_new_node: bool,
    perf_ctrl: PerfCtrl,
    tcp_binding: TCPBinding<Event, EventHandler>,
    udp_binding: UDPBinding<Event, EventHandler>,
    event_handler: EventHandler,
    started: bool,
    config: RunnerConfig,
    heartbeat_misses: HashMap<NodeInfo, usize>,
    cluster_id: Option<uuid::Uuid>,
}

impl Ctrl {
    pub async fn build(config: RunnerConfig) -> Ctrl {
        // Create the channel trough which the handler will send the incoming events
        let (sender, receiver) = mpsc::channel(config.event_channel_size);

        // create the perf component and its handler for network events
        // create the performances controller
        let perf_ctrl = PerfCtrl::new(NodeInfo::new((&*config.ip_address, config.iperf3_port)), config.perf_test_duration_seconds).await;

        // Create the main handler for incoming events:
        let event_handler = EventHandler::new(sender.clone());
        // bind the handler to the UDP and TCP incoming requests
        let tcp_binding = TCP::bind_addr(("0.0.0.0", config.event_port), Some(event_handler.clone()))
            .await
            .unwrap();
        let udp_binding = UDP::bind_addr(("0.0.0.0", config.event_port), Some(event_handler.clone()))
            .await
            .unwrap();

        let my_info = NodeInfo::new((&*config.ip_address, config.event_port));
        let my_speed = config.local_speed;

        Ctrl {
            events_receiver: receiver,
            event_sender: sender.clone(),
            cgraph: WCGraph::new(),
            my_info,
            my_speed,
            is_leader: config.leader,
            adding_new_node: false,
            perf_ctrl,
            tcp_binding,
            udp_binding,
            event_handler,
            started: false,
            config,
            heartbeat_misses: HashMap::new(),
            cluster_id: None,
        }
    }

    pub async fn init(&mut self) -> Result<()> {
        if !self.started.borrow() {
            // set ignore myself on UDP to not respond to my own broadcasts made with this binding
            self.udp_binding.ignore(self.my_info.clone());
            // start listening with both protocols
            self.tcp_binding.listen()?;
            self.udp_binding.listen()?;
            Ok(())
        } else {
            Err(Error::new("controller init", ErrorKind::AlreadyStarted, "Controller already started"))
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        println!("Starting the controller...");

        // store the number of retry for joining a cluster
        let mut cjq_request_remaining_retries = self.config.cjq_retry;

        // If we do not start as leader, we must make a CJQRequest (Cluster Joining Query Request)
        // to see if there is a cluster available in my subnet.
        if !self.is_leader {
            println!("This node is not a leader, searching for leader on the subnet.");
            self.send_cluster_joining_request().await?;
        } else {
            self.create_cluster()?;
        }

        while let Some(event) = self.events_receiver.recv().await {
            println!("[EVENT RECEIVED]: {}", event);
            match event {
                EventMessage { event: Event::CGraphGet, sender: Some(sender) } => {
                    // I am in TCP
                    sender.send(Some(Event::CGraphSend(self.cgraph.clone())))
                        .unwrap();
                }
                EventMessage { event: Event::CGraphUpdate(graph), sender: Some(sender) } => {
                    // I am in TCP
                    // The event CGraphUpdate is the last to receive after adding a node,
                    // we are not currently adding a new node.
                    self.adding_new_node = false;
                    if graph.is_empty() { // means we are not the leader anymore
                        println!("[CGRAPH UPDATE]: I am not the leader anymore, stopping heartbeat check...");
                        self.is_leader = false;
                        // verify that we are not making a heartbeat
                    } else {
                        println!("[CGRAPH UPDATE]: I am the leader of the cluster");

                        let was_leader = self.is_leader;
                        self.cgraph = graph;
                        self.update_is_leader();

                        // if I am the leader, begin to send HeartBeat if I wasn't
                        if self.is_leader && !was_leader {
                            println!("[CGRAPH UPDATE]: I am a new leader, starting heartbeat check...");
                            self.send_event_after(Duration::from_secs(1), Event::CHeartbeatCheck);
                        }
                    }
                    // nothing to send back
                    sender.send(None).unwrap();
                }
                EventMessage { event: Event::CJQResponse(kind), sender: None } => {
                    // This is an internal call forwarded by the method who sends the request

                    // When receiving a response, the number of retries is reset
                    cjq_request_remaining_retries = self.config.cjq_retry;

                    match kind {
                        CJQResponseKind::Accepted((info, cluster_id)) => {
                            // Check if I am the leader. It can happens if I receive an
                            // accept after the timeout
                            // If it is the case, prioritize joining another cluster
                            self.is_leader = false;


                            match self.add_myself_in_cluster(info.clone()).await {
                                Ok(response) => {
                                    println!("[CLUSTER ADD]: Finish adding myself in the cluster");
                                    // SWITCH TO TCP
                                    TCP::bind(Some(self.event_handler.clone()))
                                        .await?
                                        .send_to(response, info).await.unwrap();

                                    self.cluster_id = Some(uuid::Uuid::parse_str(&*cluster_id).unwrap());
                                    if self.is_leader {
                                        println!("[CLUSTER ADD]: I am the new leader of the cluster, starting Heartbeat");
                                        self.perform_heartbeat_check().unwrap();
                                    }
                                }
                                Err(e) => {
                                    println!("[CLUSTER ADD]: Error adding myself in the cluster, sending an ABORT. Error: {}", e);
                                    let mut binding: TCPBinding<Event, Responder<Event>> = TCP::bind(None).await.unwrap();
                                    binding.send_to(Event::CJQAbort, info).await.unwrap();

                                    println!("[CLUSTER ADD]: Waiting a bit and then retry");
                                    // launch a retry in case it failed, reset the number of retries
                                    self.send_event_after(Duration::from_secs(self.config.cjq_timeout_duration_seconds), Event::CJQRetry);
                                }
                            }
                        }
                        CJQResponseKind::Wait(duration) => {
                            println!("Need to wait {:#?} before adding myself", duration);
                            self.send_event_after(duration, Event::CJQRetry);
                        }
                    }
                }
                EventMessage { event: Event::CJQRequest(info), sender: Some(sender) } => {
                    // I am in UDP
                    if self.is_leader {
                        if self.adding_new_node { // we are currently adding a new node
                            println!("[CJQ REQUEST]: Already adding a new node, send wait");
                            sender.send(Some(Event::CJQResponse(CJQResponseKind::Wait(
                                Duration::from_secs(self.config.cjq_waiting_time_seconds)
                            )))).unwrap();
                        } else {
                            println!("[CJQ REQUEST]: Accept request");
                            if self.cgraph.nodes().iter().map(|n| n.info()).collect::<HashSet<NodeInfo>>().contains(&info) {
                                println!("[CJQ REQUEST]: Detecting reboot of node already in cluster");
                                // in the case of a reboot, accept it and clear its missed heartbeats
                                self.heartbeat_misses.remove(&info);
                            }
                            sender.send(Some(Event::CJQResponse(CJQResponseKind::Accepted(
                                (self.my_info.clone(), self.cluster_id.unwrap().to_string())
                            )))).unwrap();

                            // I am adding a node inside my cluster
                            self.adding_new_node = true;
                        }
                    } else {
                        sender.send(None).unwrap() // do not answer.
                    }
                }
                EventMessage { event: Event::CJQAbort, sender: Some(sender) } => {
                    self.adding_new_node = false;
                    sender.send(None).unwrap();
                }
                EventMessage { event: Event::CJQRetry, sender: None } => {
                    // retry to join
                    if cjq_request_remaining_retries > 0 {
                        println!("[CJQ RETRY]: Remaining retries: {}", cjq_request_remaining_retries);
                        cjq_request_remaining_retries = cjq_request_remaining_retries - 1;
                        self.send_cluster_joining_request().await?;
                    } else {
                        println!("[CJQ RETRY]: No more retries, create my own cluster");
                        self.create_cluster()?;
                    }
                }
                EventMessage { event: Event::CHeartbeat((_, other)), sender: Some(sender) } => {
                    if let Ok(other_id) = uuid::Uuid::parse_str(&*other) {
                        if let Some(cid) = self.cluster_id {
                            if cid == other_id {
                                sender.send(Some(Event::CHeartbeat((self.my_info.clone(), other)))).unwrap();
                                // continue the loop, next event
                                continue;
                            }
                        }
                    }
                    sender.send(None).unwrap();
                }
                EventMessage { event: Event::NodeFailure(info), sender: Some(sender) } => {
                    if let Some(missed_heartbeat) = self.heartbeat_misses.get(&info) {
                        if missed_heartbeat + 1 >= self.config.heartbeat_misses {
                            // consider this one as lost
                            let previous_size = self.cgraph.size();
                            self.cgraph.remove_one_by(|n| n.info().eq(&info)).unwrap();
                            assert_eq!(previous_size - 1, self.cgraph.size());
                            if previous_size - 1 != self.cgraph.size() || self.cgraph.nodes().iter()
                                .map(|n| n.info())
                                .collect::<HashSet<NodeInfo>>()
                                .contains(&info) {
                                eprintln!("[NODE FAILURE]: Issue removing a node from the CGraph");
                            }
                            self.heartbeat_misses.remove(&info).unwrap();
                        } else {
                            // increment the number of missed heartbeats, must be present, so unwrap
                            self.heartbeat_misses.insert(info, missed_heartbeat + 1).unwrap();
                        }
                    } else {
                        self.heartbeat_misses.insert(info, 1);
                    }

                    // send back the updated CGraph
                    sender.send(None).unwrap();
                }
                EventMessage { event: Event::CHeartbeatReset, sender: Some(sender) } => {
                    // reset all the heartbeat counts
                    self.heartbeat_misses.clear();
                    sender.send(None).unwrap();
                }
                EventMessage { event: Event::CHeartbeatCheck, sender: None } => {
                    // If I am the leader, execute the Heartbeat check, if not, it will stops the loop.
                    if self.is_leader {
                        self.perform_heartbeat_check().unwrap();
                    }
                }
                EventMessage { event: Event::PClient, sender: Some(sender) } => {
                    println!("[PERF]: Received perf request, starting the server...");
                    self.perf_ctrl.launch_server().await?;
                    println!("[PERF]: Server started");
                    sender.send(Some(Event::PServer(NodeInfo::new((&*self.my_info.ip_addr, self.config.iperf3_port))))).unwrap()
                }
                _ => eprintln!("Event not recognized"),
            }
        }
        Ok(())
    }


    fn perform_heartbeat_check(&mut self) -> Result<()> {
        // making some clones to push them into the Tokio task
        let cgraph = self.cgraph.clone();
        let my_info = self.my_info.clone();
        let event_sender = self.event_sender.clone();
        let event_port = self.config.event_port;
        let heartbeat_timeout = self.config.heartbeat_timeout_seconds;
        let heartbeat_sleep = self.config.heartbeat_sleep_seconds;
        let cluster_id = self.cluster_id.unwrap();

        tokio::spawn(async move {
            // sender and receivers for the network handler. The handler use this channel
            // to communicate the received heartbeat. After a while, the receiver is drained
            // to collect all the heartbeat answers.
            let (sender, mut receiver) = mpsc::channel(cgraph.size() * 2);
            // creating the network handler and the binding
            let handler = HeartBeatHandler::new(sender);
            let mut heartbeat_binding = UDP::bind(Some(handler))
                .await
                .unwrap();
            // listen on this port for returns of HeartBeat
            heartbeat_binding.listen().unwrap();

            // Send the broadcast on this socket
            println!("[HEARTBEAT CHECK]: Sending the Heartbeat broadcast");
            heartbeat_binding
                .broadcast(Event::CHeartbeat((my_info.clone(), cluster_id.to_string())), event_port)
                .await
                .unwrap();

            // wait few seconds to let time to the other to send back their heart beats
            sleep(Duration::from_secs(heartbeat_timeout)).await;


            // collect the results
            println!("[HEARTBEAT CHECK]: Collecting the Heartbeat responses");
            let mut received_info = HashSet::new();
            while let Ok(info) = receiver.try_recv() {
                received_info.insert(info);
            }

            // compare the cgraph set with this set and for all remaining, send an event
            // telling that their are missing
            // Do not count myself
            let remaining: HashSet<NodeInfo> = cgraph.nodes().iter()
                .filter(|n| !received_info.contains(&n.info()))
                .map(|n| n.info()).collect();
            println!("[HEARTBEAT CHECK]: Missing nodes: {:?}", remaining);


            // Ask to clear the missed heartbeat map if the node has rejoined the cluster
            if remaining.is_empty() {
                let (tx, rx) = oneshot::channel::<Option<Event>>();
                let mess = EventMessage { event: Event::CHeartbeatReset, sender: Some(tx) };
                event_sender.send(mess).await.unwrap();
                rx.await.unwrap();
            }


            for failed_node in remaining {
                // force waiting on the removal of the node to make full synchronization
                // before restarting a loop of Heartbeat Check. This is a security.
                let (tx, rx) = oneshot::channel::<Option<Event>>();
                let mess = EventMessage { event: Event::NodeFailure(failed_node.clone()), sender: Some(tx) };
                event_sender.send(mess).await.unwrap();
                // wait for the updated CGraph
                rx.await.unwrap();
            }

            // loop by putting itself on the events
            Ctrl::send_event_after_chan(
                event_sender,
                Duration::from_secs(heartbeat_sleep), Event::CHeartbeatCheck);
        });
        Ok(())
    }

    fn create_cluster(&mut self) -> Result<()> {
        println!("[CREATE CLUSTER]: Creating a cluster");
        self.adding_new_node = false;
        self.cgraph = WCGraph::new();
        self.cgraph.add_node(self.my_speed, self.my_info.clone())?;
        self.cluster_id = Some(uuid::Uuid::new_v4());
        // launch preventive heartbeat check
        self.send_event_after(Duration::from_secs(1), Event::CHeartbeatCheck);
        Ok(())
    }

    async fn send_cluster_joining_request(&mut self) -> Result<()> {
        // We will create a new handler with new channel for communication between the handler and here
        // This new binding will only intercept the CJQ Response and when it intercepts one, it
        // forwards it to the main event queue.
        let (sender, mut receiver) = mpsc::channel(self.config.event_channel_size);
        let handler = EventHandler::new(sender);

        let mut binding = UDP::bind(Some(handler)).await?;
        binding.listen()?;
        binding.broadcast(Event::CJQRequest(self.my_info.clone()), self.config.event_port)
            .await?;
        println!("[CJQ REQUEST] BROADCAST");

        // cloning for passing into the async move
        let event_sender = self.event_sender.clone();

        let waiting_on_response = async move {
            while let Some(event_msg) = receiver.recv().await {
                match event_msg {
                    EventMessage { event: Event::CJQResponse(kind), sender: Some(sender) } => {
                        println!("[CJQ REQUEST]: Received response, forwarding it");
                        event_sender.send(EventMessage { event: Event::CJQResponse(kind), sender: None })
                            .await
                            .unwrap();
                        // send nothing back on the network
                        sender.send(None).unwrap();
                        break; // finish the execution
                    }
                    _ => eprintln!("[CJQ REQUEST]: Received bad event on binding"),
                }
            }
        };

        // use tokio timeout to timeout this execution
        // Wrap the future with a `Timeout`.
        if let Err(_) = tokio::time::timeout(Duration::from_secs(self.config.cjq_timeout_duration_seconds), waiting_on_response).await {
            println!("[CJQ REQUEST]: Timeout occurred");
            self.event_sender.send(EventMessage { event: Event::CJQRetry, sender: None }).await.unwrap();
        }
        Ok(())
    }

    fn send_event_after(&self, duration: Duration, event: Event) {
        Ctrl::send_event_after_chan(self.event_sender.clone(), duration, event)
    }

    fn send_event_after_chan(sender: mpsc::Sender<EventMessage>, duration: Duration, event: Event) {
        tokio::spawn(async move {
            sleep(duration).await;
            // prepare the event to send to the controller:
            let message = EventMessage { event, sender: None };
            sender.send(message).await.unwrap();
        });
    }

    fn update_is_leader(&mut self) {
        let current_leader = self.cgraph.find_leader().unwrap();
        // check if I still the leader
        self.is_leader = current_leader.info()
            .eq(self.my_info.clone().borrow());
    }

    async fn add_myself_in_cluster(&mut self, leader_info: NodeInfo) -> Result<Event> {
        println!("[CLUSTER ADD]: Adding myself in the cluster. The leader is: {}", leader_info);
        // We are going to download the CGraph via TCP this time
        let mut conn: TCPBinding<Event, Responder<Event>> = TCP::bind(None).await?;
        conn.send_to(Event::CGraphGet, leader_info.clone()).await?;
        println!("[CLUSTER ADD]: CGraphGet request sent to the leader");
        let (tx, rx) = oneshot::channel::<WCGraph>();
        conn.receive_once(handler_once_box(move |b| async move {
            let event = Event::from_bytes(b).unwrap();
            let cgraph = match event {
                Event::CGraphSend(cgraph) => {
                    println!("[CLUSTER ADD]: CGraph received by the leader");
                    cgraph
                }
                _ => {
                    println!("[CLUSTER ADD]: Expected CGraphSend!");
                    return None;
                }
            };
            tx.send(cgraph).unwrap();
            None
        })).await.unwrap();
        // Get the CGraph we received
        match rx.await {
            Ok(cgraph) => {
                // We got the CGraph!!!
                self.cgraph = cgraph;
                // Start looping to make the tests
                println!("[CLUSTER ADD]: Completing the graph with tests...");
                self.complete_graph(self.config.sufficient_speed, self.config.perf_test_retries)
                    .await?;
                println!("[CLUSTER ADD]: Completion of the graph finished");
                // Now we made the tests with all the others, check if we are the leader
                self.update_is_leader();
                // 3 cases scenarios now:
                // 1. The old leader still the leader
                // 2. We are the new leader
                // 3. Another node became the new leader
                let current_leader = self.cgraph.find_leader()?;
                if current_leader.info().eq(&leader_info.clone()) {
                    println!("[CLUSTER ADD]: The old leader still the leader");
                    self.is_leader = false;
                    // The old leader stay the leader, send him update of the cgraph
                    Ok(Event::CGraphUpdate(self.cgraph.clone()))
                } else if current_leader.info().eq(&self.my_info.clone()) {
                    println!("[CLUSTER ADD]: I am the new leader");
                    // I am the new leader, send him update with empty cgraph
                    self.is_leader = true;
                    Ok(Event::CGraphUpdate(WCGraph::new()))
                } else {
                    println!("[CLUSTER ADD]: The new leader is {}", current_leader.info());
                    self.is_leader = false;
                    // Inform the new leader that he is the new leader by sending him
                    // a complete CGraph
                    let mut binding: TCPBinding<Event, Responder<Event>> = TCP::bind(None)
                        .await?;
                    binding.send_to(
                        Event::CGraphUpdate(self.cgraph.clone()),
                        current_leader.info())
                        .await?;
                    // Another person is leader, send to the leader empty cgraph
                    Ok(Event::CGraphUpdate(WCGraph::new()))
                }
            }
            Err(e) => {
                Err(Error::wrap("cluster add",
                    ErrorKind::NoResource,
                    "Failed to get the CGraph from the distant server", e))
            }
        }
    }

    async fn complete_graph(&mut self, sufficient_speed: usize, test_retries: usize) -> Result<()> {
        // add myself to the node
        let me = self.cgraph.add_node(self.my_speed, self.my_info.clone())?;

        while let Some(other) = self.cgraph.find_missing_from_me(me.clone(), sufficient_speed) {
            println!("[PERF]: Missing information between me and {}, starting test...", other.info());
            // make the test against the other node

            let mut speed = 0;
            // adding one to make "do-while" style.
            let test_retries = test_retries + 1;
            for i in 0..test_retries {
                match self.perf_ctrl.launch_test(other.info()).await {
                    Ok(new_speed) => {
                        speed = new_speed;
                        break;
                    }
                    Err(e) => {
                        let remaining_tries = test_retries - 1 - i;
                        eprintln!("[PERF]: Issue when making test. Remaining tries: [{}]. Error: {}", remaining_tries, e);
                        if remaining_tries <= 0 {
                            return Err(Error::wrap("perf", ErrorKind::PerfTestFailed, "Exceeded number of tries.", e));
                        }
                    }
                }
            }


            println!("[PERF]: Test finished");
            println!("[PERF]: me -> {} = {}", other.info(), speed);
            self.cgraph.add_link_direct_test(me.clone(), other, speed as usize)?;
        }

        Ok(())
    }
}
