//! The parts of the hub's [`super::source::LiveSource`] that are not series reads: events, calls and traces in a
//! scope, the "Metrics cost" rows, budgets, rate-limit headroom, outside services, node links, and the
//! level in force.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use apz_metrics::query::AggFn;
use apz_metrics::trace::Trace;
use apz_metrics::{EntityKind, Event, Level, SeriesKey, Tier};
use apz_metrics_view::source::{Budget, CallRow, PriceRow, Prices, SelfCost};

use super::scope;
use super::source::LiveSource;
use super::store::CallRecord;
use crate::state::App;

/// The entity the hub's own series are keyed to (its service id).
const HUB_SERVICE: &str = "hub:metrics";

/// The entity the hub-wide series (storage growth, alert evaluation, kept traces) are keyed to.
const HUB: &str = "hub";

/// The scope id that means the whole plaza.
const GLOBAL_SCOPE: &str = "global";

const DAY_MS: u64 = 86_400_000;

type ScopeFilter = Box<dyn Fn(&SeriesKey) -> bool + Send>;

/// Whether series fall under a scope. Each kind's filter is built once (a filter resolves the scope's members up
/// front) and then reused for every record of that kind.
struct Keeper<'a> {
    app: &'a App,
    scope: &'a str,
    filters: RefCell<HashMap<EntityKind, ScopeFilter>>,
}

impl<'a> Keeper<'a> {
    fn new(app: &'a App, scope: &'a str) -> Keeper<'a> {
        Keeper {
            app,
            scope,
            filters: RefCell::new(HashMap::new()),
        }
    }

    /// Whether the series falls under the scope. The global scope keeps everything.
    fn keeps(&self, key: &SeriesKey) -> bool {
        if self.scope == GLOBAL_SCOPE {
            return true;
        }
        let mut filters = self.filters.borrow_mut();
        let filter = filters
            .entry(key.kind)
            .or_insert_with(|| scope::filter(self.app, self.scope, key.kind));
        filter(key)
    }
}

/// The level in force and whether the governor stepped it down from the owner's choice.
pub fn level(src: &LiveSource<'_>) -> (Level, bool) {
    let g = super::lock(&src.m.governor);
    (g.effective, g.effective < g.chosen)
}

/// Events in the scope and the time range, of the named kinds (all kinds when `kinds` is empty), newest first.
pub fn events(
    src: &LiveSource<'_>,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    kinds: &[&str],
    limit: usize,
) -> Vec<Event> {
    let keeper = Keeper::new(src.app, scope);
    let keep = |e: &Event| {
        (kinds.is_empty() || kinds.contains(&e.kind.as_str()))
            && (e.entity_kind == EntityKind::Global
                || keeper.keeps(&SeriesKey::new("", e.entity_kind, &e.entity)))
    };
    src.m.store.events(from_ms, to_ms, &keep, limit)
}

/// Model calls by agents in the scope, before `before_ms`, newest first.
pub fn calls(src: &LiveSource<'_>, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
    let keeper = Keeper::new(src.app, scope);
    let keep = |r: &CallRecord| keeper.keeps(&call_key(r));
    src.m
        .store
        .calls(before_ms, limit, &keep)
        .iter()
        .map(|r| call_row(src.app, r))
        .collect()
}

/// The agent-keyed series a call belongs to, labelled with its provider and model so that provider and model
/// scopes apply to it.
fn call_key(r: &CallRecord) -> SeriesKey {
    let mut key = SeriesKey::new("", EntityKind::Agent, &r.cid.to_string());
    key.labels = vec![
        ("model".to_string(), r.model.clone()),
        ("provider".to_string(), r.provider.clone()),
    ];
    key
}

fn call_row(app: &App, r: &CallRecord) -> CallRow {
    CallRow {
        at_ms: r.at_ms,
        cid: r.cid,
        agent: scope::agent(app, r.cid).map_or_else(|| r.role.clone(), |agent| agent.name),
        provider: r.provider.clone(),
        model: r.model.clone(),
        tokens_in: r.tokens_in,
        cache_read: r.cache_read,
        tokens_out: r.tokens_out,
        ttft_ms: r.ttft_ms,
        duration_ms: r.duration_ms,
        cost_usd: r.cost_usd,
        status: r.status,
        trace: r.trace.clone(),
    }
}

