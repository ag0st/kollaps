use std::thread::sleep;
use std::time::Duration;
use reporter::UsageAnalyzer;


#[tokio::main]
async fn main() {

    let mut u = UsageAnalyzer::build("loopback-socket-1", "eth0").await.unwrap();
    u.start().await;

    loop {
        sleep(Duration::from_secs(1))
    }
}
