use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use tokio::sync::mpsc;


#[derive(Debug)]
pub enum Command {
    ACTIVATE,
    DEACTIVATE,
    UPDATE,
}

#[derive(Debug, Eq, Clone, Copy)]
pub struct Flow {
    pub source: usize,
    pub destination: usize,
    bandwidth: usize,
    pub target_bandwidth: usize,
}

impl PartialEq for Flow {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.destination == other.destination
    }
}

impl Hash for Flow {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.source, self.destination).hash(state)
    }
}

impl Flow {
    pub fn build(source: usize, destination: usize, bandwidth: usize, target_bandwidth: usize) -> Flow {
        Flow { source, destination, bandwidth, target_bandwidth }
    }
}

pub async fn launch_node(graph: netgraph::Network, node_list: Vec<mpsc::Sender<(Command, Flow)>>, mut receiver: mpsc::Receiver<(Command, Flow)>, id: usize) {
    // Used to store the active flows.
    // Node_id (destination) : Bandwidth
    let mut active_flows: HashSet<Flow> = HashSet::new();

    // Used to store the available bandwidth on the graph
    let mut bw_graph = graph.clone();

    // Start listening on events
    while let Some(event) = receiver.recv().await {
        match event {
            (Command::ACTIVATE, flow) => {
                if flow.target_bandwidth == 0 {
                    eprintln!("Cannot create a node with a target bandwidth to 0");
                    continue; // loop on the next message
                }
                println!("[{}] Received new flow to activate", id);

                (active_flows, bw_graph) = update_flow(active_flows, flow, &graph, bw_graph);
                println!("[{}] Finish adding, new flows: {:?}", id, active_flows);

                // Finally, update everyone
                // Now we can get the flow we added and transmit it to the other guys
                let new_flow = active_flows.get(&flow).unwrap().clone();
                println!("[{}] Updating the other guys", id);
                broadcast_flow(&node_list, new_flow, id).await;
            }
            (Command::DEACTIVATE, flow) => {
                // Check if we are legit to deactivate the flow
                if !active_flows.contains(&flow) || flow.source != id {
                    eprintln!("Cannot deactivate non-existing flow, or I am not the source.");
                    continue; // loop on the next message
                }
                (active_flows, bw_graph) = remove_flow(active_flows, flow, &graph);
                // Finally update everyone!
                // make sure that we set the flow bandwidth to 0
                let mut new_flow = flow.clone();
                new_flow.bandwidth = 0;
                new_flow.target_bandwidth = 0;
                println!("[{}] Updating the other guys", id);
                broadcast_flow(&node_list, new_flow, id).await;
            }
            (Command::UPDATE, flow) => {
                if flow.target_bandwidth == 0 { // we are in deactivation
                    println!("[{}] Received update of a flow, need to remove it, calculating new flows", id);
                    (active_flows, bw_graph) = remove_flow(active_flows, flow, &graph);
                } else { // we need to add / update a new flow
                    println!("[{}] Received update of a flow, need to update my state", id);
                    (active_flows, bw_graph) = update_flow(active_flows, flow, &graph, bw_graph);
                }
                println!("[{}] Finish update, new flows: {:?}", id, active_flows);
            }
        }
    }
}

fn update_flow(mut active_flows: HashSet<Flow>, flow: Flow, graph: &netgraph::Network, mut bw_graph: netgraph::Network) -> (HashSet<Flow>, netgraph::Network){
    // This is the new flow to add/update. This variable will contain at then end
    // of the branch bellow the value to report to the other. Updated in each branches (if/else).
    // If there is enough bandwidth remaining, create the flow and send it to the others

    // If there is enough bandwidth remaining, create the flow and send it to the others
    if bw_graph.bandwidth_between(flow.source, flow.destination) >= flow.target_bandwidth {
        bw_graph.update_edges_along_path_by(flow.source, flow.destination, |old_speed| old_speed - flow.target_bandwidth);
        // Add the new flow inside the active flow.
        active_flows.insert(flow);
    } else {
        // new_flows contains now all active flows impacted by the update/creation of the flow
        // with their new values. It also contain the updated/created flow with its values.
        let new_flows = update_active_flows(flow, active_flows.clone(), &graph);

        // we must replace the old values of the flows inside our "active flows set"
        for fl in new_flows {
            active_flows.replace(fl);
        }

        // we must now adapt the bandwidth graph with the new flows, we better have
        // to recalculate everything
        bw_graph = calculate_bandwidth_graph(&graph, &active_flows);
    }
    (active_flows, bw_graph)
}

fn remove_flow(mut active_flows: HashSet<Flow>, flow: Flow, graph: &netgraph::Network) -> (HashSet<Flow>, netgraph::Network) {
    // remove the flow from the active flows
    active_flows.remove(&flow);
    let new_flows = update_active_flows(flow, active_flows.clone(), &graph);
    // we must replace the old values of the flows inside our "active flows set"
    for fl in new_flows {
        active_flows.replace(fl);
    }
    // we must now adapt the bandwidth graph with the new flows, we better have
    // to recalculate everything
    let bw_graph = calculate_bandwidth_graph(&graph, &active_flows);
    (active_flows, bw_graph)
}

