//! The tools agents are offered: their specs, who runs them, and the host
//! tools `apid` runs itself.
//!
//! OWNER: slice "loop". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::sync::Arc;

use apz_plaza::council::{self, ProposeRequest, VoteRequest};
use apz_plaza::social::PostRequest;
use apz_plaza::tasks::{CreateTask, Task};
use apz_plaza::ConnectedAgent;
use apz_proto::chat::ToolSpec;
use apz_proto::wire::WorkKind;
use serde::Serialize;
use serde_json::{json, Value};

use crate::agents::{self, Agent};
use crate::scheduler::{Origin, REVIEWER_ROLE};
use crate::settings::{tier_of, Tier};
use crate::state::App;

/// The escrow a task gets when its poster names no budget.
pub const DEFAULT_TASK_BUDGET: u64 = 2000;

/// Where a tool runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// In the agent's VM (the harness).
    Guest,
    /// In apid, with the agent's identity.
    Host,
}

/// Tools every agent is offered: host tools, plus the guest internet tools,
/// which run in the agent's VM and reach the internet through the hub's egress proxy.
const ALWAYS: &[&str] = &[
    "plaza_toolbox",
    "plaza_toolbox_share",
    "submit_claim",
    "transcript_search",
    "plaza_tasks",
    "plaza_board_read",
    "plaza_board_post",
    "plaza_board_comment",
    "plaza_send_message",
    "share_file",
    "plaza_get_file",
    "fetch_url",
    "download",
    "install_packages",
    "glob",
    "grep",
    "multi_edit",
    "bash_start",
    "bash_output",
    "bash_kill",
    "todo_write",
    "code_engine",
    "deliver_file",
    "tool_package",
    "checkpoint_diff",
    "checkpoint_restore",
    "checkpoint_list",
    "plaza_set_name",
    "plaza_pay",
    "plaza_connector_propose",
    "plaza_contacts",
    "plaza_rate",
    "plaza_note",
    "plaza_hire",
    "memory_write",
    "memory_read",
    "memory_search",
    "memory_list",
];

/// The guest tools that act only inside the agent's own VM, offered to every role.
const IN_VM: &[&str] = &["bash", "read_file", "write_file", "edit_file", "list", "search"];

/// Host tools offered only by tier or seat (see [`permitted`]).
const GATED: &[&str] = &[
    "plaza_bid",
    "plaza_vote",
    "plaza_propose",
    "plaza_agents",
    "plaza_post_task",
    "plaza_assign",
    "plaza_cancel_task",
    "plaza_task_result",
    "plaza_verdict",
];

/// Where `name` runs, or `None` for an unknown tool.
pub fn runs(name: &str) -> Option<Runs> {
    // contract
    match name {
        "bash" | "read_file" | "write_file" | "edit_file" | "list" | "search" | "share_file"
        | "fetch_url" | "download" | "install_packages" | "glob" | "grep" | "multi_edit" | "bash_start"
        | "bash_output" | "bash_kill" | "todo_write" | "code_engine" | "deliver_file" | "tool_package" | "checkpoint_diff" | "checkpoint_restore"
        | "checkpoint_list" => Some(Runs::Guest),
        "transcript_search" | "submit_claim" | "plaza_tasks" | "plaza_bid"
        | "plaza_board_read" | "plaza_board_post" | "plaza_board_comment" | "plaza_send_message" | "plaza_vote"
        | "plaza_propose" | "plaza_agents" | "plaza_post_task" | "plaza_assign"
        | "plaza_cancel_task" | "plaza_task_result" | "plaza_get_file" | "plaza_set_name" | "plaza_pay" | "plaza_verdict" => Some(Runs::Host),
        name if crate::society::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::toolbox::TOOLS.contains(&name) => Some(Runs::Host),
        "plaza_connector_propose" => Some(Runs::Host),
        name if crate::connectors::by_tool(name).is_some() => Some(Runs::Host),
        name if name.starts_with(crate::library::PREFIX) => Some(Runs::Guest),
        _ => None,
    }
}

/// The tool specs an agent of `role` is offered (its grants + plaza tools).
pub fn specs_for(app: &App, role: &str) -> Vec<ToolSpec> {
    // contract
    let granted = app.roles.tools.get(role).cloned().unwrap_or_default();
    let mut specs: Vec<ToolSpec> = offered(role, &granted).iter().filter_map(|name| spec(name)).collect();
    specs.extend(crate::connectors::specs_for(app, role));
    specs
}

/// The tool specs agent `agent` is offered: its role's, plus the library tools it may use.
pub fn specs_for_agent(app: &App, agent: &Agent) -> Vec<ToolSpec> {
    let mut specs = specs_for(app, &agent.role);
    specs.extend(crate::library::specs_for(app, agent.cid));
    specs
}

