//! The node's metrics, on `GET /v1/metrics` in the Prometheus text format (behind the node's token, like every route).
//! Each VM operation is timed here and also writes one `timing op=<op>` line on stderr (see `crate::registry`).

use apz_metrics::{Counter, Histogram, Metric};

/// A wake: the restore from the sleep snapshot, to the VM running.
pub static WAKE: Histogram = Histogram::new(
    "apz_node_wake_seconds",
    "A VM wake, from the request to the VM running",
);
/// Wakes that could not restore and booted the VM cold instead.
pub static WAKE_COLD: Counter = Counter::new(
    "apz_node_wake_cold_fallbacks_total",
    "VM wakes that could not restore and booted cold instead",
);
/// A sleep: the snapshot and the stop.
pub static SLEEP: Histogram = Histogram::new(
    "apz_node_sleep_seconds",
    "A VM sleep: its snapshot and its stop",
);
/// A checkpoint: the VM's memory and disk copied while it is paused.
pub static CHECKPOINT: Histogram = Histogram::new("apz_node_checkpoint_seconds", "A VM checkpoint");
/// A fork: the copies made and started.
pub static FORK: Histogram =
    Histogram::new("apz_node_fork_seconds", "A VM fork, until its copies run");
/// One VM start (boot or restore), from the spawn call to the VM running.
pub static SPAWN: Histogram = Histogram::new(
    "apz_node_spawn_seconds",
    "One VM start, from the spawn call to the VM running",
);

/// The Prometheus text of every node metric.
pub fn render() -> String {
    apz_metrics::render(&[
        &WAKE as &dyn Metric,
        &WAKE_COLD,
        &SLEEP,
        &CHECKPOINT,
        &FORK,
        &SPAWN,
    ])
}
