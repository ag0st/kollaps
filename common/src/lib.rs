mod runner_config;
mod error;
mod subnet;
mod monitor;
mod tc_message;
mod reporter_config;

use serde::{Deserialize, Serialize};
// re-exporting the configs objects
pub use runner_config::RunnerConfig;
pub use reporter_config::ReporterConfig;

// Exporting error handling
pub use error::Error;
pub use error::ErrorKind;
pub use error::Result;
pub use error::ErrorProducer;

// Exporting subnet and other structures
pub use subnet::Subnet;
pub use subnet::IpMask;

// Exporting monitor structures
pub use monitor::{SocketAddr, message};

// Exporting tc messages for communication between main app en reporter
pub use tc_message::{TCMessage, FlowConf, TCConf, Container};


// Utils functions
pub fn deserialize<'a, T: Deserialize<'a>>(data: &'a str) -> Result<T> {
    Ok(serde_json::from_str::<T>(data)?)
}

pub fn serialize<'a, T: Serialize>(data: &T) -> Vec<u8> {
    let json = serde_json::to_string(data).unwrap();
    json.as_bytes().to_vec()
}
