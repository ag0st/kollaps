mod config;
mod error;
mod subnet;
mod monitor;

// re-exporting the config object
pub use config::Config;

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