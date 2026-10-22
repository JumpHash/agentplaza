//! The observe tools: read only. Each reads through the service the HTTP API uses.

use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::Response;
use serde_json::{Value, json};

use super::{Ctx, Plaza, Tool, version_arg};
use crate::hub_mcp::auth::Scope;
use crate::state::App;

/// The keys a roster row carries in `list_agents`.
const ROW_KEYS: [&str; 8] = [
    "cid", "name", "role", "tier", "status", "node", "model", "work",
];
const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 200;
/// A transcript tool result longer than this is cut, in bytes.
const TOOL_RESULT_CAP: usize = 4096;
/// The journal entries `treasury_summary` shows.
const JOURNAL_LIMIT: usize = 20;
const DAY_MS: u64 = 86_400_000;

pub struct PlazaStatus;
pub struct ListAgents;
pub struct GetAgent;
pub struct GetTranscript;
pub struct ListTasks;
pub struct GetTask;
pub struct ListBoard;
pub struct ListProjects;
pub struct GetProject;
pub struct ProjectGraph;
pub struct ListApprovals;
pub struct ListMachines;
pub struct TreasurySummary;
pub struct UsageSummary;
pub struct ListLaws;
pub struct SearchLibrary;
pub struct PromptsList;
pub struct PromptGet;
pub struct SkillsList;
pub struct SkillGet;

impl Plaza for App {
    fn prompts(&self) -> &crate::prompts::Prompts {
        &self.prompts
    }

    fn skills(&self) -> &crate::skills::Skills {
        &self.skills
    }

    fn announce(&self, kind: &str, id: &str, action: &str) {
        self.bus
            .publish(kind, &json!({ "id": id, "action": action }));
    }

    fn stations(&self) -> Vec<Value> {
        crate::stations::tools::list(self)
    }

    fn station_call(&self, name: &str, args: &Value) -> Result<Value, String> {
        // The owner acts as cid 0: its own role and actor in the laws and the station audit.
        crate::stations::tools::call(self, 0, name, args)
    }

    fn search_library(&self, query: &str, limit: usize) -> Vec<Value> {
        self.library
            .search(query, limit)
            .iter()
            .filter_map(|entry| serde_json::to_value(entry).ok())
            .collect()
    }

    fn create_task(&self, input: &Value) -> Result<Value, String> {
        response_json(crate::plaza::create_task(
            self,
            input.to_string().as_bytes(),
        ))
    }

    fn restart_agent(&self, cid: u64) -> Result<Value, String> {
        let (run, vm) = crate::lifecycle::place(self, cid).map_err(response_text)?;
        self.work.cancel(self, cid, "restarted by the owner");
        crate::lifecycle::noded_call(self, cid, &run, vm, "restart").map_err(response_text)?;
        Ok(json!({ "ok": true, "cid": cid }))
    }

    fn record_decision(
        &self,
        approval_id: &str,
        decision: &str,
        note: &str,
    ) -> Result<Value, String> {
        let approval = self
            .approvals
            .list()
            .into_iter()
            .find(|approval| approval.id == approval_id)
            .ok_or_else(|| format!("no approval {approval_id}"))?;
        let row = serde_json::to_value(&approval).map_err(|err| err.to_string())?;
        if !row["state"]
            .as_str()
            .is_some_and(|state| state.eq_ignore_ascii_case("pending"))
        {
            return Err(format!("approval {approval_id} is not pending"));
        }
        // A recommendation only: the approval is decided by the owner, never here.
        let recorded = super::govern::record(approval_id, decision, note);
        self.bus.publish("decision_recommendation", &recorded);
        Ok(
            json!({ "recorded": true, "decided": false, "approval_id": approval_id, "decision": decision }),
        )
    }

    fn status(&self) -> Value {
        crate::api::overview_value(self)
    }

    fn agents(&self) -> Vec<Value> {
        crate::agents::list_views(self)
    }

    fn agent(&self, cid: u64) -> Option<Value> {
        self.agents
            .get(cid)
            .map(|agent| crate::agents::view(self, &agent))
    }

    fn transcript(&self, cid: u64, after: u64, limit: usize) -> Vec<Value> {
        self.transcripts
            .after(cid, after, limit)
            .iter()
            .filter_map(|entry| serde_json::to_value(entry).ok())
            .collect()
    }

