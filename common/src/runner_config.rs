use clap::Parser;

#[derive(Parser, Default, Debug, Clone)]
#[clap(author="Romain Agostinelli", version, about)]
/// Base application used for Kollaps to maintain a cluster of machines
/// This is in development!
pub struct RunnerConfig {
    #[clap(default_value_t=false, long)]
    /// Start this application as leader
    pub leader: bool,
    #[clap(short('a'), long)]
    /// Local ip address
    pub ip_address: String,
    #[clap(short('s'), long)]
    /// Limiting speed to which we can access this device.
    /// Can be used as limiter.
    /// The speed must be smaller than the Auto-Negotiated speed for the current connection.
    pub local_speed: usize,
    #[clap(default_value_t=String::new(), long)]
    /// MANDATORY IF LEADER. IP range that can be used for the different container in the subnet. In
    /// CIDR format. For example: 192.168.1.200/28 = [200..215]
    pub ip_range: String,
    #[clap(long)]
    /// Current subnet. In CIDR format. For example: 192.168.1.0/24 = [200..215]
    pub subnet: String,
    #[clap(long)]
    /// Gateway of the subnet.
    pub gateway: String,
    #[clap(long)]
    /// Network interface to use for transmission
    pub interface: String,
    #[clap(default_value_t=usize::MAX, long)]
    /// The sufficient speed needed across the cluster. Take effect only if this node
    /// is the leader.
    pub sufficient_speed: usize,
    #[clap(default_value_t=8081, long)]
    /// Port used by TCP and UDP connection to communicate events regarding cluster management
    /// between the nodes of the cluster.
    pub cmanager_event_port: u16,
    #[clap(default_value_t=8082, long)]
    /// Port used by TCP connection to communicate events regarding emulations between the nodes
    /// of the cluster.
    pub emulation_event_port: u16,
    #[clap(default_value_t=5, long)]
    /// Time in second planned after all emulation core on the different machines are set, to begin
    /// the emulation. It allows to synchronize the emulation cores between themself.
    pub emulation_begin_delay: u16,
    #[clap(default_value_t=8080, long)]
    /// Port used by TCP connection to send new topology to emulate. This port is only used and opened
    /// by the leader to accept new topology. This is the only port that can be used from outside of the
    /// subnet. It allows user from outside to send new topologies to emulate.
    pub topology_submission_port: u16,
    #[clap(default_value_t=8083, long)]
    /// Port used to negotiate performance testing configuration between nodes.
    pub perf_port: u16,
    #[clap(default_value_t=5201, long)]
    /// Port used by iPerf3 during a performance test.
    pub iperf3_port: u16,
    #[clap(default_value_t=1000, long)]
    /// Size of the controller event channel.
    pub event_channel_size: usize,
    #[clap(default_value_t=3, long)]
    /// Number of heartbeat a node must miss before being considered as no more accessible.
    pub heartbeat_misses: usize,
    #[clap(default_value_t=5, long("heartbeat-timeout"))]
    /// In seconds. Time to wait for response between an heartbeat broadcast and the answers
    pub heartbeat_timeout_seconds: u64,
    #[clap(default_value_t=5, long("heartbeat-sleep"))]
    /// In seconds. Time to wait after finishing a round of heartbeat before beginning another one.
    pub heartbeat_sleep_seconds: u64,
    #[clap(default_value_t=15, long("perf-test-duration"))]
    /// In seconds.
    /// Duration of a performance test made by iPerf3.
    pub perf_test_duration_seconds: u8,
    #[clap(default_value_t=3, long)]
    /// Duration of a performance test made by iPerf3.
    pub perf_test_retries: usize,
    #[clap(default_value_t=20, long("cjq-timeout"))]
    /// In seconds.
    /// Timeout of Cluster Joining Request. After sending a request to detect local cluster,
    /// if no response arrive before the timeout, this node will
    /// promote itself as Leader of a new cluster
    pub cjq_timeout_duration_seconds: u64,
    #[clap(default_value_t=2, long)]
    /// After a CJQ Timeout, number of CJQ Request to retry.
    pub cjq_retry: usize,
    #[clap(default_value_t=30, long("cjq-wait-time"))]
    /// In seconds.
    /// Only one node can be added to the cluster at the time, so if a request arrive while
    /// there is already a node adding itself,
    /// we send a Waiting time to retry to the second request.
    pub cjq_waiting_time_seconds: u64,
    /// This is the path of the netmod executable to use.
    pub reporter_exec_path: String,
}
