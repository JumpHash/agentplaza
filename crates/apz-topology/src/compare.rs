//! Comparing a candidate's metrics with a baseline's (PLAN 22.8, 22.11): each metric's change against the last qualified
//! stable release, judged by the project's limits. Pure: the hub reads the values and files the failures as findings.
//!
//! A latency, a bench or a size is a percentage, and a regression is a rise past its limit. A throughput is a percentage
//! too, a regression being a fall past its limit. An error rate is a ratio, and its limit is in percentage points.

use serde::{Deserialize, Serialize};

/// The limit a metric gets when the project sets none for it: the default table's entry, else [`FALLBACK_PERCENT`].
pub const DEFAULTS: [(&str, f64); 7] = [
    ("load.latency.p50_ms", 15.0),
    ("load.latency.p95_ms", 15.0),
    ("load.latency.p99_ms", 25.0),
    ("load.latency.max_ms", 50.0),
    ("load.throughput_rps", 10.0),
    ("load.error_rate", 1.0),
    ("bench.*", 10.0),
];
/// The limit of a metric the table does not name.
pub const FALLBACK_PERCENT: f64 = 10.0;

/// One metric's value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
}

/// A project's limit on one metric: `name` exactly, or a prefix ending in `*`. `max` is a percentage, or percentage
/// points for an error rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub metric: String,
    pub max: f64,
}

/// One metric compared: both values, the change, the limit, and whether it passed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub metric: String,
    pub baseline: f64,
    pub candidate: f64,
    /// The change: a percentage, or percentage points for a ratio (positive is a rise).
    pub change: f64,
    pub limit: f64,
    /// Whether the change is within its limit in the metric's good direction.
    pub passed: bool,
    /// `percent` or `points`.
    pub unit: String,
}

/// Whether a higher value is the good direction.
pub fn higher_is_better(name: &str) -> bool {
    name.ends_with("_rps") || name.ends_with("throughput")
}

/// Whether the metric is a ratio, judged in percentage points.
pub fn is_ratio(name: &str) -> bool {
    name.ends_with("error_rate")
}

/// The limit of `name`: the project's exact or longest matching prefix rule, else the default table, else the fallback.
pub fn limit_for(name: &str, limits: &[Limit]) -> f64 {
    let matches = |pattern: &str| match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    };
    let exact = limits.iter().find(|l| l.metric == name);
    if let Some(limit) = exact {
        return limit.max;
    }
    let best = limits
        .iter()
        .filter(|l| l.metric.ends_with('*') && matches(&l.metric))
        .max_by_key(|l| l.metric.len());
    if let Some(limit) = best {
        return limit.max;
    }
    DEFAULTS
        .iter()
        .find(|(pattern, _)| matches(pattern))
        .map_or(FALLBACK_PERCENT, |(_, max)| *max)
}

/// Compares each candidate metric that the baseline also has and that has a positive baseline value. A metric missing
/// from either side is not compared.
pub fn compare(baseline: &[Metric], candidate: &[Metric], limits: &[Limit]) -> Vec<Comparison> {
    candidate
        .iter()
        .filter_map(|cand| {
            let base = baseline.iter().find(|b| b.name == cand.name)?;
            if base.value <= 0.0 || !base.value.is_finite() || !cand.value.is_finite() {
                return None;
            }
            let limit = limit_for(&cand.name, limits);
            let (change, unit) = if is_ratio(&cand.name) {
                ((cand.value - base.value) * 100.0, "points")
            } else {
                ((cand.value - base.value) / base.value * 100.0, "percent")
            };
            let regression = if higher_is_better(&cand.name) {
                -change
            } else {
                change
            };
            Some(Comparison {
                metric: cand.name.clone(),
                baseline: base.value,
                candidate: cand.value,
                change,
                limit,
                passed: regression <= limit,
                unit: unit.into(),
            })
        })
        .collect()
}

