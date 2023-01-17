extern crate core;
extern crate serde_json;


use tokio::sync::mpsc::Sender;
use cgraph::CGraphUpdate;
use crate::controller::Ctrl;
use common::{RunnerConfig, Error};

mod controller;
mod perf;
mod data;

// todo: find automatically the local address and speed of auto-negotiation


pub async fn run(config: RunnerConfig, cgraph_update: Sender<CGraphUpdate>) -> Result<(), Error> {
    // Create the controller:
    println!("Creating the controller...");
    let mut controller = Ctrl::build(config).await?;
    println!("Controller created.");
    controller.init().await?;
    println!("Initialization of the controller...");
    controller.start(cgraph_update).await
}