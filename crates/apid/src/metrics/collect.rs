//! The hub's own collectors and the hooks the rest of the hub calls. Every hook is a few atomic operations on a
//! handle; the tick thread does the rest.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_metrics::trace::{Span, SpanKind};
use apz_metrics::wire::Batch;
use apz_metrics::{Counter, EntityKind, Hist, Level, SeriesKey};

use super::store::CallRecord;
use super::{Metrics, export, ingest, live, scope};
use crate::state::App;

/// The entity of the metrics system's own `self.*` series.
const SELF_ENTITY: &str = "metrics";
/// The entity of the hub-wide `hub.*` series.
const HUB_ENTITY: &str = "hub";
/// The estimated memory one series costs, for `self.mem_mb`.
const BYTES_PER_SERIES: f64 = 200.0;
/// The metrics system may use this much of one CPU, in percent, before the governor steps down.
const CPU_BUDGET_PCT: f64 = 1.0;
const HOUR_MS: u64 = 3_600_000;

/// The handles of each route family: (latency, requests, errors).
static ROUTES: LazyLock<Mutex<HashMap<String, (Hist, Counter, Counter)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// When the hourly retention pass last ran (ms); zero before the first.
static LAST_HOURLY: Mutex<u64> = Mutex::new(0);

/// Each agent's current state and when it began (ms).
static AGENT_STATES: LazyLock<Mutex<HashMap<u64, (String, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Agents that no longer exist, whose series the registry releases after the next scrape (so their last values,
/// the booked state time among them, are scraped first).
static FORGET: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Starts the tick thread (every [`super::SCRAPE_MS`], or [`super::LIVE_SCRAPE_MS`] while a lease is active):
/// scrape, ingest, alert checks, live frames, governor, store flush; and once an hour: retention, store sizes.
pub fn spawn(app: Arc<App>) {
    crate::events::instrument_bus(&app);
    let observed = Arc::clone(&app);
    crate::hub_mcp::observe_calls(Box::new(move |tool, ms, ok| {
        hub_tool_call(&observed, tool, ms, ok)
    }));
    std::thread::Builder::new()
        .name("metrics".into())
        .spawn(move || {
            loop {
                let now = wall_ms();
                tick(&app.metrics, &app, now);
                let interval = if app.metrics.live.active(now).is_empty() {
                    super::SCRAPE_MS
                } else {
                    super::LIVE_SCRAPE_MS
                };
                std::thread::sleep(Duration::from_millis(interval));
            }
        })
        .expect("the metrics thread starts");
}

/// One tick (see [`spawn`]).
pub fn tick(m: &Metrics, app: &App, now_ms: u64) {
    let registry_size = m.registry.len() as f64;
    m.registry
        .gauge(SeriesKey::new(
            "self.series",
            EntityKind::Service,
            SELF_ENTITY,
        ))
        .set(registry_size);
    m.registry
        .gauge(SeriesKey::new(
            "self.mem_mb",
            EntityKind::Service,
            SELF_ENTITY,
        ))
        .set(registry_size * BYTES_PER_SERIES / 1e6);
    // The wake queue by lane, as the hub's own Prometheus page counts it.
    let (interactive, background) = crate::sleep::queued_by_lane();
    for (lane, waiting) in [("interactive", interactive), ("background", background)] {
        m.registry
            .gauge(
                SeriesKey::new("hub.wake_queue", EntityKind::Hub, HUB_ENTITY).label("lane", lane),
            )
            .set(waiting as f64);
    }

    let samples = m.registry.scrape(now_ms, false);
    ingest::samples(m, &samples, now_ms);
    // The agents deleted before this scrape: their series are scraped, so the registry lets them go.
    for cid in std::mem::take(&mut *super::lock(&FORGET)) {
        m.registry.forget(EntityKind::Agent, &cid.to_string());
    }

    if let Ok((_, body)) = app.noded.get("/v1/metrics/batch")
        && let Ok(batch) = serde_json::from_value::<Batch>(body)
    {
        ingest::batch(m, "n0", batch, now_ms);
    }

    let closed = super::lock(&m.rollup).flush_due(now_ms);
    if !closed.is_empty() {
        m.store.put(&closed);
    }

    for transition in m.alerts.check(m, app, now_ms) {
        app.bus.publish("metrics.alert", &transition);
    }

    live::frames(m, app, now_ms);
    prune_agent_states(m, app, now_ms);
    m.store.flush();
    govern(m, now_ms);
    hourly(m, now_ms);
}

/// Drops the state of each agent that no longer exists, booking the time it spent in that state up to now to
/// `agent.state_ms`, as a transition would. A live agent keeps its entry: its time is booked when it changes.
fn prune_agent_states(m: &Metrics, app: &App, now_ms: u64) {
    let gone = drop_gone_agent_states(&mut super::lock(&AGENT_STATES), now_ms, |cid| {
        app.agents.get(cid).is_some()
    });
    for (cid, state, ms) in gone {
        m.registry
            .counter(
                SeriesKey::new("agent.state_ms", EntityKind::Agent, &cid.to_string())
                    .label("state", &state),
            )
            .add(ms as f64);
        super::lock(&FORGET).push(cid);
        crate::steps::forget(cid);
    }
}

/// Removes the entries of the agents `is_live` does not know, returning (agent, state, ms spent in it so far).
fn drop_gone_agent_states(
    states: &mut HashMap<u64, (String, u64)>,
    now_ms: u64,
    is_live: impl Fn(u64) -> bool,
) -> Vec<(u64, String, u64)> {
    let mut booked = Vec::new();
    states.retain(|cid, (state, since)| {
        if is_live(*cid) {
            return true;
        }
        booked.push((*cid, state.clone(), now_ms.saturating_sub(*since)));
        false
    });
    booked
}

/// Drops the latest point of each series that has not reported for an hour: the series of a VM stopped or an
/// agent deleted would otherwise stay in the map for as long as the hub runs.
fn drop_stale_latest(latest: &mut HashMap<SeriesKey, (u64, f64)>, now_ms: u64) {
    latest.retain(|_, (t_ms, _)| now_ms.saturating_sub(*t_ms) < HOUR_MS);
}

/// Feeds the governor whether the metrics system is over its CPU budget, and applies a changed level to the
/// trace sampler (every trace is kept at Full).
fn govern(m: &Metrics, now_ms: u64) {
    let cpu_pct = latest_value(m, "self.cpu_pct").unwrap_or(0.0);
    let changed = super::lock(&m.governor).observe(cpu_pct > CPU_BUDGET_PCT, now_ms);
    if let Some(level) = changed {
        m.traces.keep_all(level == Level::Full);
    }
}

/// Runs the retention pass and records store sizes, at most once an hour.
fn hourly(m: &Metrics, now_ms: u64) {
    {
        let mut last = super::lock(&LAST_HOURLY);
        if now_ms.saturating_sub(*last) < HOUR_MS {
            return;
        }
        *last = now_ms;
    }
    drop_stale_latest(&mut super::lock(&m.latest), now_ms);
    m.store.prune(now_ms, &m.settings.get().retention);
    for (name, bytes) in m.store.sizes() {
        let entity = format!("store:sqlite · {name}");
        m.registry
            .gauge(SeriesKey::new("store.bytes", EntityKind::Service, &entity))
            .set(bytes as f64);
    }
}

/// The most recent value of a series with this metric name, whatever its entity.
fn latest_value(m: &Metrics, metric: &str) -> Option<f64> {
    super::lock(&m.latest)
        .iter()
        .find(|(key, _)| key.metric == metric)
        .map(|(_, (_, value))| *value)
}

/// The route family of a request path, for `hub.http_*`: ids and numbers become `{id}`, and only the first four
/// segments are kept (`/v1/agents/4417/transcript` → `/v1/agents/{id}/transcript`); the method is prefixed
/// (`GET /v1/agents/{id}/transcript`). The console's files are one family (their names change with every build),
/// and so is every request no route matched (`status` 404), so neither grows the series without bound.
pub fn route_family(method: &str, path: &str, status: u16) -> String {
    let path = path.split('?').next().unwrap_or("");
    if status == 404 {
        return format!("{method} unmatched");
    }
    let api = ["/v1/", "/internal/", "/mcp", "/metrics"]
        .iter()
        .any(|prefix| path.starts_with(prefix));
    if !api {
        return format!("{method} console files");
    }
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .take(4)
        .map(|segment| if is_id(segment) { "{id}" } else { segment })
        .collect();
    format!("{method} /{}", segments.join("/"))
}

/// Whether a path segment is an identifier: a number (which starts with a digit), or a hex string of at least
/// 16 characters, dashes allowed (a uuid).
fn is_id(segment: &str) -> bool {
    let uuid_like =
        segment.len() >= 16 && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    segment.starts_with(|c: char| c.is_ascii_digit()) || uuid_like
}

/// Records one API request (called around the router in `state.rs`).
pub fn request(app: &App, method: &str, path: &str, status: u16, ms: f64) {
    if path.starts_with("/v1/ws") || path.starts_with("/v1/metrics/live") {
        return;
    }
    let family = route_family(method, path, status);
    let registry = &app.metrics.registry;
    let mut routes = super::lock(&ROUTES);
    let (latency, requests, errors) = routes.entry(family.clone()).or_insert_with(|| {
        (
            registry.hist(SeriesKey::new("hub.http_ms", EntityKind::Route, &family)),
            registry.counter(SeriesKey::new(
                "hub.http_requests",
                EntityKind::Route,
                &family,
            )),
            registry.counter(SeriesKey::new(
                "hub.http_errors",
                EntityKind::Route,
                &family,
            )),
        )
    });
    latency.record(ms);
    requests.inc();
    if status >= 500 {
        errors.inc();
    }
}

/// What the hub knows of a finished model call.
#[derive(Clone, Debug, Default)]
pub struct ModelCall {
    pub cid: u64,
    pub provider: String,
    pub model: String,
    /// plaza harness, ACP engine, hub MCP.
    pub engine: String,
    pub tokens_in: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub ttft_ms: Option<f64>,
    pub duration_ms: Option<f64>,
    pub status: u16,
    pub retries: u32,
    pub task: Option<String>,
    /// The estimated context split: (part, tokens).
    pub context: Vec<(String, u64)>,
    /// Requests and tokens left in the provider's rate-limit window, in percent, from the response headers.
    pub ratelimit: (Option<f64>, Option<f64>),
}

/// Records a model call: the `llm.*` counters and histograms, `ctx.*` gauges when a split is given, the call row,
/// and a Model span on the agent's running trace.
pub fn model_call(app: &App, c: &ModelCall) {
    let m = &app.metrics;
    let registry = &m.registry;
    let now = wall_ms();
    let info = scope::agent(app, c.cid);
    let call_key = |metric: &str| {
        SeriesKey::new(metric, EntityKind::Agent, &c.cid.to_string())
            .label("provider", &c.provider)
            .label("model", &c.model)
    };
    let agent_key = |metric: &str| SeriesKey::new(metric, EntityKind::Agent, &c.cid.to_string());

    registry.counter(call_key("llm.calls")).inc();
    if c.status >= 400 {
        registry.counter(call_key("llm.errors")).inc();
    }
    registry
        .counter(call_key("llm.retries"))
        .add(f64::from(c.retries));
    if let Some(ttft) = c.ttft_ms {
        registry.hist(call_key("llm.ttft_ms")).record(ttft);
    }
    if let Some(duration) = c.duration_ms {
        registry.hist(call_key("llm.duration_ms")).record(duration);
    }
    if let Some(speed) = c
        .ttft_ms
        .zip(c.duration_ms)
        .and_then(|(ttft, duration)| output_speed(c.tokens_out, duration, ttft))
    {
        registry.hist(call_key("llm.out_tps")).record(speed);
    }
    registry
        .counter(call_key("llm.tokens_in"))
        .add(c.tokens_in as f64);
    registry
        .counter(call_key("llm.tokens_cache_read"))
        .add(c.cache_read as f64);
    registry
        .counter(call_key("llm.tokens_cache_write"))
        .add(c.cache_write as f64);
    registry
        .counter(call_key("llm.tokens_out"))
        .add(c.tokens_out as f64);
    registry.counter(call_key("llm.spend_usd")).add(c.cost_usd);
    registry
        .counter(SeriesKey::new(
            "llm.engine_spend_usd",
            EntityKind::Engine,
            &c.engine,
        ))
        .add(c.cost_usd);

    let (requests_left, tokens_left) = c.ratelimit;
    if let Some(pct) = requests_left {
        registry
            .gauge(SeriesKey::new(
                "llm.ratelimit_requests_left_pct",
                EntityKind::Provider,
                &c.provider,
            ))
            .set(pct);
    }
    if let Some(pct) = tokens_left {
        registry
            .gauge(SeriesKey::new(
                "llm.ratelimit_tokens_left_pct",
                EntityKind::Provider,
                &c.provider,
            ))
            .set(pct);
    }

    if !c.context.is_empty() {
        let mut total = 0u64;
        for (part, tokens) in &c.context {
            total += tokens;
            registry
                .gauge(agent_key("ctx.tokens").label("part", part))
                .set(*tokens as f64);
        }
        if let Some(window) = info.as_ref().and_then(|i| i.window).filter(|w| *w > 0) {
            registry
                .gauge(agent_key("ctx.fill_pct"))
                .set(total as f64 / window as f64 * 100.0);
            registry.gauge(agent_key("ctx.window")).set(window as f64);
        }
    }

    m.store.put_calls(&[CallRecord {
        at_ms: now,
        cid: c.cid,
        role: info.as_ref().map(|i| i.role.clone()).unwrap_or_default(),
        run: info.as_ref().map(|i| i.run.clone()).unwrap_or_default(),
        node: info.as_ref().map(|i| i.node.clone()).unwrap_or_default(),
        provider: c.provider.clone(),
        model: c.model.clone(),
        engine: c.engine.clone(),
        tokens_in: c.tokens_in,
        cache_read: c.cache_read,
        cache_write: c.cache_write,
        tokens_out: c.tokens_out,
        cost_usd: c.cost_usd,
        ttft_ms: c.ttft_ms,
        duration_ms: c.duration_ms,
        status: c.status,
        retries: c.retries,
        task: c.task.clone(),
        context: c.context.clone(),
        trace: None,
    }]);

    let duration = c.duration_ms.unwrap_or(0.0);
    m.traces.span(
        c.cid,
        Span {
            id: m.traces.span_id(),
            parent: None,
            kind: SpanKind::Model,
            name: format!("model call · {}/{}", c.provider, c.model),
            start_ms: now.saturating_sub(duration as u64),
            dur_ms: duration,
            first_token_ms: c.ttft_ms,
            ok: c.status < 400,
            attrs: vec![
                ("model".into(), c.model.clone()),
                ("input tokens".into(), c.tokens_in.to_string()),
                ("cache read".into(), c.cache_read.to_string()),
                ("output tokens".into(), c.tokens_out.to_string()),
                (
                    "first token".into(),
                    c.ttft_ms
                        .map_or("-".to_string(), |ttft| format!("{ttft:.0} ms")),
                ),
                ("cost".into(), format!("${:.4}", c.cost_usd)),
            ],
        },
    );
}

/// Output tokens per second over the time after the first token, or `None` when that time is not positive.
fn output_speed(tokens_out: u64, duration_ms: f64, ttft_ms: f64) -> Option<f64> {
    let generating_ms = duration_ms - ttft_ms;
    (generating_ms > 0.0).then(|| tokens_out as f64 / (generating_ms / 1000.0))
}

/// Records a call to the hub's MCP server from an outside client (a token, not an agent): `tool.*` under `hub.<tool>`,
/// the name agents' calls of the same tools carry.
fn hub_tool_call(app: &App, tool: &str, ms: f64, ok: bool) {
    let registry = &app.metrics.registry;
    let tool = format!("hub.{}", tool.chars().take(64).collect::<String>());
    registry
        .hist(SeriesKey::new("tool.duration_ms", EntityKind::Tool, &tool))
        .record(ms);
    registry
        .counter(SeriesKey::new("tool.calls", EntityKind::Tool, &tool))
        .inc();
    if !ok {
        registry
            .counter(SeriesKey::new("tool.errors", EntityKind::Tool, &tool))
            .inc();
    }
}

/// Records a tool or MCP call: `tool.*` and a Tool (or HubMcp, for `hub.` tools) span.
pub fn tool_call(app: &App, cid: u64, tool: &str, ms: f64, ok: bool) {
    let m = &app.metrics;
    let registry = &m.registry;
    registry
        .hist(SeriesKey::new("tool.duration_ms", EntityKind::Tool, tool))
        .record(ms);
    registry
        .counter(SeriesKey::new("tool.calls", EntityKind::Tool, tool))
        .inc();
    if !ok {
        registry
            .counter(SeriesKey::new("tool.errors", EntityKind::Tool, tool))
            .inc();
        registry
            .counter(SeriesKey::new(
                "tool.errors_by_agent",
                EntityKind::Agent,
                &cid.to_string(),
            ))
            .inc();
    }

    let hub = tool.starts_with("hub.");
    let kind = if hub {
        SpanKind::HubMcp
    } else {
        SpanKind::Tool
    };
    let label = if hub { "hub MCP" } else { "tool" };
    let outcome = if ok { "ok" } else { "error" };
    m.traces.span(
        cid,
        Span {
            id: m.traces.span_id(),
            parent: None,
            kind,
            name: format!("{label} · {tool}"),
            start_ms: wall_ms().saturating_sub(ms as u64),
            dur_ms: ms,
            first_token_ms: None,
            ok,
            attrs: vec![
                ("tool".into(), tool.to_string()),
                ("outcome".into(), outcome.to_string()),
            ],
        },
    );
}

/// An agent's turn ended: `turn.duration_ms`, and the trace is kept or dropped by the tail sampler.
pub fn turn_end(app: &App, cid: u64, ms: f64, ok: bool) {
    let m = &app.metrics;
    m.registry
        .hist(SeriesKey::new(
            "turn.duration_ms",
            EntityKind::Agent,
            &cid.to_string(),
        ))
        .record(ms);
    let role = scope::agent(app, cid)
        .map(|info| info.role)
        .unwrap_or_default();
    if let Some(trace) = m.traces.end(m.store.as_ref(), cid, &role, ok, wall_ms()) {
        export::otlp(m, &trace);
    }
}

/// An agent changed state (`streaming`, `first_token`, `tools`, `approval`, `idle`, `asleep`): the time in the
/// previous state is added to `agent.state_ms`.
pub fn agent_state(app: &App, cid: u64, state: &str) {
    // The same change is the agent's current step, for its view and the `agent_step` event.
    crate::steps::state(app, cid, state);
    let now = wall_ms();
    let previous = super::lock(&AGENT_STATES).insert(cid, (state.to_string(), now));
    if let Some((previous_state, since)) = previous {
        app.metrics
            .registry
            .counter(
                SeriesKey::new("agent.state_ms", EntityKind::Agent, &cid.to_string())
                    .label("state", &previous_state),
            )
            .add(now.saturating_sub(since) as f64);
    }
}

/// The wall clock in milliseconds since the Unix epoch.
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_with_an_id_and_a_query_becomes_its_family() {
        assert_eq!(
            route_family("GET", "/v1/agents/4417/transcript?x=1", 200),
            "GET /v1/agents/{id}/transcript"
        );
    }

    #[test]
    fn console_files_and_unmatched_paths_are_one_family_each() {
        assert_eq!(
            route_family("GET", "/main-3A33JYFC.css", 200),
            "GET console files"
        );
        assert_eq!(route_family("GET", "/", 200), "GET console files");
        assert_eq!(route_family("GET", "/wp-login.php", 404), "GET unmatched");
        assert_eq!(route_family("POST", "/v1/nope/x", 404), "POST unmatched");
        assert_eq!(route_family("GET", "/metrics", 200), "GET /metrics");
    }

    #[test]
    fn output_speed_counts_only_the_time_after_the_first_token() {
        assert_eq!(output_speed(500, 3_000.0, 1_000.0), Some(250.0));
        assert_eq!(output_speed(500, 1_000.0, 1_000.0), None);
    }

    #[test]
    fn a_deleted_agents_state_is_dropped_and_the_time_in_it_booked() {
        let mut states: HashMap<u64, (String, u64)> = HashMap::new();
        states.insert(1, ("streaming".to_string(), 1_000));
        states.insert(2, ("idle".to_string(), 2_000));
        let booked = drop_gone_agent_states(&mut states, 5_000, |cid| cid == 2);
        assert_eq!(booked, vec![(1, "streaming".to_string(), 4_000)]);
        assert_eq!(states.len(), 1);
        assert!(states.contains_key(&2));
    }

    #[test]
    fn a_series_silent_for_an_hour_leaves_the_latest_map() {
        let mut latest: HashMap<SeriesKey, (u64, f64)> = HashMap::new();
        let fresh = SeriesKey::new("vm.blk_ops", EntityKind::Vm, "r1/0");
        let gone = SeriesKey::new("vm.blk_ops", EntityKind::Vm, "r1/9");
        latest.insert(fresh.clone(), (9_000_000, 1.0));
        latest.insert(gone, (1_000, 2.0));
        drop_stale_latest(&mut latest, 9_000_000 + 1_000);
        assert_eq!(latest.len(), 1);
        assert!(latest.contains_key(&fresh));
    }
}
