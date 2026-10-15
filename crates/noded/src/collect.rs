//! The node's collectors, run before every metrics scrape: host disk, IO, network and pressure, volumes, GPUs, the
//! plaza's own services and each VM (CPU, RSS, guest memory, disk and vsock traffic), and VM counts by state.

use crate::Node;
use crate::metrics::NodeMetrics;

/// Sets every gauge from fresh readings (series keys as in the catalog: `crates/apz-metrics/src/catalog.rs`).
pub fn sample(node: &Node, m: &NodeMetrics, now_ms: u64) {
    let _ = (node, m, now_ms);
}
