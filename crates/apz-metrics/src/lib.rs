//! The plaza's metrics core, shared by the hub (apid), the node daemon (noded), the node link (apz-link), the
//! guest harness and the CLI. Pure data and maths: no I/O, no threads, no clocks (callers pass `now_ms`), except
//! [`prom`].
//!
//! * [`catalog`] — every metric the plaza records: id, unit, kind, collection level, scopes, labels, text.
//! * [`hist`] — fixed log-scale histograms (±2 %), atomic for recording, sparse snapshots that merge exactly.
//! * [`registry`] — handles registered once (counter, gauge, histogram); recording is one atomic operation;
//!   a hard cap per entity kind folds the overflow into an `other` series.
//! * [`chunk`] — Gorilla-style compression of `(t, v)` points (delta-of-delta times, XOR'd floats).
//! * [`agg`] — window aggregates (min, max, sum, count, last, histogram) and the in-memory rollup that closes
//!   10 s, 1 min and 1 h windows as samples arrive.
//! * [`query`] — tier choice, bucketing, reducers (sum, avg, rate, percentiles) and LTTB downsampling.
//! * [`wire`] — the node → hub batch: delta-encoded columns with skips for unchanged values.
//! * [`alerts`] — rules, the ok → pending → firing → resolved tracker, service levels.
//! * [`forecast`] — least-squares projections with a p10–p90 band, crossing times, month-end spend.
//! * [`trace`] — spans, the per-agent ring buffer and the tail sampler.
//! * [`movers`] — "what moved most" between two windows.
//! * [`openmetrics`] — the OpenMetrics text exposition of aggregates (catalog labels only).
//! * [`level`] — collection levels and the budget governor that steps them down and back up.
//! * [`prom`] — the daemons' static counters, gauges and latency histograms in the Prometheus text format (their
//!   `GET /v1/metrics`), and the one-line `timing op=…` records on stderr. It keeps its own clock and writes.

#![forbid(unsafe_code)]

pub mod agg;
pub mod alerts;
pub mod catalog;
pub mod chunk;
pub mod forecast;
pub mod hist;
pub mod level;
pub mod movers;
pub mod openmetrics;
pub mod prom;
pub mod query;
pub mod registry;
pub mod trace;
pub mod wire;

use serde::{Deserialize, Serialize};

pub use agg::{Agg, Closed, Rollup, Tier};
pub use catalog::{CATALOG, MetricDef, MetricKind, Unit};
pub use hist::{AtomicHist, HistSnapshot};
pub use level::Level;
pub use registry::{Counter, Gauge, Hist, Registry, Sample, Value};

/// A point: milliseconds since the Unix epoch, and a value.
pub type Point = (u64, f64);

/// What a series is about. The hub resolves a scope (a node, a role, a provider…) into the entities under it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    apz_store::Column,
)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Global,
    Node,
    Run,
    Vm,
    Workstation,
    Sandbox,
    Agent,
    Role,
    Provider,
    Model,
    Tool,
    Route,
    Service,
    Volume,
    Engine,
    Hub,
}

impl EntityKind {
    /// The scope-id prefix of this kind (`node:n1`, `vm:r12/3`, `agent:4417` …); `global` has no id part.
    pub fn prefix(self) -> &'static str {
        match self {
            EntityKind::Global => "global",
            EntityKind::Node => "node",
            EntityKind::Run => "run",
            EntityKind::Vm => "vm",
            EntityKind::Workstation => "ws",
            EntityKind::Sandbox => "sb",
            EntityKind::Agent => "agent",
            EntityKind::Role => "role",
            EntityKind::Provider => "provider",
            EntityKind::Model => "model",
            EntityKind::Tool => "tool",
            EntityKind::Route => "route",
            EntityKind::Service => "service",
            EntityKind::Volume => "volume",
            EntityKind::Engine => "engine",
            EntityKind::Hub => "hub",
        }
    }

    /// The kind of a scope-id prefix, the inverse of [`EntityKind::prefix`].
    pub fn from_prefix(p: &str) -> Option<EntityKind> {
        ALL_KINDS.iter().copied().find(|k| k.prefix() == p)
    }
}

/// Every kind, in declaration order.
pub const ALL_KINDS: [EntityKind; 16] = [
    EntityKind::Global,
    EntityKind::Node,
    EntityKind::Run,
    EntityKind::Vm,
    EntityKind::Workstation,
    EntityKind::Sandbox,
    EntityKind::Agent,
    EntityKind::Role,
    EntityKind::Provider,
    EntityKind::Model,
    EntityKind::Tool,
    EntityKind::Route,
    EntityKind::Service,
    EntityKind::Volume,
    EntityKind::Engine,
    EntityKind::Hub,
];

