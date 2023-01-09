extern crate core;
extern crate serde_json;


use crate::controller::Ctrl;
use common::{Config, Error};

mod controller;
mod perf;
mod data;

// todo: find automatically the local address and speed of auto-negotiation


pub async fn run(config: Config) -> Result<(), Error> {
    // Create the controller:
    println!("Creating the controller...");
    let mut controller = Ctrl::build(config).await;
    println!("Controller created.");
    controller.init().await?;
    println!("Initialization of the controller...");
    controller.start().await
}