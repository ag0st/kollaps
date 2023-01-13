use std::thread::sleep;
use std::time::Duration;
use common::ReporterConfig;
use reporter::UsageAnalyzer;


#[tokio::main]
async fn main() {
    // Parse the config
    let mut config = ReporterConfig::parse();
    
    let mut usage_analyzer = UsageAnalyzer::build(&config).await.unwrap();
    usage_analyzer.start().await;
}