    fn tasks(&self) -> Vec<Value> {
        self.plaza
            .tasks
            .list()
            .iter()
            .filter_map(|task| serde_json::to_value(task.view()).ok())
            .collect()
    }

    fn task(&self, id: &str) -> Option<Value> {
        self.plaza
            .tasks
            .find(id)
            .and_then(|task| serde_json::to_value(task.view()).ok())
    }

    fn board(&self, limit: usize) -> Value {
        let connected: Vec<u64> = self
            .agents
            .list()
            .iter()
            .filter(|agent| agent.status != crate::agents::Status::Offline)
            .map(|agent| agent.cid)
            .collect();
        serde_json::to_value(self.plaza.social.board(&connected, limit)).unwrap_or(Value::Null)
    }

    fn projects(&self) -> Vec<Value> {
        self.projects
            .list()
            .iter()
            .filter_map(|project| serde_json::to_value(project).ok())
            .collect()
    }

    fn project(&self, id: &str) -> Option<Value> {
        self.projects
            .get(id)
            .and_then(|project| serde_json::to_value(project).ok())
    }

    fn project_graph(&self, id: &str) -> Option<Value> {
        crate::task_graph::of_project(self, id).and_then(|graph| serde_json::to_value(graph).ok())
    }

    fn approvals(&self) -> Vec<Value> {
        self.approvals
            .list()
            .iter()
            .filter_map(|approval| {
                let mut row = serde_json::to_value(approval).ok()?;
                if let Some(recommendation) = super::govern::recommendation_for(&approval.id) {
                    row["recommendation"] = recommendation;
                }
                Some(row)
            })
            .collect()
    }

    fn machines(&self) -> Vec<Value> {
        self.nodes
            .list()
            .iter()
            .filter_map(|node| serde_json::to_value(node).ok())
            .collect()
    }

    fn treasury(&self, journal_limit: usize) -> Value {
        json!({
            "view": serde_json::to_value(self.plaza.treasury.view()).unwrap_or(Value::Null),
            "journal": serde_json::to_value(self.plaza.treasury.journal(journal_limit)).unwrap_or(Value::Null),
            "plaza_budget": self.plaza.treasury.plaza_budget_balance(),
        })
    }

    fn usage(&self, from_ms: u64, to_ms: u64) -> Value {
        serde_json::to_value(self.usage.summary(from_ms, to_ms)).unwrap_or(Value::Null)
    }

    fn laws(&self) -> Vec<Value> {
        self.laws
            .catalog()
            .iter()
            .filter_map(|law| serde_json::to_value(law).ok())
            .collect()
    }

    fn message_orchestrator(&self, text: &str, chat: Option<&str>) -> Result<Value, String> {
        let chat = chat.unwrap_or(crate::chats::MAIN);
        let said = crate::work_routes::say(self, chat, text, &[]).map_err(|(_, e)| e)?;
        Ok(json!({ "chat": chat, "message": said.seq }))
    }

    fn tell_agent(&self, cid: u64, text: &str) -> Result<Value, String> {
        crate::agents::check_chat_text(text)?;
        let agent = self
            .agents
            .get(cid)
            .ok_or_else(|| format!("no agent {cid}"))?;
        if let Some(refusal) = chat_refusal(&agent.kind, agent.status) {
            return Err(refusal.to_string());
        }
        // An agent whose project is paused or over takes no chat (it would only wait).
        if let Some(why) = crate::projects::held(self, &agent) {
            return Err(why);
        }
        Ok(queue_owner_chat(self, cid, text))
    }

    fn stop_agent(&self, cid: u64, reason: &str) -> Result<Value, String> {
        gateway_reply(crate::fleet::gateway(
            self,
            cid,
            "POST",
            &format!("/v1/agents/{cid}/stop"),
            Some(&json!({ "reason": reason })),
        ))
    }
}

