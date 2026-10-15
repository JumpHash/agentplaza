//! Process-wide counters, gauges and latency histograms for the daemons (apid, noded, vsockd), rendered in the
//! Prometheus text format for their `/v1/metrics` routes, and one-line timing records on stderr.
//!
//! Every metric is a `static`: `Counter::new`, `Gauge::new` and `Histogram::new` are `const`, so a module declares
//! `static WAKES: Counter = Counter::new("apz_wakes_total", "Agent wakes started");` and a route renders the ones it
//! owns with [`render`]. Nothing allocates on the hot path: a record is a few relaxed atomic adds.
//!
//! A timing record is one stderr line, `timing op=<op> id=<id> total_ms=<n> <phase>_ms=<n> ...`, written by
//! [`Phases::finish`]. The lifecycle benchmark (`vmctl bench`) and an operator reading the logs join records of one
//! operation across daemons by `op` and `id`.

use std::fmt::{Display, Write as _};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The upper bounds, in milliseconds, of a [`Histogram`]'s buckets. A last, implicit bucket holds everything above.
pub const BUCKETS_MS: [u64; 14] = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 30000];

/// Something [`render`] can write.
pub trait Metric: Sync {
    /// Appends this metric in the Prometheus text exposition format: `# HELP`, `# TYPE`, then its samples.
    fn render(&self, out: &mut String);
}

/// A monotonically increasing count.
pub struct Counter {
    name: &'static str,
    help: &'static str,
    value: AtomicU64,
}

impl Counter {
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self { name, help, value: AtomicU64::new(0) }
    }

    pub fn inc(&self) {
        todo!()
    }

    pub fn add(&self, n: u64) {
        todo!()
    }

    pub fn get(&self) -> u64 {
        todo!()
    }
}

/// A value that goes up and down (sessions open, wakes in flight, spawn queue depth).
pub struct Gauge {
    name: &'static str,
    help: &'static str,
    value: AtomicI64,
}

impl Gauge {
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self { name, help, value: AtomicI64::new(0) }
    }

    pub fn set(&self, v: i64) {
        todo!()
    }

    pub fn add(&self, delta: i64) {
        todo!()
    }

    pub fn get(&self) -> i64 {
        todo!()
    }
}

/// A latency histogram in milliseconds over [`BUCKETS_MS`], with a sum and a count, so p50/p95 can be estimated.
pub struct Histogram {
    name: &'static str,
    help: &'static str,
    /// One count per bucket of `BUCKETS_MS`, plus the overflow bucket last (not cumulative).
    counts: [AtomicU64; 15],
    /// The sum of every observation, in microseconds.
    sum_us: AtomicU64,
}

impl Histogram {
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            counts: [const { AtomicU64::new(0) }; 15],
            sum_us: AtomicU64::new(0),
        }
    }

    /// Records one observation.
    pub fn observe(&self, elapsed: Duration) {
        todo!()
    }

    /// How many observations were recorded.
    pub fn count(&self) -> u64 {
        todo!()
    }

    /// An estimate of quantile `q` (0.0..=1.0) in milliseconds: the upper bound of the bucket the quantile falls in
    /// (the overflow bucket reports its lower bound). None with no observations.
    pub fn quantile_ms(&self, q: f64) -> Option<u64> {
        todo!()
    }
}

/// Renders `metrics` in order, in the Prometheus text exposition format (version 0.0.4).
pub fn render(metrics: &[&dyn Metric]) -> String {
    let mut out = String::new();
    for metric in metrics {
        metric.render(&mut out);
    }
    out
}

/// The phases of one operation (a wake, a sleep, a fork), timed from its start. [`Phases::finish`] writes them as
/// one `timing` line on stderr and, when given, records the total in a histogram.
pub struct Phases {
    op: &'static str,
    id: String,
    start: Instant,
    last: Instant,
    marks: Vec<(&'static str, Duration)>,
}

impl Phases {
    /// Starts timing operation `op` (`wake`, `sleep`, `fork`, ...) on `id` (a CID, a run and VM, ...).
    pub fn start(op: &'static str, id: impl Display) -> Self {
        todo!()
    }

    /// Ends the current phase as `name`: its time is from the previous mark (or the start) to now.
    pub fn mark(&mut self, name: &'static str) {
        todo!()
    }

    /// The time since the start.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// The line [`finish`](Self::finish) writes, without writing it: `timing op=<op> id=<id> total_ms=<t>` then
    /// `<phase>_ms=<n>` for each mark in order, then each `extra` as `key=value`. Milliseconds have one decimal.
    pub fn line(&self, extra: &[(&str, &dyn Display)]) -> String {
        todo!()
    }

    /// Writes [`line`](Self::line) to stderr and records the total in `total` when given.
    pub fn finish(self, total: Option<&Histogram>, extra: &[(&str, &dyn Display)]) {
        todo!()
    }
}
