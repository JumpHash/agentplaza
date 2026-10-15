//! The node's metrics for the hub: gauges, counters and histograms the collectors record into a registry, scraped
//! every 10 s (2 s while the hub streams this node live) into a buffer, plus lifecycle events and latest-only
//! reports. The node link (apz-link) takes the buffer as an `apz_metrics::wire::Batch` with every heartbeat
//! (`GET /v1/metrics/batch`), and every 2 s while live (`GET /v1/metrics/batch?live=1`, 204 when not live). The hub
//! asks for live batches with `POST /v1/metrics/live {"until_ms": …}`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use apz_metrics::registry::{Counter, Gauge, Hist, Registry, Sample};
use apz_metrics::wire::Batch;
use apz_metrics::{Event, SeriesKey};

/// The scrape interval, and the interval while live.
pub const INTERVAL_MS: u64 = 10_000;
pub const LIVE_INTERVAL_MS: u64 = 2_000;
/// At most this much is buffered when nothing collects it (the link is down): older samples are dropped.
pub const BUFFER_MS: u64 = 15 * 60_000;

#[derive(Default)]
struct Buffer {
    samples: Vec<Sample>,
    events: Vec<Event>,
    reports: serde_json::Map<String, serde_json::Value>,
}

pub struct NodeMetrics {
    registry: Registry,
    buffer: Mutex<Buffer>,
    live_until: AtomicU64,
}

impl Default for NodeMetrics {
    fn default() -> Self {
        NodeMetrics::new()
    }
}

impl NodeMetrics {
    pub fn new() -> NodeMetrics {
        NodeMetrics {
            registry: Registry::default(),
            buffer: Mutex::new(Buffer::default()),
            live_until: AtomicU64::new(0),
        }
    }

    pub fn gauge(&self, key: SeriesKey) -> Gauge {
        self.registry.gauge(key)
    }

    pub fn counter(&self, key: SeriesKey) -> Counter {
        self.registry.counter(key)
    }

    pub fn hist(&self, key: SeriesKey) -> Hist {
        self.registry.hist(key)
    }

    /// Records a lifecycle event (boot, wake, sleep, pause, resume, snapshot, exit) for the next batch.
    pub fn event(&self, e: Event) {
        let _ = (e, &self.buffer);
        todo!()
    }

    /// Replaces a latest-only report (`top_processes`, `gpus`, `volumes`).
    pub fn report(&self, name: &str, value: serde_json::Value) {
        let _ = (name, value);
        todo!()
    }

    /// Scrapes the registry into the buffer (every gauge, so a quiet value is still sent), dropping samples older
    /// than [`BUFFER_MS`].
    pub fn tick(&self, now_ms: u64) {
        let _ = now_ms;
        todo!()
    }

    /// Drains the buffer into a batch: one column per gauge or counter series (evenly spaced at the interval the
    /// samples were taken at; a missing step repeats for gauges and is zero for counters), histogram points, the
    /// events and the reports.
    pub fn batch(&self, now_ms: u64) -> Batch {
        let _ = now_ms;
        todo!()
    }

    /// The hub asked for live batches until `until_ms`.
    pub fn live(&self, until_ms: u64) {
        self.live_until.fetch_max(until_ms, Ordering::Relaxed);
    }

    pub fn is_live(&self, now_ms: u64) -> bool {
        self.live_until.load(Ordering::Relaxed) > now_ms
    }

    /// The scrape interval now.
    pub fn interval_ms(&self, now_ms: u64) -> u64 {
        if self.is_live(now_ms) {
            LIVE_INTERVAL_MS
        } else {
            INTERVAL_MS
        }
    }
}