impl Tool for PromptsList {
    fn name(&self) -> &'static str {
        "prompts_list"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "Lists the live prompts (system prompts, AGENTS.md files and fragments): id, kind, title, binding, live and latest version, and the live version's score."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "kind": { "type": "string", "enum": ["system", "agents_md", "fragment"] } },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "prompts": { "type": "array", "items": { "type": "object" } } },
            "required": ["prompts"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let kind = match args.get("kind") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str().ok_or("kind must be a string")?),
        };
        Ok(json!({ "prompts": ctx.plaza.prompts_list(kind)? }))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} prompt(s).",
            out["prompts"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for SkillsList {
    fn name(&self) -> &'static str {
        "skills_list"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "Lists the live skills: id, name, title, description, bindings, active and latest version, the live version's score."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "skills": { "type": "array" } }, "required": ["skills"] })
    }

    fn call(&self, ctx: &Ctx<'_>, _args: &Value) -> Result<Value, String> {
        Ok(json!({ "skills": ctx.plaza.skills_list()? }))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} live skills.",
            out["skills"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for SkillGet {
    fn name(&self) -> &'static str {
        "skill_get"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "One version of a skill (default: the live one, else the latest): its SKILL.md, files, score and findings."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "version": { "type": "integer", "minimum": 1 },
            },
            "required": ["id"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "version": { "type": "integer" },
                "skill_md": { "type": "string" },
                "score": { "type": "integer" },
                "files": { "type": "array" },
                "findings": { "type": "array" },
            },
            "required": ["id", "version", "skill_md", "score"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or("id must be a string")?;
        let version = match args.get("version") {
            None | Some(Value::Null) => None,
            Some(_) => Some(version_arg(args, "version")?),
        };
        ctx.plaza.skill_get(id, version)
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Skill {} version {}, score {}.",
            out["id"].as_str().unwrap_or("?"),
            out["version"],
            out["score"]
        )
    }
}

impl Tool for PromptGet {
    fn name(&self) -> &'static str {
        "prompt_get"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "Reads one version of a prompt: its body, score and findings. Without a version, the live one, or the latest when none is live."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "version": { "type": "integer", "minimum": 1 },
            },
            "required": ["id"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "version": { "type": "integer" },
                "body": { "type": "string" },
                "score": { "type": "integer" },
                "findings": { "type": "array" },
            },
            "required": ["id", "version", "body", "score", "findings"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or("id must be a string")?;
        let version = match args.get("version") {
            None | Some(Value::Null) => None,
            Some(_) => Some(version_arg(args, "version")?),
        };
        ctx.plaza.prompt_get(id, version)
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Prompt {} version {}, score {}.",
            out["id"].as_str().unwrap_or("?"),
            out["version"],
            out["score"]
        )
    }
}

/// The body of a refused service call, as a tool's error.
fn response_text(resp: Response) -> String {
    String::from_utf8_lossy(resp.bytes()).into_owned()
}

/// A service answer as JSON: the body on success, or the refusal's text.
fn response_json(resp: Response) -> Result<Value, String> {
    if (200..300).contains(&resp.status) {
        serde_json::from_slice(resp.bytes()).map_err(|err| err.to_string())
    } else {
        Err(String::from_utf8_lossy(resp.bytes()).into_owned())
    }
}

/// Why an agent of `kind` in `status` cannot take the owner's chat, if it cannot.
fn chat_refusal(kind: &str, status: crate::agents::Status) -> Option<&'static str> {
    if kind == "model" {
        Some("a model agent has no chat")
    } else if status == crate::agents::Status::Offline {
        Some("agent is offline")
    } else {
        None
    }
}

/// Queues `text` as the owner's chat on agent `cid`: the work the owner's chat route queues,
/// on the owner's thread, with the owner's origin.
fn queue_owner_chat(app: &App, cid: u64, text: &str) -> Value {
    let origin = crate::scheduler::Origin {
        owner: true,
        mcp: true,
        ..Default::default()
    };
    let work = app.work.enqueue(
        app,
        cid,
        apz_proto::wire::WorkKind::Chat,
        format!("op-{cid}"),
        text.to_string(),
        origin,
    );
    json!({ "work": work, "cid": cid })
}

/// A gateway call's answer: the body on success, or the node's refusal.
fn gateway_reply(result: Result<(u16, Value), String>) -> Result<Value, String> {
    let (status, body) = result?;
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(format!("the agent's node answered {status}: {body}"))
    }
}

fn count(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

fn up(value: &Value) -> &'static str {
    if value.as_bool() == Some(true) {
        "up"
    } else {
        "down"
    }
}

fn compact(agent: &Value) -> Value {
    let mut row = serde_json::Map::new();
    for key in ROW_KEYS {
        row.insert(
            key.to_string(),
            agent.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(row)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn cid_arg(args: &Value) -> Result<u64, String> {
    args.get("cid")
        .and_then(Value::as_u64)
        .ok_or_else(|| "cid must be a whole number".to_string())
}

fn limit_of(args: &Value) -> Result<usize, String> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT as usize),
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| "limit must be a whole number from 1 to 200".to_string()),
    }
}

