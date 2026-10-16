//! Process-wide counters, gauges and latency histograms for the daemons (apid, noded, vsockd), rendered in the
//! Prometheus text format for their `GET /v1/metrics` routes, and one-line timing records on stderr. Separate from
//! the [`crate::registry`] series that feed the Metrics view: these are fixed statics for scraping and benchmarks.
//!
//! Every metric is a `static`: `Counter::new`, `Gauge::new` and `Histogram::new` are `const`, so a module declares
//! `static WAKES: prom::Counter = prom::Counter::new("apz_wakes_total", "Agent wakes started");` and a route
//! renders the ones it owns with [`render`]. Nothing allocates on the hot path: a record is a few relaxed atomic
//! adds.
//!
//! A timing record is one stderr line, `timing op=<op> id=<id> total_ms=<n> <phase>_ms=<n> ...`, written by
//! [`Phases::finish`]. The lifecycle benchmark (`vmctl bench`) and an operator reading the logs join records of one
//! operation across daemons by `op` and `id`.

use std::fmt::{Display, Write as _};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The upper bounds, in milliseconds, of a [`Histogram`]'s buckets. A last, implicit bucket holds everything above.
pub const BUCKETS_MS: [u64; 14] = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 30000,
];

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
        Self {
            name,
            help,
            value: AtomicU64::new(0),
        }
    }

    pub fn inc(&self) {
        self.add(1);
    }

    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

impl Metric for Counter {
    fn render(&self, out: &mut String) {
        let _ = writeln!(out, "# HELP {} {}", self.name, self.help);
        let _ = writeln!(out, "# TYPE {} counter", self.name);
        let _ = writeln!(out, "{} {}", self.name, self.get());
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
        Self {
            name,
            help,
            value: AtomicI64::new(0),
        }
    }

    pub fn set(&self, v: i64) {
        self.value.store(v, Ordering::Relaxed);
    }

    pub fn add(&self, delta: i64) {
        self.value.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }
}

impl Metric for Gauge {
    fn render(&self, out: &mut String) {
        let _ = writeln!(out, "# HELP {} {}", self.name, self.help);
        let _ = writeln!(out, "# TYPE {} gauge", self.name);
        let _ = writeln!(out, "{} {}", self.name, self.get());
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
        let ms = elapsed.as_secs_f64() * 1000.0;
        let index = BUCKETS_MS
            .iter()
            .position(|&bound| ms <= bound as f64)
            .unwrap_or(BUCKETS_MS.len());
        self.counts[index].fetch_add(1, Ordering::Relaxed);
        let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
    }

    /// How many observations were recorded.
    pub fn count(&self) -> u64 {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
    }

    /// An estimate of quantile `q` (0.0..=1.0) in milliseconds: the upper bound of the bucket the quantile falls in
    /// (the overflow bucket reports its lower bound). None with no observations.
    pub fn quantile_ms(&self, q: f64) -> Option<u64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let q = q.clamp(0.0, 1.0);
        let target = ((q * total as f64).ceil() as u64).max(1);
        let mut running = 0u64;
        for (i, count) in self.counts.iter().enumerate() {
            running += count.load(Ordering::Relaxed);
            if running >= target {
                return Some(BUCKETS_MS[i.min(BUCKETS_MS.len() - 1)]);
            }
        }
        Some(BUCKETS_MS[BUCKETS_MS.len() - 1])
    }
}

impl Metric for Histogram {
    fn render(&self, out: &mut String) {
        let name = self.name;
        let counts: [u64; 15] = std::array::from_fn(|i| self.counts[i].load(Ordering::Relaxed));
        let _ = writeln!(out, "# HELP {} {}", name, self.help);
        let _ = writeln!(out, "# TYPE {name} histogram");
        let mut cumulative = 0u64;
        for (bound, count) in BUCKETS_MS.iter().zip(counts.iter()) {
            cumulative += *count;
            let le = *bound as f64 / 1000.0;
            let _ = writeln!(out, "{name}_bucket{{le=\"{le}\"}} {cumulative}");
        }
        let total: u64 = counts.iter().sum();
        let sum_s = self.sum_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {total}");
        let _ = writeln!(out, "{name}_sum {sum_s}");
        let _ = writeln!(out, "{name}_count {total}");
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
        let now = Instant::now();
        Self {
            op,
            id: id.to_string(),
            start: now,
            last: now,
            marks: Vec::new(),
        }
    }

