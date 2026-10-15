//! The node → hub batch, carried in the heartbeat body under `metrics` (and posted on its own to
//! `/v1/nodes/{id}/metrics` when it is too big for a beat, or every 2 s while a scope on the node is watched live).
//!
//! Gauges and counters travel as columns: a start time, a step, and values where the first is absolute and each
//! later one a delta from the previous *sent* value; `null` is a skip (the value did not change by more than the
//! threshold, so the receiver repeats the previous one). Histograms travel as snapshots. Times are the node's
//! clock; the hub corrects them by the node's measured offset.

use crate::hist::HistSnapshot;
use crate::{Event, Point, SeriesKey};
use serde::{Deserialize, Serialize};

/// A gauge counts as unchanged within this relative difference.
pub const SKIP_REL: f64 = 0.005;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub node: String,
    /// The node's clock when the batch was sent.
    pub sent_ms: u64,
    #[serde(default)]
    pub series: Vec<Column>,
    #[serde(default)]
    pub hists: Vec<HistPoint>,
    #[serde(default)]
    pub events: Vec<Event>,
    /// Latest-only reports that are not series: top guest processes, GPU names, volume mount points.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub reports: serde_json::Value,
}

/// One gauge or counter series over the batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub key: SeriesKey,
    /// True for a counter (values are increases; a skip means 0, not a repeat).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub counter: bool,
    pub t0_ms: u64,
    pub step_ms: u64,
    /// First absolute, then deltas from the previous sent value; `None` = skip.
    pub v: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistPoint {
    pub key: SeriesKey,
    pub t_ms: u64,
    pub hist: HistSnapshot,
}

/// Encodes evenly spaced points (`points[i].0 == t0 + i·step`) of a gauge, skipping values within [`SKIP_REL`] of
/// the last sent one (a skip is never the first value).
pub fn encode_gauge(key: SeriesKey, t0_ms: u64, step_ms: u64, values: &[f64]) -> Column {
    let mut v = Vec::with_capacity(values.len());
    let mut last_sent: Option<f64> = None;
    for &x in values {
        match last_sent {
            None => {
                v.push(Some(x));
                last_sent = Some(x);
            }
            Some(last) => {
                if (x - last).abs() <= SKIP_REL * last.abs().max(1e-9) {
                    v.push(None);
                } else {
                    v.push(Some(x - last));
                    last_sent = Some(x);
                }
            }
        }
    }
    Column {
        key,
        counter: false,
        t0_ms,
        step_ms,
        v,
    }
}

/// Encodes a counter's per-step increases, skipping zeros.
pub fn encode_counter(key: SeriesKey, t0_ms: u64, step_ms: u64, increases: &[f64]) -> Column {
    let v = increases
        .iter()
        .map(|&x| if x == 0.0 { None } else { Some(x) })
        .collect();
    Column {
        key,
        counter: true,
        t0_ms,
        step_ms,
        v,
    }
}

/// The points of a column. A gauge's skips repeat the previous value; a counter's skips are dropped (no point).
pub fn decode(c: &Column) -> Vec<Point> {
    let mut points = Vec::with_capacity(c.v.len());
    let mut cur: Option<f64> = None;
    for (i, v) in c.v.iter().enumerate() {
        let t = c.t0_ms + i as u64 * c.step_ms;
        if c.counter {
            if let Some(x) = v {
                points.push((t, *x));
            }
            continue;
        }
        cur = match (v, cur) {
            (Some(a), None) => Some(*a),
            (Some(d), Some(prev)) => Some(prev + d),
            (None, prev) => prev,
        };
        if let Some(x) = cur {
            points.push((t, x));
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityKind;

    fn key(metric: &str) -> SeriesKey {
        SeriesKey::new(metric, EntityKind::Node, "node-1")
    }

    #[test]
    fn a_gauge_round_trips_within_skip_rel_and_skips_unchanged_values() {
        let values = [10.0, 10.01, 10.02, 12.0, 12.0, 12.01, 9.0];
        let col = encode_gauge(key("cpu"), 1_000, 500, &values);
        assert_eq!(col.v[0], Some(10.0));
        // 10.01 and 10.02 are within 0.5% of the last sent 10.0, so they are skipped.
        assert_eq!(col.v[1], None);
        assert_eq!(col.v[2], None);
        assert_eq!(col.v[4], None);

        let points = decode(&col);
        assert_eq!(points.len(), values.len());
        for (i, (&(t, got), &want)) in points.iter().zip(values.iter()).enumerate() {
            assert_eq!(t, 1_000 + i as u64 * 500);
            assert!(
                (got - want).abs() <= SKIP_REL * want.abs().max(1e-9) + 1e-9,
                "point {i}: decoded {got}, original {want}"
            );
        }
    }

    #[test]
    fn a_counter_keeps_only_its_non_zero_steps() {
        let col = encode_counter(key("requests"), 0, 1_000, &[0.0, 3.0, 0.0, 0.0, 5.0]);
        assert!(col.counter);
        assert_eq!(col.v, vec![None, Some(3.0), None, None, Some(5.0)]);
        assert_eq!(decode(&col), vec![(1_000, 3.0), (4_000, 5.0)]);
    }

    #[test]
    fn a_batch_serializes_skips_as_nulls_and_round_trips_through_json() {
        let batch = Batch {
            node: "node-1".to_string(),
            sent_ms: 42,
            series: vec![encode_gauge(key("cpu"), 0, 1_000, &[1.0, 1.0, 2.0])],
            ..Batch::default()
        };
        let json = serde_json::to_string(&batch).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["series"][0]["v"], serde_json::json!([1.0, null, 1.0]));

        let back: Batch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, batch);
    }
}
