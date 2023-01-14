use common::ReporterConfig;
use reporter::UsageAnalyzer;
use clap::Parser;


#[tokio::main]
async fn main() {
    // Parse the config
    println!("Reporter launching");

    let config = ReporterConfig::parse();

    println!("reporter config correctly parsed");
    
    let mut usage_analyzer = UsageAnalyzer::build(&config).await.unwrap();
    if let Err(e) = usage_analyzer.start().await {
        eprintln!("Error in the reporter, exiting: {:?}", e)
    }
}
