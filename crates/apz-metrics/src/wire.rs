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
    let _ = (key, t0_ms, step_ms, values);
    todo!()
}

/// Encodes a counter's per-step increases, skipping zeros.
pub fn encode_counter(key: SeriesKey, t0_ms: u64, step_ms: u64, increases: &[f64]) -> Column {
    let _ = (key, t0_ms, step_ms, increases);
    todo!()
}

/// The points of a column. A gauge's skips repeat the previous value; a counter's skips are dropped (no point).
pub fn decode(c: &Column) -> Vec<Point> {
    let _ = c;
    todo!()
}
