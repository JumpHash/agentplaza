//! [`Source`] over the hub: series from the store plus the rollup's open windows, the scope tree and entities from
//! the roster, the nodes and the node reports, calls and traces from the store, alerts from the engine.
//!
//! Scope resolution (which series of a metric fall under a scope) goes through the metric's catalog entity kind:
//! an agent-keyed metric under `node:n1` reads the agents placed on n1; a VM-keyed metric under `run:r12` reads
//! r12's VMs; under `provider:anthropic` an agent-keyed metric keeps the series labelled `provider=anthropic`.

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
        misc::level(self)
    }
    fn series(&self, read: &Read) -> Vec<Group> {
        reads::series(self, read)
    }
    fn scalar(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64> {
        reads::scalar(self, metric, scope, from_ms, to_ms, agg)
    }
    fn hist(&self, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot {
        reads::hist(self, metric, scope, from_ms, to_ms)
    }
    fn hist_steps(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot> {
        reads::hist_steps(self, metric, scope, from_ms, to_ms, step_ms)
    }
    fn events(
        &self,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        kinds: &[&str],
        limit: usize,
    ) -> Vec<Event> {
        misc::events(self, scope, from_ms, to_ms, kinds, limit)
    }
    fn scopes(&self) -> Vec<ScopeNode> {
        super::scope::tree(self.app)
    }
    fn entities(&self, scope: &str, kind: &str) -> Vec<Entity> {
        super::scope::entities(self.app, scope, kind)
    }
    fn agent(&self, cid: u64) -> Option<AgentInfo> {
        super::scope::agent(self.app, cid)
    }
    fn calls(&self, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
        misc::calls(self, scope, before_ms, limit)
    }
    fn traces(&self, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace> {
        misc::traces(self, scope, from_ms, to_ms, limit)
    }
    fn trace(&self, id: &str) -> Option<Trace> {
        self.m.store.trace(id)
    }
    fn alerts(&self) -> AlertsState {
        self.m.alerts.state(self.m, self.app, self.now_ms)
    }
    fn self_cost(&self) -> SelfCost {
        misc::self_cost(self)
    }
    fn budgets(&self) -> Vec<Budget> {
        misc::budgets(self)
    }
    fn monthly_budget(&self) -> Option<f64> {
        self.m.settings.get().monthly_budget_usd
    }
    fn treasury(&self) -> Option<(f64, f64)> {
        misc::treasury(self)
    }
    fn report(&self, node: &str) -> serde_json::Value {
        let node = node.strip_prefix("node:").unwrap_or(node);
        super::lock(&self.m.links)
            .get(node)
            .map(|l| l.reports.clone())
            .unwrap_or(serde_json::Value::Null)
    }
    fn rate_limits(&self) -> Vec<(String, Option<f64>, Option<f64>)> {
        misc::rate_limits(self)
    }
    fn outside(&self) -> Vec<(String, bool, Option<f64>, String)> {
        misc::outside(self)
    }
    fn links(&self) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
        misc::links(self)
    }
    fn saved(&self) -> Saved {
        self.m
            .store
            .kv_get("dashboards")
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| Saved {
                starter: true,
                ..Saved::default()
            })
    }
    fn basis_days(&self) -> u32 {
        self.m.settings.get().basis_days
    }
}

/// Series reads: the store's closed windows plus the rollup's open ones, filtered by scope, grouped, merged and
/// bucketed.
pub mod reads {
    use super::*;

    /// See [`Source::series`].
    pub fn series(src: &LiveSource<'_>, read: &Read) -> Vec<Group> {
        let _ = (src, read);
        todo!()
    }

    /// See [`Source::scalar`].
    pub fn scalar(
        src: &LiveSource<'_>,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64> {
        let _ = (src, metric, scope, from_ms, to_ms, agg);
        todo!()
    }

    /// See [`Source::hist`].
    pub fn hist(
        src: &LiveSource<'_>,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
    ) -> HistSnapshot {
        let _ = (src, metric, scope, from_ms, to_ms);
        todo!()
    }

    /// See [`Source::hist_steps`].
    pub fn hist_steps(
        src: &LiveSource<'_>,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot> {
        let _ = (src, metric, scope, from_ms, to_ms, step_ms);
        todo!()
    }
}

pub use super::source_misc as misc;
