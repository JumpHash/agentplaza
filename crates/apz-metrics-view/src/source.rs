//! What the composers read. The hub implements [`Source`] over its store, rollup, roster and alert engine; the
//! fixture ([`crate::fixture`]) implements it from generated data, so every tab composes and tests without a hub.
//!
//! Scopes are ids: `global`, `node:n1`, `run:r12`, `vm:r12/3`, `ws:design`, `sb:7f3a`, `agent:4417`,
//! `role:builder`, `provider:anthropic`, `model:anthropic/opus-5.5`. A source resolves a scope to the entities
//! of a metric's kind under it (the agents placed on a node, the VMs of a run, the agents using a provider).

use apz_metrics::alerts::{Rule, Slo, SloStatus, State};
use apz_metrics::query::AggFn;
use apz_metrics::trace::Trace;
use apz_metrics::{Event, HistSnapshot, Level};
use serde::{Deserialize, Serialize};

use crate::model::{Saved, ScopeNode};

/// How a read groups its series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupBy {
    /// One series for the whole scope.
    All,
    /// By a label value (`provider`, `model`, `part`, `state`, `law`, `gpu`, `run`).
    Label(String),
    /// By the entity of the metric (each node, each agent, each VM, each tool).
    Entity,
    /// By a property of the agent behind an agent-keyed series: `role`, `run`, `node`, `engine`.
    AgentProp(String),
}

impl GroupBy {
    /// The group-by of the view's "group by" choice (`provider`, `model`, `role`, `run`, `agent`, `engine`).
    pub fn of_choice(choice: &str) -> GroupBy {
        match choice {
            "model" => GroupBy::Label("model".into()),
            "role" | "run" | "engine" => GroupBy::AgentProp(choice.into()),
            "agent" => GroupBy::Entity,
            _ => GroupBy::Label("provider".into()),
        }
    }
}

/// One read of a metric within a scope.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub metric: String,
    pub scope: String,
    pub from_ms: u64,
    pub to_ms: u64,
    pub step_ms: u64,
    pub agg: AggFn,
    pub group: GroupBy,
    /// Keep the `top` largest groups (by their total over the window), folding the rest into "other".
    pub top: Option<usize>,
}

/// One grouped series of a read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    /// The group's label: a provider, a model, "n1 · rack-a", "ada (4417)", "other".
    pub name: String,
    /// The scope id a click on it narrows to, when it is a scope.
    pub scope: Option<String>,
    /// One value per step; `None` is a gap.
    pub values: Vec<Option<f64>>,
}

impl Group {
    /// The sum of the values (gaps skipped).
    pub fn total(&self) -> f64 {
        self.values.iter().flatten().sum()
    }
    /// The latest value.
    pub fn last(&self) -> Option<f64> {
        self.values.iter().rev().flatten().next().copied()
    }
}

/// An entity under a scope: its scope id and label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub label: String,
    /// running, asleep, paused … for VMs and agents; empty otherwise.
    #[serde(default)]
    pub state: String,
}

/// What the view needs to know of an agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub cid: u64,
    pub name: String,
    pub role: String,
    pub run: String,
    pub node: String,
    pub provider: String,
    pub model: String,
    /// The model's context window in tokens, when known.
    pub window: Option<u64>,
    /// idle, working, asleep …
    pub status: String,
}

/// One model call, as the call log shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallRow {
    pub at_ms: u64,
    pub cid: u64,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub tokens_in: u64,
    pub cache_read: u64,
    pub tokens_out: u64,
    pub ttft_ms: Option<f64>,
    pub duration_ms: Option<f64>,
    pub cost_usd: f64,
    /// HTTP status of the upstream call (200 when unknown and it succeeded).
    pub status: u16,
    /// The kept trace of the turn this call was in, if any.
    pub trace: Option<String>,
}

/// The model prices behind the spend estimates: when they were fetched, from which lists, and each model's rates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    /// 0 before the first fetch.
    pub fetched_at_ms: u64,
    /// The lists that answered ("OpenRouter", "LiteLLM").
    pub sources: Vec<String>,
    pub rows: Vec<PriceRow>,
}