fn cursor_of(args: &Value) -> Result<usize, String> {
    match args.get("cursor").and_then(Value::as_str) {
        None => Ok(0),
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| "cursor is not a next_cursor from this tool".to_string()),
    }
}

/// One page of `rows` under `key`, with the cursor for the next page when there is one.
fn page(rows: Vec<Value>, args: &Value, key: &str) -> Result<Value, String> {
    let limit = limit_of(args)?;
    let cursor = cursor_of(args)?;
    let items: Vec<Value> = rows.iter().skip(cursor).take(limit).cloned().collect();
    let next = cursor + items.len();
    let next_cursor = (next < rows.len()).then(|| next.to_string());
    Ok(json!({ key: items, "next_cursor": next_cursor }))
}

/// The rows whose `field` equals `wanted`, when a filter is given.
fn keep(rows: Vec<Value>, field: &str, wanted: Option<&Value>) -> Vec<Value> {
    match wanted {
        None | Some(Value::Null) => rows,
        Some(wanted) => rows
            .into_iter()
            .filter(|row| &row[field] == wanted)
            .collect(),
    }
}

/// The task rows of project `wanted`, when a project is given: the rows that name it, and the ones
/// whose ids it lists in its `tasks` (a task posted before tasks named their project is only listed
/// there). A project that does not exist and that no task names has none.
fn in_project(plaza: &dyn Plaza, rows: Vec<Value>, wanted: Option<&Value>) -> Vec<Value> {
    let Some(wanted) = wanted.filter(|wanted| !wanted.is_null()) else {
        return rows;
    };
    let tasks = wanted
        .as_str()
        .and_then(|id| plaza.project(id))
        .map(|project| project["tasks"].clone());
    let ids = tasks.as_ref().and_then(Value::as_array);
    rows.into_iter()
        .filter(|row| &row["project"] == wanted || ids.is_some_and(|ids| ids.contains(&row["id"])))
        .collect()
}

/// A transcript entry, with a tool result cut to the cap and a preview kept.
fn capped(mut entry: Value) -> Value {
    let kind = entry["kind"].as_str().unwrap_or_default();
    let is_result = kind.contains("tool") || kind.contains("result");
    let text = entry["data"].to_string();
    if is_result && text.len() > TOOL_RESULT_CAP {
        let mut cut = TOOL_RESULT_CAP;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        entry["data"] = json!({ "truncated": true, "bytes": text.len(), "preview": &text[..cut] });
    }
    entry
}

impl Tool for PlazaStatus {
    fn name(&self) -> &'static str {
        "plaza_status"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The plaza's health: agent counts by status, task counts by state, the default models, and whether noded and the hub answer."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agents": { "type": "object" },
                "tasks": { "type": "object" },
                "default_model": { "type": "string" },
                "orchestrator_model": { "type": "string" },
                "noded": { "type": "boolean" },
                "hub": { "type": "boolean" },
            },
            "required": ["agents", "tasks", "noded", "hub"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, _args: &Value) -> Result<Value, String> {
        Ok(ctx.plaza.status())
    }

    fn summary(&self, out: &Value) -> String {
        let agents = &out["agents"];
        format!(
            "{} agents ({} working, {} idle, {} offline). Noded {}, hub {}.",
            count(&agents["total"]),
            count(&agents["working"]),
            count(&agents["idle"]),
            count(&agents["offline"]),
            up(&out["noded"]),
            up(&out["hub"]),
        )
    }
}

