use std::cmp::min;
use std::collections::{HashMap, HashSet};
use netgraph::{Link};

use crate::data::Flow;

pub fn update_flows<'a, T: netgraph::Vertex>(mut active_flows: HashSet<Flow<'a, T>>, flow: &mut Flow<'a, T>, graph: &netgraph::Network<T>, mut bw_graph: netgraph::Network<T>) -> (HashSet<Flow<'a, T>>, netgraph::Network<T>) {
    if let Some(_) = bw_graph.bandwidth_between(&flow.source, &flow.destination) {
        let new_flows = update_active_flows(flow, &active_flows, &graph);

        for fl in new_flows {
            active_flows.replace(fl);
        }
        bw_graph = calculate_bandwidth_graph(&graph, &active_flows);
        // }
    } else {
        eprintln!("[UPDATE_FLOW]: There is no path between {} and {}", flow.source, flow.destination)
    }

    (active_flows, bw_graph)
}

pub fn remove_flow<'a, T: netgraph::Vertex>(mut active_flows: HashSet<Flow<'a, T>>, flow: &mut Flow<'a, T>, graph: &netgraph::Network<T>) -> (HashSet<Flow<'a, T>>, netgraph::Network<T>) {
    // remove the flow from the active flows
    active_flows.remove(&flow);
    let new_flows = update_active_flows(flow, &active_flows, &graph);
    // we must replace the old values of the flows inside our "active flows set"
    for fl in new_flows {
        active_flows.replace(fl);
    }
    // we must now adapt the bandwidth graph with the new flows, we better have
    // to recalculate everything
    let bw_graph = calculate_bandwidth_graph(&graph, &active_flows);
    (active_flows, bw_graph)
}

fn calculate_bandwidth_graph<'a, T: netgraph::Vertex>(graph: &netgraph::Network<T>, active_flows: &HashSet<Flow<'a, T>>) -> netgraph::Network<T> {
    let mut bw_graph = graph.clone();
    for flow in active_flows {
        bw_graph.update_edges_along_path_by(&flow.source, &flow.destination, |old_speed| old_speed - flow.used_bandwidth);
    }
    bw_graph
}

fn update_active_flows<'a, T: netgraph::Vertex>(flow_updated: &mut Flow<'a, T>, active_flows: &HashSet<Flow<'a, T>>, original_graph: &netgraph::Network<T>) -> HashSet<Flow<'a, T>> {
    // copy past hungryness if exists
    if active_flows.contains(flow_updated) {
        flow_updated.set_hungry(active_flows.get(flow_updated).unwrap().is_hungry())
    }

    // We need to adapt the flows using the same links as the updated flow. For this,
    // we need to go through each link constituting the path from source to destination of the updated flow,
    // and for each links, adapt the flows that use this link.
    let links_to_check = get_flows_by_shared_links(flow_updated, active_flows, original_graph);

    // A same flow can be attach to multiple links, if it share multiple links of the path between source and dest of the updated flow.
    // Some links can restrict more than others. This is why we keep only the most restrictive calculation.

    // This is the first pass
    let mut updated_active_flows = HashSet::new();
    for (link, flows) in links_to_check.into_iter() {
        // Finally, we can compute a distribution of the bandwidth across all the flows that are impacted.
        let new_flows = distribute_bandwidth_across_flow(link, flows);
        // update active flows keep the most restrictive distribution for each flow.
        updated_active_flows = updated_active_flows.merge(new_flows);
    }
    updated_active_flows
}

fn get_flows_by_shared_links<'a, T: netgraph::Vertex>(flow_updated: &Flow<'a, T>, all_flows: &HashSet<Flow<'a, T>>, graph: &netgraph::Network<T>) -> HashMap<Link<T>, HashSet<Flow<'a, T>>> {
    let flow_path = graph.get_path_between(&flow_updated.source, &flow_updated.destination).unwrap();
    flow_path.links_set().iter().fold(HashMap::new(), |mut acc, link| {
        // Collect all the flows that use this link
        // Create a path from source to destination and use it to compare with other flows
        let one_link_set = HashSet::from([link.clone()]);
        let mut set_of_flows = all_flows.iter().filter_map(|flow| {
            let path = graph.get_path_between(&flow.source, &flow.destination).unwrap();
            if let Some(_) = path.links_set()
                .intersection(&one_link_set).last() {
                Some(flow.clone())
            } else {
                None
            }
        }).fold(HashSet::new(), |mut set, flow| {
            set.insert(flow);
            set
        });
        // if the updated flow == 0, remove it, else use replace to change or insert it
        if flow_updated.used_bandwidth == 0 {
            set_of_flows.remove(&flow_updated);
        } else {
            // add the new flow inside the flows_sharing_links if not already active, or
            // update it with new values, replace add if not exists or update existing one.
            set_of_flows.replace(flow_updated.clone());
        }
        acc.insert(link.clone(), set_of_flows);
        acc
    })
}


