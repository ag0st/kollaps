use std::borrow::BorrowMut;
use std::cmp::{max, min, Ordering};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::{fmt, mem};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;

use matrix::SymMatrix;

pub trait Vertex: Eq + Hash + Clone + Display + Serialize + for<'a> Deserialize<'a> {}

impl<T: Eq + Hash + Clone + Display + Serialize + for<'a> Deserialize<'a>> Vertex for T {}

#[derive(Clone)]
pub struct Path<T: Vertex> {
    _links: Vec<Link<T>>,
    links_set: HashSet<Link<T>>,
}

impl<T: Vertex> Path<T> {
    pub fn new() -> Path<T> {
        Path {
            _links: Vec::new(),
            links_set: HashSet::new(),
        }
    }

    pub fn shared_edges(&self, other: &Path<T>) -> HashSet<Link<T>> {
        self.links_set
            .intersection(&other.links_set)
            .map(|e| e.clone())
            .collect::<HashSet<Link<T>>>()
    }

    pub fn links_set(&self) -> HashSet<Link<T>> {
        self.links_set.clone()
    }
}

#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
pub enum Duplex {
    HalfDuplex,
    FullDuplex,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Link<T> {
    /// Internal source and destination store the index
    /// in the graph where point source, resp. destination.
    /// It is use then to be able to produce the same hash for two links with permuted (source, destination)
    /// as usize can be ordered and we can place the smaller first in the hash method to produce consistent hash.
    /// We could have ask to implement Ord for T but it does not make sense for the user of the lib.
    internal_source: usize,
    internal_destination: usize,
    pub source: T,
    pub destination: T,
    pub bandwidth: u32,
    pub drop: f32,
    pub latency_jitter: (f32, f32),
    pub duplex: Duplex,
}

impl<T: Vertex> Clone for Link<T> {
    fn clone(&self) -> Self {
        Self {
            internal_source: self.internal_source.clone(),
            internal_destination: self.internal_destination.clone(),
            source: self.source.clone(),
            destination: self.destination.clone(),
            bandwidth: self.bandwidth,
            latency_jitter: self.latency_jitter,
            drop: self.drop,
            duplex: self.duplex,
        }
    }
}

impl<T: Vertex> Display for Link<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} -> {} : {} \n", self.source, self.destination, self.bandwidth)
    }
}

// For now, bidirectional edges
impl<T: Vertex> PartialEq for Link<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.source == other.source && self.destination == other.destination) ||
            (self.destination == other.source && self.source == other.destination)
    }
}

impl<T: Vertex> Eq for Link<T> {}

/// The Hash of a link is consistent. It means that for two links A, B
/// if (A.source = B.destination and A.destination = B.source) or (A.source = B.source and A.destination = B.destination) then,
/// the hash will be the same for A and B.
impl<T: Vertex> Hash for Link<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut src = self.internal_source;
        let mut dest = self.internal_destination;
        if src > dest {
            mem::swap(&mut src, &mut dest);
        }
        (src, dest).hash(state)
    }
}

/// A link is ordered regarding its speed.
impl<T: Vertex> PartialOrd for Link<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.bandwidth() < other.bandwidth() {
            Some(Ordering::Less)
        } else if self.bandwidth() > other.bandwidth() {
            Some(Ordering::Greater)
        } else {
            Some(Ordering::Equal)
        }
    }
}

impl<T: Vertex> Ord for Link<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl<T: Vertex> Link<T> {
    fn build(source: T, destination: T, internal_source: usize, internal_destination: usize, bandwidth: u32) -> Link<T> {
        Link {
            internal_source,
            internal_destination,
            source,
            destination,
            bandwidth,
            latency_jitter: (0f32, 0f32),
            drop: 0f32,
            duplex: Duplex::FullDuplex,
        }
    }
    fn build_all(source: T, destination: T, internal_source: usize, internal_destination: usize, bandwidth: u32, latency_jitter: (f32, f32), drop: f32) -> Link<T> {
        Link {
            internal_source,
            internal_destination,
            source,
            destination,
            bandwidth,
            latency_jitter,
            drop,
            duplex: Duplex::FullDuplex,
        }
    }

    pub fn set_duplex(&mut self, duplex: Duplex) {
        self.duplex = duplex;
    }

