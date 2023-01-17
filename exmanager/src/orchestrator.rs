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
}

impl Orchestrator {
    pub fn new(cgraph: CGraph<ClusterNodeInfo>) -> Orchestrator {
        let (residual_graph, host_mapping) = cgraph.speeds();
        Orchestrator {
            residual_graph,
            host_mapping,
            attributed_emul: HashMap::new(),
            emulations: HashMap::new(),
        }
    }

    pub fn new_emulation(&mut self, mut network: Network<Node>, emul_id: Uuid) -> Result<Option<Network<Node>>> {
        // First thing is to convert the network into a SymMatrix that represent the bandwidth between each nodes
        let (matrix, mut nodes) = network.bandwidth_matrix(|n| n.is_app());
        let equivalence = self.find_isomorphism(matrix.clone())?;
        if let None = equivalence {
            // no isomorphism found for this
            return Ok(None);
        }
        let equivalence = equivalence.unwrap();
        for i in 0..matrix.size() {
            let host = self.host_mapping[equivalence[i]].clone();
            nodes[i].as_app_mut().set_host(host);
            let h = nodes[i].as_app().host();
            // replace it in the network
            network.edit_vertex(nodes[i].clone());
            // Save that we attributed somebody to this host
            self.attributed_emul.get_mut(&h).unwrap().insert(emul_id);
        }
        // Now, we must reduce the bandwidth used by what we assigned
        for i in 0..matrix.size() {
            for j in 0..i {
                if i == j { continue; }
                self.residual_graph[(equivalence[i], equivalence[j])] -= matrix[(i, j)]
            }
        }

        // Save the emulation for when we need to remove it
        self.emulations.insert(emul_id, (matrix, equivalence));

        Ok(Some(network))
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
        } else {
            return Err(Self::err_producer().create(ErrorKind::InvalidData, "no emulation registered"));
        }

        // Finally, remove the key from the map
        self.emulations.remove_entry(emul_id);
        Ok(())
    }

    pub fn update_with_cgraph(&mut self, cgraph: CGraph<ClusterNodeInfo>) -> Result<()> {
        let (matrix, mut nodes) = cgraph.speeds();
        // check all nodes that are / are not in our cluster
        let to_remove = self.host_mapping.iter().filter(|n| !nodes.contains(n)).map(|n| n.clone()).collect::<Vec<ClusterNodeInfo>>();
        let to_add = nodes.iter().filter(|n| self.host_mapping.contains(n)).map(|n| n.clone()).collect::<Vec<ClusterNodeInfo>>();


        for node in to_remove {
            let _ = self.remove_cluster_node(node);
        }

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
                self.residual_graph.grow_fn(1, |row, col| {
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
        Ok(())
    }

    pub fn remove_cluster_node(&mut self, node: ClusterNodeInfo) -> Result<Vec<Uuid>> {
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
        self.attributed_emul.remove_entry(&node).unwrap();

        Ok(uuids_concerned)
    }

    fn find_isomorphism(&self, graph: SymMatrix<u32>) -> Result<Option<Vec<usize>>> {
        // Do we have enough nodes inside our residual graph? it defines with how many clones we have to test
        let clones = max(0, graph.size() - self.residual_graph.size());
        Self::find_iso_internal(&graph, &self.residual_graph, clones, Vec::new(), 5)
    }

    fn find_iso_internal(g1: &SymMatrix<u32>, residual: &SymMatrix<u32>, clones: usize, mut tested_comb: Vec<HashSet<usize>>, max_depth: usize) -> Result<Option<Vec<usize>>> {
        if max_depth == 0 {
            return Ok(None);
        }
        // If the number of clones is greater than 0, it means we must generate a new "ensemble"
        // by adding node in the residual
        let mut searching_set_size = residual.size();
        if clones > 0 {
            searching_set_size = searching_set_size + residual.size() * clones;
        }


        let mut comb = (0..g1.size()).collect::<Vec<usize>>();
        // For each combination in this set, test it
        while let Some(combination) = next_comb(comb.clone(), g1.size(), searching_set_size) {
            let combination_set = combination.iter().fold(HashSet::new(), |mut acc, val| {
                // This trick allows to not check two times the same combination by reducing every clone
                // to its smallest modulo class.
                let mut val = val % residual.size();
                while !acc.insert(val.clone()) {
                    val = val * residual.size()
                }
                acc
            });
            if tested_comb.contains(&combination_set) {
                // Skip this combination
                comb = combination;
                continue;
            }
            // create the sub-graph from the searching set and the combination
            let sub_graph = SymMatrix::new_fn(combination.len(), |row, col| {
                residual[(combination[row], combination[col])]
            });

            let result = Self::graph_isomorphism(g1, &sub_graph)?;
            match result {
                None => {
                    // Not found yet, try the next combination
                    comb = combination;
                    tested_comb.push(combination_set);
                    continue;
                }
                Some(mut equivalence) => {
                    // We found an equivalence!
                    // We mus transform the equivalence to correspond to the real residual
                    for i in 0..equivalence.len() {
                        equivalence[i] = equivalence[i] % residual.size();
                    }
                    // Now we can give back the equivalence graph
                    return Ok(Some(equivalence));
                }
            }
        }

        // If we found nothing, try with more clones
        Self::find_iso_internal(g1, residual, clones + 1, tested_comb, max_depth - 1)
    }

    fn graph_isomorphism(g1: &SymMatrix<u32>, residual: &SymMatrix<u32>) -> Result<Option<Vec<usize>>> {
        if g1.size() != residual.size() {
            return Err(Self::err_producer().create(ErrorKind::InvalidData, "the graph must have the same size"));
        }
        let mut equivalence = (0..g1.size()).collect::<Vec<usize>>();
        let mut exchange_counter = vec![0; g1.size()];
        let mut exchange_index = 1;

        // First check
        if Self::verify_permutation(g1, residual, &equivalence) {
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
                if Self::verify_permutation(g1, residual, &equivalence) {
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

    fn verify_permutation(g1: &SymMatrix<u32>, residual: &SymMatrix<u32>, equivalence: &Vec<usize>) -> bool {
        for i in 0..equivalence.len() {
            for j in 0..i {
                // If the requested bandwidth is greater than the one available in the residual graph,
                // it is not good
                if g1[(i, j)] > residual[(equivalence[i], equivalence[j])] {
                    return false;
                }
            }
        }
        true
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