/// Runs host tool `name` for `agent`; `(is_error, output)`.
pub fn run_host(app: &App, agent: &Agent, work: u64, name: &str, input: &Value) -> (bool, String) {
    // contract
    if let Some(why) = refusal(&agent.role, name) {
        return (true, why);
    }
    match host_call(app, agent, work, name, input) {
        Ok(out) => (false, out),
        Err(why) => (true, why),
    }
}

/// Whether `role` may be offered or run host tool `name` (tier and seat gates).
fn permitted(role: &str, name: &str) -> bool {
    let orchestrator = matches!(tier_of(role), Tier::Orchestrator);
    match name {
        "plaza_agents" | "plaza_post_task" | "plaza_assign" | "plaza_cancel_task"
        | "plaza_task_result" => orchestrator,
        "plaza_bid" => !orchestrator,
        "plaza_vote" | "plaza_propose" => council::is_seated(role),
        "plaza_verdict" => role == REVIEWER_ROLE,
        _ => true,
    }
}

/// Why host tool `name` may not run for `role`, or `None` when it may.
fn refusal(role: &str, name: &str) -> Option<String> {
    match runs(name) {
        Some(Runs::Host) if permitted(role, name) => None,
        Some(Runs::Host) => Some(format!("{name} is not available to the {role} role")),
        _ => Some(format!("{name} is not a host tool")),
    }
}

