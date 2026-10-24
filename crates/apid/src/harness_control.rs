//! Harness controls: what the orchestrator (and the owner) can do to agents'
//! running work and routines — compress a long conversation, steer it with a note, give it
//! more turns, change an agent's limits, and run loops (a prompt sent to an agent on a
//! schedule until it says it is done or a run count is reached).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_proto::wire::{Control, WorkKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::scheduler::Origin;
use crate::state::App;

/// The host tools this module serves (offered to the orchestrator tier).
pub const TOOLS: &[&str] = &["plaza_agent_control", "plaza_loop"];

/// Compress a conversation past this many serialized bytes (about 60k tokens).
pub const DEFAULT_COMPRESS_AT: u64 = 240_000;
/// Turns kept verbatim when compressing.
pub const DEFAULT_KEEP_RECENT: u32 = 6;
/// What a looped prompt's result must contain for the loop to end itself.
pub const LOOP_DONE: &str = "LOOP DONE";
/// The longest interval a loop can have: its run times (in milliseconds) must fit the store's integers.
const MAX_EVERY_SECS: u64 = i64::MAX as u64 / 2_000;

/// Who may use the controls: the orchestrator tier.
pub fn may_control(role: &str) -> bool {
    crate::settings::tier_of(role) == crate::settings::Tier::Orchestrator
}

/// Per-agent limits that override the plaza's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Limits {
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub compress_at: Option<u64>,
    #[serde(default)]
    pub keep_recent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
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

/// The harness's state: its loops (oldest first) and each agent's limits by cid. It is the `harness` table: one row,
/// with the loops and the limits as child tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "harness")]
pub struct Harness {
    #[table(child)]
    pub loops: Vec<Loop>,
    #[table(map)]
    pub limits: BTreeMap<u64, Limits>,
}

/// Loops and per-agent limits, in memory: the hub's saver writes them to the `harness` table.
#[derive(Default)]
pub struct Store {
    inner: Mutex<Harness>,
}

/// Checks a loop's own fields (its prompt and interval, before the agent it runs for is looked at) and returns the
/// trimmed prompt.
pub fn check_loop(prompt: &str, every_secs: u64) -> Result<&str, String> {
    let prompt = prompt.trim();
    let chars = prompt.chars().count();
    if chars == 0 || chars > 4000 {
        return Err("a loop's prompt must be 1 to 4000 characters".into());
    }
    if every_secs < 60 {
        return Err("a loop runs at most once a minute (every_minutes >= 1)".into());
    }
    if every_secs > MAX_EVERY_SECS {
        return Err("every_minutes is too large".into());
    }
    Ok(prompt)
}

impl Store {
    fn locked(&self) -> MutexGuard<'_, Harness> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Newest first.
    pub fn loops(&self) -> Vec<Loop> {
        self.locked().loops.iter().rev().cloned().collect()
    }

    pub fn limits(&self, cid: u64) -> Limits {
        self.locked().limits.get(&cid).cloned().unwrap_or_default()
    }

    /// Validates the given fields, merges them over the agent's stored limits, and returns the merged limits.
    pub fn set_limits(&self, cid: u64, limits: Limits) -> Result<Limits, String> {
        if let Some(v) = limits.max_turns
            && !(1..=500).contains(&v)
        {
            return Err("max_turns must be 1 to 500".into());
        }
        if let Some(v) = limits.compress_at
            && v != 0
            && !(20_000..=4_000_000).contains(&v)
        {
            return Err("compress_at must be 0 (never) or 20000 to 4000000 bytes".into());
        }
        if let Some(v) = limits.keep_recent
            && !(1..=50).contains(&v)
        {
            return Err("keep_recent must be 1 to 50".into());
        }
        let mut guard = self.locked();
        let mut next = guard.limits.get(&cid).cloned().unwrap_or_default();
        if limits.max_turns.is_some() {
            next.max_turns = limits.max_turns;
        }
        if limits.compress_at.is_some() {
            next.compress_at = limits.compress_at;
        }
        if limits.keep_recent.is_some() {
            next.keep_recent = limits.keep_recent;
        }
        guard.limits.insert(cid, next.clone());
        Ok(next)
    }