impl Tool for ListAgents {
    fn name(&self) -> &'static str {
        "list_agents"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The agents on the roster: cid, name, role, tier, status, node, model and current work. Filter by role or status; page with next_cursor."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "role": { "type": "string" },
                "status": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "cursor": { "type": "string", "description": "the next_cursor of the previous page" },
            },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agents": { "type": "array", "items": { "type": "object" } },
                "next_cursor": { "type": ["string", "null"] },
            },
            "required": ["agents", "next_cursor"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let role = args.get("role").and_then(Value::as_str);
        let status = args.get("status").and_then(Value::as_str);
        let limit = limit_of(args)?;
        let cursor = cursor_of(args)?;
        let rows: Vec<Value> = ctx
            .plaza
            .agents()
            .into_iter()
            .filter(|agent| role.is_none_or(|wanted| agent["role"].as_str() == Some(wanted)))
            .filter(|agent| status.is_none_or(|wanted| agent["status"].as_str() == Some(wanted)))
            .collect();
        let page: Vec<Value> = rows.iter().skip(cursor).take(limit).map(compact).collect();
        let next = cursor + page.len();
        let next_cursor = (next < rows.len()).then(|| next.to_string());
        Ok(json!({ "agents": page, "next_cursor": next_cursor }))
    }

    fn summary(&self, out: &Value) -> String {
        let shown = out["agents"].as_array().map_or(0, Vec::len);
        let more = if out["next_cursor"].is_string() {
            ", more on the next page"
        } else {
            ""
        };
        format!("{shown} agents on this page{more}.")
    }
}

impl Tool for GetAgent {
    fn name(&self) -> &'static str {
        "get_agent"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "One agent's full view: profile, model, tier, current work and queue."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "cid": { "type": "integer", "minimum": 0 } },
            "required": ["cid"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "the agent's view, as GET /v1/agents/{cid} returns it",
            "properties": { "cid": { "type": "integer" } },
            "required": ["cid"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let cid = cid_arg(args)?;
        ctx.plaza
            .agent(cid)
            .ok_or_else(|| format!("no agent with cid {cid}"))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Agent {} ({}), {}.",
            out["name"].as_str().unwrap_or("unnamed"),
            out["role"].as_str().unwrap_or("no role"),
            out["status"].as_str().unwrap_or("status unknown"),
        )
    }
}

impl Tool for GetTranscript {
    fn name(&self) -> &'static str {
        "get_transcript"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "An agent's transcript after entry `after`, oldest first. Each tool result is cut to 4 KB. Continue from next_after."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cid": { "type": "integer", "minimum": 0 },
                "after": { "type": "integer", "minimum": 0, "description": "the seq of the last entry you have" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
            },
            "required": ["cid"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entries": { "type": "array", "items": { "type": "object" } },
                "next_after": { "type": "integer" },
            },
            "required": ["entries", "next_after"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let cid = cid_arg(args)?;
        if ctx.plaza.agent(cid).is_none() {
            return Err(format!("no agent with cid {cid}"));
        }
        let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
        let limit = limit_of(args)?;
        let entries: Vec<Value> = ctx
            .plaza
            .transcript(cid, after, limit)
            .into_iter()
            .map(capped)
            .collect();
        let next_after = entries
            .last()
            .and_then(|entry| entry["seq"].as_u64())
            .unwrap_or(after);
        Ok(json!({ "entries": entries, "next_after": next_after }))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} transcript entries.",
            out["entries"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for ListTasks {
    fn name(&self) -> &'static str {
        "list_tasks"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The plaza's tasks: id, goal, state, budget, role, project, the tasks each runs after (after) and whether it can be claimed now (ready, waiting_on, blocked_by), assignee and outcome. Filter by status (the task's state), project or assignee; page with next_cursor."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "description": "a task state: open, claimed, awaiting_review, done, failed or withdrawn" },
                "project": { "type": "string", "description": "a project id: the tasks that name it, or that it lists" },
                "assignee": { "type": ["integer", "string"], "description": "the claiming agent: its cid, or the task's assignee_id (its role, or cid:<n>)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "cursor": { "type": "string" },
            },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tasks": { "type": "array", "items": { "type": "object" } },
                "next_cursor": { "type": ["string", "null"] },
            },
            "required": ["tasks", "next_cursor"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        // A row is the ledger's task view: its status is `state`, its claimant `agent_cid` and
        // `assignee_id`, and its project `project` (a project also lists its tasks' ids).
        let rows = keep(ctx.plaza.tasks(), "state", args.get("status"));
        let rows = in_project(ctx.plaza, rows, args.get("project"));
        let assignee = args.get("assignee");
        let claimant = if assignee.is_some_and(Value::is_string) {
            "assignee_id"
        } else {
            "agent_cid"
        };
        let rows = keep(rows, claimant, assignee);
        page(rows, args, "tasks")
    }

    fn summary(&self, out: &Value) -> String {
        let shown = out["tasks"].as_array().map_or(0, Vec::len);
        let more = if out["next_cursor"].is_string() {
            ", more on the next page"
        } else {
            ""
        };
        format!("{shown} tasks on this page{more}.")
    }
}

