//! The roster (agents by CID), the roles they can have, and their profiles.
//!
//! OWNER: slice "agents". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use apz_http::{Params, Request, Response, Router};
use apz_proto::chat::Usage;
use apz_proto::wire::{AgentProfile, HostMessage, WorkKind};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::role::Role;
use crate::scheduler::Origin;
use crate::settings::{tier_of, Tier};
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

/// Agent names by CID, kept at `<state-dir>/names.json` and written atomically.
pub struct Names {
    path: PathBuf,
    names: Mutex<BTreeMap<u64, String>>,
    /// CIDs already asked to choose a name in this run.
    asked: Mutex<BTreeSet<u64>>,
}

impl Names {
    /// Loads the store; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let names: BTreeMap<u64, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, names: Mutex::new(names), asked: Mutex::new(BTreeSet::new()) }
    }

    pub fn get(&self, cid: u64) -> Option<String> {
        self.guard().get(&cid).cloned()
    }

    /// Whether another agent already uses `name` (case-insensitive).
    pub fn used_by_other(&self, cid: u64, name: &str) -> bool {
        let wanted = name.to_lowercase();
        self.guard().iter().any(|(other, used)| *other != cid && used.to_lowercase() == wanted)
    }

    /// Stores `name` for `cid` and writes the file; on failure nothing changes.
    pub fn set(&self, cid: u64, name: &str) -> std::io::Result<()> {
        let mut names = self.guard();
        let mut next = names.clone();
        next.insert(cid, name.to_owned());
        let text = serde_json::to_string_pretty(&next).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        *names = next;
        Ok(())
    }

    /// True the first time it is called for `cid` in this run.
    pub fn first_ask(&self, cid: u64) -> bool {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).insert(cid)
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, String>> {
        self.names.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Checks a proposed name for agent `cid` and returns it trimmed: 2 to 16
/// characters, letters only, at most one inner space or hyphen, not a role
/// name, and not another agent's name.
pub fn check_name(app: &App, cid: u64, raw: &str) -> Result<String, String> {
    let name = raw.trim().to_owned();
    let len = name.chars().count();
    if !(2..=16).contains(&len) {
        return Err(format!("name must be 2 to 16 characters; it is {len}"));
    }
    let separators = name.chars().filter(|c| *c == ' ' || *c == '-').count();
    let edged = name.starts_with([' ', '-']) || name.ends_with([' ', '-']);
    let letters = name.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '-');
    if separators > 1 || edged || !letters {
        return Err("name must be letters only, with at most one inner space or hyphen".into());
    }
    if Role::parse(&name.to_lowercase().replace(' ', "-")).is_some() {
        return Err(format!("{name} is a role name; choose a personal name"));
    }
    if app.names.used_by_other(cid, &name) {
        return Err(format!("the name {name} is already used by another agent"));
    }
    Ok(name)
}

/// Stores a checked name: the file first, then the roster.
fn store_name(app: &App, cid: u64, name: &str) -> Result<(), String> {
    app.names.set(cid, name).map_err(|e| format!("name not saved: {e}"))?;
    app.agents.update(cid, |agent| agent.name = name.to_owned());
    Ok(())
}

/// Renames agent `cid` (checked), pushes its profile to the guest and publishes it.
pub fn rename(app: &App, cid: u64, raw: &str) -> Result<Agent, String> {
    let name = check_name(app, cid, raw)?;
    store_name(app, cid, &name)?;
    let agent = app.agents.get(cid).ok_or("no such agent")?;
    let push = json!({"message": HostMessage::Profile { profile: profile(app, &agent) }});
    let _ = app.hub.post(&format!("/v1/agents/{cid}/send"), &push);
    app.bus.publish("agent", &agent);
    Ok(agent)
}

/// Pushes every connected agent its current profile (tools, model), e.g. after the
/// operator enabled a connector.
pub fn push_profiles(app: &App) {
    for agent in app.agents.list() {
        if agent.status == Status::Offline {
            continue;
        }
        let push = json!({"message": HostMessage::Profile { profile: profile(app, &agent) }});
        let _ = app.hub.post(&format!("/v1/agents/{}/send", agent.cid), &push);
    }
}