    pub fn start_loop(
        &self,
        cid: u64,
        prompt: &str,
        every_secs: u64,
        max_runs: u32,
        by: &str,
        by_cid: u64,
    ) -> Result<Loop, String> {
        let prompt = check_loop(prompt, every_secs)?;
        let mut guard = self.locked();
        let running = guard
            .loops
            .iter()
            .filter(|l| l.cid == cid && l.state == "running")
            .count();
        if running >= 20 {
            return Err("this agent already has 20 running loops".into());
        }
        let now = now_ms();
        let lp = Loop {
            id: next_id(&guard.loops),
            cid,
            prompt: prompt.to_string(),
            every_secs,
            max_runs,
            runs: 0,
            next_at_ms: now.saturating_add(every_secs.saturating_mul(1000)),
            state: "running".into(),
            by: by.into(),
            by_cid,
            created_ms: now,
            last_work: None,
            last_summary: None,
        };
        guard.loops.push(lp.clone());
        Ok(lp)
    }

    pub fn stop_loop(&self, id: &str) -> Result<Loop, String> {
        let mut guard = self.locked();
        let index = guard
            .loops
            .iter()
            .position(|l| l.id == id)
            .ok_or_else(|| format!("no loop {id}"))?;
        if guard.loops[index].state != "running" {
            return Ok(guard.loops[index].clone());
        }
        guard.loops[index].state = "stopped".into();
        Ok(guard.loops[index].clone())
    }
}

/// `l<n+1>` where n is the largest existing loop number.
fn next_id(loops: &[Loop]) -> String {
    let max = loops
        .iter()
        .filter_map(|l| l.id.strip_prefix('l')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("l{}", max + 1)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called once a second by apid's ticker: sends each due loop's prompt to its agent.
pub fn tick(app: &App) {
    if app.control.paused() {
        return; // held by STOP ALL: loops wait until the owner resumes
    }
    app.work.dispatch_idle(app);
    let now = now_ms();
    let due: Vec<Loop> = app
        .harness
        .locked()
        .loops
        .iter()
        .filter(|l| l.state == "running" && l.next_at_ms <= now)
        .cloned()
        .collect();
    for l in due {
        // An agent whose project is paused or over is not prompted: the loop waits until it works again.
        if crate::projects::held_cid(app, l.cid).is_some() {
            continue;
        }
        // The agent is still busy with the previous run of this loop: wait for it.
        if let Some(previous) = l.last_work
            && app.work.current(l.cid).is_some_and(|w| w.id == previous)
        {
            continue;
        }
        let run = l.runs + 1;
        let of = if l.max_runs > 0 {
            format!("/{}", l.max_runs)
        } else {
            String::new()
        };
        let text = format!(
            "[loop {id}, run {run}{of}] {prompt}\n\nWhen the loop's purpose is fulfilled, include the words {done} in your final answer.",
            id = l.id,
            prompt = l.prompt,
            done = LOOP_DONE
        );
        // Enqueue without holding the store lock: the work may end synchronously.
        let work = app.work.enqueue(
            app,
            l.cid,
            WorkKind::Message,
            format!("loop-{}", l.id),
            text,
            Origin::default(),
        );
        let updated = {
            let mut guard = app.harness.locked();
            let Some(index) = guard.loops.iter().position(|x| x.id == l.id) else {
                continue;
            };
            if guard.loops[index].state != "running" {
                continue;
            }
            let lp = &mut guard.loops[index];
            lp.last_work = Some(work);
            lp.runs += 1;
            lp.next_at_ms = now.saturating_add(lp.every_secs.saturating_mul(1000));
            lp.clone()
        };
        app.bus.publish("loop", &updated);
    }
}

/// Called when an agent's work ends: a loop whose last run this was records the summary,
/// and ends itself when the summary contains LOOP_DONE or max_runs is reached.
pub fn work_ended(app: &App, cid: u64, work: u64, summary: &str) {
    let ended = {
        let mut guard = app.harness.locked();
        let Some(index) = guard
            .loops
            .iter()
            .position(|l| l.cid == cid && l.last_work == Some(work))
        else {
            return;
        };
        let lp = &mut guard.loops[index];
        lp.last_summary = Some(summary.chars().take(2000).collect());
        let finished = summary.contains(LOOP_DONE) || (lp.max_runs > 0 && lp.runs >= lp.max_runs);
        if finished && lp.state == "running" {
            lp.state = "done".into();
        }
        lp.clone()
    };
    app.bus.publish("loop", &ended);
}

/// Starts the loop ticker: `tick` once a second, for the life of the process.
pub fn spawn_ticker(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            tick(&app);
        }
    });
}

