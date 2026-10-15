//! Fixed log-scale histograms. Bucket `b` holds values in `[MIN·GROWTH^b, MIN·GROWTH^(b+1))`; bucket 0 also holds
//! everything below `MIN`, the last bucket everything above. With `GROWTH = 1.04` a quantile read from a bucket's
//! midpoint is within about 2 % of the true value. Because every histogram shares the same buckets, snapshots
//! merge exactly (bucket by bucket) across agents, nodes and time tiers; a p95 of p95s is never computed.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of buckets.
pub const BUCKETS: usize = 512;
/// Ratio between consecutive bucket bounds.
pub const GROWTH: f64 = 1.04;
/// Lower bound of bucket 1 (bucket 0 holds everything below).
pub const MIN: f64 = 0.01;

/// The bucket of a value (NaN and negatives go to bucket 0).
pub fn bucket_of(v: f64) -> u16 {
    if v.is_nan() || v <= MIN {
        return 0;
    }
    let b = ((v / MIN).ln() / GROWTH.ln()).floor();
    if b >= (BUCKETS - 1) as f64 {
        (BUCKETS - 1) as u16
    } else {
        b as u16
    }
}

/// The representative value of a bucket: the geometric midpoint of its bounds.
pub fn bucket_value(b: u16) -> f64 {
    MIN * GROWTH.powf(f64::from(b) + 0.5)
}

/// A histogram that many threads record into. Recording is lock-free: one atomic add on the bucket, plus the
/// count and the sum/min/max (kept as f64 bits, updated with compare-and-swap).
pub struct AtomicHist {
    counts: Box<[AtomicU64]>,
    count: AtomicU64,
    sum_bits: AtomicU64,
    min_bits: AtomicU64,
    max_bits: AtomicU64,
}

impl Default for AtomicHist {
    fn default() -> Self {
        AtomicHist::new()
    }
}

impl AtomicHist {
    pub fn new() -> AtomicHist {
        AtomicHist {
            counts: (0..BUCKETS).map(|_| AtomicU64::new(0)).collect(),
            count: AtomicU64::new(0),
            sum_bits: AtomicU64::new(0f64.to_bits()),
            min_bits: AtomicU64::new(f64::INFINITY.to_bits()),
            max_bits: AtomicU64::new(f64::NEG_INFINITY.to_bits()),
        }
    }

