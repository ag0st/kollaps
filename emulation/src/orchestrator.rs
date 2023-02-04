use std::cmp::max;
use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use cgraph::CGraph;
use common::{ClusterNodeInfo, Result, Error, ErrorKind, ErrorProducer};
use matrix::SymMatrix;
use netgraph::Network;

use crate::data::Node;

pub struct Orchestrator {
    residual_graph: SymMatrix<u32>,
    host_mapping: Vec<ClusterNodeInfo>,
    attributed_emul: HashMap<ClusterNodeInfo, HashSet<Uuid>>,
    emulations: HashMap<Uuid, (SymMatrix<u32>, Vec<usize>)>,
    current_load: HashMap<ClusterNodeInfo, HashMap<Uuid, usize>>,
    max_load: usize,
    emanagers_port: u16,
}

impl Orchestrator {
    pub fn new(mut cgraph: CGraph<ClusterNodeInfo>, emanagers_port: u16, max_load: usize) -> Orchestrator {
        Self::convert_cgraph_nodeinfo(emanagers_port, &mut cgraph);
        let (residual_graph, host_mapping) = cgraph.speeds();
        Orchestrator {
            residual_graph,
            host_mapping,
            attributed_emul: HashMap::new(),
            emulations: HashMap::new(),
            current_load: HashMap::new(),
            max_load,
            emanagers_port,
        }
    }

    pub fn new_emulation(&mut self, mut network: Network<Node>, emul_id: Uuid) -> Result<Option<(Network<Node>, Vec<ClusterNodeInfo>)>> {
        // First thing is to convert the network into a SymMatrix that represent the bandwidth between each nodes
        let (matrix, mut nodes) = network.bandwidth_matrix(|n| n.is_app());
        let equivalence = self.find_isomorphism(matrix.clone())?;
        if let None = equivalence {
            // no isomorphism found for this
            return Ok(None);
        }

        // Use an hashset to be sure it does not contains doublons
        let mut cluster_nodes_affected = HashSet::new();

        let equivalence = equivalence.unwrap();
        for i in 0..matrix.size() {
            let host = self.host_mapping[equivalence[i]].clone();
            nodes[i].as_app_mut().set_host(host.clone());
            nodes[i].as_app_mut().set_index(i);

            // increase the load for this host
            if let Some(map) = self.current_load.get_mut(&host) {
                if let Some(load) = map.get_mut(&emul_id) {
                    *load += 1;
                } else {
                    (*map).insert(emul_id.clone(), 1);
                }
            } else {
                let mut map = HashMap::new();
                map.insert(emul_id.clone(), 1);
                self.current_load.insert(host.clone(), map);
            }

            let host = nodes[i].as_app().host();
            // replace it in the network
            network.edit_vertex(nodes[i].clone());
            // Save that we attributed somebody to this host
            if let Some(entry) = self.attributed_emul.get_mut(&host) {
                entry.insert(emul_id);
            } else {
                let mut set = HashSet::new();
                set.insert(emul_id);
                self.attributed_emul.insert(host.clone(), set);
            }

            cluster_nodes_affected.insert(host.clone());
        }
        // Now, we must reduce the bandwidth used by what we assigned
        for i in 0..matrix.size() {
            for j in 0..i {
                // Do not decrease the speed if the node equivalence for i and j are the same. It means
                // That the nodes i and j have been deployed on the same machine and it doesn't affect the bandwidth
                // of the host as the bandwidth with ourself is "unlimited"
                if i == j || equivalence[i] == equivalence[j] { continue; }
                self.residual_graph[(equivalence[i], equivalence[j])] -= matrix[(i, j)]
            }
        }

        // Save the emulation for when we need to remove it
        self.emulations.insert(emul_id, (matrix, equivalence));

        let cluster_nodes_affected = cluster_nodes_affected.into_iter().collect::<Vec<ClusterNodeInfo>>();
        Ok(Some((network, cluster_nodes_affected)))
    }

