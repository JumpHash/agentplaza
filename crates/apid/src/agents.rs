//! The roster (agents by CID), the roles they can have, and their profiles.
//!
//! OWNER: slice "agents". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use apz_http::{Params, Request, Response, Router};
use apz_proto::chat::Usage;
use apz_proto::wire::{AgentProfile, HostMessage, WorkKind};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::scheduler::Origin;
use crate::settings::tier_of;
use crate::state::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Offline,
    Idle,
    Working,
}

/// One agent. `cid` is its identity.
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// The operator's per-agent model, if any (else role/tier default).
    pub model_override: Option<String>,
    pub status: Status,
    pub connected_at_ms: u64,
    pub last_seen_ms: u64,
    pub run: Option<String>,
    pub vm: Option<u32>,
    pub usage: Usage,
    pub credits: f64,
}

#[derive(Default)]
pub struct Roster {
    agents: Mutex<BTreeMap<u64, Agent>>,
}

impl Roster {
    pub fn get(&self, cid: u64) -> Option<Agent> {
        // contract
        self.lock().get(&cid).cloned()
    }

    pub fn list(&self) -> Vec<Agent> {
        // contract
        self.lock().values().cloned().collect()
    }

    /// Applies `f` to the agent, if it exists; returns whether it did.
    pub fn update(&self, cid: u64, f: impl FnOnce(&mut Agent)) -> bool {
        // contract
        match self.lock().get_mut(&cid) {
            Some(agent) => {
                f(agent);
                true
            }
            None => false,
        }
    }

    /// Inserts or replaces an agent.
    pub fn upsert(&self, agent: Agent) {
        // contract
        self.lock().insert(agent.cid, agent);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Agent>> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The roles manifest and the role prompts.
pub struct Roles {
    /// role → granted tool names (manifest names, e.g. `bash`, `submit_claim`).
    pub tools: BTreeMap<String, Vec<String>>,
    /// role → prompt text (`<prompts>/<role>/prompt.md`, empty if absent).
    pub prompts: BTreeMap<String, String>,
}

impl Roles {
    pub fn load(manifest: &Path, prompts: &Path) -> std::io::Result<Self> {
        // contract — TODO(agents slice)
        let _ = (manifest, prompts);
        Ok(Self { tools: BTreeMap::new(), prompts: BTreeMap::new() })
    }
}

/// The plaza's view of an agent, for apz-plaza calls.
pub fn connected_agent(agent: &Agent) -> apz_plaza::ConnectedAgent {
    // contract
    apz_plaza::ConnectedAgent {
        cid: agent.cid,
        role: Some(agent.role.clone()),
        run: agent.run.clone().unwrap_or_default(),
        vm: agent.vm.unwrap_or(0),
    }
}

/// The agent's resolved model.
pub fn model_of(app: &App, agent: &Agent) -> String {
    // contract
    app.settings.resolve(&agent.role, agent.model_override.as_deref())
}

/// The agent's profile: name, role, model, system prompt, tools, limits.
pub fn profile(app: &App, agent: &Agent) -> AgentProfile {
    // contract — TODO(agents slice): system prompt = preamble + role prompt
    AgentProfile {
        name: agent.name.clone(),
        role: agent.role.clone(),
        model: model_of(app, agent),
        system: String::new(),
        tools: crate::tools::specs_for(app, &agent.role),
        max_turns: app.settings.get().max_turns,
    }
}

/// A session said hello: register the agent (idle) and return its profile.
pub fn hello(app: &App, cid: u64, role: &str, vm: u32) -> Option<AgentProfile> {
    // contract — TODO(agents slice)
    let _ = (app, cid, role, vm);
    None
}

/// A session ended: mark the agent offline and fail its work.
pub fn gone(app: &App, cid: u64, reason: &str) {
    // contract — TODO(agents slice)
    let _ = (app, cid, reason);
}

/// Every 10 s: reconcile the roster with the hub's sessions and noded's runs.
pub fn spawn_reconcile(app: Arc<App>) {
    // contract — TODO(agents slice)
    let _ = app;
}

/// `/v1/agents…` (roster, spawn, detail, patch, delete, chat, assign, cancel,
/// stop, tell, transcript).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agents", |app, _req, _p| list(app))
        .post("/v1/agents", |app, req, _p| spawn(app, req))
        .get("/v1/agents/{cid}", |app, _req, p| match agent_at(app, p) {
            Ok(agent) => Response::json(200, &view(app, &agent)),
            Err(resp) => resp,
        })
        .route("PATCH", "/v1/agents/{cid}", |app, req, p| patch(app, req, p))
        .delete("/v1/agents/{cid}", |app, _req, p| delete(app, p))
        .post("/v1/agents/{cid}/chat", |app, req, p| chat(app, req, p))
        .post("/v1/agents/{cid}/assign", |app, req, p| assign(app, req, p))
        .post("/v1/agents/{cid}/cancel", |app, _req, p| cancel(app, p))
        .post("/v1/agents/{cid}/stop", |app, req, p| to_hub(app, req, p, "stop"))
        .post("/v1/agents/{cid}/tell", |app, req, p| to_hub(app, req, p, "tell"))
        .get("/v1/agents/{cid}/transcript", |app, req, p| transcript(app, req, p))
}

/// Chat text limit, in characters.
const MAX_CHAT_CHARS: usize = 20_000;

/// Where an agent's model comes from: its own override, else the role's
/// configured model, else the tier default.
pub fn model_source(has_override: bool, has_role_model: bool) -> &'static str {
    if has_override {
        "agent"
    } else if has_role_model {
        "role"
    } else {
        "default"
    }
}