    pub fn bandwidth(&self) -> u32 {
        match self.duplex {
            Duplex::FullDuplex => self.bandwidth,
            Duplex::HalfDuplex => self.bandwidth / 2,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum PathStrategy {
    WidestPath,
    ShortestPath,
}

#[derive(Debug, Clone)]
struct Mapper<T>(HashMap<T, usize>);

impl<T: Eq + Hash> Default for Mapper<T> {
    fn default() -> Self {
        Mapper(HashMap::new())
    }
}

impl<T> Deref for Mapper<T> {
    type Target = HashMap<T, usize>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Mapper<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> Serialize for Mapper<T>
    where
        T: Serialize + Eq + Hash,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(v, k)?;
        }
        map.end()
    }
}

struct MapperVisitor<T> {
    marker: PhantomData<fn() -> Mapper<T>>
}

impl<T> MapperVisitor<T> {
    fn new() -> Self {
        MapperVisitor {
            marker: PhantomData
        }
    }
}

impl<'de, T> Visitor<'de> for MapperVisitor<T>
    where
        T: Deserialize<'de> + Eq + Hash,
{
    // The type that our Visitor is going to produce.
    type Value = Mapper<T>;

    // Format a message stating what data this Visitor expects to receive.
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("Mapper for network")
    }

    // Deserialize MyMap from an abstract "map" provided by the
    // Deserializer. The MapAccess input is a callback provided by
    // the Deserializer to let us see each entry in the map.
    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
    {
        let mut map = HashMap::with_capacity(access.size_hint().unwrap_or(0));

        // While there are entries remaining in the input, add them
        // into our map.
        while let Some((key, value)) = access.next_entry()? {
            // Adding in reverse
            if let Some(_) = map.insert(value, key) {
                eprintln!("Duplicate index in deserialization");
            }
        }

        Ok(Mapper(map))
    }
}

impl<'de, T> Deserialize<'de> for Mapper<T>
    where
        T: Deserialize<'de> + Eq + Hash,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
    {
        // Instantiate our Visitor and ask the Deserializer to drive
        // it over the input data, resulting in an instance of MyMap.
        deserializer.deserialize_map(MapperVisitor::new())
    }
}


#[derive(Serialize, Deserialize, Debug)]
/// Network represents a network with its nodes and inter-connection.
/// This structure offers multiple method and path strategy that helps working with the network.
/// Be aware that this structure manage caches for path finding and because of this, it is not safe
/// to make concurrent requests on a same instance. You need to protect it with a mutex.
pub struct Network<T: Eq + Hash> {
    links: SymMatrix<Option<Link<T>>>,
    mapper: Mapper<T>,
    path_strategy: PathStrategy,
    // #[serde(skip, default = "default_cache")]
    // shortest_path_cache: RefCell<HashMap<usize, (Vec<Option<u32>>, Vec<Option<usize>>)>>,
    // #[serde(skip, default = "default_cache")]
    // widest_path_cache: RefCell<HashMap<usize, (Vec<Option<u32>>, Vec<Option<usize>>)>>,
}

/// Functions for deserialization
// fn default_cache() -> RefCell<HashMap<usize, (Vec<Option<u32>>, Vec<Option<usize>>)>> {
//     RefCell::new(HashMap::new())
// }

impl<T: Vertex> Clone for Network<T> {
    fn clone(&self) -> Self {
        Network {
            links: self.links.clone(),
            mapper: self.mapper.clone(),
            path_strategy: self.path_strategy.clone(),
            // shortest_path_cache: RefCell::new(self.shortest_path_cache.borrow().clone()),
            // widest_path_cache: RefCell::new(self.widest_path_cache.borrow().clone()),
        }
    }
}

impl<T: Vertex> Display for Network<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut data = String::new();
        for i in 0..self.links.size() {
            for j in 0..i {
                if let Some(link) = self.links[(i, j)].as_ref() {
                    data.push_str(&*link.to_string());
                }
            }
        }
        write!(f, "{}", data)
    }
}

