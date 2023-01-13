use common::ReporterConfig;
use reporter::UsageAnalyzer;
use clap::Parser;


#[tokio::main]
async fn main() {
    // Parse the config
    let config = ReporterConfig::parse();
    
    let mut usage_analyzer = UsageAnalyzer::build(&config).await.unwrap();
    usage_analyzer.start().await;
}