/// Checks a chat turn: 1 to 20,000 characters.
pub fn check_chat_text(text: &str) -> Result<(), String> {
    let n = text.chars().count();
    if n == 0 {
        return Err("text must not be empty".into());
    }
    if n > MAX_CHAT_CHARS {
        return Err(format!("text is {n} characters; the limit is {MAX_CHAT_CHARS}"));
    }
    Ok(())
}

/// The API view of an agent: its fields, resolved model, tier, work, queue.
fn view(app: &App, agent: &Agent) -> Value {
    let has_role_model = app.settings.get().role_models.contains_key(&agent.role);
    let mut v = serde_json::to_value(agent).unwrap_or(Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("model".into(), json!(model_of(app, agent)));
        obj.insert(
            "model_source".into(),
            json!(model_source(agent.model_override.is_some(), has_role_model)),
        );
        obj.insert("tier".into(), json!(tier_of(&agent.role)));
        obj.insert("work".into(), json!(app.work.current(agent.cid)));
        obj.insert("queue".into(), json!(app.work.queued(agent.cid).len()));
    }
    v
}

/// The agent named by `{cid}`, or the 400/404 response.
fn agent_at(app: &App, params: &Params<'_>) -> Result<Agent, Response> {
    let cid = params
        .get("cid")
        .parse::<u64>()
        .map_err(|_| Response::error(400, "cid must be a number"))?;
    app.agents.get(cid).ok_or_else(|| Response::error(404, "no such agent"))
}

/// The request body as `T`, or a 400 response.
fn body<T: DeserializeOwned>(req: &Request) -> Result<T, Response> {
    req.json::<T>().map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

#[derive(Deserialize)]
struct SpawnBody {
    role: String,
    count: Option<u32>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

#[derive(Deserialize)]
struct AssignBody {
    task_id: String,
}

fn list(app: &App) -> Response {
    let views: Vec<Value> = app.agents.list().iter().map(|a| view(app, a)).collect();
    Response::json(200, &views)
}

/// Spawns `count` runs of `role` via noded. `model` is validated against the
/// tier policy but not carried into the spawned agent's `hello`, and an
/// unknown `name` field is ignored: set both with `PATCH` once connected.
fn spawn(app: &App, req: &Request) -> Response {
    let b: SpawnBody = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    if !app.roles.tools.contains_key(&b.role) {
        return Response::error(400, &format!("unknown role {}", b.role));
    }
    let count = b.count.unwrap_or(1);
    if !(1..=16).contains(&count) {
        return Response::error(400, "count must be 1..=16");
    }
    if let Some(model) = &b.model {
        if let Err(e) = app.settings.allowed(tier_of(&b.role), model) {
            return Response::error(400, &e);
        }
    }
    let payload = json!({"n": count, "role": b.role, "vcpus": 1, "memory_mib": 256});
    match app.noded.post("/v1/runs", &payload) {
        Ok((status, value)) => Response::json(status, &value),
        Err(e) => Response::error(502, &format!("noded: {e}")),
    }
}

/// `PATCH`: sets or clears the model (`null`), renames, pushes the new profile
/// to the agent through the hub, and publishes the change.
fn patch(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let b: Value = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let model: Option<Option<String>> = match b.get("model") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(m)) => {
            if let Err(e) = app.settings.allowed(tier_of(&agent.role), m) {
                return Response::error(400, &e);
            }
            Some(Some(m.clone()))
        }
        Some(_) => return Response::error(400, "model must be a string or null"),
    };
    let name = match b.get("name") {
        None => None,
        Some(Value::String(n)) if !n.trim().is_empty() => Some(n.trim().to_string()),
        Some(_) => return Response::error(400, "name must be a non-empty string"),
    };
    app.agents.update(agent.cid, |a| {
        if let Some(m) = model {
            a.model_override = m;
        }
        if let Some(n) = name {
            a.name = n;
        }
    });
    let Some(updated) = app.agents.get(agent.cid) else {
        return Response::error(404, "no such agent");
    };
    // Best effort: the agent's next hello carries the profile anyway.
    let push = json!({"message": HostMessage::Profile { profile: profile(app, &updated) }});
    let _ = app.hub.post(&format!("/v1/agents/{}/send", updated.cid), &push);
    let v = view(app, &updated);
    app.bus.publish("agent", &v);
    Response::json(200, &v)
}

