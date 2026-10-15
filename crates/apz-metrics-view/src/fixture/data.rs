//! The fixture plaza's shape and its generated series.

use apz_metrics::HistSnapshot;
use apz_metrics::query::AggFn;

use super::Fixture;
use crate::model::ScopeNode;
use crate::source::{AgentInfo, Entity, Group, Read};

/// Grouped series of a read.
pub fn series(f: &Fixture, read: &Read) -> Vec<Group> {
    let _ = (f, read);
    Vec::new()
}

/// One number for a metric over a window.
pub fn scalar(
    f: &Fixture,
    metric: &str,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    agg: AggFn,
) -> Option<f64> {
    let _ = (f, metric, scope, from_ms, to_ms, agg);
    None
}

/// A distribution over a window.
pub fn hist(f: &Fixture, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot {
    let _ = (f, metric, scope, from_ms, to_ms);
    HistSnapshot::default()
}

/// One distribution per step.
pub fn hist_steps(
    f: &Fixture,
    metric: &str,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    step_ms: u64,
) -> Vec<HistSnapshot> {
    let _ = (f, metric, scope, from_ms, to_ms, step_ms);
    Vec::new()
}

/// The scope tree.
pub fn scopes() -> Vec<ScopeNode> {
    vec![ScopeNode {
        id: "global".into(),
        label: "Global".into(),
        kind: "global".into(),
        group: String::new(),
        depth: 0,
        tag: String::new(),
        status: None,
    }]
}

/// Entities of a kind under a scope.
pub fn entities(scope: &str, kind: &str) -> Vec<Entity> {
    let _ = (scope, kind);
    Vec::new()
}

/// An agent of the fixture.
pub fn agent(cid: u64) -> Option<AgentInfo> {
    let _ = cid;
    None
}
