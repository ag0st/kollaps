use std::borrow::Borrow;
use std::fmt::{Debug, Display, Formatter};
use std::{result};


pub type Result<T> = result::Result<T, Error>;


pub enum ErrorKind {
    NotFound,
    AlreadyStarted,
    NoResource,
    PerfTestFailed,
    CommandFailed,
    OpcodeNotRecognized,
    NotASocketAddr,
    BadWrite,
    ConnectionInterrupted,
    BadMode,
    NoStream,
    UnexpectedEof,
    EmptyGraph,
    AlreadyExists,
    InconsistentState,
    InvalidData,
    DockerInit,
    /// Must not be used, special case when implement From trait from other error to this one.
    /// It will simply encapsulate the error.
    Wrapped,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            ErrorKind::NotFound => "Not Found",
            ErrorKind::AlreadyStarted => "Already Started",
            ErrorKind::NoResource => "No Resource",
            ErrorKind::PerfTestFailed => "Performance Test Failed",
            ErrorKind::CommandFailed => "Command Failed",
            ErrorKind::OpcodeNotRecognized => "Opcode Not Recognized",
            ErrorKind::Wrapped => "Wrapped error",
            ErrorKind::NotASocketAddr => "Not A Socket Address",
            ErrorKind::BadWrite => "Bad Write",
            ErrorKind::ConnectionInterrupted => "Connection Interrupted",
            ErrorKind::BadMode => "Bad Mode",
            ErrorKind::NoStream => "No Stream",
            ErrorKind::UnexpectedEof => "Unexpected EOF",
            ErrorKind::EmptyGraph => "Empty Graph",
            ErrorKind::AlreadyExists => "Already Exists",
            ErrorKind::InconsistentState => "Inconsistent State",
            ErrorKind::InvalidData => "Invalid Data",
            ErrorKind::DockerInit => "Docker Initialization"
        };
        write!(f, "{message}")
    }
}

pub struct ErrorProducer {
    location: String,
}

impl ErrorProducer {
    pub fn create(&self, kind: ErrorKind, message: &str) -> Error {
        Error::new(&*(self.location.clone()), kind, message)
    }
    pub fn wrap(&self, kind: ErrorKind, message: &str, error: impl std::error::Error + 'static + Send) -> Error {
        Error::wrap(&*(self.location.clone()), kind, message, error)
    }
}

pub struct Error {
    location: String,
    kind: ErrorKind,
    message: String,
    sub_error: Option<Box<dyn std::error::Error + Send>>,
}

impl Error {
    pub fn new(location: &str, kind: ErrorKind, message: &str) -> Error {
        let location = location.to_uppercase().to_owned();
        Error {
            location,
            kind,
            message: message.to_string(),
            sub_error: None,
        }
    }

    pub fn producer(location: &str) -> ErrorProducer {
        ErrorProducer { location: location.to_string() }
    }
    pub fn wrap(location: &str, kind: ErrorKind, message: &str, error: impl std::error::Error + 'static + Send) -> Error {
        let mut err = Error::new(location, kind, message);
        err.sub_error = Some(Box::new(error));
        err
    }
}


impl Debug for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(e) = self.sub_error.borrow() {
            write!(f, "[{}]: Error type: {} \t Error message: {}. \n{}", self.location, self.kind, self.message, e)
        } else {
            write!(f, "[{}]: Error type: {} \t Error message: {}.", self.location, self.kind, self.message)
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error {
            location: "".to_string(),
            kind: ErrorKind::Wrapped,
            message: "".to_string(),
            sub_error: Some(Box::new(value)),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error {
            location: "".to_string(),
            kind: ErrorKind::Wrapped,
            message: "".to_string(),
            sub_error: Some(Box::new(value)),
        }
    }
}