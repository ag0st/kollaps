use std::{env, fs};

use common::{Config, Error, ErrorKind};
use cmanager;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let needed_software = vec!["docker", "iperf3"];
    check_software_dependency(needed_software)?;
    let config = Config::parse();
    cmanager::run(config).await
}


fn check_software_dependency(software: Vec<&str>) -> Result<(), Error> {
    for cmd in software {
        let mut found = false;
        if let Ok(path) = env::var("PATH") {
            for p in path.split(":") {
                let p_str = format!("{}/{}", p, cmd);
                if fs::metadata(p_str).is_ok() {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Err(Error::new("program check", ErrorKind::NotFound, &*format!("Command {} not found in your $PATH", cmd)));
        }
    }
    Ok(())
}