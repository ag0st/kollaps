use std::cmp::min;
use cgraph::CGraph;
use crate::netgraph::Network;

mod netgraph;

struct TestResult {
    error_count: usize,
    nb_tests: usize,
    nb_avoided: usize,
    nb_useful: usize,
}

#[test]
fn tests() {
    let ranges = [5, 10, 15, 20/*, 30, 40, 50, 60*/];
    let total_retry_per_range = 100;
    let diff_speeds = 500;
    let sufficient_speed = 1000;
    let mut csv = "".to_owned();
    for i in 0..ranges.len() {
        let mut total_tests = 0;
        let mut total_avoided_tests = 0;
        let mut total_error_count = 0;
        let mut total_useful = 0;
        let max_switches = min(ranges[i] / 2, 50);
        for _ in 0..total_retry_per_range {
            let test_res = launch_test(ranges[i], max_switches, diff_speeds, sufficient_speed);
            total_tests += test_res.nb_tests;
            total_avoided_tests += test_res.nb_avoided;
            total_error_count += test_res.error_count;
            total_useful += test_res.nb_useful;
        }
        println!("Tests for {} (terminal nodes) and {} internal node", ranges[i], max_switches);

        let avg_nb_tests = total_tests as f64 / total_retry_per_range as f64;
        let avg_nb_avoided_tests = total_avoided_tests as f64 / total_retry_per_range as f64;
        let avg_percent_avoided_tests = if total_tests > 0 {
            100 - total_tests * 100 / (total_tests + total_avoided_tests)
        } else { 0 };
        let avg_tests_per_nodes = avg_nb_tests / ranges[i] as f64;
        let percentage_useful = total_useful as f64 * 100.0 / total_tests as f64;
        println!("ErrorCount {total_error_count} \n\
            Average number of tests: {avg_nb_tests} \n\
            Average number of avoided tests: {avg_nb_avoided_tests} \n\
            Average avoided tests in percent: {avg_percent_avoided_tests}%\n\
            # Tests / terminal node: {avg_tests_per_nodes}\
            \n\n---------------------------------------------\n");
        csv.push_str(&*format!("{diff_speeds} # diff. speeds \t{} #nodes \t{:.2} #t/n \t{:.1}% useful \n", ranges[i], avg_tests_per_nodes, percentage_useful));
    }
    println!("{}", csv);
}

fn launch_test(nb_ter: usize, nb_inter: usize, diff_speed: usize, sufficient: usize) -> TestResult {
    // create a result object
    let mut result = TestResult {
        error_count: 0,
        nb_tests: 0,
        nb_avoided: 0,
        nb_useful: 0,
    };

    // First, generate a random graph
    let net = Network::generate_random_network(nb_inter, nb_ter, diff_speed);

    // create a CGraph
    let mut cgraph = CGraph::<usize>::new();

    let leader_speed = net.node_speed(0);

    cgraph.add_node(leader_speed, 0).expect("Cannot add node");
    // Add one by one the other nodes
    for i in 1..nb_ter {
        // Add the node
        let edge_speed = net.node_speed(i);
        let me = cgraph.add_node(edge_speed, i).expect("cannot add node");
        while let Some(other) = cgraph.find_missing_from_me(me, sufficient) {
            let speed = net.max_speed(i, other.info());
            result.nb_tests += 1;
            let useful = cgraph.add_link_direct_test(me, other, speed).expect("cannot add link");
            if useful { result.nb_useful += 1; }
        }
    }
    // check the errors
    let speeds_matrix = cgraph.speeds();
    for i in 0..speeds_matrix.size() {
        for j in 0..speeds_matrix.size() {
            if i == j { continue; }
            if speeds_matrix[(i, j)] != net.max_speed(i, j) {
                result.error_count += 1
            }
        }
    }
    // check number of avoided tests
    // total test for n nodes
    result.nb_avoided = nb_ter * ((1 + nb_ter) / 2);
    //net.print_graph();
    result
}