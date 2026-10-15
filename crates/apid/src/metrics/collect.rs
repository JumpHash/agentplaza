//! The hub's own collectors and the hooks the rest of the hub calls. Every hook is a few atomic operations on a
//! handle; the tick thread does the rest.

use std::sync::Arc;

use super::Metrics;
use crate::state::App;

/// Starts the tick thread (every [`super::SCRAPE_MS`], or [`super::LIVE_SCRAPE_MS`] while a lease is active):
/// scrape, ingest, alert checks, live frames, governor, store flush; and once an hour: retention, store sizes.
pub fn spawn(app: Arc<App>) {
    let _ = app;
    todo!()
}

/// One tick (see [`spawn`]).
pub fn tick(m: &Metrics, app: &App, now_ms: u64) {
    let _ = (m, app, now_ms);
    todo!()
}

/// The route family of a request path, for `hub.http_*`: ids and numbers become `{id}`, and only the first four
/// segments are kept (`/v1/agents/4417/transcript` → `/v1/agents/{id}/transcript`); the method is prefixed
/// (`GET /v1/agents/{id}/transcript`).
pub fn route_family(method: &str, path: &str) -> String {
    let _ = (method, path);
    todo!()
}

/// Records one API request (called around the router in `state.rs`).
pub fn request(app: &App, method: &str, path: &str, status: u16, ms: f64) {
    let _ = (app, method, path, status, ms);
    todo!()
}

/// Records one database round trip (called from `db::Db::run`; a global handle, since the database layer has no
/// `App`).
pub fn database(ms: f64) {
    let _ = ms;
    todo!()
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
    let _ = (app, c);
    todo!()
}

/// Records a tool or MCP call: `tool.*` and a Tool (or HubMcp, for `hub.` tools) span.
pub fn tool_call(app: &App, cid: u64, tool: &str, ms: f64, ok: bool) {
    let _ = (app, cid, tool, ms, ok);
    todo!()
}

/// An agent's turn ended: `turn.duration_ms`, and the trace is kept or dropped by the tail sampler.
pub fn turn_end(app: &App, cid: u64, ms: f64, ok: bool) {
    let _ = (app, cid, ms, ok);
    todo!()
}

/// An agent changed state (`streaming`, `first_token`, `tools`, `approval`, `idle`, `asleep`): the time in the
/// previous state is added to `agent.state_ms`.
pub fn agent_state(app: &App, cid: u64, state: &str) {
    let _ = (app, cid, state);
    todo!()
}
