//! Harness controls: what the orchestrator and the mayor (and the owner) can do to agents'
//! running work and routines — compress a long conversation, steer it with a note, give it
//! more turns, change an agent's limits, and run loops (a prompt sent to an agent on a
//! schedule until it says it is done or a run count is reached).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use apz_proto::wire::Control;
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
pub fn control(app: &App, cid: u64, control: Control) -> Result<u64, String> {
    let item = app.work.current(cid).ok_or_else(|| format!("agent {cid} has no running work"))?;
    let data = json!({ "control": control.clone() });
    let message = apz_proto::wire::HostMessage::Control { work: item.id, control };
    match app.hub.post(&format!("/v1/agents/{cid}/send"), &json!({ "message": message })) {
        Ok((code, _)) if (200..300).contains(&code) => {}
        Ok((code, _)) => return Err(format!("agent {cid} refused the control (HTTP {code})")),
        Err(e) => return Err(format!("agent {cid} is unreachable: {e}")),
    }
    app.transcripts.record(&app.bus, cid, item.id, "control", data);
    Ok(item.id)
}

/// One harness action, shared by the tool (`by` is the calling agent's name) and the owner's
/// route (`by` is "owner"). `input` carries `action` and the action's fields.
fn act(app: &App, cid: u64, input: &Value, by: &str) -> Result<String, String> {
    let agent = app.agents.get(cid).ok_or_else(|| format!("no agent {cid}"))?;
    match input["action"].as_str().unwrap_or("") {
        "compress" => control(app, cid, Control::Compress).map(|w| format!("compression queued for work {w}")),
        "steer" => {
            let text = input["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("steer needs text")?;
            if text.chars().count() > 2000 {
                return Err("steer text is over 2000 characters".into());
            }
            let note = Control::Steer { from: by.to_string(), text: text.to_string() };
            control(app, cid, note).map(|w| format!("steer queued for work {w}"))
        }
        "extend_turns" => {
            let turns = input["turns"].as_u64().filter(|t| (1..=200).contains(t)).ok_or("extend_turns needs turns from 1 to 200")?;
            control(app, cid, Control::ExtendTurns { turns: turns as u32 }).map(|w| format!("work {w} may take {turns} more turns"))
        }
        "set_limits" => {
            let max_turns = input["max_turns"].as_u64().and_then(|v| u32::try_from(v).ok());
            let compress_at = input["compress_at"].as_u64();
            let keep_recent = input["keep_recent"].as_u64().and_then(|v| u32::try_from(v).ok());
            if max_turns.is_none() && compress_at.is_none() && keep_recent.is_none() {
                return Err("set_limits needs max_turns, compress_at or keep_recent".into());
            }
            let mut limits = app.harness.limits(cid);
            if max_turns.is_some() {
                limits.max_turns = max_turns;
            }
            if compress_at.is_some() {
                limits.compress_at = compress_at;
            }
            if keep_recent.is_some() {
                limits.keep_recent = keep_recent;
            }
            let saved = app.harness.set_limits(cid, limits)?;
            crate::agents::push_profile(app, &agent);
            Ok(json!(saved).to_string())
        }
        "show" => {
            let running_work = app.work.current(cid).map(|w| w.id);
            let loops: Vec<Loop> = app.harness.loops().into_iter().filter(|l| l.cid == cid).collect();
            Ok(json!({ "limits": app.harness.limits(cid), "running_work": running_work, "loops": loops }).to_string())
        }
        other => Err(format!("unknown action {other:?}; use compress, steer, extend_turns, set_limits or show")),
    }
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

pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    if !may_control(&agent.role) {
        return Err("only the orchestrator and the mayor manage harnesses".into());
    }
    match name {
        "plaza_agent_control" => {
            let cid = cid_of(input)?;
            if cid == agent.cid {
                return Err("an agent may not control itself".into());
            }
            act(app, cid, input, caller(agent))
        }
        "plaza_loop" => run_loop(app, agent, input),
        other => Err(format!("unknown harness tool {other:?}")),
    }
}

fn run_loop(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    match input["action"].as_str().unwrap_or("") {
        "start" => {
            let cid = cid_of(input)?;
            if cid == agent.cid {
                return Err("an agent may not loop itself".into());
            }
            app.agents.get(cid).ok_or_else(|| format!("no agent {cid}"))?;
            let prompt = input["prompt"].as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or("start needs a prompt")?;
            let minutes = input["every_minutes"].as_u64().filter(|m| *m >= 1).ok_or("every_minutes must be at least 1")?;
            let max_runs = u32::try_from(input["max_runs"].as_u64().unwrap_or(0)).map_err(|_| "max_runs is too large".to_string())?;
            let looped = app.harness.start_loop(cid, prompt, minutes.saturating_mul(60), max_runs, caller(agent), agent.cid)?;
            app.bus.publish("loop", &looped);
            Ok(format!("loop {} started: every {minutes} minutes for agent {cid}", looped.id))
        }
        "stop" => {
            let id = input["id"].as_str().filter(|s| !s.is_empty()).ok_or("stop needs the loop id")?;
            let stopped = app.harness.stop_loop(id)?;
            app.bus.publish("loop", &stopped);
            Ok(format!("loop {} stopped after {} runs", stopped.id, stopped.runs))
        }
        "list" => {
            let lines: Vec<String> = app
                .harness
                .loops()
                .iter()
                .map(|l| json!({ "id": l.id, "cid": l.cid, "state": l.state, "runs": l.runs, "max_runs": l.max_runs, "every_secs": l.every_secs, "prompt": l.prompt }).to_string())
                .collect();
            Ok(if lines.is_empty() { "no loops".to_string() } else { lines.join("\n") })
        }
        other => Err(format!("unknown loop action {other:?}; use start, stop or list")),
    }
}

/// The name a tool call acts under (the role when the agent has no name).
fn caller(agent: &Agent) -> &str {
    if agent.name.is_empty() {
        &agent.role
    } else {
        &agent.name
    }
}

fn cid_of(input: &Value) -> Result<u64, String> {
    input["cid"].as_u64().ok_or_else(|| "cid is required".to_string())
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
fn control_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Ok(cid) = p.get("cid").parse::<u64>() else {
        return Response::error(400, "bad agent id");
    };
    let body: Value = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    match act(app, cid, &body, "owner") {
        Ok(result) => Response::json(200, &json!({ "ok": true, "result": result })),
        Err(e) => Response::error(409, &e),
    }
}