impl<T: Vertex> Network<T> {
    pub fn new(vertices: &Vec<T>, path_strategy: PathStrategy) -> Network<T> {
        Network {
            links: SymMatrix::new_fn(vertices.len(), |_, _| None),
            mapper: Mapper((0..vertices.len())
                .fold(HashMap::new(), |mut acc, i| {
                    acc.insert(vertices[i].clone(), i);
                    acc
                })),
            path_strategy,
            // shortest_path_cache: RefCell::new(HashMap::new()),
            // widest_path_cache: RefCell::new(HashMap::new()),
        }
    }

    #[allow(dead_code)]
    pub fn generate_random_network(nb_inter: usize, nb_term: usize, diff_speeds: usize, path_strategy: PathStrategy) -> (Network<usize>, Vec<usize>) {
        // create random generator
        let mut rng = rand::thread_rng();
        // create the network
        let vertices = (0..nb_term + nb_inter).collect();
        let mut net = Network::new(&vertices, path_strategy);

        // vector to collect the speed generated for each terminal nodes
        let mut speeds = vec![0; nb_term];

        // creating internal nodes
        let mut visited_nodes: Vec<usize> = Vec::new();
        let mut available_nodes: Vec<usize> = (0..nb_inter).map(|i| i + nb_term).collect();

        // visit a first internal node
        let first_node = available_nodes.remove_random(&mut rng).unwrap();
        visited_nodes.insert_set(first_node);


        while let Some(a) = available_nodes.remove_random(&mut rng) {
            // select a random visited node
            let v = visited_nodes.get_random(&mut rng).unwrap();
            // set the used available node in visited
            if !visited_nodes.insert_set(a) {
                panic!("cannot insert a node already existing inside the visited_nodes");
            }
            // create a speed
            let edge_speed = rng.gen_range(0..diff_speeds) as u32;
            net.add_edge(&v, &a, edge_speed);
        }

        // The internal backbone is created, we can add terminal nodes to it.
        let mut remaining_terminal = nb_term;
        while remaining_terminal > 0 {
            // select a visited node (one of the backbone)
            let v = visited_nodes.get_random(&mut rng).unwrap();
            // select the number of terminal nodes to place
            // min = 1 / max = 3
            let to_place = min(rng.gen_range(0..3) + 1, remaining_terminal);

            for i in 0..to_place {
                let current_node = nb_term - remaining_terminal + i;
                // generate an edge speed
                let edge_speed = rng.gen_range(0..diff_speeds);
                speeds[current_node] = edge_speed;
                net.add_edge(&v, &current_node, edge_speed as u32);
            }
            remaining_terminal -= to_place;
        }

        (net, speeds)
    }

    /// Map allow to easily convert a vertex given by a user into its internal representation
    /// (which is its index in the graph)
    /// If the vertex is not in the graph, None is returned.
    fn map(&self, vertex: &T) -> Option<usize> {
        if let Some(vertex) = self.mapper.get(vertex) {
            Some(vertex.clone())
        } else {
            eprintln!("[NETGRAPH]: Vertex {} is not in the graph", vertex);
            None
        }
    }

    /// Utility function that call self.map two times and return only if both vertex are in the graph.
    fn map_two(&self, vertex1: &T, vertex2: &T) -> Option<(usize, usize)> {
        let ver1 = self.map(vertex1);
        let ver2 = self.map(vertex2);
        match (ver1, ver2) {
            (Some(a), Some(b)) => Some((a.clone(), b.clone())),
            _ => None
        }
    }

    /// Vertices returns the set of vertices inside the network. It clones the vertices
    pub fn vertices(&self) -> HashSet<T> {
        self.mapper.keys().map(|k| k.clone()).collect::<HashSet<T>>()
    }

    pub fn bandwidth_matrix(&self, predicate: impl Fn(&&T) -> bool) -> (SymMatrix<u32>, Vec<T>) {
        let nodes = self.mapper.keys().filter(|n| predicate(n)).map(|v| v.clone()).collect::<Vec<T>>();
        let matrix = SymMatrix::new_fn(nodes.len(), |row, col|
            if row == col {
                u32::MAX
            } else {
                self.bandwidth_between(&nodes[row], &nodes[col]).unwrap()
            });
        (matrix, nodes)
    }

    pub fn edit_vertex(&mut self, new_vertex: T) {
        if let Some(v) = self.mapper.remove(&new_vertex) {
            self.mapper.insert(new_vertex, v);
        }
    }

