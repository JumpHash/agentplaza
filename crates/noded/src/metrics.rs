//! The node's metrics for the hub: gauges, counters and histograms the collectors record into a registry, scraped
//! every 10 s (2 s while the hub streams this node live) into a buffer, plus lifecycle events and latest-only
//! reports. The node link (apz-link) takes the buffer as an `apz_metrics::wire::Batch` with every heartbeat
//! (`GET /v1/metrics/batch`), and every 2 s while live (`GET /v1/metrics/batch?live=1`, 204 when not live). The hub
//! asks for live batches with `POST /v1/metrics/live {"until_ms": …}`.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use apz_metrics::prom;
use apz_metrics::registry::{Counter, Gauge, Hist, Registry, Sample, Value};
use apz_metrics::wire::{self, Batch, HistPoint};
use apz_metrics::{Event, SeriesKey};

/// The scrape interval, and the interval while live.
pub const INTERVAL_MS: u64 = 10_000;
pub const LIVE_INTERVAL_MS: u64 = 2_000;
/// At most this much is buffered when nothing collects it (the link is down): older samples are dropped.
pub const BUFFER_MS: u64 = 15 * 60_000;
/// At most this many events are buffered: the oldest are dropped first.
const MAX_EVENTS: usize = 2_000;

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
        let mut buffer = self.buffer.lock().unwrap();
        buffer.events.push(e);
        if buffer.events.len() > MAX_EVENTS {
            let excess = buffer.events.len() - MAX_EVENTS;
            buffer.events.drain(..excess);
        }
    }

    /// Replaces a latest-only report (`top_processes`, `gpus`, `volumes`).
    pub fn report(&self, name: &str, value: serde_json::Value) {
        self.buffer
            .lock()
            .unwrap()
            .reports
            .insert(name.to_string(), value);
    }

    /// Scrapes the registry into the buffer (every gauge, so a quiet value is still sent), dropping samples older
    /// than [`BUFFER_MS`].
    pub fn tick(&self, now_ms: u64) {
        let scraped = self.registry.scrape(now_ms, true);
        let oldest = now_ms.saturating_sub(BUFFER_MS);
        let mut buffer = self.buffer.lock().unwrap();
        buffer.samples.extend(scraped);
        buffer.samples.retain(|s| s.t_ms >= oldest);
    }

    /// Drains the buffer into a batch: one column per gauge or counter series (evenly spaced at the interval the
    /// samples were taken at; a missing step repeats for gauges and is zero for counters), histogram points, the
    /// events and the reports.
    pub fn batch(&self, now_ms: u64) -> Batch {
        let mut buffer = std::mem::take(&mut *self.buffer.lock().unwrap());

        let mut scalars: BTreeMap<(SeriesKey, bool), Vec<(u64, f64)>> = BTreeMap::new();
        let mut hists = Vec::new();
        for sample in std::mem::take(&mut buffer.samples) {
            match sample.value {
                Value::Gauge(v) => scalars
                    .entry((sample.key, false))
                    .or_default()
                    .push((sample.t_ms, v)),
                Value::Counter(v) => scalars
                    .entry((sample.key, true))
                    .or_default()
                    .push((sample.t_ms, v)),
                Value::Hist(hist) => hists.push(HistPoint {
                    key: sample.key,
                    t_ms: sample.t_ms,
                    hist,
                }),
            }
        }

        let mut series = Vec::with_capacity(scalars.len());
        for ((key, counter), mut points) in scalars {
            points.sort_by_key(|p| p.0);
            series.push(column(key, counter, &points));
        }

        let reports = if buffer.reports.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::Object(std::mem::take(&mut buffer.reports))
        };

        Batch {
            node: String::new(),
            sent_ms: now_ms,
            series,
            hists,
            events: std::mem::take(&mut buffer.events),
            reports,
        }
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

/// Lays one series' points (sorted by time, at least one) on an evenly spaced grid from its first point to its last.
/// The step is the median gap between points, or [`INTERVAL_MS`] when there is no gap. A slot with no point repeats
/// the previous value for a gauge and is zero for a counter; two points in one slot keep the last value for a gauge
/// and add up for a counter.
fn column(key: SeriesKey, counter: bool, points: &[(u64, f64)]) -> wire::Column {
    let t0 = points[0].0;
    let step = median_gap(points).unwrap_or(INTERVAL_MS);
    let slot_of = |t: u64| ((t - t0) + step / 2) / step;
    let last_slot = slot_of(points[points.len() - 1].0) as usize;

    let mut slots: Vec<Option<f64>> = vec![None; last_slot + 1];
    for &(t, v) in points {
        let slot = &mut slots[slot_of(t) as usize];
        *slot = Some(match (counter, *slot) {
            (true, Some(sum)) => sum + v,
            _ => v,
        });
    }

    if counter {
        let values: Vec<f64> = slots.iter().map(|slot| slot.unwrap_or(0.0)).collect();
        wire::encode_counter(key, t0, step, &values)
    } else {
        let mut previous = 0.0;
        let values: Vec<f64> = slots
            .iter()
            .map(|slot| {
                if let Some(v) = slot {
                    previous = *v;
                }
                previous
            })
            .collect();
        wire::encode_gauge(key, t0, step, &values)
    }
}

