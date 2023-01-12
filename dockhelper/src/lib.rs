use std::borrow::Borrow;
use std::net::IpAddr;
use std::process::Output;
use std::str::FromStr;

use cmd::{BaseCommand, NetworkCommand, NetworkCreateOpts, NetworkDriver, NetworkLsOpts, RunOpts};
use common::{Error, ErrorKind, Result, Subnet};

mod cmd;

const NETWORK_NAME: &str = "kollaps_ipvlan";

pub struct DockerHelper {
    network_name: String,
}

impl DockerHelper {
    pub async fn init(interface: &str, subnet: Subnet, gateway: &str) -> Result<DockerHelper> {
        // First, be sure that docker is started
        //check_command(cmd::start_docker().await)?;

        // Check if the IPVlan network exists
        let cmd = BaseCommand::new().network().ls(
            NetworkLsOpts::new()
                .quiet(true)
                .filter(("name", NETWORK_NAME))
        ).await;

        let str_output = check_command(cmd)?;
        // check if we got something
        if str_output.len() == 0 {
            println!("[DOCKER HELPER]: Network {NETWORK_NAME} is not created, creating it...");
            create_ipvlan_network(interface, subnet, gateway).await?;
            //return Err(Error::new("docker helper", ErrorKind::AlreadyExists, &*format!("IPVlan network {NETWORK_NAME} already exists")));
        } else {
            println!("[DOCKER HELPER]: Network {NETWORK_NAME} already exists, skipping...");
        }

        let helper = DockerHelper {
            network_name: NETWORK_NAME.to_string()
        };
        Ok(helper)
    }

    pub async fn launch_container(&self, image: &str, name: &str, ip: IpAddr, cmd: Option<&str>) -> Result<String> {
        let mut container_config = RunOpts::new();
        container_config
            .rm(true)
            .interactive(true)
            .tty(true)
            .detach(true)
            .network(&*self.network_name)
            .name(name)
            .ip(ip);
        let cmd = BaseCommand::new().run(&mut container_config, image, cmd).await;
        check_command(cmd)
    }

    pub async fn stop_container(&self, name: &str) -> Result<String> {
        let cmd = BaseCommand::new().stop(name).await;
        check_command(cmd)
    }

    pub async fn delete_network(self) -> Result<String> {
        let cmd = BaseCommand::new().network().rm(&*self.network_name).await;
        check_command(cmd)
    }
}



async fn create_ipvlan_network(interface: &str, subnet: Subnet, gateway: &str) -> Result<()> {
    // If the network does not exists, create a new one
    let cmd = BaseCommand::new().network().create(
        NetworkCreateOpts::new()
            .driver(NetworkDriver::IpVlan)
            .gateway(IpAddr::from_str(gateway).unwrap())
            .subnet(subnet)
            .opt(("parent", interface)), NETWORK_NAME,
    ).await;
    match check_command(cmd) {
        Ok(_) => Ok(()),
        Err(e) => Err(Error::wrap("docker helper", ErrorKind::DockerInit, "Cannot create the docker network", e))
    }
}

pub fn check_command(res: Result<Output>) -> Result<String> {
    match res {
        Ok(output) => {
            let output = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(output)
        }
        Err(e) => {
            let err = Error::wrap("docker helper", ErrorKind::CommandFailed, "cannot execute command", e);
            eprintln!("{}", err);
            Err(err)
        }
    }
}