    pub fn add_edge(&mut self, from: &T, to: &T, bandwidth: u32) {
        if let Some((from_inter, to_inter)) = self.map_two(from, to) {
            self.links[(from_inter, to_inter)] = Some(Link::build(from.clone(), to.clone(), from_inter, to_inter, bandwidth));
            // invalidate shortest / widest paths cache!
            self.clear_path_caches();
        }
    }

    pub fn add_edge_with_props(&mut self, from: &T, to: &T, bandwidth: u32, latency_jitter: (f32, f32), drop: f32) {
        if let Some((from_inter, to_inter)) = self.map_two(from, to) {
            self.links[(from_inter, to_inter)] = Some(Link::build_all(from.clone(), to.clone(), from_inter, to_inter, bandwidth, latency_jitter, drop));
            // invalidate shortest / widest paths cache!
            self.clear_path_caches();
        }
    }

    /// Exported method of the update_bandwidth_inter
    pub fn update_bandwidth(&mut self, from: &T, to: &T, bandwidth: u32) {
        if let Some((from, to)) = self.map_two(from, to) {
            self.update_bandwidth_inter(from, to, bandwidth);
        }
    }

    /// Update the bandwidth of a link represented by the source and destination
    fn update_bandwidth_inter(&mut self, from: usize, to: usize, bandwidth: u32) {
        if let Some(link) = self.links[(from, to)].borrow_mut() {
            link.bandwidth = bandwidth;
            self.clear_path_caches();
        }
    }

    fn clear_path_caches(&self) {
        // self.widest_path_cache.borrow_mut().clear();
        // self.shortest_path_cache.borrow_mut().clear();
    }

    /// Get the bandwidth between two nodes. Uses the strategy defined to find the path,
    /// and then compute the bandwidth between the two nodes along the path.
    pub fn bandwidth_between(&self, from: &T, to: &T) -> Option<u32> {
        if let Some((from, to)) = self.map_two(from, to) {
            let (val, path) = self.path_between(from);
            // if the strategy is widest path, the "val" contains already the bandwidth between
            // the nodes. Do not need to compute it again.
            if let PathStrategy::WidestPath = self.path_strategy {
                return val[to];
            }

            // check if a link exists
            if let None = path[to] {
                return None;
            }

            let mut dest = to;
            let mut speed = u32::MAX;
            while let Some(parent) = path[dest] {
                speed = min(speed, self.links[(dest, parent)].as_ref().unwrap().bandwidth());
                dest = parent;
            }
            Some(speed)
        } else {
            None
        }
    }

    /// Calculate the properties between the paths, bandwidth, drop, (latency, jitter)
    pub fn properties_between(&self, from: &T, to: &T) -> Option<(u32, f32, (f32, f32))> {
        if let Some((from, to)) = self.map_two(from, to) {
            let (_, path) = self.path_between(from);
            // check if a link exists
            if let None = path[to] {
                return None;
            }

            let mut dest = to;
            let mut bandwidth = u32::MAX;
            let mut drop = 0f32;
            let mut latency_jitter = (0f32, 0f32);
            while let Some(parent) = path[dest] {
                let link = self.links[(dest, parent)].as_ref().unwrap();
                bandwidth = min(bandwidth, link.bandwidth());
                drop = drop + link.drop;
                latency_jitter = (latency_jitter.0 + link.latency_jitter.0, latency_jitter.1 + link.latency_jitter.1);
                dest = parent;
            }
            Some((bandwidth, drop, latency_jitter))
        } else {
            None
        }
    }

    /// Update the bandwidth along a path between two nodes by applying the function given in parameter.
    /// The function "func" is Fn(old_bandwidth) -> new_bandwidth
    pub fn update_edges_along_path_by(&mut self, from: &T, to: &T, func: impl Fn(u32) -> u32) {
        if let Some((from, to)) = self.map_two(from, to) {
            let (_, path) = self.path_between(from);
            let mut last_child = to;
            while let Some(parent) = path[last_child] {
                let edge_speed = self.links[(parent, last_child)].as_ref().unwrap().bandwidth();
                self.update_bandwidth_inter(parent, last_child, func(edge_speed));
                last_child = parent;
            }
        }
    }