/// The human label of a role, or the raw name when it is not a known role.
pub fn role_label(role: &str) -> &str {
    Role::parse(role).map(Role::label).unwrap_or(role)
}

/// The roles manifest and the role prompts.
pub struct Roles {
    /// role → granted tool names (manifest names, e.g. `bash`, `submit_claim`).
    pub tools: BTreeMap<String, Vec<String>>,
    /// role → prompt text (`<prompts>/<role>/prompt.md`, empty if absent).
    pub prompts: BTreeMap<String, String>,
}

impl Roles {
    /// Reads the manifest's roles and each role's `<prompts>/<role>/prompt.md`.
    /// A missing manifest yields empty roles (with a warning); a malformed one is an error.
    pub fn load(manifest: &Path, prompts: &Path) -> std::io::Result<Self> {
        // contract
        let mut roles = Self { tools: BTreeMap::new(), prompts: BTreeMap::new() };
        let text = match std::fs::read_to_string(manifest) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("apid: roles manifest {} not read ({err}); no roles loaded", manifest.display());
                return Ok(roles);
            }
        };
        let doc: Value = serde_json::from_str(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let Some(entries) = doc["roles"].as_object() else {
            return Ok(roles);
        };
        for (role, entry) in entries {
            let tools = entry["tools"]
                .as_array()
                .map(|list| list.iter().filter_map(|tool| tool.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            roles.tools.insert(role.clone(), tools);
            let prompt = std::fs::read_to_string(prompts.join(role).join("prompt.md")).unwrap_or_default();
            roles.prompts.insert(role.clone(), strip_comment(&prompt));
        }
        Ok(roles)
    }
}

/// Drops a leading one-line `<!-- … -->` comment and the blank lines after it.
fn strip_comment(prompt: &str) -> String {
    let trimmed = prompt.trim_start();
    if trimmed.starts_with("<!--") {
        if let Some(end) = trimmed.find("-->") {
            return trimmed[end + 3..].trim_start().to_owned();
        }
    }
    prompt.to_owned()
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
    // contract
    let role_prompt = app.roles.prompts.get(&agent.role).map_or("", String::as_str);
    AgentProfile {
        name: agent.name.clone(),
        role: agent.role.clone(),
        model: model_of(app, agent),
        system: system_prompt(agent, role_prompt),
        tools: crate::tools::specs_for(app, &agent.role),
        max_turns: app.settings.get().max_turns,
    }
}

/// The system prompt: the plaza preamble, then the role's own prompt.
fn system_prompt(agent: &Agent, role_prompt: &str) -> String {
    let orchestrator = tier_of(&agent.role) == Tier::Orchestrator;
    let label = role_label(&agent.role);
    let who = if agent.name.is_empty() {
        format!("You have no name yet. You are the {label} agent of an AgentPlaza (CID {}).", agent.cid)
    } else {
        format!("You are {}, the {label} agent of an AgentPlaza (CID {}).", agent.name, agent.cid)
    };
    let mut text = format!(
        "{who}\n\n\
         You work in your own virtual machine. Guest tools (bash, file tools) act only on your own \
         /workspace inside that VM. Host tools (names starting with plaza_) act on the shared plaza that \
         every agent uses. Use only the tools you are given.\n\n\
         When a task is finished, call submit_claim, then end your turn with a one-paragraph summary of \
         what you did. Keep answers concise and factual. If you could not do something, say so plainly \
         and say what is missing.\n\n\
         Plaza laws: never seek, copy or reveal credentials, keys or tokens. Never impersonate another \
         agent or the operator. Never act outside the tools you were given. Report honestly; never claim \
         work you did not do.",
    );
    if orchestrator {
        text.push_str(
            "\n\nYou plan and delegate. Break the operator's goal into tasks, each with clear acceptance \
             criteria. Assign each task to the best worker: list workers with plaza_agents, post with \
             plaza_post_task, and hand off with plaza_assign. Follow up with plaza_task_result and report \
             a concise summary to the operator. Never do the workers' tasks yourself.",
        );
    }
    text.push_str("\n\n");
    text.push_str(crate::society::PROMPT);
    let role_prompt = role_prompt.trim();
    if !role_prompt.is_empty() {
        text.push_str("\n\n");
        text.push_str(role_prompt);
    }
    text
}

/// A session said hello: register the agent (idle) and return its profile.
pub fn hello(app: &App, cid: u64, role: &str, vm: u32) -> Option<AgentProfile> {
    // contract
    let now = apz_plaza::now_ms();
    let previous = app.agents.get(cid);
    let registered = app.spawned.hello(cid, role);
    let agent = Agent {
        cid,
        name: app.names.get(cid).unwrap_or_default(),
        role: role.to_owned(),
        model_override: previous.as_ref().and_then(|old| old.model_override.clone()).or(registered),
        status: Status::Idle,
        connected_at_ms: now,
        last_seen_ms: now,
        run: previous.as_ref().and_then(|old| old.run.clone()),
        vm: Some(vm),
        usage: previous.as_ref().map(|old| old.usage.clone()).unwrap_or_default(),
    };
    app.agents.upsert(agent.clone());
    app.bus.publish("agent", &agent);
    Some(profile(app, &agent))
}

/// A session ended: mark the agent offline and fail its work.
pub fn gone(app: &App, cid: u64, reason: &str) {
    // contract
    let now = apz_plaza::now_ms();
    let changed = app.agents.update(cid, |agent| {
        agent.status = Status::Offline;
        agent.last_seen_ms = now;
    });
    if !changed {
        return;
    }
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
    eprintln!("apid: agent {cid} offline: {reason}");
    // The roster lock is released above; the scheduler may take its own locks.
    app.work.agent_gone(app, cid);
}

/// Every 10 s: reconcile the roster with the hub's sessions and noded's runs.
pub fn spawn_reconcile(app: Arc<App>) {
    // contract
    std::thread::spawn(move || loop {
        reconcile_once(&app);
        std::thread::sleep(Duration::from_secs(10));
    });
}

/// One reconcile pass. Hub or noded errors are ignored: either may be down.
fn reconcile_once(app: &App) {
    // Sessions: register unknown or offline sessions, then mark vanished agents offline.
    if let Ok((200, body)) = app.hub.get("/v1/status") {
        let rows = body["agents"].as_array().cloned().unwrap_or_default();
        let live: Vec<u64> = rows.iter().filter_map(|row| row["cid"].as_u64()).collect();
        for row in &rows {
            let (Some(cid), Some(role)) = (row["cid"].as_u64(), row["role"].as_str()) else {
                continue;
            };
            let known_online = app.agents.get(cid).is_some_and(|agent| agent.status != Status::Offline);
            if !known_online {
                hello(app, cid, role, 0);
            }
        }
        for agent in app.agents.list() {
            if agent.status != Status::Offline && !live.contains(&agent.cid) {
                gone(app, agent.cid, "session not on the hub");
            }
        }
    }
    // Runs: point each agent at the run and VM whose guest CID it is.
    if let Ok((200, body)) = app.noded.get("/v1/runs") {
        let mut placements: BTreeMap<u64, (String, u32)> = BTreeMap::new();
        for run in body["runs"].as_array().into_iter().flatten() {
            if !run["ended_at"].is_null() {
                continue;
            }
            let Some(run_id) = run["id"].as_str() else {
                continue;
            };
            for vm in run["vms"].as_array().into_iter().flatten() {
                if let (Some(guest), Some(id)) = (vm["guest_cid"].as_u64(), vm["id"].as_u64()) {
                    placements.insert(guest, (run_id.to_owned(), id as u32));
                }
            }
        }
        for agent in app.agents.list() {
            let Some((run, vm)) = placements.get(&agent.cid).cloned() else {
                continue;
            };
            if agent.run.as_deref() != Some(run.as_str()) || agent.vm != Some(vm) {
                let updated = app.agents.update(agent.cid, |slot| {
                    slot.run = Some(run);
                    slot.vm = Some(vm);
                });
                if updated {
                    if let Some(fresh) = app.agents.get(agent.cid) {
                        app.bus.publish("agent", &fresh);
                    }
                }
            }
        }
    }
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
        obj.insert("role_label".into(), json!(role_label(&agent.role)));
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
/// tier policy and applied at each spawned agent's first `hello`; an unknown
/// `name` field is ignored: set it with `PATCH` once connected.
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
        Ok((status, value)) => {
            if status < 300 {
                for _ in 0..count {
                    app.spawned.expect(&b.role, b.model.clone());
                }
            }
            Response::json(status, &value)
        }
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
        Some(Value::String(n)) => match check_name(app, agent.cid, n) {
            Ok(valid) => Some(valid),
            Err(e) => return Response::error(400, &e),
        },
        Some(_) => return Response::error(400, "name must be a string"),
    };
    if let Some(m) = &model {
        if let Err(e) = app.spawned.set_model(agent.cid, m.as_deref()) {
            eprintln!("apid: model override not saved: {e}");
        }
    }
    if let Some(n) = &name {
        let _ = app.spawned.set_name(agent.cid, n);
    }
    app.agents.update(agent.cid, |a| {
        if let Some(m) = model {
            a.model_override = m;
        }
    });
    if let Some(n) = &name {
        if let Err(e) = store_name(app, agent.cid, n) {
            return Response::error(500, &e);
        }
    }
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
    if let Err(e) = app.spawned.forget(agent.cid) {
        eprintln!("apid: spawned.json not saved: {e}");
    }
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

#[cfg(test)]
mod route_tests {
    use super::*;

    fn agent(role: &str) -> Agent {
        Agent {
            cid: 42,
            name: format!("{role}-42"),
            role: role.to_owned(),
            model_override: None,
            status: Status::Idle,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: Usage::default(),
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir = std::env::temp_dir().join(format!("apid-agents-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roles_load_reads_tools_and_prompts() {
        let dir = scratch("roles");
        let manifest = dir.join("roles.json");
        std::fs::write(
            &manifest,
            r#"{"source":"test","roles":{"builder":{"tools":["bash","submit_claim"]},"reviewer":{"tools":["search"]}}}"#,
        )
        .unwrap();
        let prompts = dir.join("prompts");
        std::fs::create_dir_all(prompts.join("builder")).unwrap();
        std::fs::write(prompts.join("builder").join("prompt.md"), "<!-- internal -->\n\nBuild carefully.\n").unwrap();

        let roles = Roles::load(&manifest, &prompts).unwrap();
        assert_eq!(roles.tools["builder"], vec!["bash", "submit_claim"]);
        assert_eq!(roles.tools["reviewer"], vec!["search"]);
        assert_eq!(roles.prompts["builder"], "Build carefully.\n");
        assert_eq!(roles.prompts["reviewer"], "");

        let missing = Roles::load(&dir.join("absent.json"), &prompts).unwrap();
        assert!(missing.tools.is_empty() && missing.prompts.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn orchestrator_prompt_delegates_and_worker_does_not() {
        let orchestrator = system_prompt(&agent("orchestrator"), "");
        let worker = system_prompt(&agent("builder"), "Write the code.");
        assert!(orchestrator.contains("You plan and delegate"));
        assert!(orchestrator.contains("plaza_assign"));
        assert!(!worker.contains("delegate"));
        assert!(worker.contains("You are builder-42, the Builder agent"));
        assert!(worker.ends_with("Write the code."));
        let unnamed = Agent { name: String::new(), ..agent("qa") };
        let text = system_prompt(&unnamed, "");
        assert!(text.contains("You have no name yet. You are the QA agent"));
    }
}