    pub fn stop_emulation(&mut self, emul_id: &Uuid) -> Result<()> {
        // Get the emulation
        if let Some((matrix, equivalence)) = self.emulations.get(emul_id) {
            // Add back the used bandwidth on the residual graph
            for i in 0..matrix.size() {
                for j in 0..i {
                    if i == j { continue; }
                    self.residual_graph[(equivalence[i], equivalence[j])] += matrix[(i, j)];
                }
            }

            // remove the emulation from attributed emul
            for (_, set) in self.attributed_emul.iter_mut() {
                set.remove(emul_id);
            }
            // remove the current load assigned with this emulation
            for (_, map) in self.current_load.iter_mut() {
                map.remove_entry(emul_id);
            }
        } else {
            return Err(Self::err_producer().create(ErrorKind::InvalidData, "no emulation registered"));
        }

        // Finally, remove the key from the map
        self.emulations.remove_entry(emul_id);
        Ok(())
    }

    pub fn stop_app_emulation_on(&mut self, emul_id: &Uuid, node: &ClusterNodeInfo, app_index: usize) -> Result<()> {
        // Get the emulation
        if let Some((matrix, equivalence)) = self.emulations.get(emul_id) {
            // Add back the used bandwidth on the residual graph
            for i in 0..matrix.size() {
                if i == app_index { continue; }
                self.residual_graph[(equivalence[app_index], equivalence[i])] += matrix[(app_index, i)];
            }
        } else {
            return Err(Self::err_producer().create(ErrorKind::InvalidData, "no emulation registered"));
        }

        // Decrease the current load
        let mut need_deletion = false;
        // Use it with pattern matching. It can happen that we already removed the emulation for this node
        // when aborting and this node was sending the clean stop in the same time. This way, we ensure no panic.
        if let Some(current_load) = self.current_load.get_mut(&node).unwrap().get_mut(&emul_id) {
            *current_load -= 1;
            need_deletion = *current_load == 0;
        }
        // Delete the emulation if the current load is 0
        if need_deletion {
            self.current_load.get_mut(&node).unwrap().remove_entry(&emul_id);
            self.attributed_emul.get_mut(node).as_mut().unwrap().remove(emul_id);
        }

        // Check if the emulation is still present on other host, if it is not the case, remove the emulation
        if let None = self.attributed_emul.iter().filter(|(_, l)| l.contains(emul_id)).last() {
            // delete the emulation
            self.emulations.remove_entry(emul_id);
        }
        Ok(())
    }

    pub fn update_with_cgraph(&mut self, mut cgraph: CGraph<ClusterNodeInfo>) -> Result<Vec<Uuid>> {
        println!("[Orchestrator] : Updating with CGraph");
        Self::convert_cgraph_nodeinfo(self.emanagers_port, &mut cgraph);
        println!("[Orchestrator] : Old Residual Graph Size : {}", self.residual_graph.size());

        let (matrix, nodes) = cgraph.speeds();
        // check all nodes that are / are not in our cluster
        let to_remove = self.host_mapping.iter().filter(|n| !nodes.contains(n)).map(|n| n.clone()).collect::<Vec<ClusterNodeInfo>>();
        let to_add = nodes.iter().filter(|n| !self.host_mapping.contains(n)).map(|n| n.clone()).collect::<Vec<ClusterNodeInfo>>();
        println!("[Orchestrator] : To Remove : {:?}", to_remove);
        println!("[Orchestrator] : To Add : {:?}", to_add);

        let mut uuid_affected_by_remove: HashSet<Uuid> = HashSet::new();
        for node in to_remove {
            let affected_uuids = self.remove_cluster_node(node).unwrap();
            for au in affected_uuids {
                uuid_affected_by_remove.insert(au);
            }
        }
        let uuid_affected_by_remove = uuid_affected_by_remove.iter().map(|u| u.clone()).collect::<Vec<Uuid>>();
        println!("[Orchestrator] : UUID Affected by removes : {:?}", uuid_affected_by_remove);

        if !to_add.is_empty() {
            let nodes_to_index = (0..nodes.len()).fold(HashMap::with_capacity(nodes.len()), |mut acc, index| {
                acc.insert(nodes[index].clone(), index);
                acc
            });
            for node in to_add.into_iter() {
                // First add the node in the host mapping
                self.host_mapping.push(node);
                let n = self.host_mapping.last().unwrap();
                // Then we will grow the residual graph and add it the correct speeds
                // growing
                let node_index = nodes_to_index.get(n).unwrap().clone();
                self.residual_graph = self.residual_graph.grow_fn(1, |row, col| {
                    if row == col {
                        u32::MAX
                    } else {
                        let other = if row == node_index {
                            col
                        } else {
                            row
                        };
                        let other_node_index = nodes_to_index.get(&self.host_mapping[other]).unwrap().clone();
                        matrix[(node_index, other_node_index)]
                    }
                });
            }
        }
        println!("[Orchestrator] : New Residual Graph Size : {}", self.residual_graph.size());
        Ok(uuid_affected_by_remove)
    }