    /// Ends the current phase as `name`: its time is from the previous mark (or the start) to now.
    pub fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.marks.push((name, now - self.last));
        self.last = now;
    }

    /// The time since the start.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// The line [`finish`](Self::finish) writes, without writing it: `timing op=<op> id=<id> total_ms=<t>` then
    /// `<phase>_ms=<n>` for each mark in order, then each `extra` as `key=value`. Milliseconds have one decimal.
    pub fn line(&self, extra: &[(&str, &dyn Display)]) -> String {
        let mut line = format!(
            "timing op={} id={} total_ms={:.1}",
            self.op,
            self.id,
            millis(self.elapsed())
        );
        for (name, duration) in &self.marks {
            let _ = write!(line, " {name}_ms={:.1}", millis(*duration));
        }
        for (key, value) in extra {
            let _ = write!(line, " {key}={value}");
        }
        line
    }

    /// Writes [`line`](Self::line) to stderr and records the total in `total` when given.
    pub fn finish(self, total: Option<&Histogram>, extra: &[(&str, &dyn Display)]) {
        eprintln!("{}", self.line(extra));
        if let Some(total) = total {
            total.observe(self.elapsed());
        }
    }
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_one(metric: &dyn Metric) -> String {
        let mut out = String::new();
        metric.render(&mut out);
        out
    }

    #[test]
    fn counter_renders_exactly() {
        let c = Counter::new("apz_wakes_total", "Agent wakes started");
        c.inc();
        c.add(2);
        assert_eq!(c.get(), 3);
        assert_eq!(
            render_one(&c),
            "# HELP apz_wakes_total Agent wakes started\n# TYPE apz_wakes_total counter\napz_wakes_total 3\n"
        );
    }

    #[test]
    fn gauge_renders_exactly() {
        let g = Gauge::new("apz_sessions", "Open sessions");
        g.set(5);
        g.add(-2);
        assert_eq!(g.get(), 3);
        assert_eq!(
            render_one(&g),
            "# HELP apz_sessions Open sessions\n# TYPE apz_sessions gauge\napz_sessions 3\n"
        );
    }

    #[test]
    fn histogram_places_observation_in_its_bucket() {
        let h = Histogram::new("apz_wake_seconds", "Wake latency");
        h.observe(Duration::from_millis(3));
        assert_eq!(h.count(), 1);
        let text = render_one(&h);
        assert!(text.starts_with(
            "# HELP apz_wake_seconds Wake latency\n# TYPE apz_wake_seconds histogram\n"
        ));
        // Cumulative: nothing at or below 2 ms, the observation from le=0.005 (5 ms) on.
        assert!(text.contains("apz_wake_seconds_bucket{le=\"0.001\"} 0\n"));
        assert!(text.contains("apz_wake_seconds_bucket{le=\"0.002\"} 0\n"));
        assert!(text.contains("apz_wake_seconds_bucket{le=\"0.005\"} 1\n"));
        assert!(text.contains("apz_wake_seconds_bucket{le=\"0.01\"} 1\n"));
        assert!(text.contains("apz_wake_seconds_bucket{le=\"30\"} 1\n"));
        assert!(text.contains("apz_wake_seconds_bucket{le=\"+Inf\"} 1\n"));
        assert!(text.contains("apz_wake_seconds_sum 0.003\n"));
        assert!(text.contains("apz_wake_seconds_count 1\n"));
    }

    #[test]
    fn histogram_overflow_counts_only_in_inf() {
        let h = Histogram::new("apz_slow_seconds", "Slow");
        h.observe(Duration::from_secs(60));
        let text = render_one(&h);
        assert!(text.contains("apz_slow_seconds_bucket{le=\"30\"} 0\n"));
        assert!(text.contains("apz_slow_seconds_bucket{le=\"+Inf\"} 1\n"));
        assert!(text.contains("apz_slow_seconds_count 1\n"));
    }

    #[test]
    fn quantiles_follow_bucket_bounds() {
        let h = Histogram::new("apz_q_seconds", "Quantiles");
        assert_eq!(h.quantile_ms(0.5), None);
        for _ in 0..100 {
            h.observe(Duration::from_millis(4));
        }
        h.observe(Duration::from_millis(900));
        assert_eq!(h.count(), 101);
        assert_eq!(h.quantile_ms(0.5), Some(5));
        assert!(h.quantile_ms(0.99).unwrap() <= 1000);
        assert_eq!(h.quantile_ms(1.0), Some(1000));
    }

    #[test]
    fn timing_line_has_marks_and_extras() {
        let mut p = Phases::start("wake", 7);
        p.mark("spawn");
        let extra: [(&str, &dyn Display); 1] = [("cold", &false)];
        let line = p.line(&extra);
        assert!(line.starts_with("timing op=wake id=7 total_ms="), "{line}");
        assert!(line.contains("spawn_ms="), "{line}");
        assert!(line.contains("cold=false"), "{line}");
    }
}