/// The comparisons that failed.
pub fn failures(comparisons: &[Comparison]) -> Vec<&Comparison> {
    comparisons.iter().filter(|c| !c.passed).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(name: &str, value: f64) -> Metric {
        Metric {
            name: name.into(),
            value,
        }
    }

    #[test]
    fn a_twenty_percent_slowdown_fails_the_default_latency_limit() {
        let base = [
            metric("load.latency.p50_ms", 100.0),
            metric("load.latency.p95_ms", 140.0),
        ];
        let slow = [
            metric("load.latency.p50_ms", 120.0),
            metric("load.latency.p95_ms", 168.0),
        ];
        let comparisons = compare(&base, &slow, &[]);
        assert_eq!(comparisons.len(), 2);
        assert!(comparisons.iter().all(|c| !c.passed));
        assert!((comparisons[0].change - 20.0).abs() < 1e-9);
        assert_eq!(comparisons[0].limit, 15.0);
        assert_eq!(failures(&comparisons).len(), 2);
    }

    #[test]
    fn an_unchanged_latency_passes() {
        let base = [
            metric("load.latency.p50_ms", 100.0),
            metric("load.error_rate", 0.0),
        ];
        let same = [
            metric("load.latency.p50_ms", 100.0),
            metric("load.error_rate", 0.0),
        ];
        let comparisons = compare(&base, &same, &[]);
        assert!(comparisons.iter().all(|c| c.passed));
        assert!(failures(&comparisons).is_empty());
    }

    #[test]
    fn a_faster_candidate_passes_and_a_lower_throughput_fails() {
        let base = [
            metric("load.latency.p50_ms", 100.0),
            metric("load.throughput_rps", 200.0),
        ];
        let better = [
            metric("load.latency.p50_ms", 60.0),
            metric("load.throughput_rps", 210.0),
        ];
        assert!(compare(&base, &better, &[]).iter().all(|c| c.passed));
        let worse = [
            metric("load.latency.p50_ms", 100.0),
            metric("load.throughput_rps", 160.0),
        ];
        let comparisons = compare(&base, &worse, &[]);
        assert!(
            !comparisons[1].passed,
            "a 20% fall in throughput is past its 10% limit"
        );
    }

    #[test]
    fn an_error_rate_is_judged_in_points_against_its_limit() {
        let base = [metric("load.error_rate", 0.001)];
        let within = [metric("load.error_rate", 0.005)];
        let past = [metric("load.error_rate", 0.03)];
        let a = compare(&base, &within, &[]);
        assert!(a[0].passed, "{a:?}");
        assert_eq!(a[0].unit, "points");
        assert!(!compare(&base, &past, &[]).iter().all(|c| c.passed));
    }

    #[test]
    fn the_project_limits_win_over_the_defaults_and_prefixes_match() {
        let limits = [
            Limit {
                metric: "load.latency.p50_ms".into(),
                max: 30.0,
            },
            Limit {
                metric: "bench.*".into(),
                max: 5.0,
            },
            Limit {
                metric: "bench.parse*".into(),
                max: 40.0,
            },
        ];
        assert_eq!(limit_for("load.latency.p50_ms", &limits), 30.0);
        assert_eq!(limit_for("bench.encode.ns", &limits), 5.0);
        assert_eq!(limit_for("bench.parse_fast.ns", &limits), 40.0);
        assert_eq!(limit_for("bench.parse_fast.ns", &[]), 10.0);
        assert_eq!(limit_for("other", &[]), FALLBACK_PERCENT);
    }

    #[test]
    fn a_metric_without_a_baseline_or_a_zero_baseline_is_not_compared() {
        let base = [metric("load.latency.p50_ms", 0.0)];
        let cand = [
            metric("load.latency.p50_ms", 50.0),
            metric("load.latency.p95_ms", 50.0),
        ];
        assert!(compare(&base, &cand, &[]).is_empty());
    }
}