/// The most labels a series may carry. Labels come from the catalog, never from request text.
pub const MAX_LABELS: usize = 2;

/// One series: a metric about one entity, with at most [`MAX_LABELS`] catalog labels (sorted by name).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SeriesKey {
    pub metric: String,
    pub kind: EntityKind,
    pub entity: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<(String, String)>,
}

impl SeriesKey {
    pub fn new(metric: &str, kind: EntityKind, entity: &str) -> SeriesKey {
        SeriesKey {
            metric: metric.to_string(),
            kind,
            entity: entity.to_string(),
            labels: Vec::new(),
        }
    }

    /// Adds a label, keeping labels sorted by name; a label past [`MAX_LABELS`] is ignored and an existing
    /// name is replaced.
    pub fn label(mut self, name: &str, value: &str) -> SeriesKey {
        if let Some(l) = self.labels.iter_mut().find(|l| l.0 == name) {
            l.1 = value.to_string();
        } else if self.labels.len() < MAX_LABELS {
            self.labels.push((name.to_string(), value.to_string()));
            self.labels.sort();
        }
        self
    }

    /// The value of a label, if set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|l| l.0 == name)
            .map(|l| l.1.as_str())
    }

    /// The scope id of the entity (`node:n1`, `agent:4417`, `global`).
    pub fn scope(&self) -> String {
        match self.kind {
            EntityKind::Global => "global".to_string(),
            k => format!("{}:{}", k.prefix(), self.entity),
        }
    }

    /// A stable one-line text form, used as the storage key: `metric|kind|entity|name=value,name=value`.
    pub fn id(&self) -> String {
        let labels: Vec<String> = self
            .labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        format!(
            "{}|{}|{}|{}",
            self.metric,
            self.kind.prefix(),
            self.entity,
            labels.join(",")
        )
    }

    /// Parses [`SeriesKey::id`].
    pub fn parse(id: &str) -> Option<SeriesKey> {
        let mut parts = id.splitn(4, '|');
        let metric = parts.next()?;
        let kind = EntityKind::from_prefix(parts.next()?)?;
        let entity = parts.next()?;
        let mut key = SeriesKey::new(metric, kind, entity);
        for pair in parts
            .next()
            .unwrap_or("")
            .split(',')
            .filter(|s| !s.is_empty())
        {
            let (k, v) = pair.split_once('=')?;
            key = key.label(k, v);
        }
        Some(key)
    }
}

/// Something that happened, with how long it took when that is the point of it: a VM boot, a wake (with its
/// restore and harness-ready parts), a compaction, a scaling decision, an alert change, a release.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub t_ms: u64,
    /// One of [`EVENT_KINDS`].
    pub kind: String,
    pub entity_kind: EntityKind,
    pub entity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// One plain line for people: "ready in 412 ms (restore 160 ms)".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Named parts of the duration or numbers that go with it: `restore_ms`, `ready_ms`, `before`, `after`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<(String, f64)>,
}

impl Event {
    pub fn part(&self, name: &str) -> Option<f64> {
        self.parts.iter().find(|p| p.0 == name).map(|p| p.1)
    }
}

/// The event kinds. Lifecycle: boot, resume, wake, sleep, pause, snapshot, fork, sandbox_create, move, exit.
/// Work: compaction, error, scale, law, approval. Alerts: alert. Annotations: release, law_change, model_switch,
/// setting.
pub const EVENT_KINDS: &[&str] = &[
    "boot",
    "resume",
    "wake",
    "sleep",
    "pause",
    "snapshot",
    "fork",
    "sandbox_create",
    "move",
    "exit",
    "compaction",
    "error",
    "scale",
    "law",
    "approval",
    "alert",
    "release",
    "law_change",
    "model_switch",
    "setting",
];

/// The event kinds drawn as annotations on every time chart.
pub const ANNOTATION_KINDS: &[&str] = &["release", "law_change", "model_switch", "setting"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_key_round_trips_through_its_id_and_keeps_two_sorted_labels() {
        let k = SeriesKey::new("llm.ttft_ms", EntityKind::Agent, "4417")
            .label("provider", "anthropic")
            .label("model", "opus")
            .label("extra", "ignored");
        assert_eq!(
            k.labels,
            vec![
                ("model".into(), "opus".into()),
                ("provider".into(), "anthropic".into())
            ]
        );
        assert_eq!(SeriesKey::parse(&k.id()), Some(k.clone()));
        assert_eq!(k.scope(), "agent:4417");
    }
}