/// `DELETE`: asks the hub to stop the agent (best effort) and deletes its run.
fn delete(app: &App, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let _ = app.hub.post(&format!("/v1/agents/{}/stop", agent.cid), &json!({}));
    let run_status = agent.run.as_ref().and_then(|run| {
        app.noded.call("DELETE", &format!("/v1/runs/{run}"), None).map(|(status, _)| status).ok()
    });
    Response::json(200, &json!({"deleted": true, "run_status": run_status}))
}

/// Queues a chat turn for the agent. 409 when the agent is offline.
fn chat(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    if agent.status == Status::Offline {
        return Response::error(409, "agent is offline");
    }
    let b: TextBody = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    if let Err(e) = check_chat_text(&b.text) {
        return Response::error(400, &e);
    }
    let thread = format!("op-{}", agent.cid);
    let work = app.work.enqueue(app, agent.cid, WorkKind::Chat, thread, b.text, Origin::default());
    Response::json(200, &json!({"work": work}))
}

/// Claims an open plaza task for the agent and queues it as task work.
fn assign(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    if agent.status == Status::Offline {
        return Response::error(409, "agent is offline");
    }
    let b: AssignBody = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let Some(task) = app.plaza.tasks.find(&b.task_id) else {
        return Response::error(404, "no such task");
    };
    if !task.is_open() {
        return Response::error(409, "task is not open");
    }
    if let Err(e) = task.claim(&connected_agent(&agent)) {
        return Response::error(409, &e.to_string());
    }
    let id = task.id().to_string();
    let mut prompt = task.goal().to_string();
    if let Some(acceptance) = task.acceptance() {
        prompt.push_str("\n\nAcceptance: ");
        prompt.push_str(acceptance);
    }
    let origin = Origin { task: Some(id.clone()), ..Origin::default() };
    let work = app.work.enqueue(app, agent.cid, WorkKind::Task, format!("task-{id}"), prompt, origin);
    Response::json(200, &json!({"work": work, "task": id}))
}

/// Cancels the agent's work in flight; `{"cancelled": bool}`.
fn cancel(app: &App, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let cancelled = app.work.cancel(app, agent.cid, "cancelled by the operator");
    Response::json(200, &json!({"cancelled": cancelled}))
}

/// Proxies `POST /v1/agents/{cid}/<action>` to the hub with the same body.
fn to_hub(app: &App, req: &Request, params: &Params<'_>, action: &str) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    app.hub.proxy("POST", &format!("/v1/agents/{}/{action}", agent.cid), &req.body)
}

/// The agent's transcript after entry `after`, at most `limit` (max 1000).
fn transcript(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let after = req.query_param("after").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let limit = req.query_param("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(200);
    Response::json(200, &app.transcripts.after(agent.cid, after, limit.min(1000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_source_prefers_agent_then_role_then_default() {
        assert_eq!(model_source(true, true), "agent");
        assert_eq!(model_source(true, false), "agent");
        assert_eq!(model_source(false, true), "role");
        assert_eq!(model_source(false, false), "default");
    }

    #[test]
    fn chat_text_is_one_to_twenty_thousand_characters() {
        assert!(check_chat_text("").is_err());
        assert!(check_chat_text("hi").is_ok());
        let at_limit: String = "é".repeat(MAX_CHAT_CHARS);
        assert!(check_chat_text(&at_limit).is_ok());
        let over: String = "é".repeat(MAX_CHAT_CHARS + 1);
        assert!(check_chat_text(&over).is_err());
    }
}