    pub fn remove_cluster_node(&mut self, mut node: ClusterNodeInfo) -> Result<Vec<Uuid>> {
        node.port = self.emanagers_port;
        // Get the id of the node
        let mut id: Option<usize> = None;
        for i in 0..self.host_mapping.len() {
            if self.host_mapping[i].eq(&node) {
                id = Some(i);
                break;
            }
        }

        if let None = id {
            return Err(Self::err_producer().create(ErrorKind::InvalidData, "Cannot find the node to remove"));
        }

        let uuids_concerned = self.attributed_emul.values().flatten().map(|id| id.clone()).collect::<Vec<Uuid>>();
        for i in 0..uuids_concerned.len() {
            // We may try to remove to stop the same
            let _ = self.stop_emulation(&uuids_concerned[i]);
        }

        // All the emulations are now considered stopped, so the bandwidth regarding this node are
        // like in default, we can safely remove it from the SymMatrix
        self.residual_graph = self.residual_graph.remove_id(id.unwrap());
        self.host_mapping.remove(id.unwrap());
        self.attributed_emul.remove_entry(&node);

        Ok(uuids_concerned)
    }

    fn convert_cgraph_nodeinfo(port: u16, graph: &mut CGraph<ClusterNodeInfo>) {
        graph.map_nodes(|cni| cni.port = port)
    }

    fn find_isomorphism(&self, graph: SymMatrix<u32>) -> Result<Option<Vec<usize>>> {
        // Do we have enough nodes inside our residual graph? it defines with how many clones we have to test
        let clones = max(0, graph.size() as i32 - self.residual_graph.size() as i32) as usize;
        self.find_iso_internal(&graph, clones, Vec::new(), 5)
    }

    fn find_iso_internal(&self, g1: &SymMatrix<u32>, clones: usize, mut tested_comb: Vec<HashSet<usize>>, max_depth: usize) -> Result<Option<Vec<usize>>> {
        if max_depth == 0 {
            return Ok(None);
        }
        // If the number of clones is greater than 0, it means we must generate a new "ensemble"
        // by adding node in the residual
        let mut searching_set_size = self.residual_graph.size() - 1;
        if clones > 0 {
            searching_set_size = searching_set_size + self.residual_graph.size() * clones;
        }

        // Do while
        let mut combination = (0..g1.size()).collect::<Vec<usize>>();
        loop {
            // Transform the combination into a Set
            let combination_set = combination.iter().fold(HashSet::new(), |mut acc, val| {
                // This trick allows to not check two times the same combination by reducing every clone
                // to its smallest modulo class.
                let mut val = val % self.residual_graph.size();
                while !acc.insert(val.clone()) {
                    val = val + self.residual_graph.size()
                }
                acc
            });
            // Test if the combination has not already been tested. If not, test it. Otherwise,
            // Get the next combination.
            // We can also directly test if there is still remaining place on the chosen combination
            let mut remaining = true;
            for i in &combination_set {
                let index = i % self.residual_graph.size();
                remaining &= self.get_actual_load(&index) < self.max_load;
                if !remaining {
                    break;
                }
            }
            if !tested_comb.contains(&combination_set) & remaining {
                // create the sub-graph from the searching set and the combination
                let sub_graph = SymMatrix::new_fn(combination.len(), |row, col| {
                    let i1 = combination[row] % self.residual_graph.size();
                    let i2 = combination[col] % self.residual_graph.size();

                    // divide the available bandwidth by the number of the clones if they are not the same vertices
                    self.residual_graph[(i1, i2)]
                });

                let result = self.graph_isomorphism(g1, &sub_graph, &combination)?;
                match result {
                    None => {
                        // Not found yet, try the next combination
                        tested_comb.push(combination_set);
                    }
                    Some(mut equivalence) => {
                        // We found an equivalence!
                        // We mus transform the equivalence to correspond to the real residual
                        for i in 0..equivalence.len() {
                            equivalence[i] = combination[equivalence[i]] % self.residual_graph.size();
                        }
                        // Now we can give back the equivalence graph
                        return Ok(Some(equivalence));
                    }
                }
            }
            if let Some(comb) = next_comb(combination.clone(), g1.size(), searching_set_size) {
                combination = comb;
            } else {
                // No combination found for this number of clones
                break;
            }
        }
        // If we found nothing, try with more clones
        self.find_iso_internal(g1, clones + 1, tested_comb, max_depth - 1)
    }

