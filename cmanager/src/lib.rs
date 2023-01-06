extern crate core;
extern crate serde_json;

use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use crate::controller::Ctrl;
use common::Config;

mod controller;
mod perf;
mod data;

// todo: find automatically the local address and speed of auto-negotiation

#[derive(Debug)]
struct PError {
    message: String
}

impl PError {
    fn new(error: impl Error) -> PError {
        PError {
            message: format!("{}", error)
        }
    }
}

impl Display for PError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.message, f)
    }
}

impl From<std::io::Error> for PError {
    fn from(value: std::io::Error) -> Self {
        PError::new(value)
    }
}

impl Error for PError {}

pub async fn run(config: Config) -> Result<(), impl Error> {
    // Create the controller:
    println!("Creating the controller...");
    let mut controller = Ctrl::build(config).await;
    println!("Controller created.");
    controller.init().await?;
    println!("Initialization of the controller...");
    match controller.start().await {
        Ok(_) => Ok(()),
        Err(e) => Err(PError::new(e))
    }
}