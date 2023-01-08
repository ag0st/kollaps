use tokio::io;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use std::str;
use std::str::FromStr;
use bwsync::launch_node;
use bwsync::Command;
use bwsync::Flow;

#[tokio::main]
async fn main() {
    // configuration
    let nb_term = 3;

    println!("Creating a graph...");
    let mut net = netgraph::Network::new(2, nb_term);
    // 0, 1, 2 = terminal nodes and 3, 4 = internal nodes
    // 0 -> 3 100
    // 1 -> 3 50
    // 3 -> 4 100
    // 4 -> 2 100
    net.add_edge(0, 3, 100);
    net.add_edge(1, 3, 50);
    net.add_edge(3, 4, 100);
    net.add_edge(4, 2, 100);
    println!("Graph created!");

    println!("Creating communication channels");

    // The communication channels are done as follow:
    // (the sender, the bandwidth) when something is received by a node on its channel,
    // it means that a flow has been activated from the sender to him with the bandwidth indicated.
    let mut receivers = Vec::new();
    let mut senders = Vec::new();
    for _ in 0..nb_term {
        // create the channel
        let (sender, receiver) = mpsc::channel(100);
        receivers.push(Some(receiver));
        senders.push(sender);
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

        let mut flow = Flow::build(0, 0, 0, 0);
        if let Ok(src) = usize::from_str(iter.next().unwrap()) {
            flow.source = src;
        } else {
            eprintln!("Cannot parse the source.");
            continue;
        }

        if let Ok(dest) = usize::from_str(iter.next().unwrap()) {
            flow.destination = dest;
        } else {
            eprintln!("Cannot parse the destination.");
            continue;
        }

        if let Command::ACTIVATE = command {
            if let Ok(bandwidth) = usize::from_str(iter.next().unwrap()) {
                flow.target_bandwidth = bandwidth;
            } else {
                eprintln!("Cannot parse the destination.");
                continue;
            }
        }

        // send the command to the node
        println!("sending command to the node");
        senders[flow.source].send((command, flow)).await.unwrap();
    }
}