impl Tool for GetTask {
    fn name(&self) -> &'static str {
        "get_task"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "One task in detail: goal, acceptance, budget, role, project, the tasks it runs after (after, ready, waiting_on, blocked_by), state, outcome, assignee, note, and when it was created and last changed."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the task view, as GET /v1/tasks/{id} returns it" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "id must be a string".to_string())?;
        ctx.plaza
            .task(id)
            .ok_or_else(|| format!("no task with id {id}"))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Task {}: {}.",
            out["id"].as_str().unwrap_or("?"),
            out["state"].as_str().unwrap_or("status unknown")
        )
    }
}

impl Tool for ListBoard {
    fn name(&self) -> &'static str {
        "list_board"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The newest board posts, with their comment counts, up to limit. The board has no cursor yet."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 200 } },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the board view, as GET /v1/board returns it, capped at limit" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let limit = limit_of(args)?;
        Ok(ctx.plaza.board(limit))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Board: {} posts shown.",
            out["posts"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for ListProjects {
    fn name(&self) -> &'static str {
        "list_projects"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The projects: id, name, status, owner agent and progress. Filter by status; page with next_cursor."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "cursor": { "type": "string" },
            },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "projects": { "type": "array", "items": { "type": "object" } },
                "next_cursor": { "type": ["string", "null"] },
            },
            "required": ["projects", "next_cursor"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        page(
            keep(ctx.plaza.projects(), "status", args.get("status")),
            args,
            "projects",
        )
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} projects on this page.",
            out["projects"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for GetProject {
    fn name(&self) -> &'static str {
        "get_project"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "One project in detail: its docs, tasks and events."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the project, as GET /v1/projects/{id} returns it" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "id must be a string".to_string())?;
        ctx.plaza
            .project(id)
            .ok_or_else(|| format!("no project with id {id}"))
    }

    fn summary(&self, out: &Value) -> String {
        format!("Project {}.", out["name"].as_str().unwrap_or("unnamed"))
    }
}

impl Tool for ProjectGraph {
    fn name(&self) -> &'static str {
        "project_graph"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "A project's tasks in the order their after links let them run: each task's depth, state, readiness, claimant, reviewer and the step its agent is at now; the links as [from, to] edges; the frontier (being worked on or ready); the longest chain still to finish; and the ready tasks waiting for an agent, by role."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "project": { "type": "string", "description": "a project id" } },
            "required": ["project"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "the graph, as GET /v1/projects/{id}/graph returns it",
            "properties": {
                "project": { "type": "string" },
                "stage": { "type": "string" },
                "tasks": { "type": "array", "items": { "type": "object" } },
                "edges": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } },
                "frontier": { "type": "array", "items": { "type": "string" } },
                "longest_chain": { "type": "array", "items": { "type": "string" } },
                "waiting": { "type": "array", "items": { "type": "object" } },
            },
            "required": ["project", "stage", "tasks", "edges", "frontier", "longest_chain", "waiting"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args
            .get("project")
            .and_then(Value::as_str)
            .ok_or_else(|| "project must be a string".to_string())?;
        ctx.plaza
            .project_graph(id)
            .ok_or_else(|| format!("no project with id {id}"))
    }

    fn summary(&self, out: &Value) -> String {
        let count = |key: &str| out[key].as_array().map_or(0, Vec::len);
        let waiting: usize = out["waiting"].as_array().map_or(0, |groups| {
            groups
                .iter()
                .map(|group| group["tasks"].as_array().map_or(0, Vec::len))
                .sum()
        });
        format!(
            "{} tasks: {} being worked on or ready, {waiting} waiting for an agent, {} in the longest chain left.",
            count("tasks"),
            count("frontier"),
            count("longest_chain")
        )
    }
}

impl Tool for ListApprovals {
    fn name(&self) -> &'static str {
        "list_approvals"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "Approval requests, pending by default: id, kind, who asks, what, risk and created time. Pass status \"all\" for every state."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "description": "pending (the default), all, or a state name" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "cursor": { "type": "string" },
            },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "approvals": { "type": "array", "items": { "type": "object" } },
                "next_cursor": { "type": ["string", "null"] },
            },
            "required": ["approvals", "next_cursor"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let status = args
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let rows: Vec<Value> = ctx
            .plaza
            .approvals()
            .into_iter()
            .filter(|row| {
                status == "all"
                    || row["state"]
                        .as_str()
                        .is_some_and(|state| state.eq_ignore_ascii_case(status))
            })
            .collect();
        page(rows, args, "approvals")
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} approvals on this page.",
            out["approvals"].as_array().map_or(0, Vec::len)
        )
    }
}

