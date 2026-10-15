//! The parts of the hub's [`super::source::LiveSource`] that are not series reads: events, calls and traces in a
//! scope, the "Metrics cost" rows, budgets, the treasury, rate-limit headroom, outside services, node links, and the
//! level in force.

use apz_metrics::trace::Trace;
use apz_metrics::{Event, Level};
use apz_metrics_view::source::{Budget, CallRow, SelfCost};

use super::source::LiveSource;

/// The level in force and whether the governor stepped it down from the owner's choice.
pub fn level(src: &LiveSource<'_>) -> (Level, bool) {
    let g = super::lock(&src.m.governor);
    (g.effective, g.effective < g.chosen)
}

pub fn events(
    src: &LiveSource<'_>,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    kinds: &[&str],
    limit: usize,
) -> Vec<Event> {
    let _ = (src, scope, from_ms, to_ms, kinds, limit);
    todo!()
}

pub fn calls(src: &LiveSource<'_>, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
    let _ = (src, scope, before_ms, limit);
    todo!()
}

pub fn traces(
    src: &LiveSource<'_>,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    limit: usize,
) -> Vec<Trace> {
    let _ = (src, scope, from_ms, to_ms, limit);
    todo!()
}

pub fn self_cost(src: &LiveSource<'_>) -> SelfCost {
    let _ = src;
    todo!()
}

pub fn budgets(src: &LiveSource<'_>) -> Vec<Budget> {
    let _ = src;
    todo!()
}

pub fn treasury(src: &LiveSource<'_>) -> Option<(f64, f64)> {
    let _ = src;
    todo!()
}

pub fn rate_limits(src: &LiveSource<'_>) -> Vec<(String, Option<f64>, Option<f64>)> {
    let _ = src;
    todo!()
}

pub fn outside(src: &LiveSource<'_>) -> Vec<(String, bool, Option<f64>, String)> {
    let _ = src;
    todo!()
}

pub fn links(src: &LiveSource<'_>) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
    let _ = src;
    todo!()
}
