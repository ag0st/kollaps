mod config;
mod error;


// re-exporting the config object
pub use config::Config;

// Exporting error handling
pub use error::Error;
pub use error::ErrorKind;
pub use error::Result;
pub use error::ErrorProducer;