//! A source of generated data: the fixture both consoles show with `fixture=1`, and the data the composers' tests
//! run against. Deterministic: every value is a function of the metric, the group, the time bucket and the seed,
//! so two windows that overlap agree, and the web and desktop screenshots of the fixture match.
//!
//! The plaza it describes (the same as the design sample): nodes `n0` hub box, `n1` rack-a (runs r12 with 4 VMs,
//! r15 with 3), `n2` rack-b (run r19 with 5 VMs; offline for 14 minutes about 44 % into the last day); a QEMU
//! workstation `ws:design` on n1 and a sandbox `sb:7f3a` on n2; agents ada 4417, kit 4418 (builder, r12),
//! rhea 4421 (reviewer, r12), juno 4430 (researcher, r15), otto 4431 (ops, r15), mira 4440 (builder, r15);
//! providers anthropic (opus-5.5, sonnet-5.5, haiku-5.5), openai (gpt-5), local (qwen3-32b).
//!
//! * [`data`] — the plaza's shape, the generators, `series`, `scalar`, `hist`, `hist_steps`, `scopes`,
//!   `entities`, `agent`.
//! * [`extra`] — events, calls, traces, alerts, self cost, budgets, node reports, rate limits,
//!   outside services, node links, saved pins.

use apz_metrics::query::AggFn;
use apz_metrics::trace::Trace;
use apz_metrics::{Event, HistSnapshot, Level};

use crate::model::{Saved, ScopeNode};
use crate::source::*;

pub mod data;
pub mod extra;

/// The fixture plaza at a moment.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub now_ms: u64,
}

impl Fixture {
    pub fn new(now_ms: u64) -> Fixture {
        Fixture { now_ms }
    }
}

impl Source for Fixture {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
    fn level(&self) -> (Level, bool) {
        (Level::Standard, false)
    }
    fn series(&self, read: &Read) -> Vec<Group> {
        data::series(self, read)
    }
    fn scalar(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64> {
        data::scalar(self, metric, scope, from_ms, to_ms, agg)
    }
    fn hist(&self, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot {
        data::hist(self, metric, scope, from_ms, to_ms)
    }
    fn hist_steps(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot> {
        data::hist_steps(self, metric, scope, from_ms, to_ms, step_ms)
    }
    fn events(
        &self,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        kinds: &[&str],
        limit: usize,
    ) -> Vec<Event> {
        extra::events(self, scope, from_ms, to_ms, kinds, limit)
    }
    fn scopes(&self) -> Vec<ScopeNode> {
        data::scopes()
    }
    fn entities(&self, scope: &str, kind: &str) -> Vec<Entity> {
        data::entities(scope, kind)
    }
    fn agent(&self, cid: u64) -> Option<AgentInfo> {
        data::agent(cid)
    }
    fn calls(&self, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
        extra::calls(self, scope, before_ms, limit)
    }
    fn prices(&self, models: &[String]) -> Prices {
        extra::price_list(self, models)
    }
    fn traces(&self, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace> {
        extra::traces(self, scope, from_ms, to_ms, limit)
    }
    fn trace(&self, id: &str) -> Option<Trace> {
        extra::trace(self, id)
    }
    fn alerts(&self) -> AlertsState {
        extra::alerts(self)
    }
    fn self_cost(&self) -> SelfCost {
        extra::self_cost()
    }
    fn budgets(&self) -> Vec<Budget> {
        extra::budgets()
    }
    fn monthly_budget(&self) -> Option<f64> {
        Some(1200.0)
    }
    fn report(&self, node: &str) -> serde_json::Value {
        extra::report(node)
    }
    fn rate_limits(&self) -> Vec<(String, Option<f64>, Option<f64>)> {
        extra::rate_limits()
    }
    fn outside(&self) -> Vec<(String, bool, Option<f64>, String)> {
        extra::outside()
    }
    fn links(&self) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
        extra::links()
    }
    fn saved(&self) -> Saved {
        Saved {
            starter: true,
            ..Saved::default()
        }
    }
    fn basis_days(&self) -> u32 {
        14
    }
}
