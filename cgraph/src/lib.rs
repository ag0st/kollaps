use io::Result;
use std::cmp::min;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::{Error, ErrorKind};

use serde::{Deserialize, Serialize};

pub mod matrix;

use crate::matrix::SymMatrix;

// is_limited : bool, is_direct_tested: bool, speed: usize
#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
pub struct TestData(bool, bool, usize);

impl TestData {
    fn default(_: usize, _: usize) -> TestData {
        TestData(true, false, 0)
    }
    fn set_limited(&mut self, is_limited: bool) {
        self.0 = is_limited
    }
    fn set_direct_tested(&mut self, is_directly_tested: bool) {
        self.1 = is_directly_tested
    }
    fn set_speed(&mut self, speed: usize) {
        self.2 = speed
    }
    fn is_limited(&self) -> bool {
        self.0
    }
    fn is_directly_tested(&self) -> bool {
        self.1
    }
    fn speed(&self) -> usize {
        self.2
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, Debug)]
pub struct Node<I: Clone + Eq> {
    index: usize,
    speed: usize,
    info: I,
}

impl<I: Clone + Eq> Hash for Node<I> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state)
    }
}

impl<I: Clone + Eq> PartialEq<Self> for Node<I> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<I: Clone + Eq> Eq for Node<I> {}

