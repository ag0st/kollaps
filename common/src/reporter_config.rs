use clap::Parser;

#[derive(Parser, Default, Debug, Copy)]
#[clap(author="Romain Agostinelli", version, about)]
/// Application to attach to a network namespace and capable or reporting traffic usage information,
/// Flow creation an update and also capable of modifying the traffic control settings.
pub struct ReporterConfig {
    #[clap(long)]
    /// Socket used by the callee (this app) to send traffic information to the caller.
    pub flow_socket: String,
    #[clap(long)]
    /// The socket used by the caller to send Traffic Control commands
    pub tc_socket: String,
    #[clap(long)]
    /// The id of the software. It is used to know which instance send info on the flow socket.
    pub id: u32,
    #[clap(default_value_t=5, short, long)]
    /// Percentage variation used to determine if a flow has changed or not.
    pub percentage_variation: u32,
    #[clap(default_value_t=5, short, long)]
    /// Threshold defining which bandwidth report to considerate. All flow with a throughput under
    /// or equal to this threshold will be ignored.
    /// It means that if a flow of 1Mb is detected and the threshold is set to 5, it will not be reported
    /// and considered.
    pub ignore_threshold: u32,
    #[clap(default_value_t=2000, short, long)]
    /// Duration in milliseconds after what a flow is considered finished. If not detection to a certain
    /// destination is made after this period of time, the flow will be considered as no more existent.
    pub kill_flow_duration_ms: u32,
    #[clap(default_value_t=String::from("eth0"), long)]
    /// Network interface to monitor
    pub network_interface: u32,

}