    fn graph_isomorphism(&self, g1: &SymMatrix<u32>, residual: &SymMatrix<u32>, mapper: &Vec<usize>) -> Result<Option<Vec<usize>>> {
        if g1.size() != residual.size() {
            return Err(Self::err_producer().create(ErrorKind::InvalidData, "the graph must have the same size"));
        }
        let mut equivalence = (0..g1.size()).collect::<Vec<usize>>();
        let mut exchange_counter = vec![0; g1.size()];
        let mut exchange_index = 1;

        // First check
        if self.verify_permutation(g1, residual, &equivalence, mapper) {
            return Ok(Some(equivalence));
        }

        /*
         * Permutations are based on Heap's algorithm, which generates all possible
         * permutations for a given set of elements.
        */
        while exchange_index < g1.size() {
            if exchange_counter[exchange_index] < exchange_index {
                if exchange_index % 2 == 0 {
                    equivalence.swap(0, exchange_index);
                } else {
                    equivalence.swap(exchange_counter[exchange_index], exchange_index);
                }
                if self.verify_permutation(g1, residual, &equivalence, mapper) {
                    return Ok(Some(equivalence));
                }
                exchange_counter[exchange_index] += 1;
                exchange_index = 1;
            } else {
                exchange_counter[exchange_index] = 0;
                exchange_index = exchange_index + 1;
            }
        }
        Ok(None)
    }

    fn verify_permutation(&self, g1: &SymMatrix<u32>, residual: &SymMatrix<u32>, equivalence: &Vec<usize>, mapper: &Vec<usize>) -> bool {
        let size_before_cloning = self.residual_graph.size();
        let mut checked_hosts = HashSet::new();
        for i in 0..residual.size() {
            // Get the real host with the mapper
            let host = mapper[i] % size_before_cloning;
            if checked_hosts.contains(&host) {
                continue; // Do not recheck a same host if it contains clones.
            }
            checked_hosts.insert(host);

            // Get all the apps that are deployed on the host for this permutation
            let apps_on_same_host = (0..g1.size()).filter(|app| mapper[equivalence[*app]] % size_before_cloning == host).collect::<Vec<usize>>();

            // Check that the attributed number of app on this host is smaller than allowed ones
            // To do this, first get the actual load of the host
            let load = self.get_actual_load(&host);
            if self.max_load - load < apps_on_same_host.len() { // This node cannot support more PTs
                return false;
            }

            let apps_not_same_host = (0..g1.size()).filter(|app| mapper[equivalence[*app]] % size_before_cloning != host).collect::<Vec<usize>>();

            let mut total_required_bandwidth = 0;
            for same_host_app in &apps_on_same_host {
                for not_same_host_app in &apps_not_same_host {
                    total_required_bandwidth += g1[(*same_host_app, *not_same_host_app)];
                }
            }

            let other_hosts : Vec<usize> = equivalence.iter().filter_map(|other| {
                if mapper[*other] % size_before_cloning != host {
                    Some(mapper[*other] % size_before_cloning)
                } else {
                    None
                }
            }).collect();

            // Check that for this host to all the other hosts there is enough bandwidth.
            for other in &other_hosts {
                // If the requested bandwidth is greater than the one available in the residual graph,
                // it is not good
                if total_required_bandwidth > self.residual_graph[(host, *other)] {
                    return false;
                }
            }
        }
        true
    }

    fn get_actual_load(&self, host: &usize) -> usize {
        // Get the node
        let node = &self.host_mapping[*host];
        if let Some(map) = self.current_load.get(node) {
            map.iter().fold(0, |acc, (_, load)| acc + load)
        } else {
            0
        }
    }

