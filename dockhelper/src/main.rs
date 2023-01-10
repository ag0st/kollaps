use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use dockhelper::{BaseCommand, NetworkCreateOpts, NetworkDriver, RunOpts};
use dockhelper::subnet::Subnet;

#[tokio::main]
async fn main() {
    let network_name = "macvlan";
    let network_output = BaseCommand::new()
        .network().create(NetworkCreateOpts::new()
                              .driver(NetworkDriver::MacVLan)
                              .subnet(Subnet::from(("192.168.1.0", 24)))
                              .gateway(IpAddr::from_str("192.168.1.1").unwrap())
                              .opt(("parent", "wlp0s20f3"))
                              .range(Subnet::from(("192.168.1.200", 28))), network_name).await;
    if let Ok(output) = network_output {
        let output = String::from_utf8_lossy(&output.stdout).to_string();
        println!("{}", output);

        let container_output = BaseCommand::new()
            .run(RunOpts::new()
                     .rm(true)
                     .interactive(true)
                     .tty(true)
                     .detach(true)
                     //.ip(IpAddr::from_str("192.168.1.200").unwrap())
                     .name("test-macvlan")
                     .network(network_name),
                 "alpine:latest",
                 Some("ash"),
            ).await;
        if let Ok(output) = container_output {
            let output = String::from_utf8_lossy(&output.stdout).to_string();
            println!("{}", output);
        } else {
            eprintln!("{}", container_output.unwrap_err())
        }
    } else {
        eprintln!("{}", network_output.unwrap_err())
    }
}
