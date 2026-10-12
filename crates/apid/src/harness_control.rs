//! Harness controls: what the orchestrator and the mayor (and the owner) can do to agents'
//! running work and routines — compress a long conversation, steer it with a note, give it
//! more turns, change an agent's limits, and run loops (a prompt sent to an agent on a
//! schedule until it says it is done or a run count is reached).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::Agent;
use crate::state::App;

/// The host tools this module serves (offered to the orchestrator tier and the mayor roles).
pub const TOOLS: &[&str] = &["plaza_agent_control", "plaza_loop"];

/// Compress a conversation past this many serialized bytes (about 60k tokens).
pub const DEFAULT_COMPRESS_AT: u64 = 240_000;
/// Turns kept verbatim when compressing.
pub const DEFAULT_KEEP_RECENT: u32 = 6;
/// What a looped prompt's result must contain for the loop to end itself.
pub const LOOP_DONE: &str = "LOOP DONE";

/// Who may use the controls: the orchestrator tier, the mayor and the mayor's office.
pub fn may_control(role: &str) -> bool {
    crate::settings::tier_of(role) == crate::settings::Tier::Orchestrator || role == "mayor" || role == "mayor-office"
}

/// Per-agent limits that override the plaza's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub compress_at: Option<u64>,
    #[serde(default)]
    pub keep_recent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loop {
    /// `l<seq>`.
    pub id: String,
    pub cid: u64,
    pub prompt: String,
    pub every_secs: u64,
    /// 0 = no limit (until the agent answers LOOP_DONE or someone stops it).
    pub max_runs: u32,
    pub runs: u32,
    pub next_at_ms: u64,
    /// "running" | "done" | "stopped".
    pub state: String,
    /// Who started it (name) and their cid (0 = owner).
    pub by: String,
    pub by_cid: u64,
    pub created_ms: u64,
    /// The last run's work id, and its summary when it ended.
    #[serde(default)]
    pub last_work: Option<u64>,
    #[serde(default)]
    pub last_summary: Option<String>,
}

/// Loops and per-agent limits, kept in `<state>/harness.json`.
pub struct Store {
    path: PathBuf,
    inner: Mutex<(Vec<Loop>, std::collections::BTreeMap<u64, Limits>)>,
}

impl Store {
    pub fn load(state_dir: &Path) -> Self {
        // contract: H2
        Self { path: state_dir.join("harness.json"), inner: Mutex::new((Vec::new(), Default::default())) }
    }
    pub fn loops(&self) -> Vec<Loop> {
        let _ = &self.path;
        self.inner.lock().map(|i| i.0.clone()).unwrap_or_default() // contract: H2
    }
    pub fn limits(&self, _cid: u64) -> Limits {
        Limits::default() // contract: H2
    }
    pub fn set_limits(&self, _cid: u64, _limits: Limits) -> Result<Limits, String> {
        Err("harness controls are not available yet".into()) // contract: H2
    }
    pub fn start_loop(&self, _cid: u64, _prompt: &str, _every_secs: u64, _max_runs: u32, _by: &str, _by_cid: u64) -> Result<Loop, String> {
        Err("harness controls are not available yet".into()) // contract: H2
    }
    pub fn stop_loop(&self, _id: &str) -> Result<Loop, String> {
        Err("harness controls are not available yet".into()) // contract: H2
    }
}

/// Called once a second by apid's ticker: sends each due loop's prompt to its agent.
pub fn tick(_app: &App) {
    // contract: H2
}

/// Called when an agent's work ends: a loop whose last run this was records the summary,
/// and ends itself when the summary contains LOOP_DONE or max_runs is reached.
pub fn work_ended(_app: &App, _cid: u64, _work: u64, _summary: &str) {
    // contract: H2
}

/// Sends a control to the agent's running work (Err when it has none).
pub fn control(_app: &App, _cid: u64, _control: apz_proto::wire::Control) -> Result<u64, String> {
    Err("harness controls are not available yet".into()) // contract: H2
}

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_agent_control" => (
            "Manage an agent's harness (orchestrator and mayor): compress its running work's conversation, steer it with a note, give it more turns, or set its limits (max_turns per work item, compress_at bytes, keep_recent turns).",
            obj(json!({
                "cid": {"type": "integer"},
                "action": {"type": "string", "enum": ["compress", "steer", "extend_turns", "set_limits", "show"]},
                "text": {"type": "string"},
                "turns": {"type": "integer"},
                "max_turns": {"type": "integer"},
                "compress_at": {"type": "integer"},
                "keep_recent": {"type": "integer"}
            }), &["cid", "action"]),
        ),
        "plaza_loop" => (
            "Run a prompt on an agent repeatedly (orchestrator and mayor): start (cid, prompt, every_minutes, max_runs — 0 runs until the agent's answer contains \"LOOP DONE\"), stop (id), or list.",
            obj(json!({
                "action": {"type": "string", "enum": ["start", "stop", "list"]},
                "cid": {"type": "integer"},
                "prompt": {"type": "string"},
                "every_minutes": {"type": "integer"},
                "max_runs": {"type": "integer"},
                "id": {"type": "string"}
            }), &["action"]),
        ),
        _ => return None,
    })
}

pub fn run(_app: &App, _agent: &Agent, _name: &str, _input: &Value) -> Result<String, String> {
    Err("harness controls are not available yet".into()) // contract: H2
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/loops", |app, _, _| Response::json(200, &app.harness.loops()))
        .post("/v1/loops", start_route)
        .delete("/v1/loops/{id}", stop_route)
        .post("/v1/agents/{cid}/control", control_route)
        .get("/v1/agents/{cid}/limits", |app, _, p| Response::json(200, &app.harness.limits(p.get("cid").parse().unwrap_or(0))))
}

fn start_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "harness controls are not available yet") // contract: H2 — body {cid, prompt, every_minutes, max_runs}
}

fn stop_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "harness controls are not available yet") // contract: H2
}

/// Body: the same as plaza_agent_control's input minus `cid`; the owner acts.
fn control_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "harness controls are not available yet") // contract: H2
}