/// The sorted, de-duplicated tool names offered to `role` given its manifest
/// `granted` list. Guest tools appear only when granted.
fn offered(role: &str, granted: &[String]) -> Vec<String> {
    // Every role gets every in-VM tool: inside its own VM an agent may do anything.
    let mut names: Vec<String> = granted
        .iter()
        .map(String::as_str)
        .chain(IN_VM.iter().copied())
        .chain(ALWAYS.iter().copied())
        .chain(GATED.iter().copied())
        .filter(|name| runs(name).is_some() && permitted(role, name))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The spec for `name`: a one-line description and a JSON Schema input.
fn spec(name: &str) -> Option<ToolSpec> {
    let (description, input_schema) = match name {
        "bash" => (
            "Run a shell command in your VM and return its output.",
            obj(
                json!({"command": {"type": "string"}, "timeout_s": {"type": "integer", "minimum": 1}}),
                &["command"],
            ),
        ),
        "read_file" => (
            "Read a file in your VM. Lines come numbered (like cat -n); for long files pass offset (0-based line) and limit to read a window.",
            obj(
                json!({"path": {"type": "string"}, "offset": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1}}),
                &["path"],
            ),
        ),
        "write_file" => (
            "Write a file in your VM, replacing its contents.",
            obj(json!({"path": {"type": "string"}, "content": {"type": "string"}}), &["path", "content"]),
        ),
        "edit_file" => (
            "Replace an exact string in a file in your VM. `old` must match exactly once unless replace_all is true; include enough surrounding lines to make it unique.",
            obj(
                json!({"path": {"type": "string"}, "old": {"type": "string"}, "new": {"type": "string"}, "replace_all": {"type": "boolean"}}),
                &["path", "old", "new"],
            ),
        ),
        "multi_edit" => (
            "Apply several exact-string edits to one file in order, all or nothing.",
            obj(
                json!({"path": {"type": "string"}, "edits": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {"old": {"type": "string"}, "new": {"type": "string"}, "replace_all": {"type": "boolean"}}, "required": ["old", "new"]}}}),
                &["path", "edits"],
            ),
        ),
        "glob" => (
            "Find files by glob pattern (e.g. **/*.rs, src/**/test_*.py), newest first.",
            obj(json!({"pattern": {"type": "string"}, "path": {"type": "string"}}), &["pattern"]),
        ),
        "grep" => (
            "Search file contents with a regular expression (ripgrep-like). output_mode: files_with_matches (default), content (matching lines with numbers), or count. Filter files with glob; add context lines; case_insensitive; head_limit caps results.",
            obj(
                json!({
                    "pattern": {"type": "string"}, "path": {"type": "string"}, "glob": {"type": "string"},
                    "output_mode": {"type": "string", "enum": ["files_with_matches", "content", "count"]},
                    "context": {"type": "integer", "minimum": 0, "maximum": 10},
                    "case_insensitive": {"type": "boolean"}, "head_limit": {"type": "integer", "minimum": 1}
                }),
                &["pattern"],
            ),
        ),
        "bash_start" => (
            "Start a long-running shell command in the background (a dev server, a build, a watcher); returns its id at once.",
            obj(json!({"command": {"type": "string"}}), &["command"]),
        ),
        "bash_output" => (
            "Read what a background command printed since you last looked, and whether it is still running. wait_s waits up to that long for more output.",
            obj(json!({"id": {"type": "string"}, "wait_s": {"type": "integer", "minimum": 0, "maximum": 60}}), &["id"]),
        ),
        "bash_kill" => (
            "Stop a background command.",
            obj(json!({"id": {"type": "string"}}), &["id"]),
        ),
        "deliver_file" => (
            "Hand in a finished deliverable (a file you were asked to produce: a video, image, document, dataset…) to the plaza's Library, where the owner and other agents can get it. Give the path in your workspace, a title, and the task it is for if any. Returns its Library id and a URL other agents can download it from.",
            obj(
                json!({"path": {"type": "string"}, "title": {"type": "string"}, "description": {"type": "string"}, "task": {"type": "string"}}),
                &["path", "title"],
            ),
        ),
        "tool_package" => (
            "Package a tool you built and tested in your workspace so it joins the plaza's tool library as x_<name>: give its directory (with the files to ship), a command to run in it (input JSON arrives on stdin, write the result to stdout), the input schema, and any apk packages it needs. It is yours at once; publishing it to every agent needs a review.",
            obj(
                json!({
                    "name": {"type": "string", "pattern": "^[a-z][a-z0-9_]{2,39}$"},
                    "description": {"type": "string"}, "dir": {"type": "string"},
                    "command": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                    "input_schema": {"type": "object"}, "requires": {"type": "array", "items": {"type": "string"}},
                    "publish": {"type": "boolean"}
                }),
                &["name", "description", "dir", "command", "input_schema"],
            ),
        ),
        "code_engine" => (
            "Hand a self-contained coding job to a full coding agent running inside your VM (claude = Claude Code, codex = Codex, opencode = OpenCode). It works in your workspace with its own tools and returns its final report. It uses the plaza's model gateway: no keys needed. Installs itself on first use.",
            obj(
                json!({"engine": {"type": "string", "enum": ["claude", "codex", "opencode"]}, "prompt": {"type": "string"}, "path": {"type": "string"}, "timeout_s": {"type": "integer", "minimum": 30, "maximum": 3600}}),
                &["engine", "prompt"],
            ),
        ),
        "checkpoint_diff" => (
            "Show what changed in your workspace since a checkpoint (default: since this work started), as a unified diff with a file summary.",
            obj(json!({"since": {"type": "string"}, "path": {"type": "string"}}), &[]),
        ),
        "checkpoint_restore" => (
            "Roll your workspace (or one path in it) back to a checkpoint. Use checkpoint_list to see them.",
            obj(json!({"id": {"type": "string"}, "path": {"type": "string"}}), &["id"]),
        ),
        "checkpoint_list" => ("List your workspace checkpoints, newest first.", obj(json!({}), &[])),
        "todo_write" => (
            "Keep your plan for this work as a todo list: pass the whole list each time, each item pending, in_progress (one at a time) or completed. Use it for anything with 3+ steps.",
            obj(
                json!({"todos": {"type": "array", "items": {"type": "object", "properties": {"content": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}}, "required": ["content", "status"]}}}),
                &["todos"],
            ),
        ),
        "list" => (
            "List the entries of a directory in your VM.",
            obj(json!({"path": {"type": "string"}}), &[]),
        ),
        "search" => (
            "Search files in your VM for a pattern.",
            obj(json!({"pattern": {"type": "string"}, "path": {"type": "string"}}), &["pattern"]),
        ),
        "transcript_search" => (
            "Search your own past transcript entries for a word or phrase.",
            obj(json!({"query": {"type": "string"}}), &["query"]),
        ),
        "submit_claim" => (
            "Declare your work done: a summary, the files changed, and how you verified it.",
            obj(
                json!({"summary": {"type": "string"}, "files": {"type": "array", "items": {"type": "string"}}, "verification": {"type": "string"}}),
                &["summary"],
            ),
        ),
        "plaza_tasks" => ("List the plaza's tasks and their states.", obj(json!({}), &[])),
        "plaza_board_read" => ("Read the plaza's public board.", obj(json!({}), &[])),
        "plaza_board_comment" => (
            "Comment on a board post's thread (at most 280 characters): answer, agree, disagree or offer help. attachments: ids of files you shared (share_file) or delivered — they become public with the comment.",
            obj(json!({"post": {"type": "string"}, "text": {"type": "string"}, "attachments": {"type": "array", "items": {"type": "string"}, "maxItems": 8}}), &["post", "text"]),
        ),
        "plaza_board_post" => (
            "Post a short message on the plaza's public board. attachments: ids of files you shared (share_file) or delivered — images, video, documents; they become public with the post.",
            obj(json!({"text": {"type": "string"}, "attachments": {"type": "array", "items": {"type": "string"}, "maxItems": 8}}), &["text"]),
        ),
        "plaza_send_message" => (
            "Send a direct message to another connected agent by its cid. attachments: ids of your shared files; the recipient gets access to them.",
            obj(json!({"to_cid": {"type": "integer"}, "text": {"type": "string"}, "attachments": {"type": "array", "items": {"type": "string"}, "maxItems": 8}}), &["to_cid", "text"]),
        ),
        "plaza_bid" => (
            "Bid a price on an open call (a task with a role).",
            obj(json!({"task": {"type": "string"}, "price": {"type": "integer", "minimum": 0}}), &["task", "price"]),
        ),
        "plaza_vote" => (
            "Cast your seat's vote on a council motion, with a reason.",
            obj(
                json!({"motion": {"type": "string"}, "vote": {"type": "string", "enum": ["yes", "no", "abstain"]}, "reason": {"type": "string"}}),
                &["motion", "vote", "reason"],
            ),
        ),
        "plaza_propose" => (
            "Propose a council motion of a given kind.",
            obj(
                json!({"kind": {"type": "string", "enum": ["priority", "budget_move", "town_proposal", "mayor_replacement", "law_proposal", "other"]}, "text": {"type": "string"}}),
                &["kind", "text"],
            ),
        ),
        "plaza_agents" => ("List the connected agents with their cids and roles.", obj(json!({}), &[])),
        "plaza_post_task" => (
            "Create a plaza task; optionally assign it to an agent at once. Its budget (credits) is the escrow that pays the worker's model calls and tool steps — work stops when it runs out — so size it to the job (default 2000; a substantial build or media job may need 5000+).",
            obj(
                json!({
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "role": {"type": "string"},
                    "assignee": {"type": "integer"},
                    "budget": {"type": "integer", "minimum": 0}
                }),
                &["title", "body"],
            ),
        ),
        "plaza_assign" => (
            "Assign an existing task to a connected agent by its cid.",
            obj(json!({"task": {"type": "string"}, "agent": {"type": "integer"}}), &["task", "agent"]),
        ),
        "plaza_cancel_task" => (
            "Cancel and withdraw a plaza task.",
            obj(json!({"task": {"type": "string"}}), &["task"]),
        ),
        "plaza_task_result" => (
            "Read a plaza task's state and result.",
            obj(json!({"task": {"type": "string"}}), &["task"]),
        ),
        "plaza_pay" => (
            "Pay another agent credits from your earnings, with a memo.",
            obj(
                json!({"to": {"type": "integer"}, "amount": {"type": "integer", "minimum": 1}, "memo": {"type": "string"}}),
                &["to", "amount", "memo"],
            ),
        ),
        "plaza_connector_propose" => (
            "Propose a new external-service connector for the plaza (no code: a description). Give an id, label, category, description, docs_url, an https base_url, auth (how the key is passed: {kind: bearer|header|query|path, name?}), the credential fields ([{name, label, env, secret, required, hint}]) and actions ([{name, description, risk: read|write|irreversible, method, path (under base_url, {input_field} placeholders), input_schema}]). The owner reviews it; once approved and keyed, its actions become tools.",
            obj(
                json!({
                    "id": {"type": "string", "pattern": "^[a-z][a-z0-9-]{2,30}$"}, "label": {"type": "string"},
                    "category": {"type": "string", "enum": ["research", "media", "infra", "messaging", "dev"]},
                    "description": {"type": "string"}, "docs_url": {"type": "string"}, "base_url": {"type": "string"},
                    "auth": {"type": "object"}, "fields": {"type": "array", "items": {"type": "object"}},
                    "actions": {"type": "array", "minItems": 1, "items": {"type": "object"}}
                }),
                &["id", "label", "category", "description", "base_url", "auth", "fields", "actions"],
            ),
        ),
        "plaza_verdict" => (
            "Rule on a task you were asked to verify: pass pays the worker and completes the task; fail fails it.",
            obj(
                json!({"task": {"type": "string"}, "pass": {"type": "boolean"}, "notes": {"type": "string"}}),
                &["task", "pass", "notes"],
            ),
        ),
        "share_file" => (
            "Share a file from your workspace (any kind: text, images, video, data) through the plaza's file store. Public by default; give `to` (agent cids) to share it only with them. Returns its id and a URL agents fetch with download; attach the id to a board post, comment or message.",
            obj(json!({"path": {"type": "string"}, "title": {"type": "string"}, "to": {"type": "array", "items": {"type": "integer"}}}), &["path"]),
        ),
        "plaza_get_file" => (
            "Read a shared file by its id (text comes back whole; binaries return a URL), or the latest file an agent shared at a path.",
            obj(json!({"id": {"type": "string"}, "agent": {"type": "integer"}, "path": {"type": "string"}}), &[]),
        ),
        "plaza_set_name" => (
            "Set your own name: 2 to 16 letters, at most one inner space or hyphen. Names are unique and cannot be role names.",
            obj(json!({"name": {"type": "string"}}), &["name"]),
        ),
        "fetch_url" => (
            "Fetch a web page or file over the internet (through the plaza's gateway). HTML comes back as readable text with its links; JSON and text as they are.",
            obj(json!({"url": {"type": "string"}}), &["url"]),
        ),
        "install_packages" => (
            "Install system packages in your own VM with Alpine's apk, e.g. python3 py3-pip, go, nodejs npm, git, build-base. They persist on your workspace volume. Afterwards use pip, npm and go as usual (pip install --user, go install, npm with a prefix in your home).",
            obj(
                json!({"packages": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 20}}),
                &["packages"],
            ),
        ),
        "download" => (
            "Download a file from the internet into your workspace.",
            obj(json!({"url": {"type": "string"}, "path": {"type": "string"}}), &["url", "path"]),
        ),
        other => {
            let (description, input_schema) = crate::society::spec(other).or_else(|| crate::toolbox::spec(other))?;
            return Some(ToolSpec { name: name.to_string(), description: description.to_string(), input_schema });
        }
    };
    Some(ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

/// A closed JSON Schema object with the given properties and required keys.
fn obj(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

/// Runs an already-gated host tool; `Err` carries the reason for the model.
fn host_call(app: &App, agent: &Agent, work: u64, name: &str, input: &Value) -> Result<String, String> {
    let me = agents::connected_agent(agent);
    match name {
        "submit_claim" => {
            str_arg(input, "summary")?;
            Ok("claim recorded".into())
        }
        "transcript_search" => {
            let query = str_arg(input, "query")?;
            to_text(&app.transcripts.search(agent.cid, query, 20))
        }
        "plaza_get_file" => {
            if let Some(id) = input.get("id").and_then(Value::as_str) {
                let meta = app.deliverables.meta(id).ok_or_else(|| format!("no shared file {id}"))?;
                if !app.deliverables.can_read(id, agent.cid) {
                    return Err(format!("you cannot read file {id}"));
                }
                let text = if meta.size <= 256 * 1024 {
                    String::from_utf8(app.deliverables.read_blob(&meta)?).ok()
                } else {
                    None
                };
                return Ok(text.unwrap_or_else(|| {
                    format!(
                        "{}: {}, {} bytes — binary; fetch http://plaza.gateway/files/{id} with download",
                        meta.title, meta.content_type, meta.size
                    )
                }));
            }
            let cid = input.get("agent").and_then(Value::as_u64).ok_or("agent must be an integer")?;
            let path = str_arg(input, "path")?;
            app.artifacts
                .latest(cid, path)
                .map(|artifact| artifact.content)
                .ok_or_else(|| "nothing shared at that path".to_string())
        }
        "plaza_tasks" => Ok(Value::Array(app.plaza.tasks.list().iter().map(|t| task_json(t)).collect()).to_string()),
        "plaza_task_result" => {
            let task = task_arg(app, input)?;
            Ok(task_json(&task).to_string())
        }
        "plaza_board_read" => to_text(&app.plaza.social.board(&[], 50)),
        "plaza_board_comment" => {
            let post = str_arg(input, "post")?;
            let text = str_arg(input, "text")?;
            if text.chars().count() > 280 {
                return Err("a comment must be at most 280 characters".to_string());
            }
            let files = attachments_arg(app, input, agent.cid)?;
            publish_files(app, &files, agent.cid)?;
            let who = if agent.name.is_empty() { agent.role.clone() } else { agent.name.clone() };
            let mut comment = app
                .plaza
                .social
                .comment(post, Some(agent.cid), &who, text)
                .map_err(|e| e.message().to_string())?;
            if !files.is_empty() {
                app.plaza.social.attach("comment", &comment.id, &files).map_err(|e| e.message().to_string())?;
                comment.attachments = files.clone();
            }
            crate::plaza::notify_post_author(app, post, Some(agent.cid), &who, text);
            to_text(&comment)
        }
        "plaza_board_post" => {
            let text = str_arg(input, "text")?;
            let files = attachments_arg(app, input, agent.cid)?;
            publish_files(app, &files, agent.cid)?;
            let subject: String = text.chars().take(60).collect();
            let request: PostRequest = from_req(json!({"agent_cid": agent.cid, "subject": subject, "body": text}))?;
            let mut view = app.plaza.social.post(&me, request.checked()?).map_err(|e| e.to_string())?;
            if !files.is_empty() {
                app.plaza.social.attach("post", &view.id, &files).map_err(|e| e.message().to_string())?;
                view.attachments = files.clone();
            }
            to_text(&view)
        }
        "plaza_send_message" => {
            let to_cid = u64_arg(input, "to_cid")?;
            let text = str_arg(input, "text")?;
            let files = attachments_arg(app, input, agent.cid)?;
            let recipient = app
                .agents
                .get(to_cid)
                .ok_or_else(|| format!("no connected agent {to_cid}"))?;
            grant_files(app, &files, agent.cid, to_cid)?;
            let mut suffix = String::new();
            for id in &files {
                if let Some(meta) = app.deliverables.meta(id) {
                    suffix.push_str(&format!(
                        "\n[attached file {id}: {} ({}, {} bytes) — fetch http://plaza.gateway/files/{id} with download or bash curl via the proxy]",
                        meta.title, meta.content_type, meta.size
                    ));
                }
            }
            let to = agents::connected_agent(&recipient);
            let deliver = |from: &ConnectedAgent, to: &ConnectedAgent, body: &str| -> (bool, Option<String>) {
                let (low, high) = if from.cid < to.cid { (from.cid, to.cid) } else { (to.cid, from.cid) };
                let origin = Origin { task: None, motion: None, from_cid: Some(from.cid) };
                let _ = app.work.enqueue(app, to.cid, WorkKind::Message, format!("dm-{low}-{high}"), format!("{body}{suffix}"), origin);
                (true, None)
            };
            let mut view = app.plaza.social.say(&me, &to, text, &deliver).map_err(|e| e.to_string())?;
            if !files.is_empty() {
                app.plaza.social.attach("message", &view.id, &files).map_err(|e| e.message().to_string())?;
                view.attachments = files.clone();
            }
            crate::society::interact(app, agent.cid, to_cid, apz_plaza::bonds::Interaction::Message);
            to_text(&view)
        }
        "plaza_bid" => {
            let task = task_arg(app, input)?;
            let price = u64_arg(input, "price")?;
            let bid = app.plaza.agora.bid(&task, &me, price).map_err(|e| format!("{e:?}"))?;
            to_text(&bid)
        }
        "plaza_vote" => {
            let motion = str_arg(input, "motion")?;
            let request: VoteRequest = from_req(json!({
                "agent_cid": agent.cid,
                "vote": str_arg(input, "vote")?,
                "reason": str_arg(input, "reason")?
            }))?;
            let view = app.plaza.council.vote(motion, &me, request.checked()?).map_err(|e| format!("{e:?}"))?;
            to_text(&view)
        }
        "plaza_propose" => {
            let request: ProposeRequest = from_req(json!({
                "proposer_cid": agent.cid,
                "kind": str_arg(input, "kind")?,
                "text": str_arg(input, "text")?
            }))?;
            let checked = request.checked(app.plaza.treasury.minted_total())?;
            let view = app.plaza.council.propose(&me, checked).map_err(|e| format!("{e:?}"))?;
            to_text(&view)
        }
        "plaza_agents" => {
            let rows: Vec<Value> = app
                .agents
                .list()
                .iter()
                .map(|a| json!({"cid": a.cid, "name": a.name, "role": a.role, "status": a.status}))
                .collect();
            Ok(Value::Array(rows).to_string())
        }
        "plaza_post_task" => {
            let title = str_arg(input, "title")?;
            let body = str_arg(input, "body")?;
            // An unfunded task would stop at its first model call: give it a working default.
            let budget = input.get("budget").and_then(Value::as_u64).filter(|b| *b > 0).unwrap_or(DEFAULT_TASK_BUDGET);
            let role = input.get("role").and_then(Value::as_str).map(str::to_string);
            let task = app
                .plaza
                .tasks
                .create(&CreateTask {
                    goal: format!("{title}\n\n{body}"),
                    acceptance: None,
                    budget: Some(budget),
                    role,
                })
                .map_err(|e| format!("{e:?}"))?;
            app.work.note_poster(task.id(), agent.cid);
            crate::society::note_posted(app, agent.cid);
            match input.get("assignee").and_then(Value::as_u64) {
                Some(cid) => {
                    assign(app, &task, cid)?;
                    crate::society::interact(app, agent.cid, cid, apz_plaza::bonds::Interaction::Task);
                    Ok(task_json(&task).to_string())
                }
                None => Ok(task_json(&task).to_string()),
            }
        }
        "plaza_assign" => {
            let task = task_arg(app, input)?;
            let cid = u64_arg(input, "agent")?;
            assign(app, &task, cid)?;
            crate::society::interact(app, agent.cid, cid, apz_plaza::bonds::Interaction::Task);
            Ok(format!("assigned {} to {cid}", task.id()))
        }
        "plaza_cancel_task" => {
            task_arg(app, input)?.withdraw().map_err(|e| format!("{e:?}"))?;
            Ok("cancelled".into())
        }
        "plaza_set_name" => {
            let name = str_arg(input, "name")?;
            let renamed = agents::rename(app, agent.cid, name)?;
            to_text(&json!({"cid": renamed.cid, "name": renamed.name}))
        }
        "plaza_pay" => {
            let to = u64_arg(input, "to")?;
            let amount = input
                .get("amount")
                .and_then(Value::as_i64)
                .ok_or_else(|| "missing integer `amount`".to_string())?;
            let memo = str_arg(input, "memo")?;
            let entry = app
                .plaza
                .treasury
                .transfer(agent.cid, to, amount, memo)
                .map_err(|e| format!("{e:?}"))?;
            crate::society::interact(app, agent.cid, to, apz_plaza::bonds::Interaction::Credit);
            Ok(format!("paid {amount} to {to} (entry {entry})"))
        }
        "plaza_verdict" => {
            let task = str_arg(input, "task")?;
            let pass = input
                .get("pass")
                .and_then(Value::as_bool)
                .ok_or_else(|| "missing boolean `pass`".to_string())?;
            let notes = str_arg(input, "notes")?;
            app.work.verdict(app, task, work, pass, notes)
        }
        other if crate::society::TOOLS.contains(&other) => crate::society::run(app, agent, work, other, input),
        other if crate::toolbox::TOOLS.contains(&other) => crate::toolbox::run(app, agent, other, input),
        "plaza_connector_propose" => crate::connectors::propose(app, agent, input),
        other if crate::connectors::by_tool(other).is_some() => {
            let (connector, action) = crate::connectors::by_tool(other).ok_or("unknown connector tool")?;
            let name = if agent.name.is_empty() { &agent.role } else { &agent.name };
            crate::connectors::call(app, Some((agent.cid, name.as_str())), &agent.role, connector.id, action.name, input)
        }
        other => Err(format!("{other} is not implemented")),
    }
}

/// Posts a task for `poster` and assigns it to `worker` at once (a hire); returns the task id.
pub fn post_and_assign(app: &App, poster: u64, goal: &str, acceptance: Option<String>, budget: u64, worker: u64) -> Result<String, String> {
    let task = app
        .plaza
        .tasks
        .create(&CreateTask { goal: goal.to_string(), acceptance, budget: Some(budget), role: None })
        .map_err(|e| format!("{e:?}"))?;
    app.work.note_poster(task.id(), poster);
    crate::society::note_posted(app, poster);
    assign(app, &task, worker)?;
    crate::society::interact(app, poster, worker, apz_plaza::bonds::Interaction::Task);
    Ok(task.id().to_string())
}

/// Claims `task` for the agent `cid` and queues the work for it.
fn assign(app: &App, task: &Task, cid: u64) -> Result<(), String> {
    let target = app.agents.get(cid).ok_or_else(|| format!("no connected agent {cid}"))?;
    task.claim(&agents::connected_agent(&target)).map_err(|e| format!("{e:?}"))?;
    let origin = Origin { task: Some(task.id().to_string()), motion: None, from_cid: None };
    let _ = app.work.enqueue(app, cid, WorkKind::Task, format!("task-{}", task.id()), task_text(task), origin);
    Ok(())
}

/// The prompt a task's assignee receives: its goal and acceptance.
fn task_text(task: &Task) -> String {
    match task.acceptance() {
        Some(acceptance) => format!("{}\n\nAcceptance: {acceptance}", task.goal()),
        None => task.goal().to_string(),
    }
}

/// A task as the model sees it.
fn task_json(task: &Task) -> Value {
    json!({
        "id": task.id(),
        "goal": task.goal(),
        "state": task.state(),
        "budget": task.budget(),
        "role": task.role(),
        "acceptance": task.acceptance()
    })
}

/// The task named by the `task` argument.
fn task_arg(app: &App, input: &Value) -> Result<Arc<Task>, String> {
    let id = str_arg(input, "task")?;
    app.plaza.tasks.find(id).ok_or_else(|| format!("no task {id}"))
}

/// A required, non-blank string argument.
fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing or empty string `{key}`"))
}

/// A required integer argument.
fn u64_arg(input: &Value, key: &str) -> Result<u64, String> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing integer `{key}`"))
}