    /// Get the path between two nodes in the Path structure representation.
    pub fn get_path_between(&self, from: &T, to: &T) -> Option<Path<T>> {
        if let Some((from, to)) = self.map_two(from, to) {
            let (_, parents) = self.path_between(from);

            let mut links = Vec::new();
            let mut edges_set = HashSet::new();

            let mut last_child = to;

            while let Some(parent) = parents[last_child] {
                let link = self.links[(parent, last_child)].clone()
                    .expect("Must be present, or it is not a parent-child relation");
                links.push(link.clone());
                let newly_added = edges_set.insert(link);
                assert!(newly_added);
                last_child = parent;
            }
            links.reverse();
            Some(Path {
                _links: links,
                links_set: edges_set,
            })
        } else { None }
    }

    /// Get the path between a node to the other nodes in the graph regarding the Network strategy.
    /// In the case of Shortest Path, it applies Dijkstra's algorithm and gives (dist, parents).
    /// In the case of Widest Path, it gives (bandwidth, parents)
    fn path_between(&self, from: usize) -> (Vec<Option<u32>>, Vec<Option<usize>>) {
        match self.path_strategy {
            PathStrategy::WidestPath => {
                // let mut cache = self.widest_path_cache.borrow_mut();
                // if let Some(widest_path) = cache.get(&from) {
                //     widest_path.clone()
                // } else {
                let widest_path = self.widest_path(from);
                // cache.insert(from, widest_path.clone());
                widest_path
                // }
            }
            PathStrategy::ShortestPath => {
                // let mut cache = self.shortest_path_cache.borrow_mut();
                // if let Some(shortest_path) = cache.get(&from) {
                //     shortest_path.clone()
                // } else {
                let shortest_path = self.shortest_path(from);
                // cache.insert(from, shortest_path.clone());
                shortest_path
                // }
            }
        }
    }

    /// Calculate the shortest path to all other nodes, gives back a parents vector.
    fn shortest_path(&self, from: usize) -> (Vec<Option<u32>>, Vec<Option<usize>>) {
        let mut dist = vec![None; self.links.size()];
        dist[from] = Some(0);

        let mut parents = vec![None; self.links.size()];

        // Create vertex priority queue
        let mut to_visit: BinaryHeap<Pair> = BinaryHeap::new();

        // Add the source to the queue
        to_visit.push(Pair { node: from, val: dist[from].unwrap() });

        while let Some(vert) = to_visit.pop() {
            // No need to visit if we already found a shortest path to the current_src
            // Same energy as putting the node as "visited".
            // If the node has never been visited, it contains "None"
            // can unwrap safely because if something is in the queue, It MUST have a distance.
            if vert.val > dist[vert.node].unwrap() {
                continue;
            }

            for neigh in self.neighbours(vert.node) {
                // Add + 1 because the distance is in "hopes"
                let alt = dist[vert.node].unwrap() + 1;
                if dist[neigh] == None || alt < dist[neigh].unwrap() {
                    dist[neigh] = Some(alt);
                    parents[neigh] = Some(vert.node);
                    // add the new neighbour to the queue
                    to_visit.push(Pair { node: neigh, val: dist[neigh].unwrap() });
                }
            }
        }
        (dist, parents)
    }

    /// Gives the neighbours of a link in term of indexes.
    fn neighbours(&self, from: usize) -> Vec<usize> {
        let mut neighbours = Vec::new();
        for i in 0..self.links.size() {
            if let Some(_) = self.links[(from, i)] {
                if i != from {
                    neighbours.push(i);
                }
            }
        }
        neighbours
    }


