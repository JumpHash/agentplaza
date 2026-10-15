//! What comes in: node batches (with the node's clock corrected), events, and the hub's own samples.

use apz_metrics::Event;
use apz_metrics::registry::Sample;
use apz_metrics::wire::Batch;

use super::Metrics;

/// The histogram each lifecycle event kind feeds, with its parts: (event kind, metric, [(part name, metric)]).
pub const EVENT_HISTS: &[(&str, &str, &[(&str, &str)])] = &[
    ("boot", "life.boot_ms", &[]),
    ("resume", "life.resume_ms", &[]),
    (
        "wake",
        "life.wake_ms",
        &[
            ("restore_ms", "life.wake_restore_ms"),
            ("ready_ms", "life.wake_ready_ms"),
        ],
    ),
    (
        "snapshot",
        "life.snapshot_ms",
        &[("paused_ms", "life.pause_ms")],
    ),
    ("fork", "life.fork_ms", &[]),
    ("sandbox_create", "life.sandbox_create_ms", &[]),
    ("move", "life.move_ms", &[]),
];

/// Stores an event and records its duration (and parts) into the lifecycle histograms, keyed by the node the
/// entity is on (`node` label `run` when the entity is a run or VM). Compactions count into `ctx.compactions` and
/// `ctx.freed_tokens` of the agent.
pub fn record_event(m: &Metrics, e: Event) {
    let _ = (m, e);
    todo!()
}

/// Feeds samples (the hub's own scrape, or a decoded batch) into the rollup, remembers the latest point of each
/// series for live frames, stores the windows that closed, and hands them to the alert checks.
pub fn samples(m: &Metrics, samples: &[Sample], now_ms: u64) {
    let _ = (m, samples, now_ms);
    todo!()
}

/// Takes a node's batch: measures the node's clock offset from `sent_ms` against `hub_now_ms` (smoothed, with
/// the beat's round trip when known), shifts every time by it, decodes columns into samples, records histograms
/// and events, keeps the reports, and records `link.*` for the node.
pub fn batch(m: &Metrics, node: &str, b: Batch, hub_now_ms: u64) {
    let _ = (m, node, b, hub_now_ms);
    todo!()
}
