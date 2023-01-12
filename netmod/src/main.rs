use std::collections::HashMap;
use std::ptr;
use std::thread::sleep;
use std::time::Duration;

use futures::stream::StreamExt;
use redbpf::load::Loader;
use tokio::net::UnixStream;

use common::{message, SocketAddr};
use netmod::UsageAnalyzer;


#[tokio::main]
async fn main() {
    let mut u  = UsageAnalyzer::build("", "eth0").await.unwrap();
    u.start().await;

    loop {
        sleep(Duration::from_secs(1))
    }
}
