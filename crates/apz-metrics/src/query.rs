//! Reading series back: which tier to read, how to bucket stored windows into the chart's steps, how to reduce a
//! bucket to one number, and LTTB downsampling for lines.

use crate::agg::{Agg, Tier};
use serde::{Deserialize, Serialize};

/// How a bucket becomes one number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFn {
    /// Total of the values (a counter's increase).
    Sum,
    /// Mean of the readings.
    Avg,
    Min,
    Max,
    /// The latest reading.
    Last,
    /// A counter's increase per second over the bucket.
    Rate,
    /// Number of readings (histogram: number of values).
    Count,
    P50,
    P90,
    P95,
    P99,
}

impl AggFn {
    /// The quantile of a percentile function.
    pub fn quantile(self) -> Option<f64> {
        match self {
            AggFn::P50 => Some(0.5),
            AggFn::P90 => Some(0.9),
            AggFn::P95 => Some(0.95),
            AggFn::P99 => Some(0.99),
            _ => None,
        }
    }
}

/// The coarsest tier that still gives at least one point per two pixels of `width_px` over `range_ms`, and never
/// the 10 s tier for a range longer than 6 h.
pub fn pick_tier(range_ms: u64, width_px: u32) -> Tier {
    let _ = (range_ms, width_px);
    todo!("needed step = range / (width/2); choose the coarsest tier with step <= needed; Raw10s only when range <= 6 h, else Min1 at finest")
}

/// One number from a window's aggregate. Percentiles need a histogram (`None` without one). `Rate` divides the
/// sum by `step_ms` in seconds.
pub fn reduce(agg: &Agg, f: AggFn, step_ms: u64) -> Option<f64> {
    let _ = (agg, f, step_ms);
    todo!()
}

/// Buckets stored windows `(start_ms, Agg)` of one series into `[from, to)` by `step_ms` (each window merged into
/// the bucket holding its start), then reduces each bucket. Empty buckets are `None` (a gap, never zero).
pub fn bucketize(rows: &[(u64, Agg)], from_ms: u64, to_ms: u64, step_ms: u64, f: AggFn) -> Vec<Option<f64>> {
    let _ = (rows, from_ms, to_ms, step_ms, f);
    todo!()
}

/// Merges several series' windows (bucket-aligned starts) into one: sums for counters and gauges' sums, histogram
/// merges for distributions. Used to read a scope (a role, a provider) made of many entities.
pub fn merge_rows(series: &[Vec<(u64, Agg)>]) -> Vec<(u64, Agg)> {
    let _ = series;
    todo!("group by start_ms ascending; Agg::merge in order")
}

/// Largest-Triangle-Three-Buckets: at most `threshold` points keeping the line's shape; the first and last points
/// always stay. Fewer points than `threshold` are returned unchanged.
pub fn lttb(points: &[(f64, f64)], threshold: usize) -> Vec<(f64, f64)> {
    let _ = (points, threshold);
    todo!()
}