impl<I: Clone + Eq> Node<I> {
    fn build(index: usize, speed: usize, info: I) -> Self {
        Node { index, speed, info }
    }
    pub fn speed(&self) -> usize {
        self.speed
    }
    pub fn info(&self) -> I {
        self.info.clone()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CGraph<I: Clone + Eq> {
    c_graph: SymMatrix<TestData>,
    nodes: HashSet<Node<I>>,
}



impl<I: Clone + Eq> CGraph<I> {
    pub fn new() -> CGraph<I> {
        CGraph {
            c_graph: SymMatrix::new_fn(0, TestData::default),
            nodes: HashSet::new(),
        }
    }

    pub fn nodes(&self) -> HashSet<Node<I>> {
        self.nodes.clone()
    }

    pub fn size(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() == 0
    }

    pub fn find_leader(&self) -> Result<Node<I>> {
        // find the best average speed against all others
        if self.nodes.len() == 0 {
            return Err(Error::new(ErrorKind::Other, String::from("No element in the graph, no leader")));
        }
        let mut best_index = 0;
        let mut best_average: f64 = 0.0;
        for i in 0..self.c_graph.size() {
            let mut current_average: f64 = 0.0;
            for j in 0..self.c_graph.size() {
                if i == j { continue; }
                current_average = current_average + self.c_graph[(i, j)].speed() as f64;
            }
            let current_average = current_average / (self.c_graph.size() - 1) as f64;
            if current_average > best_average {
                best_index = i;
                best_average = current_average;
            }
        }
        let best_leader = self.nodes.iter()
            .filter(|n| n.index == best_index)
            .take(1)
            .last()
            .unwrap().clone();
        Ok(best_leader)
    }

    pub fn add_node(&mut self, speed: usize, info: I) -> Result<Node<I>> {
        // Check if the node is already in, if yes, return it
        if let Some(node) = self.nodes.iter()
            .filter(|n| n.info == info)
            .take(1)
            .last() {
            return Ok(node.clone());
        }

        // Add a row and a column to the c_graph for the node
        self.c_graph = self.c_graph.grow_fn(1, TestData::default);

        // set the speed with itself to maximum
        let index = self.c_graph.size() - 1;
        self.c_graph[(index, index)].set_speed(usize::MAX);

        // Create the node
        let node = Node::build(index, speed, info);
        if !self.nodes.insert(node.clone()) {
            Err(Error::new(
                ErrorKind::AlreadyExists,
                String::from("node already exists, big problem..."))
            )
        } else {
            Ok(node)
        }
    }

    pub fn remove_one_by(&mut self, predicate: impl FnMut(&&Node<I>) -> bool) -> Result<()> {
        let node = self.nodes.iter()
            .filter(predicate)
            .take(1)
            .last().expect("cannot find the node you want to remove, predicates returned nothing")
            .clone(); // clone to remove the immutable borrow of the set
        if !self.nodes.remove(&node) {
            let err = Error::new(
                ErrorKind::NotFound,
                String::from("cannot find the node you want to remove"));
            return Err(err);
        }
        let cgraph = self.c_graph.remove_id(node.index);

        // Now, as the matrix changed, the indexes stored inside the nodes are not correct anymore.
        // All indexes greater than the index of the node we removed must be reduced by 1.
        let new_nodes = self.nodes.iter().map(|n| {
            if n.index > node.index {
                Node::build(n.index - 1, n.speed, n.info.clone())
            } else { n.clone() }
        }).collect::<HashSet<Node<I>>>();

        if cgraph.size() != new_nodes.len() {
            Err(Error::new(ErrorKind::Other, "Inconsistency after removing node from CGraph"))
        } else {
            self.c_graph = cgraph;
            self.nodes = new_nodes;
            Ok(())
        }
    }


    pub fn add_link_direct_test(&mut self, from: Node<I>, to: Node<I>, speed: usize) -> Result<bool> {
        // check that both nodes are in the graph
        if !self.nodes.contains(&from) || !self.nodes.contains(&to) {
            return Err(
                Error::new(ErrorKind::Other,
                               String::from("your node is not in the system, add it first"),
                )
            );
        }

        // minimum between both nodes, used to check if the speed given is lower or equal
        // both nodes
        let min_speed = min(from.speed(), to.speed());

        // Check that the speed given is good
        if speed > min_speed {
            return Err(
                Error::new(
                    ErrorKind::InvalidData,
                    String::from("cannot have better performance than nodes speeds"),
                )
            );
        }

        // everything is ok, let enable the link between the two nodes
        self.set_not_limited(from.index, to.index);

        // check if the new information is useful, aka we improved the speed.
        let useful = self.c_graph[(from.index, to.index)].speed() < speed;

        // Register that a direct test has been made between the two nodes
        self.c_graph[(from.index, to.index)].set_direct_tested(true);

        // Register the speed of the test between the two nodes
        self.c_graph[(from.index, to.index)].set_speed(speed);

        // finally, update the speeds, use transitions
        self.update_speeds();

        Ok(useful)
    }

    pub fn speeds(&self) -> SymMatrix<usize> {
        SymMatrix::new_fn(self.nodes.len(), |row, col| {
            self.c_graph[(row, col)].speed().clone()
        })
    }

    // returns from and to
    pub fn find_missing_from_me(&mut self, me: Node<I>, sufficient_speed: usize) -> Option<Node<I>> {
        let mut missing = false;
        // simplification of use for the next algorithm.
        let from = &me;

        // list storing all the missing links
        let mut missing_links: Vec<Node<I>> = Vec::new();

        for to in self.nodes.iter() {
            // do not want to test against ourself
            if from == to { continue; }
            // If the speed between two nodes is limited by the path between the nodes and not the
            // limitation of each node, and if this speed has not been directly tested,
            // we need to test it to verify that the speed (found by transition) is correct.
            // We skip if the speed between the two node is sufficient or,
            // the speed has already been tested directly.
            if !self.c_graph[(from.index, to.index)].is_directly_tested() &&
                self.c_graph[(from.index, to.index)].speed() < sufficient_speed &&
                self.c_graph[(from.index, to.index)].speed() < min(from.speed(), to.speed()) {
                // variable used to store if a test must be made. There is some rules which
                // allow to be sure of the speed, even if it was extrapolated.
                let mut ignore_test = false;

                // 1st rule: If I directly tested my speed to another node "k" which is directly
                // connected (lvl1) to the target "to", If I am the limiter in my connection
                // with "k", no need to test the target "to" as I will not be able to make better
                // than a direct link from "k" to "to".
                // So: Find a "k" from which I directly tested my connection that is directly
                // linked to "to" During the search, we can create new links as "shortcut" between
                // "from" and "to" if we find a "k" compliant with the definition above.
                for k in self.nodes.iter() {
                    if k == from || k == to { continue; }

                    // Application of the rule
                    if (!self.c_graph[(from.index, k.index)].is_limited() || self.c_graph[(from.index, k.index)].is_directly_tested()) &&
                        (!self.c_graph[(k.index, to.index)].is_limited() || self.c_graph[(k.index, to.index)].is_directly_tested()) {
                        let from_k_speed = self.c_graph[(from.index, k.index)].speed();
                        let k_to_speed = self.c_graph[(k.index, to.index)].speed();
                        if from_k_speed == k_to_speed {
                            // we cannot tell anything
                            continue;
                        }
                        if from_k_speed > k_to_speed {
                            self.c_graph[(from.index, to.index)].set_speed(k_to_speed);
                            ignore_test = true;
                        } else if from_k_speed < k_to_speed {
                            self.c_graph[(from.index, to.index)].set_speed(from_k_speed);
                            // Set that we ignore the test
                            ignore_test = true;
                        }
                        if ignore_test {
                            self.c_graph[(from.index, to.index)].set_direct_tested(true);
                            break;
                        }
                    }
                }

                if ignore_test {
                    continue;
                } else {
                    missing = true;
                    // need to convert back 'from' and 'to' to node id
                    missing_links.push(to.clone());
                }
            }
        }
        // if we found missing links, choose the best one to test first
        if missing {
            let node = missing_links
                .iter()
                .max_by(|a, b| a.speed.cmp(&b.speed))
                .expect("cannot find fastest node");
            Some(node.clone())
        } else {
            None
        }
    }


    fn set_not_limited(&mut self, from_index: usize, to_index: usize) {
        self.c_graph[(from_index, to_index)].set_limited(false);
    }

    /// updates the speeds regarding possible transitions, it allows to
    /// extrapolate speeds with what we already know.
    /// It is inspired by the Floyd-Warshall algorithm
    fn update_speeds(&mut self) {
        for i in 0..self.c_graph.size() {
            for j in 0..self.c_graph.size() {
                for k in 0..self.c_graph.size() {
                    // get the minimum by transition, the minimal speed is the one bottle-necking the connection.
                    let speed_by_transition = min(self.c_graph[(i, k)].speed(), self.c_graph[(k, j)].speed());
                    // If we find a better speed by transition, adapt the speed
                    if self.c_graph[(i, j)].speed() < speed_by_transition {
                        self.c_graph[(i, j)].set_speed(speed_by_transition);

                        // If the limitation became one of the two node
                        // we can safely say that their connection is not limited by the network.
                        let min_speed = self.nodes.iter()
                            .filter(|a| a.index == i || a.index == j)
                            .map(|node| node.speed())
                            .min().expect("cannot find the two nodes in the hashset");
                        if min_speed == speed_by_transition {
                            self.set_not_limited(i, j)
                        }
                    }
                }
            }
        }
    }
//     /// gives the directly connected nodes of the node "of".
//     fn get_neighbors_lvl1(&self, of_in: usize) -> Vec<usize> {
//         let mut neighs = Vec::new();
//         for n in self.nodes_out_in.iter() {
//             let n = n.clone();
//             // not a neighbour of myself
//             if of_in == n { continue; }
//             if self.c_graph[(of_in, n)].is_limited() {
//                 neighs.push(n);
//             }
//         }
//         neighs
//     }
//
//     fn slowest_constraint(&self, a_in: usize, b_in: usize) -> usize {
//         min(self.constraints[a_in].speed, self.constraints[b_in].speed)
//     }
//
//     /// Calculate the widest path between the "from_in" node to all the other using
//     /// only level 1 constraints chaining.
//     /// The widest path is the path containing the most bandwidth.
//     /// returns a vector containing the maximal bandwidth with the other nodes and secondly
//     /// a vector containing the parent chaining to achieve this path.
//     fn widest_path_lvl1(&self, from_in: usize) -> (Vec<usize>, Vec<Option<usize>>) {
//         let mut speeds: Vec<_> = (0..self.nodes_out_in.len()).map(|_| usize::MIN).collect();
//         let mut parents: Vec<Option<usize>> = (0..self.nodes_out_in.len()).map(|_| None).collect();
//         let mut to_visit: BinaryHeap<Visit> = BinaryHeap::new();
//
//         speeds[self.nodes_in_out[from_in]] = usize::MAX;
//         to_visit.push(Visit {
//             node: self.nodes_in_out[from_in],
//             speed: 0,
//         });
//
//         while let Some(Visit { node: current_src, speed }) = to_visit.pop() {
//             // No need to visit if we already found a widest path to the current_src
//             // Same energy as putting the node as "visited".
//             if speed < speeds[current_src] {
//                 continue;
//             }
//
//             // get its neighbors
//             let neighs = self.get_neighbors_lvl1(self.nodes_out_in[current_src]);
//             for n in neighs {
//                 let n = self.nodes_in_out[n];
//                 let current_edge_weight = self.slowest_constraint(self.nodes_out_in[current_src],
//                                                                   self.nodes_out_in[n]);
//                 let current_speed = max(speeds[n],
//                                         min(speeds[current_src], current_edge_weight));
//
//                 // Relaxation
//                 if current_speed > speeds[n] {
//                     speeds[n] = current_speed;
//                     parents[n] = Some(current_src);
//                     to_visit.push(Visit { node: n, speed: current_speed as usize })
//                 }
//             }
//         }
//         return (speeds, parents);
//     }
// }
//
// #[derive(Copy, Clone, Eq, PartialEq)]
// struct Visit {
//     node: usize,
//     speed: usize,
// }
//
// // The priority queue depends on `Ord`.
// // Explicitly implement the trait so the queue becomes a min-heap
// // instead of a max-heap.
// impl Ord for Visit {
//     fn cmp(&self, other: &Visit) -> Ordering {
//         // Notice that the we flip the ordering on costs.
//         // In case of a tie we compare positions - this step is necessary
//         // to make implementations of `PartialEq` and `Ord` consistent.
//         other.speed.cmp(&self.speed)
//             .then_with(|| self.node.cmp(&other.node))
//     }
// }
//
// // `PartialOrd` needs to be implemented as well.
// impl PartialOrd for Visit {
//     fn partial_cmp(&self, other: &Visit) -> Option<Ordering> {
//         Some(self.cmp(other))
//     }
// }
}