async fn broadcast_flow(node_list: &Vec<mpsc::Sender<(Command, Flow)>>, flow: Flow, id: usize) {
    for i in 0..node_list.len() {
        if i == id { continue; } // do not update myself
        node_list[i].send((Command::UPDATE, flow)).await.unwrap()
    }
}


fn calculate_bandwidth_graph(graph: &netgraph::Network, active_flows: &HashSet<Flow>) -> netgraph::Network {
    let mut bw_graph = graph.clone();
    for flow in active_flows {
        bw_graph.update_edges_along_path_by(flow.source, flow.destination, |old_speed| old_speed - flow.bandwidth);
    }
    bw_graph
}

fn update_active_flows(flow_updated: Flow, active_flows: HashSet<Flow>, original_graph: &netgraph::Network) -> HashSet<Flow> {
    // If there is not enough bandwidth, find all active flows that share a same link
    // and calculate my share. Then create the flow and update the other guys
    // which will calculate their share.

    // 1. Find all active flows that share links and gather these links.
    let (shared_links, mut impacted_flows) =
        get_shared_links_and_impacted_flows(flow_updated, active_flows, original_graph);

    // If the flow updated ask for 0 bandwidth, it means that it wants to stop. We can remove it
    // from the impacted flows, this way the calculus will not be made with him.
    // This is done just to be sure, if the user forgot to remove the flow from its active_flows
    if flow_updated.target_bandwidth == 0 {
        println!("[update_active_flows] You forgot to remove the flow from your active_flows, made it for you...");
        impacted_flows.remove(&flow_updated);
    } else {
        // add the new flow inside the flows_sharing_links if not already active, or
        // update it with new values, replace add if not exists or update existing one.
        impacted_flows.replace(flow_updated);
    }

    // Finally, we can compute a distribution of the bandwidth across all the flows that are impacted.
    distribute_bandwidth_across_flow(shared_links, impacted_flows)
}

fn get_shared_links_and_impacted_flows(flow_updated: Flow, all_flows: HashSet<Flow>, graph: &netgraph::Network) -> (HashSet<netgraph::Edge>, HashSet<Flow>) {
    let flow_path = graph.path_from(flow_updated.source, flow_updated.destination);
    // Get the links that are shared by the flows and the flows that share links with the new flow
    all_flows.iter()
        .filter_map(|f| {
            let path = graph.path_from(f.source, f.destination);
            let shared = flow_path.shared_edges(&path);
            if shared.is_empty() {
                None
            } else {
                Some((shared, f.clone()))
            }
        }).fold(
        (HashSet::new(), HashSet::new()),
        |(acc_set, mut acc_flows), (set, flow)| {
            ({
                 // ignore ourself, if we put ourself, we will not get "only" the shared edges but
                 // also all edge that I share with myself, so all my edges.
                 if flow != flow_updated {
                     acc_set.union(&set)
                         .map(|e| e.clone())
                         .collect()
                 } else {
                     acc_set
                 }
             }, {
                 // do not ignore myself here because I am impacted too!
                 acc_flows.insert(flow);
                 acc_flows
             })
        },
    )
}

fn distribute_bandwidth_across_flow(shared_links: HashSet<netgraph::Edge>, mut flows: HashSet<Flow>) -> HashSet<Flow> {
    // The calculation is based on the slowest link and the flows that need to go through.

    // Find the link with the minimum bandwidth
    let available_bandwidth = shared_links.iter().map(|e| e.speed).min().unwrap();
    // Calculate simple partitioning: everyone has the same.
    let initial_partition = available_bandwidth as f64 / flows.len() as f64;

    // Used to store the new flows, updated, ready to return
    let mut new_flows = HashSet::new();

    // Now, check if some flows does not need the partitioning they received. If they exists,
    // divide the left over across the other remaining. Loop until everybody has the maximum it wants.

    let mut actual_partition = initial_partition;

    loop {
        let maxed_out_flows: HashSet<Flow> = flows.iter()
            .filter(|f| (f.target_bandwidth as f64) < actual_partition)
            .map(|f| f.clone())
            .collect();

        // If there is nobody maxed out, it is time to stop, everybody will get the defined partition.
        // We can then quit
        if maxed_out_flows.is_empty() {
            for f in flows {
                let mut f = f.clone();
                f.bandwidth = actual_partition as usize;
                new_flows.insert(f);
            }
            break;
        }

        for f in &maxed_out_flows {
            let mut f = f.clone();
            // Set its bandwidth to the target bandwidth, max it out babyyyy!
            f.bandwidth = f.target_bandwidth;
            new_flows.insert(f);
        }
        // remove the maxed out flows from the flows
        flows = flows.difference(&maxed_out_flows).map(|f| f.clone()).collect();

        // Add the spare bandwidth to the actual partitioning.
        let spare_bandwidth = maxed_out_flows.iter()
            .fold(0f64, |acc, flow| acc + (actual_partition - (flow.target_bandwidth as f64)));
        actual_partition = actual_partition + (spare_bandwidth / flows.len() as f64);
    }

    new_flows
}