    /// Calculate the widest path between the "from" node to all the others.
    /// The widest path is the path containing the most bandwidth.
    /// returns a vector containing the maximal bandwidth with the other nodes and secondly
    /// a vector containing the parent chaining to achieve this path.
    fn widest_path(&self, from: usize) -> (Vec<Option<u32>>, Vec<Option<usize>>) {
        let mut bandwidths: Vec<_> = (0..self.links.size()).map(|_| None).collect();
        let mut parents: Vec<Option<usize>> = (0..self.links.size()).map(|_| None).collect();
        let mut to_visit: BinaryHeap<Pair> = BinaryHeap::new();

        bandwidths[from] = Some(u32::MAX);
        to_visit.push(Pair {
            node: from,
            val: u32::MAX,
        });

        while let Some(Pair { node: current_src, val: bandwidth }) = to_visit.pop() {
            // No need to visit if we already found a widest path to the current_src
            // Same energy as putting the node as "visited".
            // If the node has never been visited, it contains "None"
            if let Some(bw) = bandwidths[current_src] {
                if bandwidth < bw {
                    continue;
                }
            }

            // get its neighbors
            let neighs = self.neighbours(current_src);
            for n in neighs {
                let current_link_bandwidth = self.links[(current_src, n)].as_ref().unwrap().bandwidth();

                let current_bandwidth = if let Some(bw) = bandwidths[n] {
                    max(bw, min(bandwidths[current_src].unwrap(), current_link_bandwidth))
                } else {
                    min(bandwidths[current_src].unwrap(), current_link_bandwidth)
                };


                // Relaxation
                if bandwidths[n] == None || current_bandwidth > bandwidths[n].unwrap() {
                    bandwidths[n] = Some(current_bandwidth);
                    parents[n] = Some(current_src);
                    to_visit.push(Pair { node: n, val: current_bandwidth })
                }
            }
        }
        return (bandwidths, parents);
    }


    // This method print the graph in latex format using tikzpicture.
    pub fn print_graph(&self, nb_term: usize) {
        let name_function = |x: usize| {
            if x < nb_term { format!("{x}") } else { format!("S{}", x - nb_term) }
        };
        let attributes = |i: usize, j: usize| {
            let mut attr = String::new();
            if i >= nb_term && j >= nb_term {
                attr.push_str(&*format!(" color=\"red\" penwidth=2.0"))
            }
            attr
        };
        println!("graph network_graph {{");
        for i in 0..self.links.size() {
            for j in 0..i {
                if let Some(edge) = self.links[(i, j)].as_ref() {
                    let name_1 = name_function(i);
                    let name_2 = name_function(j);
                    println!("\t{} -- {} [label={}{}];", name_1, name_2, edge.bandwidth(), attributes(i, j));
                }
            }
        }
        // print the color of each bridges
        for i in nb_term..self.links.size() {
            let name = name_function(i);
            println!("\t{} [color=\"red\"]", name)
        }
        println!("}}")
    }
}

/// A Pair represent a node and another value. It is usec by Widest-Path and Shortest-Path algorithm
/// to be stored inside a priority queue. It implements Ord in a way that using a Pair with BinaryHeap
/// will produce a Min-Priority-Queue based on the value.
#[derive(Copy, Clone, Eq, PartialEq)]
struct Pair {
    node: usize,
    val: u32,
}

// The priority queue depends on `Ord`.
// Explicitly implement the trait so the queue becomes a min-heap
// instead of a max-heap.
impl Ord for Pair {
    fn cmp(&self, other: &Pair) -> Ordering {
        // Notice that the we flip the ordering on val.
        // In case of a tie we compare positions - this step is necessary
        // to make implementations of `PartialEq` and `Ord` consistent.
        other.val.cmp(&self.val)
            .then_with(|| other.node.cmp(&self.node))
    }
}

// `PartialOrd` needs to be implemented as well.
impl PartialOrd for Pair {
    fn partial_cmp(&self, other: &Pair) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


/// The trait random remover allow to remove an element from a vector in a random way. Used
/// to generate random Networks!
trait RandomRemover {
    type Item;
    fn insert_set(&mut self, item: Self::Item) -> bool;
    fn get_random<R: Rng>(&self, rng: &mut R) -> Option<Self::Item>;
    fn remove_random<R: Rng>(&mut self, rng: &mut R) -> Option<Self::Item>;
}

impl<T: Clone + Eq> RandomRemover for Vec<T> {
    type Item = T;

    fn insert_set(&mut self, item: T) -> bool {
        return if self.contains(&item) {
            false
        } else {
            self.push(item);
            true
        };
    }

    fn get_random<R: Rng>(&self, rng: &mut R) -> Option<Self::Item> {
        if self.is_empty() {
            None
        } else {
            let index = rng.gen_range(0..self.len());
            self.get(index).cloned()
        }
    }

    fn remove_random<R: Rng>(&mut self, rng: &mut R) -> Option<Self::Item> {
        if self.len() == 0 {
            None
        } else {
            let index = rng.gen_range(0..self.len());
            Some(self.swap_remove(index))
        }
    }
}
