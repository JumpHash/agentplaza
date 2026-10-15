//! [`Source`] over the hub: series from the store plus the rollup's open windows, the scope tree and entities from
//! the roster, the nodes and the node reports, calls and traces from the store, alerts from the engine.
//!
//! Scope resolution (which series of a metric fall under a scope) goes through the metric's catalog entity kind:
//! an agent-keyed metric under `node:n1` reads the agents placed on n1; a VM-keyed metric under `run:r12` reads
//! r12's VMs; under `provider:anthropic` an agent-keyed metric keeps the series labelled `provider=anthropic`.

use apz_metrics::alerts::{SloStatus, State};
use apz_metrics::query::AggFn;
use apz_metrics::trace::Trace;
use apz_metrics::{Event, HistSnapshot, Level};
use apz_metrics_view::source::*;
use apz_metrics_view::{Saved, ScopeNode};

use super::Metrics;
use crate::state::App;

/// The hub as a source, at a moment.
pub struct LiveSource<'a> {
    pub app: &'a App,
    pub m: &'a Metrics,
    pub now_ms: u64,
}

impl Source for LiveSource<'_> {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
    fn level(&self) -> (Level, bool) {
        todo!()
    }
    fn series(&self, read: &Read) -> Vec<Group> {
        let _ = read;
        todo!()
    }
    fn scalar(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64> {
        let _ = (metric, scope, from_ms, to_ms, agg);
        todo!()
    }
    fn hist(&self, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot {
        let _ = (metric, scope, from_ms, to_ms);
        todo!()
    }
    fn hist_steps(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot> {
        let _ = (metric, scope, from_ms, to_ms, step_ms);
        todo!()
    }
    fn events(
        &self,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        kinds: &[&str],
        limit: usize,
    ) -> Vec<Event> {
        let _ = (scope, from_ms, to_ms, kinds, limit);
        todo!()
    }
    fn scopes(&self) -> Vec<ScopeNode> {
        todo!()
    }
    fn entities(&self, scope: &str, kind: &str) -> Vec<Entity> {
        let _ = (scope, kind);
        todo!()
    }
    fn agent(&self, cid: u64) -> Option<AgentInfo> {
        let _ = cid;
        todo!()
    }
    fn calls(&self, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
        let _ = (scope, before_ms, limit);
        todo!()
    }
    fn traces(&self, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace> {
        let _ = (scope, from_ms, to_ms, limit);
        todo!()
    }
    fn trace(&self, id: &str) -> Option<Trace> {
        let _ = id;
        todo!()
    }
    fn alerts(&self) -> AlertsState {
        todo!()
    }
    fn self_cost(&self) -> SelfCost {
        todo!()
    }
    fn budgets(&self) -> Vec<Budget> {
        todo!()
    }
    fn monthly_budget(&self) -> Option<f64> {
        self.m.settings.get().monthly_budget_usd
    }
    fn treasury(&self) -> Option<(f64, f64)> {
        todo!()
    }
    fn report(&self, node: &str) -> serde_json::Value {
        let _ = node;
        todo!()
    }
    fn rate_limits(&self) -> Vec<(String, Option<f64>, Option<f64>)> {
        todo!()
    }
    fn outside(&self) -> Vec<(String, bool, Option<f64>, String)> {
        todo!()
    }
    fn links(&self) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
        todo!()
    }
    fn saved(&self) -> Saved {
        todo!()
    }
    fn basis_days(&self) -> u32 {
        self.m.settings.get().basis_days
    }
}

#[allow(dead_code)]
fn _types(_: SloStatus, _: State) {}