/// Sends a control to the agent's running work (Err when it has none).
pub fn control(app: &App, cid: u64, control: Control) -> Result<u64, String> {
    let item = app
        .work
        .current(cid)
        .ok_or_else(|| format!("agent {cid} has no running work"))?;
    let data = serde_json::to_value(&control).unwrap_or(Value::Null);
    let message = apz_proto::wire::HostMessage::Control {
        work: item.id,
        control,
    };
    match crate::fleet::send(app, cid, &json!({ "message": message })) {
        Ok((code, _)) if (200..300).contains(&code) => {}
        Ok((code, _)) => return Err(format!("agent {cid} refused the control (HTTP {code})")),
        Err(e) => return Err(format!("agent {cid} is unreachable: {e}")),
    }
    app.transcripts.record(
        &app.bus,
        cid,
        item.id,
        crate::transcript::Entry::Control { control: data },
    );
    Ok(item.id)
}

/// One harness action, shared by the tool (`by` is the calling agent's label) and the owner's
/// route (`by` is "owner"). `input` carries `action` and the action's fields.
fn act(app: &App, cid: u64, input: &Value, by: &str) -> Result<String, String> {
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| format!("no agent {cid}"))?;
    match input["action"].as_str().unwrap_or("") {
        "compress" => {
            control(app, cid, Control::Compress).map(|w| format!("compression queued for work {w}"))
        }
        "steer" => {
            let text = input["text"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("steer needs text")?;
            if text.chars().count() > 6000 {
                return Err("steer text is over 6000 characters".into());
            }
            let note = Control::Steer {
                from: by.to_string(),
                text: text.to_string(),
            };
            control(app, cid, note).map(|w| format!("steer queued for work {w}"))
        }
        "extend_turns" => {
            let turns = input["turns"]
                .as_u64()
                .filter(|t| (1..=200).contains(t))
                .ok_or("extend_turns needs turns from 1 to 200")?;
            control(
                app,
                cid,
                Control::ExtendTurns {
                    turns: turns as u32,
                },
            )
            .map(|w| format!("work {w} may take {turns} more turns"))
        }
        "set_limits" => {
            let max_turns = input["max_turns"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok());
            let compress_at = input["compress_at"].as_u64();
            let keep_recent = input["keep_recent"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok());
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
            let loops: Vec<Loop> = app
                .harness
                .loops()
                .into_iter()
                .filter(|l| l.cid == cid)
                .collect();
            Ok(json!({ "limits": app.harness.limits(cid), "running_work": running_work, "loops": loops }).to_string())
        }
        other => Err(format!(
            "unknown action {other:?}; use compress, steer, extend_turns, set_limits or show"
        )),
    }
}

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_agent_control" => (
            "Manage an agent's harness (orchestrator): compress its running work's conversation, steer it with a note, give it more turns, or set its limits (max_turns per work item, compress_at bytes, keep_recent turns).",
            obj(
                json!({
                    "cid": {"type": "integer"},
                    "action": {"type": "string", "enum": ["compress", "steer", "extend_turns", "set_limits", "show"]},
                    "text": {"type": "string"},
                    "turns": {"type": "integer"},
                    "max_turns": {"type": "integer"},
                    "compress_at": {"type": "integer"},
                    "keep_recent": {"type": "integer"}
                }),
                &["cid", "action"],
            ),
        ),
        "plaza_loop" => (
            "Run a prompt on an agent repeatedly (orchestrator): start (cid, prompt, every_minutes, max_runs — 0 runs until the agent's answer contains \"LOOP DONE\"), stop (id), or list.",
            obj(
                json!({
                    "action": {"type": "string", "enum": ["start", "stop", "list"]},
                    "cid": {"type": "integer"},
                    "prompt": {"type": "string"},
                    "every_minutes": {"type": "integer"},
                    "max_runs": {"type": "integer"},
                    "id": {"type": "string"}
                }),
                &["action"],
            ),
        ),
        _ => return None,
    })
}

pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    if !may_control(&agent.role) {
        return Err("only the orchestrator manages harnesses".into());
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
            loop_target(app, cid)?;
            let prompt = input["prompt"]
                .as_str()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or("start needs a prompt")?;
            let minutes = input["every_minutes"]
                .as_u64()
                .filter(|m| *m >= 1)
                .ok_or("every_minutes must be at least 1")?;
            let max_runs = u32::try_from(input["max_runs"].as_u64().unwrap_or(0))
                .map_err(|_| "max_runs is too large".to_string())?;
            let looped = app.harness.start_loop(
                cid,
                prompt,
                minutes.saturating_mul(60),
                max_runs,
                caller(agent),
                agent.cid,
            )?;
            app.bus.publish("loop", &looped);
            Ok(format!(
                "loop {} started: every {minutes} minutes for agent {cid}",
                looped.id
            ))
        }
        "stop" => {
            let id = input["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("stop needs the loop id")?;
            let stopped = app.harness.stop_loop(id)?;
            app.bus.publish("loop", &stopped);
            Ok(format!(
                "loop {} stopped after {} runs",
                stopped.id, stopped.runs
            ))
        }
        "list" => {
            let lines: Vec<String> = app
                .harness
                .loops()
                .iter()
                .map(|l| json!({ "id": l.id, "cid": l.cid, "state": l.state, "runs": l.runs, "max_runs": l.max_runs, "every_secs": l.every_secs, "prompt": l.prompt }).to_string())
                .collect();
            Ok(if lines.is_empty() {
                "no loops".to_string()
            } else {
                lines.join("\n")
            })
        }
        other => Err(format!(
            "unknown loop action {other:?}; use start, stop or list"
        )),
    }
}

/// Checks that a loop can run for agent `cid`: it is on a project, or it is the orchestrator. A loop for an agent on
/// no project would never run.
fn loop_target(app: &App, cid: u64) -> Result<(), String> {
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| format!("no agent {cid}"))?;
    if !crate::settings::plaza_wide(&agent.role) && crate::projects::project_of(app, cid).is_none()
    {
        return Err(format!(
            "agent {cid} has no project: a loop runs only for an agent on a project"
        ));
    }
    Ok(())
}

/// The role a tool call acts under.
fn caller(agent: &Agent) -> &str {
    &agent.role
}

fn cid_of(input: &Value) -> Result<u64, String> {
    input["cid"]
        .as_u64()
        .ok_or_else(|| "cid is required".to_string())
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/loops", |app, _, _| {
            Response::json(200, &app.harness.loops())
        })
        .doc(RouteDoc::new("List the scheduled loops"))
        .post("/v1/loops", start_route)
        .doc(RouteDoc::new("Start a loop that prompts an agent on a schedule").errors(&[400]))
        .delete("/v1/loops/{id}", stop_route)
        .doc(RouteDoc::new("Stop a scheduled loop").errors(&[400]))
        .post("/v1/agents/{cid}/control", control_route)
        .doc(RouteDoc::new("Control an agent's running work").errors(&[400, 409]))
        .get("/v1/agents/{cid}/limits", |app, _, p| {
            let Ok(cid) = p.get("cid").parse::<u64>() else {
                return Response::error(400, "bad agent id");
            };
            Response::json(200, &app.harness.limits(cid))
        })
        .doc(RouteDoc::new("Show an agent's limits").errors(&[400]))
}

#[derive(Deserialize)]
struct StartBody {
    cid: u64,
    prompt: String,
    every_minutes: u64,
    max_runs: u32,
}

