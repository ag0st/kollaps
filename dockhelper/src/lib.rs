use std::borrow::Borrow;
use std::collections::HashMap;
use std::io::BufReader;
use std::net::IpAddr;
use std::ops::{Deref, DerefMut};
use std::process::Output;
use tokio::process;
use common::{Result, Error, ErrorKind};

use crate::subnet::Subnet;

pub mod subnet;


pub struct BaseCommand(process::Command);

impl Deref for BaseCommand {
    type Target = process::Command;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BaseCommand {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl BaseCommand {
    pub fn new() -> BaseCommand {
        let cmd = process::Command::new("docker");
        BaseCommand(cmd)
    }
    pub fn network(mut self) -> NetworkCommand {
        self.arg("network");
        NetworkCommand { command: self }
    }
    pub async fn run(mut self, opts: &mut RunOpts, image: &str, command: Option<&str>) -> Result<Output> {
        self.arg("run");

        // --rm
        if opts.rm {
            self.arg("--rm");
        }

        // --detach
        if opts.detach {
            self.arg("-d");
        }
        // --interactive
        if opts.interactive {
            self.arg("-i");
        }
        // --tty
        if opts.tty {
            self.arg("-t");
        }

        // --network
        if let Some(network) = opts.network.borrow() {
            self.arg("--network").arg(network);
        }

        // --name
        if let Some(name) = opts.name.borrow() {
            self.arg("--name").arg(name);
        }

        // --volume
        if let Some(volume) = opts.volume.borrow() {
            self.arg("-v").arg(volume);
        }

        // --ip
        if let Some(ip) = opts.ip {
            self.arg("--ip").arg(ip.to_string());
        }

        // finally add the image
        self.arg(image);

        // Add the command if some
        if let Some(command) = command {
            self.arg(command);
        }

        launch_command_and_get_output(self.0).await
    }
}

pub struct RunOpts {
    rm: bool,
    detach: bool,
    interactive: bool,
    tty: bool,
    network: Option<String>,
    name: Option<String>,
    volume: Option<String>,
    ip: Option<IpAddr>,
}

impl RunOpts {
    pub fn new() -> RunOpts {
        RunOpts {
            rm: false,
            detach: false,
            interactive: false,
            tty: false,
            network: None,
            name: None,
            volume: None,
            ip: None,
        }
    }
    pub fn rm(&mut self, rm: bool) -> &mut Self {
        self.rm = rm;
        self
    }
    pub fn detach(&mut self, detach: bool) -> &mut Self {
        self.detach = detach;
        self
    }
    pub fn interactive(&mut self, interactive: bool) -> &mut Self {
        self.interactive = interactive;
        self
    }
    pub fn tty(&mut self, tty: bool) -> &mut Self {
        self.tty = tty;
        self
    }
    pub fn network(&mut self, network: &str) -> &mut Self {
        self.network = Some(network.to_string());
        self
    }
    pub fn name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.to_string());
        self
    }
    pub fn volume(&mut self, volume: &str) -> &mut Self {
        self.volume = Some(volume.to_string());
        self
    }
    pub fn ip(&mut self, ip: IpAddr) -> &mut Self {
        self.ip = Some(ip);
        self
    }
}


pub struct NetworkCommand {
    command: BaseCommand,
}

impl NetworkCommand {
    pub async fn create(mut self, opts: &mut NetworkCreateOpts, name: &str) -> Result<Output> {
        self.command.arg("create");

        // Check the Options
        self.command.arg("-d").arg(opts.driver.to_arg());

        // --gateway
        if let Some(gateway) = opts.gateway {
            self.command.arg(format!("{}={}", "--gateway", gateway.to_string()));
        }

        // --subnet
        if let Some(subnet) = opts.subnet.borrow() {
            self.command.arg(format!("{}={}", "--subnet", subnet.to_string()));
        }

        // --opt
        if !opts.opts.is_empty() {
            opts.opts.iter().for_each(|(a, b)| {
                self.command.arg("-o").arg(format!("{}={}", a, b));
            });
        }

        // --ip-range
        if let Some(range) = opts.range.borrow() {
            self.command.arg("--ip-range").arg(range.to_string());
        }

        // Finally add the name
        self.command.arg(name);

        // launch the command
        launch_command_and_get_output(self.command.0).await
    }
}

#[derive(Clone)]
pub enum NetworkDriver {
    Bridge,
    MacVLan,
}

impl NetworkDriver {
    fn to_arg(&self) -> &str {
        match self {
            NetworkDriver::Bridge => "bridge",
            NetworkDriver::MacVLan => "macvlan"
        }
    }
}

#[derive(Clone)]
pub struct NetworkCreateOpts {
    driver: NetworkDriver,
    gateway: Option<IpAddr>,
    subnet: Option<Subnet>,
    opts: HashMap<String, String>,
    range: Option<Subnet>,
}


impl NetworkCreateOpts {
    pub fn new() -> NetworkCreateOpts {
        NetworkCreateOpts {
            driver: NetworkDriver::Bridge,
            gateway: None,
            subnet: None,
            opts: HashMap::new(),
            range: None,
        }
    }

    pub fn driver(&mut self, driver: NetworkDriver) -> &mut NetworkCreateOpts {
        self.driver = driver;
        self
    }

    pub fn gateway(&mut self, gateway: IpAddr) -> &mut NetworkCreateOpts {
        self.gateway = Some(gateway);
        self
    }

    pub fn subnet(&mut self, subnet: Subnet) -> &mut NetworkCreateOpts {
        self.subnet = Some(subnet);
        self
    }

    pub fn range(&mut self, range: Subnet) -> &mut NetworkCreateOpts {
        self.range = Some(range);
        self
    }

    pub fn opt(&mut self, opt: (&str, &str)) -> &mut NetworkCreateOpts {
        self.opts.insert(opt.0.to_string(), opt.1.to_string());
        self
    }
}


async fn launch_command_and_get_output(mut com: process::Command) -> Result<Output> {
    let res = com.output().await.unwrap();
    if res.status.success() {
        Ok(res)
    } else {
        let output = String::from_utf8_lossy(&res.stderr).to_string();
        Err(Error::new("docker helper", ErrorKind::CommandFailed, &*format!("Command {:?} as failed with response: {}", com, output)))
    }
}