/// Kept traces of agents in the scope, in the time range, newest first.
pub fn traces(
    src: &LiveSource<'_>,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    limit: usize,
) -> Vec<Trace> {
    let keeper = Keeper::new(src.app, scope);
    let keep = |t: &Trace| keeper.keeps(&SeriesKey::new("", EntityKind::Agent, &t.cid.to_string()));
    src.m.store.traces(from_ms, to_ms, &keep, limit)
}

/// The "Metrics cost" rows: what the metrics system itself uses, against its budget.
pub fn self_cost(src: &LiveSource<'_>) -> SelfCost {
    let cap_mb = f64::from(src.m.settings.get().retention.traces_cap_mb);
    let nodes = node_names(src);
    let payloads: Vec<f64> = nodes
        .iter()
        .filter_map(|node| latest_value(src, "link.payload_bytes", EntityKind::Node, node))
        .collect();
    let payload_kb = if payloads.is_empty() {
        0.0
    } else {
        payloads.iter().sum::<f64>() / payloads.len() as f64 / 1000.0
    };

    let mut rows = vec![
        (
            "hub ingest + rollups CPU".to_string(),
            latest_or_zero(src, "self.cpu_pct", EntityKind::Service, HUB_SERVICE),
            1.0,
            "% of a core".to_string(),
        ),
        (
            "hub metrics memory".to_string(),
            latest_or_zero(src, "self.mem_mb", EntityKind::Service, HUB_SERVICE),
            64.0,
            "MB".to_string(),
        ),
        (
            "heartbeat payload".to_string(),
            payload_kb,
            4.0,
            "KB avg".to_string(),
        ),
        (
            "alert rule evaluation".to_string(),
            latest_or_zero(src, "self.alert_eval_ms", EntityKind::Hub, HUB),
            200.0,
            "ms per check".to_string(),
        ),
        (
            "kept traces".to_string(),
            latest_or_zero(src, "self.traces_bytes", EntityKind::Hub, HUB) / 1e6,
            cap_mb,
            "MB of cap".to_string(),
        ),
    ];
    for node in &nodes {
        rows.push((
            format!("noded sampler CPU ({node})"),
            latest_or_zero(
                src,
                "self.cpu_pct",
                EntityKind::Service,
                &format!("{node}:sampler"),
            ),
            0.5,
            "% of a core".to_string(),
        ));
    }
    SelfCost { rows }
}

/// Each budget the owner set, with the spend today (UTC) in its scope.
pub fn budgets(src: &LiveSource<'_>) -> Vec<Budget> {
    let day_start = utc_day_start(src.now_ms);
    src.m
        .settings
        .get()
        .budgets
        .iter()
        .map(|cap| {
            let today_usd = super::source::reads::scalar(
                src,
                "llm.spend_usd",
                &cap.scope,
                day_start,
                src.now_ms,
                AggFn::Sum,
            )
            .unwrap_or(0.0);
            Budget {
                label: budget_label(&cap.scope, agent_name(src.app, &cap.scope).as_deref()),
                scope: cap.scope.clone(),
                cap_usd: cap.cap_usd,
                today_usd,
            }
        })
        .collect()
}

/// The providers the metrics system has rate-limit readings for, with the requests and tokens left (in percent).
pub fn rate_limits(src: &LiveSource<'_>) -> Vec<(String, Option<f64>, Option<f64>)> {
    let providers: BTreeSet<String> = super::lock(&src.m.latest)
        .keys()
        .filter(|key| {
            key.kind == EntityKind::Provider
                && matches!(
                    key.metric.as_str(),
                    "llm.ratelimit_requests_left_pct" | "llm.ratelimit_tokens_left_pct"
                )
        })
        .map(|key| key.entity.clone())
        .collect();
    providers
        .into_iter()
        .map(|provider| {
            let requests = latest_value(
                src,
                "llm.ratelimit_requests_left_pct",
                EntityKind::Provider,
                &provider,
            );
            let tokens = latest_value(
                src,
                "llm.ratelimit_tokens_left_pct",
                EntityKind::Provider,
                &provider,
            );
            (provider, requests, tokens)
        })
        .collect()
}