    /// Records one value.
    pub fn record(&self, v: f64) {
        if !v.is_finite() {
            return;
        }
        self.counts[bucket_of(v) as usize].fetch_add(1, Ordering::Relaxed);
        update(&self.sum_bits, |s| s + v);
        update(&self.min_bits, |m| m.min(v));
        update(&self.max_bits, |m| m.max(v));
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// The values recorded so far, without resetting.
    pub fn snapshot(&self) -> HistSnapshot {
        let buckets: Vec<(u16, u64)> = self
            .counts
            .iter()
            .enumerate()
            .filter_map(|(b, c)| {
                let n = c.load(Ordering::Relaxed);
                (n > 0).then_some((b as u16, n))
            })
            .collect();
        let count = buckets.iter().map(|b| b.1).sum();
        let bits = |a: &AtomicU64| f64::from_bits(a.load(Ordering::Relaxed));
        assemble(
            buckets,
            count,
            bits(&self.sum_bits),
            bits(&self.min_bits),
            bits(&self.max_bits),
        )
    }

    /// The values recorded since the last `take`, resetting to empty. A value recorded concurrently lands in
    /// either this snapshot or the next, never in neither.
    pub fn take(&self) -> HistSnapshot {
        let buckets: Vec<(u16, u64)> = self
            .counts
            .iter()
            .enumerate()
            .filter_map(|(b, c)| {
                let n = c.swap(0, Ordering::Relaxed);
                (n > 0).then_some((b as u16, n))
            })
            .collect();
        let count = buckets.iter().map(|b| b.1).sum::<u64>();
        self.count.fetch_sub(
            count.min(self.count.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        let swap =
            |a: &AtomicU64, empty: f64| f64::from_bits(a.swap(empty.to_bits(), Ordering::Relaxed));
        let sum = swap(&self.sum_bits, 0.0);
        let min = swap(&self.min_bits, f64::INFINITY);
        let max = swap(&self.max_bits, f64::NEG_INFINITY);
        assemble(buckets, count, sum, min, max)
    }
}

/// A histogram's contents: sparse `(bucket, count)` pairs in ascending bucket order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistSnapshot {
    pub buckets: Vec<(u16, u64)>,
    pub count: u64,
    pub sum: f64,
    /// 0.0 when empty.
    pub min: f64,
    /// 0.0 when empty.
    pub max: f64,
}

impl HistSnapshot {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Records one value into a plain (non-atomic) snapshot.
    pub fn record(&mut self, v: f64) {
        if !v.is_finite() {
            return;
        }
        let b = bucket_of(v);
        match self.buckets.binary_search_by_key(&b, |x| x.0) {
            Ok(i) => self.buckets[i].1 += 1,
            Err(i) => self.buckets.insert(i, (b, 1)),
        }
        if self.count == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.count += 1;
        self.sum += v;
    }

    /// Adds another snapshot into this one, bucket by bucket.
    pub fn merge(&mut self, other: &HistSnapshot) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = other.clone();
            return;
        }
        let mut merged = Vec::with_capacity(self.buckets.len() + other.buckets.len());
        let (mut i, mut j) = (0, 0);
        while i < self.buckets.len() || j < other.buckets.len() {
            match (self.buckets.get(i), other.buckets.get(j)) {
                (Some(a), Some(b)) if a.0 == b.0 => {
                    merged.push((a.0, a.1 + b.1));
                    i += 1;
                    j += 1;
                }
                (Some(a), Some(b)) if a.0 < b.0 => {
                    merged.push(*a);
                    i += 1;
                }
                (Some(_), Some(b)) => {
                    merged.push(*b);
                    j += 1;
                }
                (Some(a), None) => {
                    merged.push(*a);
                    i += 1;
                }
                (None, Some(b)) => {
                    merged.push(*b);
                    j += 1;
                }
                (None, None) => break,
            }
        }
        self.buckets = merged;
        self.count += other.count;
        self.sum += other.sum;
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    /// The value at quantile `q` (0..=1), read from the bucket midpoint and clamped into `[min, max]`.
    /// `None` when empty.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let rank = ((q.clamp(0.0, 1.0) * self.count as f64).ceil() as u64).max(1);
        let mut seen = 0;
        for (b, n) in &self.buckets {
            seen += n;
            if seen >= rank {
                return Some(bucket_value(*b).clamp(self.min, self.max));
            }
        }
        Some(self.max)
    }

    /// The mean, `None` when empty.
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// Applies `f` to an f64 stored as bits, with compare-and-swap.
fn update(cell: &AtomicU64, f: impl Fn(f64) -> f64) {
    let mut cur = cell.load(Ordering::Relaxed);
    loop {
        let next = f(f64::from_bits(cur)).to_bits();
        match cell.compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(now) => cur = now,
        }
    }
}

fn assemble(buckets: Vec<(u16, u64)>, count: u64, sum: f64, min: f64, max: f64) -> HistSnapshot {
    if count == 0 {
        return HistSnapshot::default();
    }
    HistSnapshot {
        buckets,
        count,
        sum,
        min,
        max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quantile_is_within_two_percent_and_inside_the_recorded_range() {
        let h = AtomicHist::new();
        for v in 1..=1000 {
            h.record(f64::from(v));
        }
        let s = h.snapshot();
        assert_eq!(s.count, 1000);
        let p95 = s.quantile(0.95).unwrap();
        assert!((p95 - 950.0).abs() / 950.0 < 0.025, "p95 {p95}");
        assert_eq!(s.quantile(1.0), Some(1000.0));
        assert!(s.quantile(0.0).unwrap() >= 1.0);
    }

    #[test]
    fn snapshots_merge_bucket_by_bucket_like_one_histogram() {
        let (mut a, mut b, mut all) = (
            HistSnapshot::default(),
            HistSnapshot::default(),
            HistSnapshot::default(),
        );
        for v in [3.0, 40.0, 500.0, 6000.0] {
            a.record(v);
            all.record(v);
        }
        for v in [3.1, 41.0, 70_000.0] {
            b.record(v);
            all.record(v);
        }
        a.merge(&b);
        assert_eq!(a, all);
    }

    #[test]
    fn take_resets_so_the_next_snapshot_holds_only_newer_values() {
        let h = AtomicHist::new();
        h.record(10.0);
        assert_eq!(h.take().count, 1);
        assert!(h.snapshot().is_empty());
        h.record(20.0);
        let s = h.take();
        assert_eq!((s.count, s.min, s.max), (1, 20.0, 20.0));
    }
}