/// One model's rates in USD per million tokens; `None` throughout when nothing prices it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PriceRow {
    /// `provider/model`.
    pub model: String,
    /// "OpenRouter", "LiteLLM", "manual"; empty when unpriced.
    pub source: String,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    /// The roles or agents set to use it ("builder, reviewer").
    pub used_by: String,
}

/// An alert as the Alerts tab lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertsState {
    pub rules: Vec<Rule>,
    /// (rule id, subject scope id, state, latest value).
    pub subjects: Vec<(String, String, State, f64)>,
    /// Rule ids muted, with the time the mute ends.
    pub muted: Vec<(String, u64)>,
    /// (rule id, subject) acknowledged while firing.
    pub acked: Vec<(String, String)>,
    /// Newest first: (time, rule id, subject, what: fired|resolved|muted|acknowledged, value, lasted ms).
    pub history: Vec<(u64, String, String, String, f64, Option<u64>)>,
    pub slos: Vec<(Slo, SloStatus)>,
}

/// What the metrics system itself costs, against its budget (the "Metrics cost" panel).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfCost {
    /// (label, value, budget, unit text).
    pub rows: Vec<(String, f64, f64, String)>,
}

/// A daily spend cap the owner set (Settings → Metrics), reported against today's spend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// "builder (role)", "ada (agent)", "plaza".
    pub label: String,
    pub scope: String,
    pub cap_usd: f64,
    pub today_usd: f64,
}

/// The data the composers read.
pub trait Source {
    /// The time the view is composed at.
    fn now_ms(&self) -> u64;

    /// The collection level in force and whether the governor stepped it down from the owner's choice.
    fn level(&self) -> (Level, bool);

    /// Grouped series of a metric within a scope.
    fn series(&self, read: &Read) -> Vec<Group>;

    /// One number for a metric within a scope over a window.
    fn scalar(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64>;

    /// The merged distribution of a histogram metric within a scope over a window.
    fn hist(&self, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot;

    /// One merged distribution per step (the latency heatmap).
    fn hist_steps(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot>;

    /// Events in a scope and window, newest first. `kinds` empty means every kind.
    fn events(
        &self,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        kinds: &[&str],
        limit: usize,
    ) -> Vec<Event>;

    /// The scope tree, in order.
    fn scopes(&self) -> Vec<ScopeNode>;

    /// The entities of a kind (`node`, `run`, `vm`, `agent`, `volume`, `service`, `tool`, `route`, `provider`,
    /// `model`, `role`) under a scope.
    fn entities(&self, scope: &str, kind: &str) -> Vec<Entity>;

    fn agent(&self, cid: u64) -> Option<AgentInfo>;

    /// Model calls in a scope, newest first, before `before_ms`.
    fn calls(&self, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow>;

    /// The prices of `models` (`provider/model`) and of the models the settings name, in that order.
    fn prices(&self, models: &[String]) -> Prices {
        let _ = models;
        Prices::default()
    }

    /// Kept traces in a scope and window, newest first.
    fn traces(&self, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace>;

    fn trace(&self, id: &str) -> Option<Trace>;

    fn alerts(&self) -> AlertsState;

    fn self_cost(&self) -> SelfCost;

    fn budgets(&self) -> Vec<Budget>;

    /// The owner's monthly budget for model spend, if set.
    fn monthly_budget(&self) -> Option<f64>;

    /// Plaza credits left in the treasury, and what one credit is worth in USD.
    fn treasury(&self) -> Option<(f64, f64)>;

    /// The latest node reports that are not series (`top_processes`, `gpus`, `volumes`), by node id.
    fn report(&self, node: &str) -> serde_json::Value;

    /// Rate-limit headroom per provider: (provider, requests left %, tokens left %).
    fn rate_limits(&self) -> Vec<(String, Option<f64>, Option<f64>)>;

    /// Outside services: (name, up, p95 ms, last error).
    fn outside(&self) -> Vec<(String, bool, Option<f64>, String)>;

    /// Node links: (node id, label, last beat age ms, p95 lag ms, round trip ms, commands queued, clock offset ms,
    /// mean payload bytes).
    fn links(&self) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)>;

    /// The owner's pins and scopes.
    fn saved(&self) -> Saved;

    /// The forecast basis in days (14 or 30).
    fn basis_days(&self) -> u32;
}