/// The outside services the hub called in the current minute: (service, no errors in that window, p95 latency,
/// last error text). The last error text is not kept per window yet, so it is empty.
pub fn outside(src: &LiveSource<'_>) -> Vec<(String, bool, Option<f64>, String)> {
    let rollup = super::lock(&src.m.rollup);
    let mut out: Vec<(String, bool, Option<f64>, String)> = rollup
        .open_at(Tier::Min1)
        .filter(|(key, _, _)| key.metric == "ext.latency_ms" && key.kind == EntityKind::Service)
        .map(|(key, _, agg)| {
            let errors = rollup
                .current(
                    &SeriesKey::new("ext.errors", EntityKind::Service, &key.entity),
                    Tier::Min1,
                )
                .map_or(0.0, |(_, window)| window.sum);
            let p95 = agg.hist.as_ref().and_then(|hist| hist.quantile(0.95));
            (key.entity.clone(), errors == 0.0, p95, String::new())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Each node's link: (node, label, ms since its last heartbeat, p95 heartbeat lag, round trip ms, commands queued,
/// clock offset ms, latest payload bytes).
pub fn links(src: &LiveSource<'_>) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
    let mut found: Vec<(String, u64, f64, f64)> = super::lock(&src.m.links)
        .iter()
        .map(|(node, link)| (node.clone(), link.last_beat_ms, link.offset_ms, link.rtt_ms))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));

    let with_latest: Vec<(String, u64, f64, f64, u32, f64)> = found
        .into_iter()
        .map(|(node, last_beat_ms, offset_ms, rtt_ms)| {
            let rtt = latest_value(src, "link.rtt_ms", EntityKind::Node, &node).unwrap_or(rtt_ms);
            let queued = latest_value(src, "link.commands_queued", EntityKind::Node, &node)
                .map_or(0, |value| value as u32);
            let payload =
                latest_value(src, "link.payload_bytes", EntityKind::Node, &node).unwrap_or(0.0);
            (node, last_beat_ms, offset_ms, rtt, queued, payload)
        })
        .collect();

    let rollup = super::lock(&src.m.rollup);
    with_latest
        .into_iter()
        .map(|(node, last_beat_ms, offset_ms, rtt, queued, payload)| {
            let lag_p95 = rollup
                .current(
                    &SeriesKey::new("link.beat_lag_ms", EntityKind::Node, &node),
                    Tier::Min1,
                )
                .and_then(|(_, window)| window.hist.as_ref())
                .and_then(|hist| hist.quantile(0.95))
                .unwrap_or(0.0);
            let label = scope::label(src.app, EntityKind::Node, &node);
            let since_beat_ms = src.now_ms.saturating_sub(last_beat_ms);
            (
                node,
                label,
                since_beat_ms,
                lag_p95,
                rtt,
                queued,
                offset_ms,
                payload,
            )
        })
        .collect()
}

/// The latest raw point of a series, if the hub has one.
fn latest_value(src: &LiveSource<'_>, metric: &str, kind: EntityKind, entity: &str) -> Option<f64> {
    super::lock(&src.m.latest)
        .get(&SeriesKey::new(metric, kind, entity))
        .map(|(_, value)| *value)
}

fn latest_or_zero(src: &LiveSource<'_>, metric: &str, kind: EntityKind, entity: &str) -> f64 {
    latest_value(src, metric, kind, entity).unwrap_or(0.0)
}

/// The names of the nodes that have a link, sorted.
fn node_names(src: &LiveSource<'_>) -> Vec<String> {
    let mut names: Vec<String> = super::lock(&src.m.links).keys().cloned().collect();
    names.sort();
    names
}

/// The name of the agent a budget scope (`agent:<cid>`) is about, if the roster still has it.
fn agent_name(app: &App, scope: &str) -> Option<String> {
    let cid: u64 = scope.strip_prefix("agent:")?.parse().ok()?;
    scope::agent(app, cid).map(|agent| agent.name)
}

/// The label of a budget: "builder (role)", "ada (agent)" or "plaza". `agent_name` is the name of the agent when
/// the scope is one agent; without it the agent's cid stands in.
fn budget_label(scope: &str, agent_name: Option<&str>) -> String {
    if scope == GLOBAL_SCOPE {
        return "plaza".to_string();
    }
    match scope.split_once(':') {
        Some(("role", role)) => format!("{role} (role)"),
        Some(("agent", cid)) => format!("{} (agent)", agent_name.unwrap_or(cid)),
        _ => scope.to_string(),
    }
}

/// The first millisecond of the UTC day that `now_ms` falls in.
fn utc_day_start(now_ms: u64) -> u64 {
    now_ms - now_ms % DAY_MS
}

/// The prices of `models` and of the models the settings name, with where each is used (the default, the
/// orchestrator, then each role, as the consoles list them).
pub fn prices(src: &LiveSource<'_>, models: &[String]) -> Prices {
    use crate::prices::Source as From;
    let settings = src.app.settings.get();
    let mut uses: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |model: &str, label: String| {
        if model.is_empty() {
            return;
        }
        match uses.iter_mut().find(|(m, _)| m == model) {
            Some((_, labels)) => labels.push(label),
            None => uses.push((model.to_string(), vec![label])),
        }
    };
    add(&settings.default_model, "default".into());
    add(&settings.orchestrator_model, "orchestrator".into());
    for (role, model) in &settings.role_models {
        add(model, format!("role {role}"));
    }
    let mut names: Vec<String> = models.to_vec();
    for (model, _) in &uses {
        if !names.contains(model) {
            names.push(model.clone());
        }
    }
    let rows = names
        .into_iter()
        .map(|model| {
            let used_by = uses
                .iter()
                .find(|(m, _)| *m == model)
                .map(|(_, labels)| labels.join(", "))
                .unwrap_or_default();
            match src.app.prices.lookup(&model, &settings.prices) {
                Some(priced) => PriceRow {
                    source: match priced.source {
                        From::Manual => "manual",
                        From::Vendor => "provider list",
                        From::Openrouter => "OpenRouter",
                        From::Litellm => "LiteLLM",
                    }
                    .into(),
                    input: Some(priced.price.input),
                    output: Some(priced.price.output),
                    cache_read: priced.price.cache_read,
                    cache_write: priced.price.cache_write,
                    used_by,
                    model,
                },
                None => PriceRow {
                    model,
                    used_by,
                    ..PriceRow::default()
                },
            }
        })
        .collect();
    let (fetched_at_ms, sources) = src.app.prices.fetched();
    Prices {
        fetched_at_ms,
        sources: sources
            .iter()
            .filter(|s| s.ok)
            .map(|s| match s.name.as_str() {
                "openrouter" => "OpenRouter".to_string(),
                "litellm" => "LiteLLM".to_string(),
                other => other.to_string(),
            })
            .collect(),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_budget_is_labelled_by_its_role_and_the_plaza_budget_as_plaza() {
        assert_eq!(budget_label("role:builder", None), "builder (role)");
        assert_eq!(budget_label("agent:4417", Some("ada")), "ada (agent)");
        assert_eq!(budget_label("agent:4417", None), "4417 (agent)");
        assert_eq!(budget_label("global", None), "plaza");
    }

    #[test]
    fn a_day_budget_starts_again_at_utc_midnight() {
        assert_eq!(utc_day_start(3 * DAY_MS), 3 * DAY_MS);
        assert_eq!(utc_day_start(3 * DAY_MS + 5_000), 3 * DAY_MS);
        assert_eq!(utc_day_start(4 * DAY_MS - 1), 3 * DAY_MS);
    }
}
