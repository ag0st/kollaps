use std::cmp::min;
use cgraph::CGraph;
use netgraph::Network;

struct TestResult {
    error_count: usize,
    nb_tests: usize,
    nb_avoided: usize,
    nb_useful: usize,
    graph: Network<usize>
}

#[test]
fn tests() {
    let ranges = [/*5, 10, 15, */20/*, 30, 40, 50, 60*/];
    let total_retry_per_range = 100;
    let diff_speeds = 10000;
    let sufficient_speed = 1000000;
    let mut csv = "".to_owned();
    let mut graph: Option<Network<usize>> = None;
    for i in 0..ranges.len() {
        let mut total_tests = 0;
        let mut total_avoided_tests = 0;
        let mut total_error_count = 0;
        let mut total_useful = 0;
        let max_switches = min(ranges[i] / 2, 50);
        let mut max_per_node_tests = 0.0;
        let mut min_per_node_tests = f64::MAX;
        for _ in 0..total_retry_per_range {
            let test_res = launch_test(ranges[i], max_switches, diff_speeds, sufficient_speed);
            total_tests += test_res.nb_tests;
            total_avoided_tests += test_res.nb_avoided;
            total_error_count += test_res.error_count;
            total_useful += test_res.nb_useful;
            let nb_tests_per_node = test_res.nb_tests as f64 / ranges[i] as f64;
            if nb_tests_per_node > max_per_node_tests {
                max_per_node_tests = nb_tests_per_node;
            }
            if nb_tests_per_node < min_per_node_tests {
                min_per_node_tests = nb_tests_per_node;
                graph = Some(test_res.graph);
            }
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
        let normal_tests = (ranges[i] - 1) * ((ranges[i]) / 2);
        csv.push_str(&*format!("{diff_speeds}\t{}\t{:.2}\t{:.1}\t{:.2}\t{:.2}\t{normal_tests}\t{avg_nb_tests}\n", ranges[i], avg_tests_per_nodes, percentage_useful, max_per_node_tests, min_per_node_tests));
        //graph.take().unwrap().print_graph(ranges[i]);
    }
    println!("{}", csv);
}

fn launch_test(nb_ter: usize, nb_inter: usize, diff_speed: usize, sufficient: usize) -> TestResult {
    // First, generate a random graph
    let (net, speeds) = Network::<usize>::generate_random_network(nb_inter, nb_ter, diff_speed, netgraph::PathStrategy::ShortestPath);

    let mut result = TestResult {
        error_count: 0,
        nb_tests: 0,
        nb_avoided: 0,
        nb_useful: 0,
        graph: net.clone()
    };

    // create a CGraph
    let mut cgraph = CGraph::<usize>::new();

    let leader_speed = speeds[0];

    cgraph.add_node(leader_speed, 0).expect("Cannot add node");
    // Add one by one the other nodes
    for i in 1..nb_ter {
        // Add the node
        let edge_speed = speeds[i];
        let me = cgraph.add_node(edge_speed, i).expect("cannot add node");
        while let Some(other) = cgraph.find_missing_from_me(me, sufficient) {
            let speed = net.bandwidth_between(&i, &other.info()).unwrap();
            result.nb_tests += 1;
            let useful = cgraph.add_link_direct_test(me, other, speed as usize).expect("cannot add link");
            if useful { result.nb_useful += 1; }
        }
    }
    // check the errors
    let (speeds_matrix, _) = cgraph.speeds();
    for i in 0..speeds_matrix.size() {
        for j in 0..speeds_matrix.size() {
            if i == j { continue; }
            if speeds_matrix[(i, j)] != net.bandwidth_between(&i, &j).unwrap() {
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