/// The median of the positive gaps between consecutive points (the mean of the two middle gaps when even), or
/// `None` when no two points are apart.
fn median_gap(points: &[(u64, f64)]) -> Option<u64> {
    let mut gaps: Vec<u64> = points
        .windows(2)
        .map(|w| w[1].0 - w[0].0)
        .filter(|&gap| gap > 0)
        .collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    let mid = gaps.len() / 2;
    Some(if gaps.len() % 2 == 1 {
        gaps[mid]
    } else {
        (gaps[mid - 1] + gaps[mid]) / 2
    })
}

// The node's Prometheus metrics, on `GET /v1/metrics` in the Prometheus text format (behind the node's token, like
// every route): fixed statics for scraping and the lifecycle benchmark. Each VM operation is timed here and also
// writes one `timing op=<op>` line on stderr (see `crate::registry`).

/// A wake: the restore from the sleep snapshot, to the VM running.
pub static WAKE: prom::Histogram = prom::Histogram::new(
    "apz_node_wake_seconds",
    "A VM wake, from the request to the VM running",
);
/// Wakes that could not restore and booted the VM cold instead.
pub static WAKE_COLD: prom::Counter = prom::Counter::new(
    "apz_node_wake_cold_fallbacks_total",
    "VM wakes that could not restore and booted cold instead",
);
/// A sleep: the snapshot and the stop.
pub static SLEEP: prom::Histogram = prom::Histogram::new(
    "apz_node_sleep_seconds",
    "A VM sleep: its snapshot and its stop",
);
/// A doze: the VM paused and its guest memory paged out of host RAM.
pub static DOZE: prom::Histogram = prom::Histogram::new(
    "apz_node_doze_seconds",
    "A VM doze: its pause and the page-out of its memory",
);
/// A checkpoint: the VM's memory and disk copied while it is paused.
pub static CHECKPOINT: prom::Histogram =
    prom::Histogram::new("apz_node_checkpoint_seconds", "A VM checkpoint");
/// A fork: the copies made and started.
pub static FORK: prom::Histogram =
    prom::Histogram::new("apz_node_fork_seconds", "A VM fork, until its copies run");
/// One VM start (boot or restore), from the spawn call to the VM running.
pub static SPAWN: prom::Histogram = prom::Histogram::new(
    "apz_node_spawn_seconds",
    "One VM start, from the spawn call to the VM running",
);

/// The Prometheus text of every node metric.
pub fn render() -> String {
    prom::render(&[
        &WAKE as &dyn prom::Metric,
        &WAKE_COLD,
        &SLEEP,
        &DOZE,
        &CHECKPOINT,
        &FORK,
        &SPAWN,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_metrics::EntityKind;

    fn node_key(metric: &str) -> SeriesKey {
        SeriesKey::new(metric, EntityKind::Node, "node-1")
    }

    #[test]
    fn a_gauge_across_two_ticks_is_one_column_that_decodes_to_both_values() {
        let metrics = NodeMetrics::new();
        let load = metrics.gauge(node_key("cpu.load"));
        load.set(10.0);
        metrics.tick(1_000);
        load.set(12.5);
        metrics.tick(11_000);

        let batch = metrics.batch(11_000);
        assert_eq!(batch.series.len(), 1);
        assert!(!batch.series[0].counter);
        assert_eq!(
            wire::decode(&batch.series[0]),
            vec![(1_000, 10.0), (11_000, 12.5)]
        );
    }

    #[test]
    fn a_counter_keeps_its_increases_at_their_times() {
        let metrics = NodeMetrics::new();
        let requests = metrics.counter(node_key("requests"));
        requests.add(3.0);
        metrics.tick(10_000);
        metrics.tick(20_000);
        requests.add(5.0);
        metrics.tick(30_000);

        let batch = metrics.batch(30_000);
        assert_eq!(batch.series.len(), 1);
        assert!(batch.series[0].counter);
        assert_eq!(
            wire::decode(&batch.series[0]),
            vec![(10_000, 3.0), (30_000, 5.0)]
        );
    }

    #[test]
    fn events_and_reports_appear_in_one_batch_and_then_the_buffer_is_empty() {
        let metrics = NodeMetrics::new();
        metrics.gauge(node_key("memory.used")).set(1.0);
        metrics.event(Event {
            t_ms: 500,
            kind: "boot".to_string(),
            entity_kind: EntityKind::Node,
            entity: "node-1".to_string(),
            duration_ms: Some(412.0),
            detail: "ready in 412 ms".to_string(),
            parts: Vec::new(),
        });
        metrics.report("gpus", serde_json::json!(["RTX 4090"]));
        metrics.tick(1_000);

        let first = metrics.batch(1_000);
        assert_eq!(first.node, "");
        assert_eq!(first.sent_ms, 1_000);
        assert_eq!(first.series.len(), 1);
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.events[0].kind, "boot");
        assert_eq!(first.reports, serde_json::json!({"gpus": ["RTX 4090"]}));

        let second = metrics.batch(2_000);
        assert!(second.series.is_empty());
        assert!(second.events.is_empty());
        assert!(second.reports.is_null());
    }
}