fn get_shared_links_and_impacted_flows<'a, T: netgraph::Vertex>(flow_updated: &Flow<'a, T>, all_flows: &HashSet<Flow<'a, T>>, graph: &netgraph::Network<T>) -> (HashSet<netgraph::Link<T>>, HashSet<Flow<'a, T>>) {
    let flow_path = graph.get_path_between(&flow_updated.source, &flow_updated.destination).unwrap();
    // Get the links that are shared by the flows and the flows that share links with the new flow
    all_flows.iter()
        .filter_map(|f| {
            let path = graph.get_path_between(&f.source, &f.destination).unwrap();
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
                 if flow.ne(flow_updated) {
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

fn distribute_bandwidth_across_flow<'a, T: netgraph::Vertex>(link: Link<T>, mut flows: HashSet<Flow<'a, T>>) -> HashSet<Flow<'a, T>> {
    // The calculation is based on the slowest link and the flows that need to go through.

    // Find the link with the minimum bandwidth
    let available_bandwidth = link.bandwidth();
    // Calculate simple partitioning: everyone has the same.
    let initial_partition = available_bandwidth as f64 / flows.len() as f64;

    // Used to store the new flows, updated, ready to return
    let mut new_flows = HashSet::new();

    // Now, check if some flows does not need the partitioning they received. If they exists,
    // divide the left over across the other remaining. Loop until everybody has the maximum it wants.

    let mut actual_partition = initial_partition;

    loop {
        let maxed_out_flows: HashSet<Flow<'a, T>> = flows.iter()
            .filter(|f| (f.used_bandwidth as f64) <= actual_partition && !f.is_hungry())
            .map(|f| f.clone())
            .collect();

        // If there is nobody maxed out, it is time to stop, everybody will get the defined partition.
        // We can then quit
        if maxed_out_flows.is_empty() {
            for f in flows {
                let mut f = f.clone();
                f.authorized_bandwidth = min(actual_partition as u32, f.max_allowed_bandwidth);
                // we had to reduce its usage, so we consider it is using everything we gave him
                f.used_bandwidth = f.authorized_bandwidth;
                f.set_hungry(true);
                new_flows.insert(f);
            }
            break;
        }

        for f in &maxed_out_flows {
            let mut f = f.clone();
            // Set its bandwidth to the max allowed bandwidth, max it out babyyyy!
            f.authorized_bandwidth = f.max_allowed_bandwidth;
            f.set_hungry(false);
            new_flows.insert(f);
        }
        // remove the maxed out flows from the flows
        flows = flows.difference(&maxed_out_flows).map(|f| f.clone()).collect();

        // Add the spare bandwidth to the actual partitioning.
        let spare_bandwidth = maxed_out_flows.iter()
            .fold(0f64, |acc, flow| acc + (actual_partition - (flow.used_bandwidth as f64)));
        actual_partition = actual_partition + (spare_bandwidth / flows.len() as f64);
    }

    new_flows
}

trait MergeStricter<'a, T: netgraph::Vertex> {
    fn merge(self, other: HashSet<Flow<'a, T>>) -> HashSet<Flow<'a, T>>;
}

impl<'a, T: netgraph::Vertex> MergeStricter<'a, T> for HashSet<Flow<'a, T>> {
    fn merge(self, other: HashSet<Flow<'a, T>>) -> HashSet<Flow<'a, T>> {
        let result = self.symmetric_difference(&other).map(|f| f.clone()).collect::<HashSet<Flow<'a, T>>>();
        let selected = self.intersection(&other).fold(HashSet::new(), |mut acc, flow| {
            // Get the most restrictive duplicate
            let first = self.get(&flow).unwrap().authorized_bandwidth;
            let second = other.get(&flow).unwrap().authorized_bandwidth;
            if first < second {
                acc.insert(self.get(&flow).unwrap().clone());
            } else {
                acc.insert(other.get(&flow).unwrap().clone());
            }
            acc
        });
        result.union(&selected).map(|f| f.clone()).collect::<HashSet<Flow<'a, T>>>()
    }
}


#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fmt::{Debug, Display, Formatter};
    use std::hash::{Hash, Hasher};
    use tokio::io;
    use tokio::io::AsyncBufReadExt;
    use tokio::sync::mpsc;
    use std::str;
    use std::str::FromStr;

    use netgraph::PathStrategy;
    use crate::bwsync::{remove_flow, update_flows};
    use crate::data::Flow;


    #[derive(Debug)]
    pub enum Command {
        ACTIVATE,
        DEACTIVATE,
        UPDATE,
    }

    #[tokio::test]
    async fn main() {
        // configuration
        let nb_term = 3;
        let nb_intern = 2;
        let vertices = (0..nb_term + nb_intern).map(|i| i).collect();

        println!("Creating a graph...");
        let mut net = netgraph::Network::new(&vertices, PathStrategy::WidestPath);
        // 0, 1, 2 = terminal nodes and 3, 4 = internal nodes
        // 0 -> 3 100
        // 1 -> 3 50
        // 3 -> 4 100
        // 4 -> 2 100
        net.add_edge(&0, &3, 100);
        net.add_edge(&1, &3, 50);
        net.add_edge(&3, &4, 100);
        net.add_edge(&4, &2, 100);
        println!("Graph created!");

        println!("Creating communication channels");

        // The communication channels are done as follow:
        // (the sender, the bandwidth) when something is received by a node on its channel,
        // it means that a flow has been activated from the sender to him with the bandwidth indicated.
        let mut receivers = Vec::new();
        let mut senders = HashMap::new();
        for i in 0..nb_term {
            // create the channel
            let (sender, receiver) = mpsc::channel(100);
            receivers.push(Some(receiver));
            senders.insert(vertices[i], sender);
        }

        for i in 0..nb_term {
            tokio::spawn(launch_node(net.clone(), senders.clone(), receivers[i].take().unwrap(), i));
        }
        println!("All nodes launched, retrieving the information...");
        loop {
            println!("Choose between : [a src dest bandwidth] or [d src dest]");
            let mut reader = io::BufReader::new(tokio::io::stdin());
            let mut buffer = Vec::new();
            reader.read_until(b'\n', &mut buffer).await.unwrap();
            // parsing the command
            let s = match str::from_utf8(&*buffer) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Cannot read your input: {}", e);
                    continue;
                }
            };
            let command: Command;
            let mut iter = s.split_whitespace();
            match iter.next().unwrap() {
                "a" => command = Command::ACTIVATE,
                "d" => command = Command::DEACTIVATE,
                _ => {
                    eprintln!("unrecognized command");
                    continue;
                }
            }

            let mut flow = Flow::build(&0, &0, 0, 0);
            if let Ok(src) = usize::from_str(iter.next().unwrap()) {
                flow.source = &src;
            } else {
                eprintln!("Cannot parse the source.");
                continue;
            }

            if let Ok(dest) = usize::from_str(iter.next().unwrap()) {
                flow.destination = &dest;
            } else {
                eprintln!("Cannot parse the destination.");
                continue;
            }

            if let Command::ACTIVATE = command {
                if let Ok(bandwidth) = u32::from_str(iter.next().unwrap()) {
                    flow.max_allowed_bandwidth = bandwidth;
                } else {
                    eprintln!("Cannot parse the destination.");
                    continue;
                }
            }

            // send the command to the node
            println!("sending command to the node");
            senders.get(&flow.source).unwrap().send((command, flow)).await.unwrap();
        }
    }


    pub async fn launch_node<T: netgraph::Vertex>(graph: netgraph::Network<T>, node_list: HashMap<T, mpsc::Sender<(Command, Flow<'_, T>)>>, mut receiver: mpsc::Receiver<(Command, Flow<'_, T>)>, id: T) {
        // Used to store the active flows.
        // Node_id (destination) : Bandwidth
        let mut active_flows: HashSet<Flow<T>> = HashSet::new();

        // Used to store the available bandwidth on the graph
        let mut bw_graph = graph.clone();

        // Start listening on events
        while let Some(event) = receiver.recv().await {
            match event {
                (Command::ACTIVATE, mut flow) => {
                    if flow.max_allowed_bandwidth == 0 {
                        eprintln!("Cannot create a node with a target bandwidth to 0");
                        continue; // loop on the next message
                    }
                    println!("[{}] Received new flow to activate", id);

                    (active_flows, bw_graph) = update_flows(active_flows, &mut flow, &graph, bw_graph);
                    println!("[{}] Finish adding, new flows: {:?}", id, active_flows);

                    // Finally, update everyone
                    // Now we can get the flow we added and transmit it to the other guys
                    let new_flow = active_flows.get(&flow).unwrap().clone();
                    println!("[{}] Updating the other guys", id);
                    //broadcast_flow(&node_list, &new_flow, &id).await;
                }
                (Command::DEACTIVATE, mut flow) => {
                    // Check if we are legit to deactivate the flow
                    if !active_flows.contains(&flow) || flow.source != id {
                        eprintln!("Cannot deactivate non-existing flow, or I am not the source.");
                        continue; // loop on the next message
                    }
                    (active_flows, bw_graph) = remove_flow(active_flows, &mut flow, &graph);
                    // Finally update everyone!
                    // make sure that we set the flow bandwidth to 0
                    let mut new_flow = flow.clone();
                    new_flow.authorized_bandwidth = 0;
                    new_flow.max_allowed_bandwidth = 0;
                    println!("[{}] Updating the other guys", id);
                    //broadcast_flow(&node_list, &new_flow, &id).await;
                }
                (Command::UPDATE, mut flow) => {
                    if flow.max_allowed_bandwidth == 0 { // we are in deactivation
                        println!("[{}] Received update of a flow, need to remove it, calculating new flows", id);
                        (active_flows, bw_graph) = remove_flow(active_flows, &mut flow, &graph);
                    } else { // we need to add / update a new flow
                        println!("[{}] Received update of a flow, need to update my state", id);
                        (active_flows, bw_graph) = update_flows(active_flows, &mut flow, &graph, bw_graph);
                    }
                    println!("[{}] Finish update, new flows: {:?}", id, active_flows);
                }
            }
        }
    }
}