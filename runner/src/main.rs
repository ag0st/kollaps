use std::error::Error;
use common::Config;
use cmanager;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), impl Error> {
    let config = Config::parse();
    cmanager::run(config).await
}
