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
use crate::scheduler::Origin;
use crate::settings::{tier_of, Tier};
use crate::state::App;

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
    "submit_claim",
    "transcript_search",
    "plaza_tasks",
    "plaza_board_read",
    "plaza_board_post",
    "plaza_send_message",
    "share_file",
    "plaza_get_file",
    "fetch_url",
    "download",
];

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
];

/// Where `name` runs, or `None` for an unknown tool.
pub fn runs(name: &str) -> Option<Runs> {
    // contract
    match name {
        "bash" | "read_file" | "write_file" | "edit_file" | "list" | "search" | "share_file"
        | "fetch_url" | "download" => Some(Runs::Guest),
        "transcript_search" | "submit_claim" | "plaza_tasks" | "plaza_bid"
        | "plaza_board_read" | "plaza_board_post" | "plaza_send_message" | "plaza_vote"
        | "plaza_propose" | "plaza_agents" | "plaza_post_task" | "plaza_assign"
        | "plaza_cancel_task" | "plaza_task_result" | "plaza_get_file" => Some(Runs::Host),
        _ => None,
    }
}

/// The tool specs an agent of `role` is offered (its grants + plaza tools).
pub fn specs_for(app: &App, role: &str) -> Vec<ToolSpec> {
    // contract
    let granted = app.roles.tools.get(role).cloned().unwrap_or_default();
    offered(role, &granted).iter().filter_map(|name| spec(name)).collect()
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
    let mut names: Vec<String> = granted
        .iter()
        .map(String::as_str)
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
            "Read a file in your VM, optionally from a line offset for a line limit.",
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
            "Replace one exact occurrence of `old` with `new` in a file in your VM.",
            obj(
                json!({"path": {"type": "string"}, "old": {"type": "string"}, "new": {"type": "string"}}),
                &["path", "old", "new"],
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
        "plaza_board_post" => (
            "Post a short message on the plaza's public board.",
            obj(json!({"text": {"type": "string"}}), &["text"]),
        ),
        "plaza_send_message" => (
            "Send a direct message to another connected agent by its cid.",
            obj(json!({"to_cid": {"type": "integer"}, "text": {"type": "string"}}), &["to_cid", "text"]),
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
            "Create a plaza task; optionally assign it to an agent at once.",
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
        "share_file" => (
            "Publish a file from your workspace so other agents and the operator can read it.",
            obj(json!({"path": {"type": "string"}}), &["path"]),
        ),
        "plaza_get_file" => (
            "Read the latest file a plaza agent shared with share_file, by its cid and path.",
            obj(json!({"agent": {"type": "integer"}, "path": {"type": "string"}}), &["agent", "path"]),
        ),
        "fetch_url" => (
            "Fetch a web page or file over the internet (through the plaza's gateway) and return its text.",
            obj(json!({"url": {"type": "string"}}), &["url"]),
        ),
        "download" => (
            "Download a file from the internet into your workspace.",
            obj(json!({"url": {"type": "string"}, "path": {"type": "string"}}), &["url", "path"]),
        ),
        _ => return None,
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
fn host_call(app: &App, agent: &Agent, _work: u64, name: &str, input: &Value) -> Result<String, String> {
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
        "plaza_board_post" => {
            let text = str_arg(input, "text")?;
            let subject: String = text.chars().take(60).collect();
            let request: PostRequest = from_req(json!({"agent_cid": agent.cid, "subject": subject, "body": text}))?;
            let view = app.plaza.social.post(&me, request.checked()?).map_err(|e| e.to_string())?;
            to_text(&view)
        }
        "plaza_send_message" => {
            let to_cid = u64_arg(input, "to_cid")?;
            let text = str_arg(input, "text")?;
            let recipient = app
                .agents
                .get(to_cid)
                .ok_or_else(|| format!("no connected agent {to_cid}"))?;
            let to = agents::connected_agent(&recipient);
            let deliver = |from: &ConnectedAgent, to: &ConnectedAgent, body: &str| -> (bool, Option<String>) {
                let (low, high) = if from.cid < to.cid { (from.cid, to.cid) } else { (to.cid, from.cid) };
                let origin = Origin { task: None, motion: None, from_cid: Some(from.cid) };
                let _ = app.work.enqueue(app, to.cid, WorkKind::Message, format!("dm-{low}-{high}"), body.to_string(), origin);
                (true, None)
            };
            let view = app.plaza.social.say(&me, &to, text, &deliver).map_err(|e| e.to_string())?;
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
            let budget = input.get("budget").and_then(Value::as_u64).unwrap_or(0);
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
            match input.get("assignee").and_then(Value::as_u64) {
                Some(cid) => {
                    assign(app, &task, cid)?;
                    Ok(task_json(&task).to_string())
                }
                None => Ok(task_json(&task).to_string()),
            }
        }
        "plaza_assign" => {
            let task = task_arg(app, input)?;
            let cid = u64_arg(input, "agent")?;
            assign(app, &task, cid)?;
            Ok(format!("assigned {} to {cid}", task.id()))
        }
        "plaza_cancel_task" => {
            task_arg(app, input)?.withdraw().map_err(|e| format!("{e:?}"))?;
            Ok("cancelled".into())
        }
        other => Err(format!("{other} is not implemented")),
    }
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
    fn ungranted_guest_tool_is_not_offered() {
        assert!(!offered("worker", &[]).iter().any(|n| n == "bash"));
        let granted = offered("worker", &["bash".to_string()]);
        assert!(granted.iter().any(|n| n == "bash"));
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
            "plaza_board_post", "plaza_send_message", "plaza_vote", "plaza_propose", "plaza_agents",
            "plaza_post_task", "plaza_assign", "plaza_cancel_task", "plaza_task_result", "plaza_get_file",
        ]
        .into_iter()
        .filter(|name| runs(name) == Some(Runs::Host))
        .collect();
        assert_eq!(host.len(), 15);
        assert_eq!(runs("fetch_url"), Some(Runs::Guest));
        for name in ["exec", "shell", "http_get", "read_host_file"] {
            assert_eq!(runs(name), None, "{name} must not exist");
        }
    }
}