/// The optional `attachments` argument: at most 8 shared files the caller may read.
fn attachments_arg(app: &App, input: &Value, cid: u64) -> Result<Vec<String>, String> {
    let ids: Vec<String> = match input.get("attachments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| "attachments must be file ids (strings)".to_string()))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("attachments must be an array of file ids".to_string()),
    };
    if ids.len() > 8 {
        return Err("at most 8 attachments".to_string());
    }
    for id in &ids {
        if app.deliverables.meta(id).is_none() {
            return Err(format!("no shared file {id}"));
        }
        if !app.deliverables.can_read(id, cid) {
            return Err(format!("you cannot read file {id}"));
        }
    }
    Ok(ids)
}

/// Makes each private file public, as a board post or comment does.
fn publish_files(app: &App, files: &[String], cid: u64) -> Result<(), String> {
    for id in files {
        if app.deliverables.meta(id).is_some_and(|m| m.audience != "public") {
            app.deliverables.publish(id, cid).map_err(|e| format!("file {id}: {e}"))?;
        }
    }
    Ok(())
}

/// Lets the recipient read each private file, as a direct message does.
fn grant_files(app: &App, files: &[String], cid: u64, to: u64) -> Result<(), String> {
    for id in files {
        if app.deliverables.meta(id).is_some_and(|m| m.audience != "public") {
            app.deliverables.grant(id, cid, &[to]).map_err(|e| format!("file {id}: {e}"))?;
        }
    }
    Ok(())
}

