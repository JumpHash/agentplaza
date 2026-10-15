//! The fixture's events, calls, traces, alerts and the other reports that are not series.

use apz_metrics::Event;
use apz_metrics::trace::Trace;

use super::Fixture;
use crate::source::{AlertsState, Budget, CallRow, SelfCost};

pub fn events(
    f: &Fixture,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    kinds: &[&str],
    limit: usize,
) -> Vec<Event> {
    let _ = (f, scope, from_ms, to_ms, kinds, limit);
    Vec::new()
}

pub fn calls(f: &Fixture, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
    let _ = (f, scope, before_ms, limit);
    Vec::new()
}

pub fn traces(f: &Fixture, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace> {
    let _ = (f, scope, from_ms, to_ms, limit);
    Vec::new()
}

pub fn trace(f: &Fixture, id: &str) -> Option<Trace> {
    let _ = (f, id);
    None
}

pub fn alerts(f: &Fixture) -> AlertsState {
    let _ = f;
    AlertsState {
        rules: Vec::new(),
        subjects: Vec::new(),
        muted: Vec::new(),
        acked: Vec::new(),
        history: Vec::new(),
        slos: Vec::new(),
    }
}

pub fn self_cost() -> SelfCost {
    SelfCost::default()
}

pub fn budgets() -> Vec<Budget> {
    Vec::new()
}

pub fn report(node: &str) -> serde_json::Value {
    let _ = node;
    serde_json::Value::Null
}

pub fn rate_limits() -> Vec<(String, Option<f64>, Option<f64>)> {
    Vec::new()
}

pub fn outside() -> Vec<(String, bool, Option<f64>, String)> {
    Vec::new()
}

pub fn links() -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
    Vec::new()
}
