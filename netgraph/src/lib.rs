use std::borrow::BorrowMut;
use std::cmp::{min, Ordering};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::mem;

use rand::Rng;

use matrix::Matrix;

#[derive(Clone)]
pub struct Path {
    edges: Vec<Edge>,
    edges_set: HashSet<Edge>,
}

impl Path {
    pub fn new() -> Path {
        Path {
            edges: Vec::new(),
            edges_set: HashSet::new(),
        }
    }

    pub fn shared_edges(&self, other: &Path) -> HashSet<Edge> {
        self.edges_set
            .intersection(&other.edges_set)
            .map(|e| e.clone())
            .collect::<HashSet<Edge>>()
    }
}

#[derive(Copy, Clone, Eq, Ord)]
pub struct Edge {
    source: usize,
    destination: usize,
    pub speed: usize,
}

// For now, bidirectional edges
impl PartialEq for Edge {
    fn eq(&self, other: &Self) -> bool {
        (self.source == other.source && self.destination == other.destination) ||
            (self.destination == other.source && self.source == other.destination)
    }
}

// For now, bidirectional edges
impl Hash for Edge {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut src = self.source;
        let mut dest = self.destination;
        if src > dest {
            mem::swap(&mut src, &mut dest);
        }
        (src, dest).hash(state)
    }
}

impl PartialOrd for Edge {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.speed < other.speed {
            Some(Ordering::Less)
        } else if self.speed > other.speed {
            Some(Ordering::Greater)
        } else {
            Some(Ordering::Equal)
        }
    }
}

impl Edge {
    pub fn build(mut source: usize, mut destination: usize, speed: usize) -> Edge {
        if source > destination {
            mem::swap(&mut source, &mut destination);
        }
        Edge {
            source,
            destination,
            speed,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct Network {
    edges: Matrix<Option<Edge>>,
    nb_inter: usize,
    nb_term: usize,
}

#[allow(dead_code)]
impl Network {
    pub fn new(nb_inter: usize, nb_term: usize) -> Network {
        Network {
            edges: Matrix::new_fn(nb_inter + nb_term, |_, _| None),
            nb_inter,
            nb_term,
        }
    }

    pub fn generate_random_network(nb_inter: usize, nb_term: usize, diff_speeds: usize) -> Network {
        // create random generator
        let mut rng = rand::thread_rng();
        // create the network
        let mut net = Network::new(nb_inter, nb_term);

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
            let edge_speed = rng.gen_range(0..diff_speeds);
            net.add_edge(v, a, edge_speed);
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
                // generate an edge speed
                let edge_speed = rng.gen_range(0..diff_speeds);
                net.add_edge(v, nb_term - remaining_terminal + i, edge_speed);
            }
            remaining_terminal -= to_place;
        }

        net
    }

    /// This method print the graph in latex format using tikzpicture.
    pub fn print_graph(&self) {
        let name_function = |x: usize| {
            if x < self.nb_term { format!("{x}") } else { format!("S{}", x - self.nb_term) }
        };
        let attributes = |i: usize, j: usize| {
            let mut attr = String::new();
            if i >= self.nb_term && j >= self.nb_term {
                attr.push_str(&*format!(" color=\"red\" penwidth=2.0"))
            }
            attr
        };
        println!("graph network_graph {{");
        for i in 0..self.edges.size() {
            for j in 0..i {
                if let Some(edge) = self.edges[(i, j)] {
                    let name_1 = name_function(i);
                    let name_2 = name_function(j);
                    println!("\t{} -- {} [label={}{}];", name_1, name_2, edge.speed, attributes(i, j));
                }
            }
        }
        // print the color of each bridges
        for i in self.nb_term..self.edges.size() {
            let name = name_function(i);
            println!("\t{} [color=\"red\"]", name)
        }
        println!("}}")
    }

    pub fn add_edge(&mut self, from: usize, to: usize, speed: usize) {
        self.edges[(from, to)] = Some(Edge::build(from, to, speed));
    }

    pub fn update_speed(&mut self, from: usize, to: usize, speed: usize) {
        if let Some(edge) = self.edges[(from, to)].borrow_mut() {
            edge.speed = speed;
        }
    }

    pub fn node_speed(&self, node: usize) -> usize {
        for i in 0..self.edges.size() {
            if let Some(edge) = self.edges[(node, i)] {
                return edge.speed;
            }
        }
        0
    }


    pub fn bandwidth_between(&self, from: usize, to: usize) -> usize {
        let paths = self.parents_from(from, to);
        // check if a link exists
        if let None = paths[to] {
            return 0;
        }

        let mut dest = to;
        let mut speed = usize::MAX;
        while let Some(parent) = paths[dest] {
            speed = min(speed, self.edges[(dest, parent)].unwrap().speed);
            dest = parent;
        }
        speed
    }

    pub fn update_edges_along_path_by(&mut self, from: usize, to: usize, func: impl Fn(usize) -> usize) {
        let path = self.parents_from(from, to);
        let mut last_child = to;
        while let Some(parent) = path[last_child] {
            let edge_speed = self.edges[(parent, last_child)].unwrap().speed;
            self.update_speed(parent, last_child, func(edge_speed));
            last_child = parent;
        }
    }

    pub fn path_from(&self, from: usize, to: usize) -> Path {
        let parents = self.parents_from(from, to);

        let mut edges = Vec::new();
        let mut edges_set = HashSet::new();

        let mut last_child = to;

        while let Some(parent) = parents[last_child] {
            let edge = self.edges[(parent, last_child)]
                .expect("Must be present, or it is not a parent-child relation");
            edges.push(edge);
            let newly_added = edges_set.insert(edge);
            assert!(newly_added);
            last_child = parent;
        }
        edges.reverse();
        Path {
            edges,
            edges_set,
        }
    }

    fn parents_from(&self, from: usize, to: usize) -> Vec<Option<usize>> {
        let mut parents: Vec<Option<usize>> = (0..self.edges.size()).map(|_| None).collect();
        let mut queue: Vec<usize> = Vec::new();
        let mut visited: HashSet<usize> = HashSet::new();

        visited.insert(from);
        queue.push(from);
        while let Some(node) = queue.pop() {
            if node == to {
                break;
            }
            for neigh in self.neighbours(node) {
                let neigh = neigh.clone();
                if visited.insert(neigh) {
                    parents[neigh] = Some(node);
                    queue.push(neigh);
                }
            }
        }
        parents
    }

    fn neighbours(&self, from: usize) -> Vec<usize> {
        let mut neighbours = Vec::new();
        for i in 0..self.edges.size() {
            if let Some(_) = self.edges[(from, i)] {
                if i != from {
                    neighbours.push(i);
                }
            }
        }
        neighbours
    }
}

pub trait RandomRemover {
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