    fn err_producer() -> ErrorProducer {
        Error::producer("orchestrator")
    }
}


/// Generates the next combination of n elements as k after comb
/// comb => the previous combination ( use (0, 1, 2, ..., k) for first)
/// k => the size of the subsets to generate
/// n => the size of the original set
///
/// Returns: True if a valid combination was found, False otherwise
/// See: https://compprog.wordpress.com/2007/10/17/generating-combinations-1/
fn next_comb(mut previous: Vec<usize>, k: usize, n: usize) -> Option<Vec<usize>> {
    let mut i = k - 1;
    previous[i] = previous[i] + 1;

    while previous[i] > n - k + 1 + i {
        i = i - 1;
        previous[i] = previous[i] + 1;
    }
    if previous[0] > n - k { // no more combination possible
        return None;
    }

    for v in i + 1..k {
        previous[v] = previous[v - 1] + 1;
    }
    Some(previous)
}


#[cfg(test)]
mod test {
    use std::net::IpAddr;
    use uuid::Uuid;
    use cgraph::{CGraph};
    use common::ClusterNodeInfo;
    use netgraph::{Network, PathStrategy};
    use crate::data;
    use crate::data::{Application, ApplicationKind};
    use crate::orchestrator::Orchestrator;

    #[test]
    fn test_distribution() {
        let cmanager_port = 8080;
        let emanager_port = 8081;
        // First create a CGraph
        let mut cgraph: CGraph<ClusterNodeInfo> = CGraph::new();
        // Add nodes in the cgraph
        let mut all_nodes: Vec<cgraph::Node<ClusterNodeInfo>> = Vec::new();
        for i in 0..10 {
            // create a random cluster node info
            let info = ClusterNodeInfo::new(IpAddr::from([192, 168, 1, i as u8 + 1]), cmanager_port);
            let node = cgraph.add_node(1000, info).unwrap();
            all_nodes.push(node);
            for j in 0..i {
                let _ = cgraph.add_link_direct_test(&all_nodes[i], &all_nodes[j], 1000);
            }
        }
        // Now we have a CGraph with interconnected nodes with 1000 MBits/s connections
        // We can create a topology: Aka a network to push
        /*
           100
        A --------            100       100
                 |------ S1 ------- S2 ------- C
        B --------
           50
         */
        // This topology must be well spread across the cluster
        let a = data::Node::App(0, Application::build(None, None, "A".to_string(), ApplicationKind::BareMetal));
        let b = data::Node::App(1, Application::build(None, None, "B".to_string(), ApplicationKind::BareMetal));
        let c = data::Node::App(2, Application::build(None, None, "C".to_string(), ApplicationKind::BareMetal));
        let s1 = data::Node::Bridge(3);
        let s2 = data::Node::Bridge(4);
        let mut services_and_bridges: Vec<data::Node> = vec![a, b, c, s1, s2];
        let mut topology = Network::new(&services_and_bridges, PathStrategy::ShortestPath);
        topology.add_edge(&services_and_bridges[0], &services_and_bridges[3], 100);
        topology.add_edge(&services_and_bridges[1], &services_and_bridges[3], 50);
        topology.add_edge(&services_and_bridges[3], &services_and_bridges[4], 100);
        topology.add_edge(&services_and_bridges[4], &services_and_bridges[2], 100);

        // Now we can create an orchestrator
        let mut orch = Orchestrator::new(cgraph, emanager_port, 4);

        // Now, we will try to add this to the maximum we can in the cluster and see if the distribution is ok
        let mut number_of_experiments = 0;
        while let Some(_) = orch.new_emulation(topology.clone(), Uuid::new_v4()).unwrap() {
            number_of_experiments += 1;
            // print the actual repartition
            // compute the max
            let max = orch.current_load.iter().map(|(_, loads)| loads.iter().fold(0, |acc, (_, load)| acc + load)).max().unwrap();
            let min = orch.current_load.iter().map(|(_, loads)| loads.iter().fold(0, |acc, (_, load)| acc + load)).min().unwrap();
            println!("{} \t {} \t {} \t {}", number_of_experiments, max, min, max - min);
            for distrib in &orch.current_load {
                println!("{:?}", distrib);
            }
        }
        assert_eq!(number_of_experiments, 13);
    }
}