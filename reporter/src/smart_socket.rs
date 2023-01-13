use std::path::{Path, PathBuf};
use tokio::net::UnixListener;

pub struct SmartSocketListener {
    path: PathBuf,
    listener: UnixListener,
}

impl SmartSocketListener {
    pub fn bind(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_owned();
        UnixListener::bind(&path).map(|listener| SmartSocketListener { path, listener })
    }
}

impl Drop for SmartSocketListener {
    fn drop(&mut self) {
        if self.path.exists() {
            // There's no way to return a useful error here
            println!("removing socket...");
            if let Err(e) = std::fs::remove_file(&self.path) {
                eprintln!("Cannot delete the socket because of: {}", e)
            }
        }
    }
}

impl std::ops::Deref for SmartSocketListener {
    type Target = UnixListener;

    fn deref(&self) -> &Self::Target {
        &self.listener
    }
}

impl std::ops::DerefMut for SmartSocketListener {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.listener
    }
}