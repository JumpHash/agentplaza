//! Reading series back: which tier to read, how to bucket stored windows into the chart's steps, how to reduce a
//! bucket to one number, and LTTB downsampling for lines.

use crate::agg::{Agg, Tier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// The longest range, in milliseconds, that may be read from the 10 s tier (6 h).
const RAW_TIER_MAX_RANGE_MS: u64 = 6 * 60 * 60 * 1_000;

/// The coarsest tier that still gives at least one point per two pixels of `width_px` over `range_ms`, and never
/// the 10 s tier for a range longer than 6 h.
pub fn pick_tier(range_ms: u64, width_px: u32) -> Tier {
    // The chart wants one point per two pixels, so the needed step is the range divided by that count.
    let divisor = u64::from(width_px / 2).max(1);
    let finest = if range_ms <= RAW_TIER_MAX_RANGE_MS {
        Tier::Raw10s
    } else {
        Tier::Min1
    };
    // The step is at most `range / divisor`, compared as `step * divisor <= range` to stay in integers.
    Tier::ALL
        .iter()
        .rev()
        .copied()
        .filter(|tier| *tier >= finest)
        .find(|tier| tier.step_ms().saturating_mul(divisor) <= range_ms)
        .unwrap_or(finest)
}

/// One number from a window's aggregate. Percentiles need a histogram (`None` without one). `Rate` divides the
/// sum by `step_ms` in seconds, and is `None` for a zero step.
pub fn reduce(agg: &Agg, f: AggFn, step_ms: u64) -> Option<f64> {
    if agg.count == 0 {
        return None;
    }
    match f {
        AggFn::Sum => Some(agg.sum),
        AggFn::Avg => agg.mean(),
        AggFn::Min => Some(agg.min),
        AggFn::Max => Some(agg.max),
        AggFn::Last => Some(agg.last),
        AggFn::Count => Some(agg.count as f64),
        AggFn::Rate => (step_ms > 0).then(|| agg.sum / (step_ms as f64 / 1_000.0)),
        AggFn::P50 | AggFn::P90 | AggFn::P95 | AggFn::P99 => {
            let q = f.quantile()?;
            agg.hist.as_ref()?.quantile(q)
        }
    }
}

/// Buckets stored windows `(start_ms, Agg)` of one series into `[from, to)` by `step_ms` (each window merged into
/// the bucket holding its start), then reduces each bucket. Empty buckets are `None` (a gap, never zero).
pub fn bucketize(
    rows: &[(u64, Agg)],
    from_ms: u64,
    to_ms: u64,
    step_ms: u64,
    f: AggFn,
) -> Vec<Option<f64>> {
    if step_ms == 0 || to_ms <= from_ms {
        return Vec::new();
    }
    // The number of buckets is the span divided by the step, rounded up.
    let span = to_ms - from_ms;
    let count = span / step_ms + u64::from(span % step_ms != 0);
    let mut buckets: Vec<Option<Agg>> = vec![None; count as usize];
    for (start_ms, agg) in rows {
        let start = *start_ms;
        if start < from_ms || start >= to_ms {
            continue;
        }
        let index = ((start - from_ms) / step_ms) as usize;
        match &mut buckets[index] {
            Some(existing) => fold(existing, agg),
            empty => *empty = Some(agg.clone()),
        }
    }
    buckets
        .iter()
        .map(|bucket| bucket.as_ref().and_then(|agg| reduce(agg, f, step_ms)))
        .collect()
}

/// Merges several series' windows (bucket-aligned starts) into one: sums for counters and gauges' sums, histogram
/// merges for distributions. Used to read a scope (a role, a provider) made of many entities.
pub fn merge_rows(series: &[Vec<(u64, Agg)>]) -> Vec<(u64, Agg)> {
    let mut merged: BTreeMap<u64, Agg> = BTreeMap::new();
    for rows in series {
        for (start_ms, agg) in rows {
            merged
                .entry(*start_ms)
                .and_modify(|existing| fold(existing, agg))
                .or_insert_with(|| agg.clone());
        }
    }
    merged.into_iter().collect()
}

/// Adds `later` into `into`, with the same rule as `Agg::merge`: an empty side takes the other, and the last
/// reading follows the later window.
fn fold(into: &mut Agg, later: &Agg) {
    if later.count == 0 {
        return;
    }
    if into.count == 0 {
        *into = later.clone();
        return;
    }
    into.count += later.count;
    into.sum += later.sum;
    into.min = into.min.min(later.min);
    into.max = into.max.max(later.max);
    into.last = later.last;
    if let Some(other) = &later.hist {
        match &mut into.hist {
            Some(hist) => hist.merge(other),
            None => into.hist = Some(other.clone()),
        }
    }
}

/// Largest-Triangle-Three-Buckets: at most `threshold` points keeping the line's shape; the first and last points
/// always stay. Fewer points than `threshold` are returned unchanged.
pub fn lttb(points: &[(f64, f64)], threshold: usize) -> Vec<(f64, f64)> {
    if threshold < 3 || points.len() <= threshold {
        return points.to_vec();
    }
    let len = points.len();
    // The interior points split into `threshold - 2` buckets of this many points each.
    let bucket_size = (len - 2) as f64 / (threshold - 2) as f64;
    let mut out = Vec::with_capacity(threshold);
    out.push(points[0]);
    let mut anchor = 0;
    for i in 0..threshold - 2 {
        // The average of the next bucket is the third corner of each candidate triangle.
        let next_start = ((i + 1) as f64 * bucket_size) as usize + 1;
        let next_end = (((i + 2) as f64 * bucket_size) as usize + 1).min(len);
        let next = &points[next_start..next_end];
        let avg_x = next.iter().map(|p| p.0).sum::<f64>() / next.len() as f64;
        let avg_y = next.iter().map(|p| p.1).sum::<f64>() / next.len() as f64;
        // Keep the point in this bucket that makes the largest triangle with the last kept point.
        let range_start = (i as f64 * bucket_size) as usize + 1;
        let range_end = ((i + 1) as f64 * bucket_size) as usize + 1;
        let (ax, ay) = points[anchor];
        let mut best = range_start;
        let mut best_area = -1.0;
        for index in range_start..range_end {
            let (px, py) = points[index];
            let area = ((ax - avg_x) * (py - ay) - (ax - px) * (avg_y - ay)).abs();
            if area > best_area {
                best_area = area;
                best = index;
            }
        }
        out.push(points[best]);
        anchor = best;
    }
    out.push(points[len - 1]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hist::HistSnapshot;

    const HOUR_MS: u64 = 3_600_000;

    /// A window holding one gauge reading of `value`.
    fn gauge(value: f64) -> Agg {
        Agg {
            count: 1,
            sum: value,
            min: value,
            max: value,
            last: value,
            hist: None,
        }
    }

    /// A window whose histogram holds `values`.
    fn histogram_window(values: &[f64]) -> Agg {
        let mut hist = HistSnapshot::default();
        for value in values {
            hist.record(*value);
        }
        let (count, sum, min, max) = (hist.count, hist.sum, hist.min, hist.max);
        Agg {
            count,
            sum,
            min,
            max,
            last: max,
            hist: Some(hist),
        }
    }

    #[test]
    fn pick_tier_picks_one_minute_for_a_day_at_900_px_and_never_ten_seconds_beyond_six_hours() {
        assert_eq!(pick_tier(24 * HOUR_MS, 900), Tier::Min1);
        assert_eq!(pick_tier(6 * HOUR_MS, 900), Tier::Raw10s);
        assert_eq!(pick_tier(6 * HOUR_MS + 1, 900), Tier::Min1);
        assert_ne!(pick_tier(7 * 24 * HOUR_MS, 100_000), Tier::Raw10s);
    }

    #[test]
    fn bucketize_leaves_empty_buckets_as_gaps_and_merges_two_windows_in_one_bucket() {
        let rows = vec![
            (0, gauge(1.0)),
            (10_000, gauge(3.0)),
            (120_000, gauge(7.0)),
            (240_000, gauge(9.0)),
        ];
        let buckets = bucketize(&rows, 0, 240_000, 60_000, AggFn::Sum);
        assert_eq!(buckets, vec![Some(4.0), None, Some(7.0), None]);
    }

    #[test]
    fn percentiles_come_from_merged_histograms() {
        let low: Vec<f64> = (1..=50).map(f64::from).collect();
        let high: Vec<f64> = (51..=100).map(f64::from).collect();
        let mut merged = histogram_window(&low);
        fold(&mut merged, &histogram_window(&high));

        let p50 = reduce(&merged, AggFn::P50, HOUR_MS).unwrap();
        let p99 = reduce(&merged, AggFn::P99, HOUR_MS).unwrap();
        assert!((p50 - 50.0).abs() / 50.0 < 0.05, "p50 was {p50}");
        assert!((p99 - 99.0).abs() / 99.0 < 0.05, "p99 was {p99}");
        assert_eq!(reduce(&gauge(1.0), AggFn::P50, HOUR_MS), None);
    }

    #[test]
    fn lttb_keeps_the_first_and_last_points_and_returns_exactly_the_threshold() {
        let points: Vec<(f64, f64)> = (0..100)
            .map(|i| (f64::from(i), (f64::from(i) / 7.0).sin() * 10.0))
            .collect();
        let out = lttb(&points, 10);
        assert_eq!(out.len(), 10);
        assert_eq!(out[0], points[0]);
        assert_eq!(out[9], points[99]);
        assert!(out.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(lttb(&points[..5], 10), points[..5].to_vec());
    }
}
