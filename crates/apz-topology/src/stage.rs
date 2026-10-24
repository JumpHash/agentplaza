//! The data, cache and messaging stages (PLAN 22.8): what a lab run measures for each stage the topology declares, how the
//! measurements are recorded at the run, and how the hub judges them. Each stage module ([`data`], [`cache`],
//! [`messaging`]) runs its checks where the guest is and reduces them to numbers; the numbers are recorded as OTLP gauges
//! (as the resilience scenarios are), and [`judge`] turns them into the verdicts the hub files as findings.
//!
//! A stage that the run recorded no numbers for did not run here: its judgment fails with that reason and measures
//! nothing, so nothing is filed for it (the resilience rule).

use serde::Serialize;
use serde_json::{Value, json};

use crate::compare::Metric;
use crate::spec::Topology;
use crate::{cache, data, messaging};

/// One judged item of a stage: what it is (`read:list-tasks`), whether it passed, and the numbers it was judged by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Judgment {
    /// The stage this item belongs to: `data`, `cache` or `messaging`.
    pub stage: &'static str,
    pub id: String,
    pub passed: bool,
    pub detail: String,
    /// Whether the run recorded numbers for it. False is a stage that did not run here.
    pub measured: bool,
}

/// The stages a topology declares, in the order a run takes them.
pub fn declared(topology: &Topology) -> Vec<&'static str> {
    [
        topology.data.is_some().then_some("data"),
        topology.cache.is_some().then_some("cache"),
        topology.messaging.is_some().then_some("messaging"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Every metric name the topology's stages record at a run.
pub fn names(topology: &Topology) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(stage) = &topology.data {
        names.extend(data::names(stage));
    }
    if topology.cache.is_some() {
        names.extend(cache::names());
    }
    if topology.messaging.is_some() {
        names.extend(messaging::names());
    }
    names
}

/// The judgments of every declared stage over the values the run recorded (`values` are the names of [`names`]). A
/// stage with no value at all is one unmeasured judgment: it did not run here.
pub fn judge(topology: &Topology, values: &[Metric]) -> Vec<Judgment> {
    let mut out = Vec::new();
    if let Some(stage) = &topology.data {
        out.extend(stamp("data", data::judge(stage, values)));
    }
    if let Some(stage) = &topology.cache {
        out.extend(stamp("cache", cache::judge(stage, values)));
    }
    if let Some(stage) = &topology.messaging {
        out.extend(stamp("messaging", messaging::judge(stage, values)));
    }
    out
}

/// The judgments of one stage, stamped with the stage's name (a missing stage is `not run`).
fn stamp(stage: &'static str, judged: Vec<(String, bool, String, bool)>) -> Vec<Judgment> {
    judged
        .into_iter()
        .map(|(id, passed, detail, measured)| Judgment {
            stage,
            id,
            passed,
            detail,
            measured,
        })
        .collect()
}

/// The judgment of a stage whose numbers are missing: failed, measuring nothing.
pub fn not_run(id: &str, what: &str) -> (String, bool, String, bool) {
    (
        id.to_string(),
        false,
        format!("{what}: not run, no measurement was recorded at this run"),
        false,
    )
}

/// The value of `name` among `values`.
pub fn value(values: &[Metric], name: &str) -> Option<f64> {
    values.iter().find(|m| m.name == name).map(|m| m.value)
}

/// The measurements as OTLP/JSON gauges, stamped `now_ms`, from the `service` the stage ran as. The hub reads them back
/// by name.
pub fn otlp(service: &str, values: &[Metric], now_ms: u64) -> Value {
    let nanos = (now_ms as u128 * 1_000_000).to_string();
    let metrics: Vec<Value> = values
        .iter()
        .map(|m| {
            json!({"name": m.name, "gauge": {"dataPoints": [
                {"timeUnixNano": nanos, "asDouble": m.value}
            ]}})
        })
        .collect();
    json!({"resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": service}}]},
        "scopeMetrics": [{"metrics": metrics}],
    }]})
}

/// The sentinel a stage records for a value it could not read (a failed query, a missing series). No real count, row
/// number or sum is this far below zero.
pub const UNREADABLE: f64 = -1.0e18;