/// Body: `{cid, prompt, every_minutes, max_runs}`; the owner acts.
fn start_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: StartBody = match req.json() {
        Ok(body) => body,
        Err(err) => return Response::error(400, &format!("bad body: {err}")),
    };
    // The loop's own fields are checked first: a bad prompt or interval is refused whoever the agent is.
    let every_secs = body.every_minutes.saturating_mul(60);
    if let Err(err) = check_loop(&body.prompt, every_secs) {
        return Response::error(400, &err);
    }
    if let Err(err) = loop_target(app, body.cid) {
        return Response::error(400, &err);
    }
    match app.harness.start_loop(
        body.cid,
        &body.prompt,
        every_secs,
        body.max_runs,
        "owner",
        0,
    ) {
        Ok(lp) => {
            app.bus.publish("loop", &lp);
            Response::json(200, &lp)
        }
        Err(err) => Response::error(400, &err),
    }
}

fn stop_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    match app.harness.stop_loop(p.get("id")) {
        Ok(lp) => {
            app.bus.publish("loop", &lp);
            Response::json(200, &lp)
        }
        Err(err) => Response::error(400, &err),
    }
}

/// Body: the same as plaza_agent_control's input minus `cid`; the owner acts.
fn control_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Ok(cid) = p.get("cid").parse::<u64>() else {
        return Response::error(400, "bad agent id");
    };
    let mut body: Value = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    // The owner's files can go with a steer: the agent may read them, and the steer says where they are.
    let files: Vec<String> = body["attachments"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if body["action"] == "steer" && !files.is_empty() {
        let lines = match crate::deliverables::attach(app, &files, cid) {
            Ok(lines) => lines,
            Err(e) => return Response::error(400, &e),
        };
        let text = body["text"].as_str().unwrap_or("").trim().to_string();
        let text = if text.is_empty() {
            "(files attached)".to_string()
        } else {
            text
        };
        body["text"] = json!(format!("{text}\n\n{lines}"));
    }
    match act(app, cid, &body, "owner") {
        Ok(result) => Response::json(200, &json!({ "ok": true, "result": result })),
        Err(e) => Response::error(409, &e),
    }
}

/// The harness's tables: `harness` (one row), with `harness_loops` and `harness_limits` as its child tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Harness)
}