/// The workstations: id, name, status, what each machine is and its leases. Read-only.
pub struct StationList;

impl Tool for StationList {
    fn name(&self) -> &'static str {
        "station_list"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The plaza's workstations: id, name, status (online, offline, draining, revoked), what each machine is and the leases on it."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object", "properties": {"stations": {"type": "array", "items": {"type": "object"}}}, "required": ["stations"]})
    }

    fn call(&self, ctx: &Ctx<'_>, _args: &Value) -> Result<Value, String> {
        Ok(json!({ "stations": ctx.plaza.stations() }))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} workstations.",
            out["stations"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for ListMachines {
    fn name(&self) -> &'static str {
        "list_machines"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The machines the plaza runs on: name, kind, status, CPU, memory, GPUs and the agents running on each."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "cursor": { "type": "string" },
            },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "machines": { "type": "array", "items": { "type": "object" } },
                "next_cursor": { "type": ["string", "null"] },
            },
            "required": ["machines", "next_cursor"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        page(ctx.plaza.machines(), args, "machines")
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} machines on this page.",
            out["machines"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for TreasurySummary {
    fn name(&self) -> &'static str {
        "treasury_summary"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The treasury: balances, the plaza budget, and the latest journal entries. The balances are all-time; there is no per-window or per-agent spend yet."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "view": { "type": "object" },
                "journal": { "type": "object" },
                "plaza_budget": { "type": "integer" },
            },
            "required": ["view", "journal", "plaza_budget"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, _args: &Value) -> Result<Value, String> {
        Ok(ctx.plaza.treasury(JOURNAL_LIMIT))
    }

    fn summary(&self, out: &Value) -> String {
        format!("Plaza budget {}.", out["plaza_budget"])
    }
}

impl Tool for UsageSummary {
    fn name(&self) -> &'static str {
        "usage_summary"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "Model calls, tokens and cost over the last day or week, by provider, model, agent and role."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "window": { "type": "string", "enum": ["day", "week"] } },
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the usage summary: totals and groups by model, agent and role" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let span = match args.get("window").and_then(Value::as_str).unwrap_or("day") {
            "day" => DAY_MS,
            "week" => 7 * DAY_MS,
            _ => return Err("window must be day or week".to_string()),
        };
        let now = now_ms();
        Ok(ctx.plaza.usage(now.saturating_sub(span), now))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Usage: {} calls.",
            out["totals"]["calls"].as_u64().unwrap_or(0)
        )
    }
}

impl Tool for ListLaws {
    fn name(&self) -> &'static str {
        "list_laws"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "The laws in force: id, tier, text and the overrides that apply to each."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "laws": { "type": "array", "items": { "type": "object" } } },
            "required": ["laws"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, _args: &Value) -> Result<Value, String> {
        Ok(json!({ "laws": ctx.plaza.laws() }))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} laws in force.",
            out["laws"].as_array().map_or(0, Vec::len)
        )
    }
}

impl Tool for SearchLibrary {
    fn name(&self) -> &'static str {
        "search_library"
    }

    fn scope(&self) -> Scope {
        Scope::Observe
    }

    fn description(&self) -> &'static str {
        "Library packages whose name, description or file path matches the query, with their versions and files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
            },
            "required": ["query"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "packages": { "type": "array", "items": { "type": "object" } } },
            "required": ["packages"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .filter(|query| !query.is_empty())
            .ok_or("query must be a non-empty string")?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT);
        Ok(json!({ "packages": ctx.plaza.search_library(query, limit as usize) }))
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} library packages match.",
            out["packages"].as_array().map_or(0, Vec::len)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::Status;

    #[test]
    fn chat_is_refused_for_a_model_agent_and_an_offline_one_only() {
        assert_eq!(
            chat_refusal("model", Status::Idle),
            Some("a model agent has no chat")
        );
        assert_eq!(
            chat_refusal("llm", Status::Offline),
            Some("agent is offline")
        );
        assert_eq!(chat_refusal("llm", Status::Idle), None);
        assert_eq!(chat_refusal("llm", Status::Working), None);
    }
}
