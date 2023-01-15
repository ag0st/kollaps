use std::io;

#[derive(Debug)]
pub enum Error {
    EBPF(redbpf::Error),
    IO(io::Error),
    NoIntAttached,
    ParseError
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Error::IO(value)
    }
}

impl From<redbpf::Error> for Error {
    fn from(value: redbpf::Error) -> Self {
        Error::EBPF(value)
    }
}