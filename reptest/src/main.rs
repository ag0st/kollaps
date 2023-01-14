use std::process::Command;
use std::str::FromStr;
use std::u32;
use tokio::io;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use common::{SocketAddr, Subnet, TCConf, TCMessage};
use dockhelper::DockerHelper;
use nethelper::{Handler, ProtoBinding, Protocol, Responder, Unix, UnixBinding};
use async_trait::async_trait;
use bytes::BytesMut;


#[derive(Clone)]
struct FlowHandler {
    sender: mpsc::Sender<TCMessage>,
}

#[async_trait]
impl Handler<TCMessage> for FlowHandler {
    async fn handle(&mut self, bytes: BytesMut) -> Option<TCMessage> {
        let mess = TCMessage::from_bytes(bytes).unwrap();
        println!("{}", mess);
        self.sender.send(mess).await.unwrap();
        None
    }
}


#[tokio::main]
async fn main() {
    println!("Launching the reporter");

    let tc_socket = "/tmp/tc_socket.sock";
    let flow_socket = "/tmp/flow_socket.sock";
    let ip = SocketAddr::from("192.168.1.200");
    let dest = SocketAddr::from("192.168.1.102");
    let gateway = "192.168.1.1";
    let subnet = Subnet::from(("192.168.1.0", 24));
    let interface = "wlp0s20f3";

    // Get the pid of the container
    let dock = DockerHelper::init(interface, subnet, gateway)
        .await.unwrap();

    // Start the container
    dock.launch_container("ebpf_test:1.0", "ebpf", ip.to_ip_addr(), None).await.unwrap();
    // get the pid of the container
    let pid = dock.get_pid("ebpf").await.unwrap();
    println!("Got the pid: {}", pid);


    println!("Listening on flow socket");
    let (sender, mut receiver) = mpsc::channel(1000);

    let mut listening = Unix::bind_addr(flow_socket, Some(FlowHandler { sender })).await.unwrap();
    listening.listen().unwrap();


    println!("Starting reporter");
    // Start the reporter
    tokio::spawn(async move {
        println!("launching reporter");
        Command::new("sudo").arg("-S") // must start with super user to enter another pid namespace
            .arg("nsenter")
            .arg("-t").arg(format!("{pid}"))
            .arg("-n")
            .arg("/home/agost/workspace/MSc/development/kollaps/target/release/reporter")
            .arg("--flow-socket").arg(flow_socket)
            .arg("--tc-socket").arg(tc_socket)
            .arg("--id").arg(format!("{}", ip.addr))
            .spawn().expect("Cannot launch reporter");
    });


    println!("Waiting for the reporter to be ready");
    if let TCMessage::SocketReady = receiver.recv().await.unwrap() {
        println!("Ready to continue!")
    } else {
        eprintln!("Received no good message, waiting for SocketReady");
        dock.stop_container("ebpf").await.unwrap();
        dock.delete_network().await.unwrap();
        panic!("Bad socket synchro")
    }

    println!("Connection to the tc socket");
    let mut binding: UnixBinding<TCMessage, Responder<TCMessage>> = Unix::bind_addr(tc_socket, None).await.unwrap();
    binding.connect().await.unwrap();
    let conf = TCConf {
        dest: dest.addr,
        bandwidth: None,
        latency_and_jitter: None,
        drop: None,
    };
    println!("init the TC");
    binding.send(TCMessage::TCInit(conf)).await.unwrap();


    loop {
        println!("Choose between : [l bandwidth] or [s]");
        let mut conf = TCConf {
            dest: *dest,
            bandwidth: None,
            latency_and_jitter: None,
            drop: None,
        };

        let mut reader = io::BufReader::new(tokio::io::stdin());
        let mut buffer = Vec::new();
        reader.read_until(b'\n', &mut buffer).await.unwrap();
        // parsing the command
        let s = match String::from_utf8(buffer) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Cannot read your input: {}", e);
                continue;
            }
        };
        let mut iter = s.split_whitespace();
        match iter.next().unwrap() {
            "l" => {}
            "s" => {
                println!("stopping");
                binding.send(TCMessage::TCTeardown).await.unwrap();
                dock.stop_container("ebpf").await.unwrap();
                dock.delete_network().await.unwrap();
                break;
            }
            _ => {
                eprintln!("unrecognized command");
                continue;
            }
        }

        if let Ok(limit) = u32::from_str(iter.next().unwrap()) {
            conf.bandwidth = Some(limit);
        } else {
            eprintln!("Cannot parse the limit.");
            continue;
        }


        // send the command to the node
        println!("sending command to the reporter");
        binding.send(TCMessage::TCUpdate(conf)).await.unwrap();
    }
}