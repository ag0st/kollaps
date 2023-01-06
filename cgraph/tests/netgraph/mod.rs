use std::cmp::min;
use std::collections::HashSet;

use rand::Rng;

use cgraph::matrix::Matrix;

#[allow(dead_code)]
pub struct Network {
    edges: Matrix<Option<usize>>,
    nb_inter: usize,
    nb_term: usize,
}

#[allow(dead_code)]
impl Network {
    pub fn new(nb_inter: usize, nb_term: usize) -> Network {
        Network {
            edges: Matrix::new_fn(nb_inter + nb_term, |_, _| None),
            nb_inter, nb_term
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
            if x < self.nb_term { format!("{x}") } else {format!("S{}", x - self.nb_term)}
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
                if let Some(weight) = self.edges[(i, j)] {
                    let name_1 = name_function(i);
                    let name_2 = name_function(j);
                    println!("\t{} -- {} [label={}{}];", name_1, name_2, weight, attributes(i, j));
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
        self.edges[(from, to)] = Some(speed);
    }

    pub fn node_speed(&self, node: usize) -> usize {
        for i in 0..self.edges.size() {
            if let Some(speed) = self.edges[(node, i)] {
                return speed;
            }
        }
        0
    }

    fn path_from(&self, from: usize, to: usize) -> Vec<Option<usize>> {
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

    pub fn max_speed(&self, from: usize, to: usize) -> usize {
        let paths = self.path_from(from, to);
        // check if a link exists
        if let None = paths[to] {
            return 0;
        }

        let mut dest = to;
        let mut speed = usize::MAX;
        while let Some(parent) = paths[dest] {
            speed = min(speed, self.edges[(dest, parent)].unwrap());
            dest = parent;
        }
        speed
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