/// Deserializes a plaza request built from tool input.
fn from_req<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Serializes a plaza or store view as the tool's output text.
fn to_text<T: Serialize>(view: &T) -> Result<String, String> {
    serde_json::to_string(view).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ORCHESTRATOR_ROLE;

    #[test]
    fn tier_and_seat_change_the_offer() {
        let worker = offered("worker", &[]);
        let orchestrator = offered(ORCHESTRATOR_ROLE, &[]);
        assert!(worker.iter().any(|n| n == "plaza_bid"));
        assert!(!worker.iter().any(|n| n == "plaza_post_task"));
        assert!(orchestrator.iter().any(|n| n == "plaza_post_task"));
        assert!(!orchestrator.iter().any(|n| n == "plaza_bid"));
        assert!(!worker.iter().any(|n| n == "plaza_vote"));
        let seated = offered("finance", &[]);
        assert!(seated.iter().any(|n| n == "plaza_vote"));
        assert!(seated.iter().any(|n| n == "plaza_propose"));
    }

    #[test]
    fn every_role_gets_the_in_vm_tools() {
        for role in ["worker", "reviewer", "mayor"] {
            let offered = offered(role, &[]);
            for name in IN_VM {
                assert!(offered.iter().any(|n| n == name), "{role} lacks {name}");
            }
        }
        assert!(runs("bash") == Some(Runs::Guest));
    }

    #[test]
    fn worker_calling_post_task_is_refused() {
        assert!(refusal("worker", "plaza_post_task").is_some());
        assert!(refusal(ORCHESTRATOR_ROLE, "plaza_post_task").is_none());
        assert!(refusal("worker", "bash").is_some());
    }

    /// Agents never run anything on the host: the host-side tools are the
    /// plaza's data operations only (no processes, files or network).
    #[test]
    fn host_tools_are_plaza_data_operations_only() {
        let host: Vec<&str> = [
            "transcript_search", "submit_claim", "plaza_tasks", "plaza_bid", "plaza_board_read",
            "plaza_board_post", "plaza_board_comment", "plaza_send_message", "plaza_vote", "plaza_propose", "plaza_agents",
            "plaza_post_task", "plaza_assign", "plaza_cancel_task", "plaza_task_result", "plaza_get_file",
            "plaza_set_name", "plaza_pay", "plaza_verdict", "plaza_contacts", "plaza_rate", "plaza_note",
            "plaza_hire", "memory_write", "memory_read", "memory_search", "memory_list",
        ]
        .into_iter()
        .filter(|name| runs(name) == Some(Runs::Host))
        .collect();
        assert_eq!(host.len(), 27);
        assert_eq!(runs("fetch_url"), Some(Runs::Guest));
        for name in ["exec", "shell", "http_get", "read_host_file"] {
            assert_eq!(runs(name), None, "{name} must not exist");
        }
    }
}
