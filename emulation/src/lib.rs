extern crate core;

use tokio::sync::mpsc::Receiver;
use cgraph::CGraphUpdate;
use common::{Error, RunnerConfig};
use crate::controller::Controller;

mod controller;
mod data;
mod emulcore;
mod scheduler;
mod bwsync;
mod xmlgraphparser;
mod orchestrator;
mod pubapi;

pub async fn run(config: RunnerConfig, cgraph_update: Option<Receiver<CGraphUpdate>>) -> Result<(), Error> {
    // Create the controller:
    println!("Creating the controller...");
    let mut controller = Controller::build(config).await?;

    
    println!("Initialization of the controller...");
    controller.start(cgraph_update).await
}