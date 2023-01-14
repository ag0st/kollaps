

/// EmulCore is an emulation core. It is spawn when the node receive an emulation
/// to execute from the leader. Its responsibilities are to assure that the all attributed
/// application are running an monitored. At changes it communicate with the others EmulCore
/// dispatched across the cluster. It is also responsible to follow the dynamic events associated
/// with their associated nodes.
struct EmulCore {

}

// 1. For each assigned application,

// The target bandwidth is calculated when detecting a new flow, we then assigned to the flow the max
// possible found in the graph.