use clap::Parser;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use common::OManagerMessage;
use nethelper::DefaultHandler;
use nethelper::MessageWrapper;
use nethelper::Protocol;
use nethelper::UDP;

#[tokio::main]
async fn main() {
    let config = ClientConfig::parse();

    let mut file = File::open(config.topology_file).await.expect("File not found");
    let mut data = String::new();
    file.read_to_string(&mut data).await
        .expect("Error while reading file");
    let message = OManagerMessage::NewTopology(data);
    let (tx, mut rx) = mpsc::channel::<MessageWrapper<OManagerMessage>>(1);
    let handler = DefaultHandler::new(tx);
    let mut binding = UDP::bind(Some(handler)).await.unwrap();
    binding.broadcast(message, config.omanger_port).await.unwrap();

    if let Some(mess) = rx.recv().await {
        println!("{}", mess.message)
    }
}

#[derive(Parser, Default, Debug, Clone)]
#[clap(author = "Romain Agostinelli", version, about)]
/// Application used to send new topology to a Kollaps cluster in the network.
pub struct ClientConfig {
    #[clap(default_value_t = String::from("/home/agost/workspace/MSc/development/kollaps/client/topology.xml"), short, long)]
    /// The socket used by the caller to send Traffic Control commands
    pub topology_file: String,
    #[clap(default_value_t=8080, long)]
    pub omanger_port: u16,
}