impl Store {
    /// The harness's row as its state is now (the saver writes what changed).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let state = self.locked();
        apz_store::RowSet::one(def(), &*state)
    }

    /// Replaces the state with what the store holds (at start); no row yet means an empty state.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Option<Harness> = apz_store::table::load_one(conn, def())?;
        *self.locked() = stored.unwrap_or_default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loops_prompt_and_interval_are_checked_before_its_agent() {
        assert_eq!(check_loop("  Check  ", 60), Ok("Check"));
        assert!(
            check_loop("   ", 60)
                .unwrap_err()
                .contains("1 to 4000 characters")
        );
        assert!(check_loop(&"x".repeat(4001), 60).is_err());
        assert!(check_loop("ok", 59).unwrap_err().contains("once a minute"));
        assert!(check_loop("ok", MAX_EVERY_SECS + 1).is_err());
    }

    /// A database holding the harness's tables, as the migration creates them.
    fn migrated() -> apz_store::rusqlite::Connection {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        let problems = apz_store::lint::schema(&conn).unwrap();
        assert!(
            problems.is_empty(),
            "the harness's tables fail the lint: {problems:?}"
        );
        conn
    }

    /// The store as a restart leaves it: its rows written to a database, then loaded into a fresh store.
    fn restarted(store: &Store) -> Store {
        let conn = migrated();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let loaded = Store::default();
        loaded.load(&conn).unwrap();
        loaded
    }

    fn state(store: &Store) -> Value {
        serde_json::to_value(&*store.locked()).unwrap()
    }

    #[test]
    fn the_state_comes_back_whole_from_its_tables() {
        let a = Store::default();
        a.start_loop(1, "check the queue", 60, 3, "alpha", 1)
            .unwrap();
        a.set_limits(
            1,
            Limits {
                max_turns: Some(40),
                ..Default::default()
            },
        )
        .unwrap();
        let b = restarted(&a);
        assert_eq!(state(&b), state(&a));
        assert_eq!(b.loops().len(), 1);
        assert_eq!(b.limits(1).max_turns, Some(40));
    }

    #[test]
    fn harness_limits_are_validated_and_merged() {
        let store = Store::default();
        let first = store
            .set_limits(
                7,
                Limits {
                    max_turns: Some(40),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            first,
            Limits {
                max_turns: Some(40),
                ..Default::default()
            }
        );
        let merged = store
            .set_limits(
                7,
                Limits {
                    keep_recent: Some(3),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            merged,
            Limits {
                max_turns: Some(40),
                compress_at: None,
                keep_recent: Some(3)
            }
        );
        assert_eq!(store.limits(7), merged);
        assert_eq!(store.limits(8), Limits::default());
        assert!(
            store
                .set_limits(
                    7,
                    Limits {
                        max_turns: Some(0),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .set_limits(
                    7,
                    Limits {
                        max_turns: Some(501),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .set_limits(
                    7,
                    Limits {
                        compress_at: Some(19_999),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .set_limits(
                    7,
                    Limits {
                        compress_at: Some(4_000_001),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .set_limits(
                    7,
                    Limits {
                        keep_recent: Some(51),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(
            store
                .set_limits(
                    7,
                    Limits {
                        compress_at: Some(0),
                        ..Default::default()
                    }
                )
                .unwrap()
                .compress_at,
            Some(0)
        );
        // Rejected values changed nothing; the accepted ones survive a restart.
        let reloaded = restarted(&store);
        assert_eq!(
            reloaded.limits(7),
            Limits {
                max_turns: Some(40),
                compress_at: Some(0),
                keep_recent: Some(3)
            }
        );
    }

    #[test]
    fn harness_loops_persist_across_load() {
        let store = Store::default();
        let a = store
            .start_loop(5, "  check the build  ", 60, 3, "owner", 0)
            .unwrap();
        assert_eq!(a.id, "l1");
        assert_eq!(a.prompt, "check the build");
        assert_eq!(a.state, "running");
        assert_eq!(a.next_at_ms, a.created_ms + 60_000);
        assert!(store.start_loop(5, "too fast", 59, 0, "owner", 0).is_err());
        assert!(store.start_loop(5, "   ", 60, 0, "owner", 0).is_err());
        assert!(
            store
                .start_loop(5, &"x".repeat(4001), 60, 0, "owner", 0)
                .is_err()
        );
        let b = store
            .start_loop(6, "second", 120, 0, "orchestrator", 9)
            .unwrap();
        assert_eq!(b.id, "l2");
        assert_eq!(store.stop_loop("l1").unwrap().state, "stopped");
        assert!(store.stop_loop("l99").is_err());

        let reloaded = restarted(&store);
        let loops = reloaded.loops();
        assert_eq!(
            loops.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
            ["l2", "l1"]
        );
        assert_eq!(loops[1].state, "stopped");
        assert_eq!(loops[0].by_cid, 9);
        assert_eq!(
            reloaded
                .start_loop(6, "third", 60, 0, "owner", 0)
                .unwrap()
                .id,
            "l3"
        );
    }

    #[test]
    fn the_longest_interval_is_stored_whole() {
        let store = Store::default();
        assert!(
            store
                .start_loop(7, "no end", u64::MAX, 0, "owner", 0)
                .is_err()
        );
        assert!(
            store
                .start_loop(7, "too far", MAX_EVERY_SECS + 1, 0, "owner", 0)
                .is_err()
        );
        let far = store
            .start_loop(7, "far away", MAX_EVERY_SECS, 0, "owner", 0)
            .unwrap();
        assert_eq!(restarted(&store).loops(), [far]);
    }

    #[test]
    fn a_database_with_no_row_loads_empty() {
        let store = Store::default();
        store.load(&migrated()).unwrap();
        assert!(store.loops().is_empty());
        assert_eq!(store.limits(1), Limits::default());
    }
}
