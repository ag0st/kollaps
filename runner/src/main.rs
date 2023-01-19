use std::{env, fs};
use std::path::Path;

use common::{RunnerConfig, Error, ErrorKind, Result};
use cmanager;
use clap::Parser;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    // Check dependencies at runtime
    let needed_software = vec!["docker", "iperf3", "ip", "sudo", "nsenter"];
    check_software_dependency(needed_software)?;
    // Get the netmod executable
    let netmod = check_reporter_executable()?;

    // Parse the config and insert the netmod executable path in it
    let mut config = RunnerConfig::parse();
    config.reporter_exec_path = netmod;

    // remove mutability
    let config = config;

    let (cgraph_update_sender, cgraph_update_receiver) = if config.leader {
        let (sender, _receiver) = mpsc::channel(10);
        (Some(sender), Some(_receiver))
    } else {
        (None, None)
    };

    // Create channels for the different part of the application can communicate

    // Launch applications
    let conf_for_cmanager = config.clone();
    tokio::spawn(async move {
        cmanager::run(conf_for_cmanager, cgraph_update_sender).await.unwrap();
    });
    emulation::run(config, cgraph_update_receiver).await.unwrap();
    Ok(())
}


fn check_software_dependency(software: Vec<&str>) -> Result<()> {
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

fn check_reporter_executable() -> Result<String> {
    let paths = vec!["./target/release/reporter", "./target/debug/reporter"];
    for pa in paths {
        if Path::new(pa).exists() {
            return Ok(pa.to_string());
        }
    }
    Err(Error::new("program check", ErrorKind::NotFound, "Cannot find the netmod executable"))
}