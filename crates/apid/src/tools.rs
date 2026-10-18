//! The tools agents are offered: their specs, who runs them, and the host
//! tools `apid` runs itself.
//!
//! OWNER: slice "loop". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::sync::Arc;

use apz_plaza::ConnectedAgent;
use apz_plaza::council::{ProposeRequest, VoteRequest};
use apz_plaza::social::PostRequest;
use apz_plaza::tasks::{CreateTask, MIN_TASK_BUDGET, Task, resolve_budget};
use apz_proto::chat::ToolSpec;
use apz_proto::wire::WorkKind;
use serde::Serialize;
use serde_json::{Value, json};

use crate::agents::{self, Agent};
use crate::scheduler::Origin;
use crate::settings::tier_of;
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
    "plaza_toolbox",
    "plaza_toolbox_share",
    "plaza_projects",
    "plaza_project_attach",
    "plaza_project_critique",
    "plaza_git",
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
    "plaza_skills",
    "plaza_skill_read",
];

/// The guest tools that act only inside the agent's own VM, offered to every role.
const IN_VM: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "edit_file",
    "list",
    "search",
];

/// Host tools offered only by tier or seat (see [`permitted`]).
const GATED: &[&str] = &[
    "plaza_agent_propose",
    "plaza_project_open",
    "plaza_project_advance",
    "plaza_project_approve",
    "plaza_agent_control",
    "plaza_loop",
    "plaza_bid",
    "plaza_vote",
    "plaza_propose",
    "plaza_agents",
    "plaza_money",
    "plaza_post_task",
    "plaza_tasks_post",
    "plaza_task_link",
    "plaza_assign",
    "plaza_cancel_task",
    "plaza_task_result",
    "plaza_verdict",
    "foundry_datasets",
    "foundry_export",
    "foundry_submit_job",
    "foundry_job_status",
    "foundry_request_shadow",
    "foundry_hf_search",
    "invoke_model",
];

/// Where `name` runs, or `None` for an unknown tool.
pub fn runs(name: &str) -> Option<Runs> {
    // contract
    match name {
        "bash" | "read_file" | "write_file" | "edit_file" | "list" | "search" | "share_file"
        | "fetch_url" | "download" | "install_packages" | "glob" | "grep" | "multi_edit"
        | "bash_start" | "bash_output" | "bash_kill" | "todo_write" | "code_engine"
        | "deliver_file" | "tool_package" | "checkpoint_diff" | "checkpoint_restore"
        | "checkpoint_list" => Some(Runs::Guest),
        "transcript_search"
        | "submit_claim"
        | "plaza_tasks"
        | "plaza_bid"
        | "plaza_board_read"
        | "plaza_board_post"
        | "plaza_board_comment"
        | "plaza_send_message"
        | "plaza_vote"
        | "plaza_propose"
        | "plaza_agents"
        | "plaza_money"
        | "plaza_post_task"
        | "plaza_tasks_post"
        | "plaza_task_link"
        | "plaza_assign"
        | "plaza_cancel_task"
        | "plaza_task_result"
        | "plaza_get_file"
        | "plaza_set_name"
        | "plaza_pay"
        | "plaza_verdict" => Some(Runs::Host),
        name if crate::skills::agent_tools::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::society::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::toolbox::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::projects::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::harness_control::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::agent_proposals::TOOLS.contains(&name) => Some(Runs::Host),
        "plaza_git" => Some(Runs::Host),
        name if crate::stations::tools::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::foundry::breeder::TOOLS.contains(&name) => Some(Runs::Host),
        "plaza_connector_propose" => Some(Runs::Host),
        name if crate::connectors::by_tool(name).is_some() => Some(Runs::Host),
        name if name.starts_with(crate::mcp::PREFIX) => Some(Runs::Host),
        name if name.starts_with(crate::library::PREFIX) => Some(Runs::Guest),
        _ => None,
    }
}

/// The tool specs an agent of `role` is offered (its grants + plaza tools).
pub fn specs_for(app: &App, role: &str) -> Vec<ToolSpec> {
    // contract
    let granted = crate::roles_custom::tools_of(app, role);
    let mut specs: Vec<ToolSpec> = offered(role, &granted)
        .iter()
        .filter_map(|name| spec(name))
        .collect();
    // A connector tool is offered only when the laws permit it (C-004a holds back the irreversible ones).
    specs.extend(
        crate::connectors::specs_for(app, role)
            .into_iter()
            .filter(|spec| permitted(role, &spec.name)),
    );
    specs
}

/// The tool specs agent `agent` is offered: its role's, plus the library tools it may use.
pub fn specs_for_agent(app: &App, agent: &Agent) -> Vec<ToolSpec> {
    let mut specs = specs_for(app, &agent.role);
    specs.extend(crate::library::specs_for(app, agent.cid));
    specs.extend(crate::mcp::specs_for_agent(app, agent));
    // The engines offered are the owner's enabled ones (`engines_enabled`).
    let enabled = app.settings.get().engines_enabled;
    for spec in specs.iter_mut().filter(|spec| spec.name == "code_engine") {
        restrict_engines(&mut spec.input_schema, &enabled);
    }
    specs
}

/// Narrows a `code_engine` schema's `engine` enum to the owner's enabled engines.
fn restrict_engines(schema: &mut Value, enabled: &[String]) {
    schema["properties"]["engine"]["enum"] = json!(enabled);
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

/// The `model` a `code_engine` call asks for, checked against the calling agent's tier through
/// `Settings::allowed`, the rule the model gateway enforces on every call: `None` keeps the engine's
/// default (no `model`, or an empty one). Called by the guest arm of `internal::dispatch`, before the
/// call is forwarded to the VM.
pub(crate) fn code_engine_model(
    app: &App,
    role: &str,
    input: &Value,
) -> Result<Option<String>, String> {
    let Some(model) = model_input(input)? else {
        model_required(input)?;
        return Ok(None);
    };
    engine_provider(input, model)?;
    // Auggie's `augment/` models too: workers may run any model, and the orchestrator only the ones
    // the owner listed for it, Augment's own included.
    app.settings.allowed(tier_of(role), model)?;
    Ok(Some(model.to_string()))
}

/// Refuses a call without a model where its engine needs one: Cline is configured with its model
/// before the call, so it has no default to fall back on.
fn model_required(input: &Value) -> Result<(), String> {
    match input.get("engine").and_then(Value::as_str) {
        Some("cline") => Err("invalid input: Cline needs \"model\"".to_string()),
        Some("kimi") => Err(
            "invalid input: Kimi Code needs \"model\": its provider and model come from it"
                .to_string(),
        ),
        Some("copilot") => Err(
            "invalid input: Copilot needs \"model\": its bring-your-own-key mode names the model"
                .to_string(),
        ),
        Some("goose") => Err(
            "invalid input: Goose needs \"model\": its provider and model come from it".to_string(),
        ),
        Some("vibe") => {
            Err("invalid input: Vibe needs \"model\": its config names the model".to_string())
        }
        _ => Ok(()),
    }
}

/// The provider an engine's model must name. Gemini CLI reaches only the gateway's Gemini route, so
/// `gemini/<model>` is the one shape it takes. Qwen Code speaks chat completions and takes the three
/// routes that serve them. Cline takes the four routes its setup maps (Anthropic-shaped
/// `anthropic/` and `openrouter/`, and `openai/` and `opencode/`). Kilo Code takes what OpenCode
/// takes, so it has no rule here (the route checks the provider's shape). GitHub Copilot takes the
/// four routes its bring-your-own-key mode maps, and Goose the four its providers map. Kimi Code takes
/// the four routes its provider mode maps, Auggie only its own `augment/` models, and Mistral Vibe the
/// three its config maps. The other
/// engines keep their own routes.
fn engine_provider(input: &Value, model: &str) -> Result<(), String> {
    let provider = model.split_once('/').map(|(provider, _)| provider);
    match input.get("engine").and_then(Value::as_str) {
        Some("gemini") if !model.starts_with("gemini/") => Err(format!(
            "invalid input: \"model\" is {model:?}; Gemini CLI takes gemini/<model> models"
        )),
        Some("qwen") if !matches!(provider, Some("openai" | "openrouter" | "opencode")) => {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Qwen Code takes openai/<model>, openrouter/<model> or opencode/<model> models"
            ))
        }
        Some("cline")
            if !matches!(
                provider,
                Some("anthropic" | "openrouter" | "openai" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Cline takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ))
        }
        Some("copilot")
            if !matches!(
                provider,
                Some("anthropic" | "openai" | "openrouter" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Copilot takes anthropic/..., openai/..., openrouter/... or opencode/... models"
            ))
        }
        Some("vibe") if !matches!(provider, Some("openai" | "openrouter" | "opencode")) => {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Vibe takes openai/..., openrouter/... or opencode/... models"
            ))
        }
        Some("goose")
            if !matches!(
                provider,
                Some("anthropic" | "openrouter" | "openai" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Goose takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ))
        }
        Some("kimi")
            if !matches!(
                provider,
                Some("anthropic" | "openrouter" | "openai" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Kimi Code takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ))
        }
        Some("auggie") if provider != Some("augment") => Err(format!(
            "invalid input: \"model\" is {model:?}; Auggie runs Augment's own models: use augment/<model>"
        )),
        _ => Ok(()),
    }
}

/// Refuses a `code_engine` call that names an engine the owner has not enabled (`engines_enabled`).
/// The guest arm of `internal::dispatch` calls it beside `code_engine_model`.
pub(crate) fn code_engine_enabled(app: &App, input: &Value) -> Result<(), String> {
    engine_enabled(&app.settings.get().engines_enabled, input)
}

/// Whether the `engine` named by `input` is one of `enabled`.
fn engine_enabled(enabled: &[String], input: &Value) -> Result<(), String> {
    let engine = input
        .get("engine")
        .and_then(Value::as_str)
        .ok_or("invalid input: \"engine\" must name an engine")?;
    if enabled.iter().any(|id| id == engine) {
        Ok(())
    } else {
        Err(format!("the owner has not enabled the {engine} engine"))
    }
}

/// The shape of the `model` input: `None` when absent or empty, else the trimmed `<provider>/<model>`
/// (the `@effort` suffix allowed).
fn model_input(input: &Value) -> Result<Option<&str>, String> {
    let model = match input.get("model") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(model)) if model.trim().is_empty() => return Ok(None),
        Some(Value::String(model)) => model.trim(),
        Some(_) => return Err("invalid input: \"model\" must be a string".to_string()),
    };
    let id = model.split_once('@').map_or(model, |(id, _)| id);
    if !id.contains('/') {
        return Err(format!(
            "invalid input: \"model\" must be <provider>/<model>, not {model:?}"
        ));
    }
    Ok(Some(model))
}

/// Whether `role` may be offered or run tool `name` (host or guest): the JHRL laws decide (`tools.jhrl`).
fn permitted(role: &str, name: &str) -> bool {
    crate::laws::global().permits(role, name)
}

/// Why host tool `name` may not run for `role`, or `None` when it may.
fn refusal(role: &str, name: &str) -> Option<String> {
    match runs(name) {
        Some(Runs::Host) if permitted(role, name) => None,
        Some(Runs::Host) => Some(format!("{name} is not available to the {role} role")),
        _ => Some(format!("{name} is not a host tool")),
    }
}

/// The tool names offered to `role` with no manifest grants; the law tests check each one.
#[cfg(test)]
pub(crate) fn offered_names(role: &str) -> Vec<String> {
    offered(role, &[])
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
    if let Some(tool) = crate::foundry::breeder::spec(name) {
        return Some(tool);
    }
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
            obj(
                json!({"path": {"type": "string"}, "content": {"type": "string"}}),
                &["path", "content"],
            ),
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
            obj(
                json!({"pattern": {"type": "string"}, "path": {"type": "string"}}),
                &["pattern"],
            ),
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
            obj(
                json!({"id": {"type": "string"}, "wait_s": {"type": "integer", "minimum": 0, "maximum": 60}}),
                &["id"],
            ),
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
            "Hand a self-contained coding job to a full coding agent running inside your VM (claude = Claude Code, codex = Codex, opencode = OpenCode, gemini = Gemini CLI, qwen = Qwen Code, kilo = Kilo Code, cline = Cline, copilot = GitHub Copilot, goose = Goose, kimi = Kimi Code, auggie = Auggie, vibe = Mistral Vibe). It works in your workspace with its own tools and returns its final report. It streams live: each action the engine takes is checked by the plaza's laws, one by one. It can be continued with `session`. It uses the plaza's model gateway: no keys needed. Installs itself on first use.",
            obj(
                json!({"engine": {"type": "string", "enum": ["claude", "codex", "opencode", "gemini", "qwen", "kilo", "cline", "copilot", "goose", "kimi", "auggie", "vibe"]}, "prompt": {"type": "string"}, "path": {"type": "string"}, "timeout_s": {"type": "integer", "minimum": 30, "maximum": 3600}, "model": {"type": "string", "description": "The model to run the engine on, as `<provider>/<model>` (for example `openrouter/anthropic/claude-sonnet-5-5` or `opencode/claude-sonnet-5-5`; Gemini CLI takes `gemini/<model>`, Qwen Code `openai/`, `openrouter/` or `opencode/` models, Cline `anthropic/`, `openrouter/`, `openai/` or `opencode/` models and needs one, GitHub Copilot `anthropic/`, `openai/`, `openrouter/` or `opencode/` models and needs one, Goose `anthropic/`, `openrouter/`, `openai/` or `opencode/` models and needs one, Kimi Code `anthropic/`, `openrouter/`, `openai/` or `opencode/` models and needs one, Auggie `augment/<model>` models, Mistral Vibe `openai/`, `openrouter/` or `opencode/` models and needs one). Omit it to use the default."}, "session": {"type": "string", "description": "Optional. Continue an earlier engine session: the id its summary gave."}, "mode": {"type": "string", "description": "Optional. A session mode the engine offers (e.g. Claude Code's default, acceptEdits, plan); bypass modes are never used."}}),
                &["engine", "prompt"],
            ),
        ),
        "checkpoint_diff" => (
            "Show what changed in your workspace since a checkpoint (default: since this work started), as a unified diff with a file summary.",
            obj(
                json!({"since": {"type": "string"}, "path": {"type": "string"}}),
                &[],
            ),
        ),
        "checkpoint_restore" => (
            "Roll your workspace (or one path in it) back to a checkpoint. Use checkpoint_list to see them.",
            obj(
                json!({"id": {"type": "string"}, "path": {"type": "string"}}),
                &["id"],
            ),
        ),
        "checkpoint_list" => (
            "List your workspace checkpoints, newest first.",
            obj(json!({}), &[]),
        ),
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
            obj(
                json!({"pattern": {"type": "string"}, "path": {"type": "string"}}),
                &["pattern"],
            ),
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
        "plaza_tasks" => (
            "List the plaza's tasks and their states.",
            obj(json!({}), &[]),
        ),
        "plaza_board_read" => ("Read the plaza's public board.", obj(json!({}), &[])),
        "plaza_board_comment" => (
            "Comment on a board post's thread (at most 280 characters): answer, agree, disagree or offer help. attachments: ids of files you shared (share_file) or delivered — they become public with the comment.",
            obj(
                json!({"post": {"type": "string"}, "text": {"type": "string"}, "attachments": {"type": "array", "items": {"type": "string"}, "maxItems": 8}}),
                &["post", "text"],
            ),
        ),
        "plaza_board_post" => (
            "Post a short message on the plaza's public board. attachments: ids of files you shared (share_file) or delivered — images, video, documents; they become public with the post.",
            obj(
                json!({"text": {"type": "string"}, "attachments": {"type": "array", "items": {"type": "string"}, "maxItems": 8}}),
                &["text"],
            ),
        ),
        "plaza_send_message" => (
            "Send a direct message to another connected agent by its cid. attachments: ids of your shared files; the recipient gets access to them.",
            obj(
                json!({"to_cid": {"type": "integer"}, "text": {"type": "string"}, "attachments": {"type": "array", "items": {"type": "string"}, "maxItems": 8}}),
                &["to_cid", "text"],
            ),
        ),
        "plaza_bid" => (
            "Bid a price on an open call (a task with a role).",
            obj(
                json!({"task": {"type": "string"}, "price": {"type": "integer", "minimum": 0}}),
                &["task", "price"],
            ),
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
        "plaza_agents" => (
            "List the connected agents with their cids and roles.",
            obj(json!({}), &[]),
        ),
        "plaza_money" => (
            "Your wallet, and any task's money: budget, escrow, spend, payouts, refunds.",
            obj(json!({"task": {"type": "string"}}), &[]),
        ),
        "plaza_post_task" => (
            "Create a plaza task; optionally assign it to an agent at once. Its budget (credits) is minted from the plaza's budget into the task's own escrow — it never touches your wallet or your daily transfer cap. The escrow pays the worker's model calls and tool steps (work stops when it runs out), so size it to the job (default 2000, minimum 200; a substantial build or media job may need 5000+). Give `acceptance`: criteria a reviewer can check. For a project's work give `project`; tasks can be posted only once its plan is approved. `after` names the ids of tasks it runs after: it is handed out once they are all done (so it cannot be assigned before). To post a plan's whole list, use plaza_tasks_post.",
            obj(
                json!({
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "role": {"type": "string"},
                    "assignee": {"type": "integer"},
                    "budget": {"type": "integer", "minimum": MIN_TASK_BUDGET},
                    "acceptance": {"type": "string"},
                    "project": {"type": "string"},
                    "after": {"type": "array", "items": {"type": "string"}, "maxItems": 32}
                }),
                &["title", "body"],
            ),
        ),
        "plaza_tasks_post" => (
            "Post a project's whole task list at once, once its plan is approved: each task with its key from the plan's tasks list, its goal, acceptance, role and budget, and `after`: the keys of the tasks of this list it runs after (or ids of tasks posted before). Every task is created or none is, and each goes to a worker as soon as everything it runs after is done. Answers with each key's task id.",
            obj(
                json!({
                    "project": {"type": "string"},
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": crate::task_plan::MAX_PLANNED,
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": {"type": "string"},
                                "goal": {"type": "string"},
                                "acceptance": {"type": "string"},
                                "role": {"type": "string"},
                                "budget": {"type": "integer", "minimum": MIN_TASK_BUDGET},
                                "after": {"type": "array", "items": {"type": "string"}, "maxItems": 32}
                            },
                            "required": ["key", "goal"],
                            "additionalProperties": false
                        }
                    }
                }),
                &["project", "tasks"],
            ),
        ),
        "plaza_task_link" => (
            "Change which tasks an open task runs after, to replan: give the whole new list of task ids ([] for none). Refused when it would close a cycle (the refusal names it) or the task is no longer open.",
            obj(
                json!({
                    "task": {"type": "string"},
                    "after": {"type": "array", "items": {"type": "string"}, "maxItems": 32}
                }),
                &["task", "after"],
            ),
        ),
        "plaza_assign" => (
            "Assign an existing task to a connected agent by its cid.",
            obj(
                json!({"task": {"type": "string"}, "agent": {"type": "integer"}}),
                &["task", "agent"],
            ),
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
                &[
                    "id",
                    "label",
                    "category",
                    "description",
                    "base_url",
                    "auth",
                    "fields",
                    "actions",
                ],
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
            obj(
                json!({"path": {"type": "string"}, "title": {"type": "string"}, "to": {"type": "array", "items": {"type": "integer"}}}),
                &["path"],
            ),
        ),
        "plaza_get_file" => (
            "Read a shared file by its id (text comes back whole; binaries return a URL).",
            obj(json!({"id": {"type": "string"}}), &["id"]),
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
            obj(
                json!({"url": {"type": "string"}, "path": {"type": "string"}}),
                &["url", "path"],
            ),
        ),
        other => {
            let (description, input_schema) = crate::society::spec(other)
                .or_else(|| crate::toolbox::spec(other))
                .or_else(|| crate::projects::spec(other))
                .or_else(|| crate::harness_control::spec(other))
                .or_else(|| crate::git::spec(other))
                .or_else(|| crate::agent_proposals::spec(other))
                .or_else(|| crate::skills::agent_tools::spec(other))
                .or_else(|| crate::stations::tools::spec(other))?;
            return Some(ToolSpec {
                name: name.to_string(),
                description: description.to_string(),
                input_schema,
            });
        }
    };
    Some(ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

/// The rules a `plaza_money` report ends with (the system prompt's "How money works", minus its last sentence).
const HOW_MONEY_WORKS: &str = "How money works: a task's budget is minted from the plaza's budget into the task's own escrow when \
     it is posted (plaza_post_task or plaza_hire) — no wallet is debited and no transfer cap applies. The \
     worker's real model and tool costs are charged to that escrow as it works; when the escrow runs out the \
     work stops. When a reviewer passes the work, the worker is paid from the escrow (a margin on the real cost, \
     at most 20%); whatever is left goes back to the plaza's budget. A failed or unreviewed task pays the worker \
     nothing: its remaining escrow is refunded to the plaza, and the cost already spent stays spent. A plaza_hire \
     bonus comes from the hirer's own wallet and is paid only if the work passes. Your wallet is only debited by \
     what you send yourself (plaza_pay, hire bonuses), up to 1000 credits a day.";

/// The plain-text `plaza_money` report: the caller's wallet, an optional task's
/// money (`(id, state, money)`), then the rules.
fn money_report(
    cid: u64,
    wallet: &apz_plaza::treasury::WalletMoney,
    task: Option<(&str, &str, &apz_plaza::treasury::TaskMoney)>,
) -> String {
    let mut lines = vec![format!(
        "your wallet (cid:{cid}): balance {} credits; sent today {} of the {} daily cap; earned {} from task payments",
        wallet.balance,
        wallet.sent_today,
        apz_plaza::treasury::TRANSFER_DAILY_CAP,
        wallet.earned
    )];
    if let Some((id, state, money)) = task {
        lines.push(format!("task {id}: state {state}"));
        lines.push(format!("budget (minted into escrow): {}", money.minted));
        lines.push(format!("escrow balance now: {}", money.escrow));
        lines.push(format!("real spend charged so far: {}", money.spent));
        if money.paid.is_empty() {
            lines.push("paid to agents: none yet".to_string());
        } else {
            let paid: Vec<String> = money
                .paid
                .iter()
                .map(|(cid, amount)| format!("cid:{cid} {amount}"))
                .collect();
            lines.push(format!("paid to agents: {}", paid.join(", ")));
        }
        lines.push(format!("refunded to the plaza: {}", money.refunded));
    }
    lines.push(String::new());
    lines.push(HOW_MONEY_WORKS.to_string());
    lines.join("\n")
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
fn host_call(
    app: &App,
    agent: &Agent,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    let me = agents::connected_agent(agent);
    match name {
        name if crate::foundry::breeder::TOOLS.contains(&name) => {
            crate::foundry::breeder::run(app, agent, name, input)
        }
        "submit_claim" => {
            str_arg(input, "summary")?;
            Ok("claim recorded".into())
        }
        "transcript_search" => {
            let query = str_arg(input, "query")?;
            to_text(&app.transcripts.search(agent.cid, query, 20))
        }
        "plaza_get_file" => {
            let id = str_arg(input, "id")?;
            let meta = app
                .deliverables
                .meta(id)
                .ok_or_else(|| format!("no shared file {id}"))?;
            if !app.deliverables.can_read(id, agent.cid) {
                return Err(format!("you cannot read file {id}"));
            }
            let text = if meta.size <= 256 * 1024 {
                String::from_utf8(app.deliverables.read_blob(&meta)?).ok()
            } else {
                None
            };
            Ok(text.unwrap_or_else(|| {
                format!(
                    "{}: {}, {} bytes — binary; fetch http://plaza.gateway/files/{id} with download",
                    meta.title, meta.content_type, meta.size
                )
            }))
        }
        "plaza_tasks" => Ok(Value::Array(
            app.plaza
                .tasks
                .list()
                .iter()
                .map(|t| task_json(t))
                .collect(),
        )
        .to_string()),
        "plaza_task_result" => {
            let task = task_arg(app, input)?;
            Ok(task_json(&task).to_string())
        }
        "plaza_money" => {
            let wallet = app.plaza.treasury.wallet_money(agent.cid);
            let Some(task) = input.get("task").and_then(Value::as_str) else {
                return Ok(money_report(agent.cid, &wallet, None));
            };
            let record = app
                .plaza
                .tasks
                .find(task)
                .ok_or_else(|| format!("no task {task}"))?;
            let json = task_json(&record);
            let state = json["state"]
                .as_str()
                .map_or_else(|| json["state"].to_string(), str::to_string);
            let money = app.plaza.treasury.task_money(task);
            Ok(money_report(
                agent.cid,
                &wallet,
                Some((task, &state, &money)),
            ))
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
            let who = if agent.name.is_empty() {
                agent.role.clone()
            } else {
                agent.name.clone()
            };
            let mut comment = app
                .plaza
                .social
                .comment(post, Some(agent.cid), &who, text)
                .map_err(|e| e.message().to_string())?;
            if !files.is_empty() {
                app.plaza
                    .social
                    .attach("comment", &comment.id, &files)
                    .map_err(|e| e.message().to_string())?;
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
            let request: PostRequest =
                from_req(json!({"agent_cid": agent.cid, "subject": subject, "body": text}))?;
            // The post is on the author's project's board (the orchestrator's on the whole board).
            let mut post = request.checked()?;
            post.project = app.members.project_of(agent.cid);
            let mut view = app
                .plaza
                .social
                .post(&me, post)
                .map_err(|e| e.to_string())?;
            if !files.is_empty() {
                app.plaza
                    .social
                    .attach("post", &view.id, &files)
                    .map_err(|e| e.message().to_string())?;
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
            may_message(app, agent, &recipient)?;
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
            let deliver = |from: &ConnectedAgent,
                           to: &ConnectedAgent,
                           body: &str|
             -> (bool, Option<String>) {
                let (low, high) = if from.cid < to.cid {
                    (from.cid, to.cid)
                } else {
                    (to.cid, from.cid)
                };
                let origin = Origin {
                    from_cid: Some(from.cid),
                    ..Origin::default()
                };
                let _ = app.work.enqueue(
                    app,
                    to.cid,
                    WorkKind::Message,
                    format!("dm-{low}-{high}"),
                    format!("{body}{suffix}"),
                    origin,
                );
                (true, None)
            };
            let mut view = app
                .plaza
                .social
                .say(&me, &to, text, &deliver)
                .map_err(|e| e.to_string())?;
            if !files.is_empty() {
                app.plaza
                    .social
                    .attach("message", &view.id, &files)
                    .map_err(|e| e.message().to_string())?;
                view.attachments = files.clone();
            }
            crate::society::interact(
                app,
                agent.cid,
                to_cid,
                apz_plaza::bonds::Interaction::Message,
            );
            to_text(&view)
        }
        "plaza_bid" => {
            let task = task_arg(app, input)?;
            let price = u64_arg(input, "price")?;
            let bid = app
                .plaza
                .agora
                .bid(&task, &me, price)
                .map_err(|e| format!("{e:?}"))?;
            to_text(&bid)
        }
        "plaza_vote" => {
            let motion = str_arg(input, "motion")?;
            let request: VoteRequest = from_req(json!({
                "agent_cid": agent.cid,
                "vote": str_arg(input, "vote")?,
                "reason": str_arg(input, "reason")?
            }))?;
            let view = crate::plaza::council_now(app)
                .vote(motion, &me, request.checked()?)
                .map_err(|e| format!("{e:?}"))?;
            to_text(&view)
        }
        "plaza_propose" => {
            let request: ProposeRequest = from_req(json!({
                "proposer_cid": agent.cid,
                "kind": str_arg(input, "kind")?,
                "text": str_arg(input, "text")?
            }))?;
            let checked = request.checked(app.plaza.treasury.minted_total())?;
            let view = crate::plaza::council_now(app)
                .propose(&me, checked)
                .map_err(|e| format!("{e:?}"))?;
            to_text(&view)
        }
        "plaza_agents" => {
            let rows: Vec<Value> = app
                .agents
                .list()
                .iter()
                .map(|a| {
                    json!({
                        "cid": a.cid,
                        "name": a.name,
                        "role": a.role,
                        "status": a.status,
                        "project": app.members.project_of(a.cid),
                    })
                })
                .collect();
            Ok(Value::Array(rows).to_string())
        }
        "plaza_post_task" => {
            let title = str_arg(input, "title")?;
            let body = str_arg(input, "body")?;
            let after = ids_arg(input, "after")?;
            // An assignee starts at once, so a task that still waits on its links cannot have one.
            if input.get("assignee").and_then(Value::as_u64).is_some() {
                let waiting = app.plaza.tasks.not_done(&after);
                if !waiting.is_empty() {
                    let verb = if waiting.len() == 1 { "is" } else { "are" };
                    return Err(format!(
                        "the assignee cannot start it yet: it runs after {}, which {verb} not done; post it without an assignee and the plaza hands it out once they are",
                        waiting.join(", ")
                    ));
                }
            }
            // An unfunded task would stop at its first model call: give it a working default.
            let budget = budget_arg(input)?;
            let role = input
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string);
            // Every task belongs to a project: the one named, else the poster's own. A project's agent posts
            // only to its own project; the orchestrator names the project it posts to.
            let named = input
                .get("project")
                .and_then(Value::as_str)
                .filter(|p| !p.trim().is_empty())
                .map(str::to_string);
            let project = own_project(app, agent, named)?;
            let pid = project.as_str();
            // A project's task is refused until the project's plan is approved, and once it is archived.
            takes_tasks(app, pid)?;
            let p = app
                .projects
                .get(pid)
                .ok_or_else(|| format!("no project {pid}"))?;
            if !matches!(p.stage.as_str(), "implementation" | "review") {
                return Err(format!(
                    "project {pid} is at {}: tasks can be posted only once its plan is approved",
                    p.stage
                ));
            }
            // The assignee works on the project: checked before the task exists, so a refusal posts nothing.
            if let Some(cid) = input.get("assignee").and_then(Value::as_u64) {
                let target = app
                    .agents
                    .get(cid)
                    .ok_or_else(|| format!("no connected agent {cid}"))?;
                crate::membership::may_take_task(app, &target, Some(pid))?;
            }
            let task = app
                .plaza
                .tasks
                .create(&CreateTask {
                    goal: format!("{title}\n\n{body}"),
                    acceptance: input
                        .get("acceptance")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map(str::to_string),
                    budget: Some(budget),
                    role,
                    after,
                    project: Some(project.clone()),
                })
                .map_err(|e| format!("{e:?}"))?;
            app.projects.link_task(pid, task.id())?;
            app.work.note_poster(task.id(), agent.cid);
            crate::society::note_posted(app, agent.cid);
            match input.get("assignee").and_then(Value::as_u64) {
                Some(cid) => {
                    assign(app, &task, cid)?;
                    crate::society::interact(
                        app,
                        agent.cid,
                        cid,
                        apz_plaza::bonds::Interaction::Task,
                    );
                    Ok(task_json(&task).to_string())
                }
                None => Ok(task_json(&task).to_string()),
            }
        }
        "plaza_tasks_post" => {
            let project = str_arg(input, "project")?;
            own_project(app, agent, Some(project.to_string()))?;
            takes_tasks(app, project)?;
            let tasks: Vec<crate::task_plan::Planned> =
                serde_json::from_value(input.get("tasks").cloned().unwrap_or(Value::Null))
                    .map_err(|e| {
                        format!("`tasks` must be a list of {{key, goal, acceptance?, role?, budget?, after?}}: {e}")
                    })?;
            let posted = crate::task_plan::post(&app.projects, &app.plaza.tasks, project, &tasks)?;
            for task in &posted {
                app.work.note_poster(&task.id, agent.cid);
                crate::society::note_posted(app, agent.cid);
            }
            Ok(format!(
                "{}\nNext: each task goes to a worker as soon as everything it runs after is done; follow them with plaza_task_result.",
                json!({ "project": project, "tasks": posted })
            ))
        }
        "plaza_task_link" => {
            let task = str_arg(input, "task")?;
            if input.get("after").is_none() {
                return Err(
                    "missing list `after`: the whole new list of the tasks it runs after ([] for none)"
                        .to_string(),
                );
            }
            let after = ids_arg(input, "after")?;
            let task = app
                .plaza
                .tasks
                .relink(task, &after)
                .map_err(|e| e.to_string())?;
            Ok(task_json(&task).to_string())
        }
        "plaza_assign" => {
            let task = task_arg(app, input)?;
            let cid = u64_arg(input, "agent")?;
            assign(app, &task, cid)?;
            crate::society::interact(app, agent.cid, cid, apz_plaza::bonds::Interaction::Task);
            Ok(format!("assigned {} to {cid}", task.id()))
        }
        "plaza_cancel_task" => {
            task_arg(app, input)?
                .withdraw()
                .map_err(|e| format!("{e:?}"))?;
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
            // A payment over the owner's threshold is held: the treasury has no holds, so nothing moves
            // until the owner approves it (approvals::decide -> payments::decided runs the transfer).
            if crate::payments::held(amount, app.settings.get().payments.approve_over) {
                let subject = format!("pay {amount} to {to}");
                let detail = json!({"cid": agent.cid, "to": to, "amount": amount, "memo": memo});
                let item = crate::approvals::submit(app, "payment", &subject, detail);
                return Ok(format!(
                    "awaiting owner approval ({}): paying {amount} to {to} is held until the owner decides",
                    item.id
                ));
            }
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
        other if crate::skills::agent_tools::TOOLS.contains(&other) => {
            crate::skills::agent_tools::run(app, agent, other, input)
        }
        other if crate::society::TOOLS.contains(&other) => {
            crate::society::run(app, agent, work, other, input)
        }
        other if crate::toolbox::TOOLS.contains(&other) => {
            crate::toolbox::run(app, agent, other, input)
        }
        other if crate::projects::TOOLS.contains(&other) => {
            crate::projects::run(app, agent, other, input)
        }
        other if crate::harness_control::TOOLS.contains(&other) => {
            crate::harness_control::run(app, agent, other, input)
        }
        "plaza_git" => crate::git::run(app, agent, input),
        other if crate::stations::tools::TOOLS.contains(&other) => {
            crate::stations::tools::run(app, agent, other, input)
        }
        "plaza_agent_propose" => crate::agent_proposals::run(app, agent, input),
        "plaza_connector_propose" => crate::connectors::propose(app, agent, input),
        other if crate::connectors::by_tool(other).is_some() => {
            let (connector, action) =
                crate::connectors::by_tool(other).ok_or("unknown connector tool")?;
            let name = if agent.name.is_empty() {
                &agent.role
            } else {
                &agent.name
            };
            crate::connectors::call(
                app,
                Some((agent.cid, name.as_str())),
                &agent.role,
                connector.id,
                action.name,
                input,
            )
        }
        other if other.starts_with(crate::mcp::PREFIX) => {
            crate::mcp::call_for_agent(app, agent, work, other, input)
        }
        other => Err(format!("{other} is not implemented")),
    }
}

/// Posts a task for `poster` and assigns it to `worker` at once (a hire); returns the task id.
pub fn post_and_assign(
    app: &App,
    poster: u64,
    goal: &str,
    acceptance: Option<String>,
    budget: u64,
    worker: u64,
    project: &str,
) -> Result<String, String> {
    let task = app
        .plaza
        .tasks
        .create(&CreateTask {
            goal: goal.to_string(),
            acceptance,
            budget: Some(budget),
            role: None,
            after: Vec::new(),
            project: Some(project.to_string()),
        })
        .map_err(|e| format!("{e:?}"))?;
    app.projects.link_task(project, task.id())?;
    app.work.note_poster(task.id(), poster);
    crate::society::note_posted(app, poster);
    assign(app, &task, worker)?;
    crate::society::interact(app, poster, worker, apz_plaza::bonds::Interaction::Task);
    Ok(task.id().to_string())
}

/// The project a task `agent` posts goes to: `named`, else the agent's own. An agent on a project posts only to
/// it; the orchestrator, which works on every project, names one.
fn own_project(app: &App, agent: &Agent, named: Option<String>) -> Result<String, String> {
    if crate::membership::plaza_wide(&agent.role) {
        return named.ok_or_else(|| {
            "a task belongs to a project: name it with `project` (plaza_projects lists them)"
                .to_string()
        });
    }
    let own = app
        .members
        .project_of(agent.cid)
        .ok_or("you are on the bench: you post tasks only for your project")?;
    match named {
        Some(named) if named != own => Err(format!(
            "you work on project {own}: you post tasks only for it, not for {named}"
        )),
        _ => Ok(own),
    }
}

/// Refuses tasks for an archived project: it is over, and its agents are back on the bench.
fn takes_tasks(app: &App, pid: &str) -> Result<(), String> {
    match app.projects.get(pid) {
        None => Err(format!("no project {pid}")),
        Some(p) if p.archived => Err(format!("project {pid} is archived: it takes no new tasks")),
        Some(_) => Ok(()),
    }
}

/// Whether `from` may message `to`: both work now, on the same project, or one of them is the orchestrator.
fn may_message(app: &App, from: &Agent, to: &Agent) -> Result<(), String> {
    if let Some(why) = crate::membership::held(app, to) {
        return Err(format!("{} cannot be messaged now: {why}", to.name));
    }
    if crate::membership::plaza_wide(&from.role) || crate::membership::plaza_wide(&to.role) {
        return Ok(());
    }
    let (mine, theirs) = (
        app.members.project_of(from.cid),
        app.members.project_of(to.cid),
    );
    if mine == theirs {
        Ok(())
    } else {
        Err(format!(
            "{} works on another project: you message only your project's agents and the orchestrator",
            to.name
        ))
    }
}

/// Claims `task` for the agent `cid` and queues the work for it.
fn assign(app: &App, task: &Task, cid: u64) -> Result<(), String> {
    let target = app
        .agents
        .get(cid)
        .ok_or_else(|| format!("no connected agent {cid}"))?;
    crate::membership::may_take_task(app, &target, task.project())?;
    task.claim(&agents::connected_agent(&target))
        .map_err(|e| e.to_string())?;
    let origin = Origin {
        task: Some(task.id().to_string()),
        ..Origin::default()
    };
    let _ = app.work.enqueue(
        app,
        cid,
        WorkKind::Task,
        format!("task-{}", task.id()),
        task_text(task),
        origin,
    );
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
    let view = task.view();
    json!({
        "id": view.id,
        "goal": view.goal,
        "state": view.state,
        "budget": view.budget,
        "role": view.role,
        "acceptance": view.acceptance,
        "project": view.project,
        "after": view.after,
        "ready": view.ready,
        "waiting_on": view.waiting_on,
        "blocked_by": view.blocked_by
    })
}

/// A list of task ids: absent is none, and anything but an array of strings is refused.
fn ids_arg(input: &Value, key: &str) -> Result<Vec<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("`{key}` must be a list of task ids"))
            })
            .collect(),
        Some(_) => Err(format!("`{key}` must be a list of task ids")),
    }
}

/// The task named by the `task` argument.
fn task_arg(app: &App, input: &Value) -> Result<Arc<Task>, String> {
    let id = str_arg(input, "task")?;
    app.plaza
        .tasks
        .find(id)
        .ok_or_else(|| format!("no task {id}"))
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

/// A task's `budget` argument, through the one default and minimum in
/// [`resolve_budget`]: absent (or null) → the default; a number under the minimum,
/// 0 included, or a value that is not a whole number → refused.
pub fn budget_arg(input: &Value) -> Result<u64, String> {
    let requested = match input.get("budget") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or("budget must be a whole number of credits")?,
        ),
    };
    resolve_budget(requested).map_err(|refusal| {
        format!("{refusal}. It is minted from the plaza's budget; your wallet is not debited.")
    })
}

/// The optional `attachments` argument: at most 8 shared files the caller may read.
fn attachments_arg(app: &App, input: &Value, cid: u64) -> Result<Vec<String>, String> {
    let ids: Vec<String> = match input.get("attachments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "attachments must be file ids (strings)".to_string())
            })
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
        if app
            .deliverables
            .meta(id)
            .is_some_and(|m| m.audience != "public")
        {
            app.deliverables
                .publish(id, cid)
                .map_err(|e| format!("file {id}: {e}"))?;
        }
    }
    Ok(())
}

/// Lets the recipient read each private file, as a direct message does.
fn grant_files(app: &App, files: &[String], cid: u64, to: u64) -> Result<(), String> {
    for id in files {
        if app
            .deliverables
            .meta(id)
            .is_some_and(|m| m.audience != "public")
        {
            app.deliverables
                .grant(id, cid, &[to])
                .map_err(|e| format!("file {id}: {e}"))?;
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
    fn code_engine_model_input_names_a_provider_or_takes_the_default() {
        let ask = |model: &str| json!({"engine": "claude", "prompt": "p", "model": model});
        // A provider/model id is passed through as written, slashes and effort suffix included.
        // An absent, null or blank model is the default.
        for (case, input, want) in [
            (
                "an openrouter id keeps its nested slashes",
                ask(" openrouter/anthropic/claude-x "),
                Ok(Some("openrouter/anthropic/claude-x")),
            ),
            (
                "an effort suffix is part of the shape",
                ask("opencode/claude-sonnet-5-5@high"),
                Ok(Some("opencode/claude-sonnet-5-5@high")),
            ),
            (
                "an absent model is the default",
                json!({"prompt": "p"}),
                Ok(None),
            ),
            (
                "a blank model is the default",
                json!({"model": "  "}),
                Ok(None),
            ),
            (
                "a null model is the default",
                json!({"model": null}),
                Ok(None),
            ),
        ] {
            assert_eq!(model_input(&input), want, "{case}");
        }
        // A bare model name is refused: the model must name a provider (code_engine_model_must_name_a_provider).
        {
            let bare = json!({"engine": "codex", "prompt": "p", "model": "claude-sonnet-5-5"});
            let err = model_input(&bare).unwrap_err();
            assert!(err.contains("<provider>/<model>"), "{err}");
            assert!(model_input(&json!({"model": 3})).is_err());
        }
    }

    #[test]
    fn code_engine_routes_take_each_engines_provider_models() {
        // Gemini CLI takes only gemini/<model> models; the other engines keep their own routes.
        {
            let ask = |engine: &str, model: &str| json!({"engine": engine, "prompt": "p", "model": model});
            assert!(
                engine_provider(
                    &ask("gemini", "gemini/gemini-2.5-pro@high"),
                    "gemini/gemini-2.5-pro@high"
                )
                .is_ok()
            );
            let err = engine_provider(&ask("gemini", "openrouter/x"), "openrouter/x").unwrap_err();
            assert!(
                err.contains("Gemini CLI takes gemini/<model> models"),
                "{err}"
            );
            // The other engines keep their own routes.
            assert!(engine_provider(&ask("claude", "openrouter/x"), "openrouter/x").is_ok());
        }

        // Qwen Code takes the openai, openrouter and opencode chat-completions routes.
        {
            let ask = |model: &str| json!({"engine": "qwen", "prompt": "p", "model": model});
            for model in [
                "openai/gpt-5.6-terra",
                "openrouter/x/y",
                "opencode/deepseek-v4-pro",
            ] {
                assert!(engine_provider(&ask(model), model).is_ok(), "{model}");
            }
            for model in ["anthropic/claude-x", "bare-model"] {
                let err = engine_provider(&ask(model), model).unwrap_err();
                assert!(err.contains("Qwen Code takes openai/<model>"), "{err}");
            }
        }

        // Copilot and Goose take their provider routes and refuse a missing model.
        {
            let copilot = |model: &str| json!({"engine": "copilot", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openai/gpt-x",
                "openrouter/anthropic/claude-x",
                "opencode/gpt-x",
            ] {
                assert!(engine_provider(&copilot(model), model).is_ok(), "{model}");
            }
            assert!(engine_provider(&copilot("gemini/x"), "gemini/x").is_err());
            let err =
                model_required(&json!({"engine": "copilot", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Copilot needs \"model\""), "{err}");
            let goose = |model: &str| json!({"engine": "goose", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openrouter/anthropic/claude-x",
                "openai/gpt-x",
            ] {
                assert!(engine_provider(&goose(model), model).is_ok(), "{model}");
            }
            assert!(engine_provider(&goose("qwen/x"), "qwen/x").is_err());
            let err =
                model_required(&json!({"engine": "goose", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Goose needs \"model\""), "{err}");
        }

        // Kimi Code takes its provider routes and needs a model.
        {
            let kimi = |model: &str| json!({"engine": "kimi", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openai/gpt-x",
                "openrouter/anthropic/claude-x",
                "opencode/gpt-x",
            ] {
                assert!(engine_provider(&kimi(model), model).is_ok(), "{model}");
            }
            assert!(engine_provider(&kimi("gemini/x"), "gemini/x").is_err());
            let err =
                model_required(&json!({"engine": "kimi", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Kimi Code needs \"model\""), "{err}");
        }

        // Auggie takes only augment/<model> and needs no model.
        {
            let auggie = |model: &str| json!({"engine": "auggie", "prompt": "p", "model": model});
            assert!(engine_provider(&auggie("augment/claude-x"), "augment/claude-x").is_ok());
            let err = engine_provider(&auggie("anthropic/claude-x"), "anthropic/claude-x")
                .expect_err("refused");
            assert!(err.contains("use augment/<model>"), "{err}");
            assert!(model_required(&json!({"engine": "auggie", "prompt": "p"})).is_ok());
        }

        // Vibe takes the three routes its config maps and needs a model.
        {
            let vibe = |model: &str| json!({"engine": "vibe", "prompt": "p", "model": model});
            for model in [
                "openai/gpt-x",
                "openrouter/mistralai/mistral-large",
                "opencode/gpt-x",
            ] {
                assert!(engine_provider(&vibe(model), model).is_ok(), "{model}");
            }
            let err = engine_provider(&vibe("anthropic/x"), "anthropic/x").unwrap_err();
            assert!(err.contains("Vibe takes openai/..."), "{err}");
            let err =
                model_required(&json!({"engine": "vibe", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Vibe needs \"model\""), "{err}");
        }

        // Cline takes the four routes and needs a model.
        {
            let ask = |model: &str| json!({"engine": "cline", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openrouter/anthropic/claude-x",
                "openai/gpt-x",
                "opencode/deepseek-v4-pro",
            ] {
                assert!(engine_provider(&ask(model), model).is_ok(), "{model}");
            }
            let err = engine_provider(&ask("gemini/x"), "gemini/x").unwrap_err();
            assert!(err.contains("Cline takes anthropic/..."), "{err}");

            let no_model = json!({"engine": "cline", "prompt": "p"});
            assert_eq!(
                model_required(&no_model).unwrap_err(),
                "invalid input: Cline needs \"model\""
            );
            assert!(model_required(&json!({"engine": "claude", "prompt": "p"})).is_ok());
        }

        // Kilo has no provider rule of its own.
        {
            let input = json!({"engine": "kilo", "prompt": "p", "model": "anthropic/x"});
            assert!(engine_provider(&input, "anthropic/x").is_ok());
        }
    }

    #[test]
    fn code_engine_offers_sessions_modes_and_only_the_enabled_engines() {
        let spec = spec("code_engine").expect("code_engine is a spec");
        assert!(spec.description.contains("streams live"));
        assert!(spec.description.contains("`session`"));
        assert!(spec.description.contains("gemini = Gemini CLI"));
        assert!(spec.description.contains("qwen = Qwen Code"));
        assert!(spec.description.contains("kilo = Kilo Code"));
        assert!(spec.description.contains("cline = Cline"));
        let props = &spec.input_schema["properties"];
        assert_eq!(
            props["engine"]["enum"],
            json!([
                "claude", "codex", "opencode", "gemini", "qwen", "kilo", "cline", "copilot",
                "goose", "kimi", "auggie", "vibe"
            ])
        );
        assert_eq!(props["session"]["type"], "string");
        assert_eq!(props["mode"]["type"], "string");
        let mut schema = spec.input_schema.clone();
        restrict_engines(&mut schema, &["codex".to_string()]);
        assert_eq!(schema["properties"]["engine"]["enum"], json!(["codex"]));
    }

    #[test]
    fn code_engine_refuses_an_engine_the_owner_has_not_enabled() {
        let enabled = vec!["claude".to_string(), "codex".to_string()];
        let ask = |engine: &str| json!({"engine": engine, "prompt": "p"});
        assert_eq!(engine_enabled(&enabled, &ask("codex")), Ok(()));
        assert_eq!(
            engine_enabled(&enabled, &ask("opencode")),
            Err("the owner has not enabled the opencode engine".to_string())
        );
        assert!(engine_enabled(&enabled, &json!({"prompt": "p"})).is_err());
    }

    #[test]
    fn role_offer_follows_tier_seat_projects_and_laws() {
        // A tier and a seat change the offer: workers bid, orchestrators post tasks, seated roles vote and propose.
        {
            let worker = offered("worker", &[]);
            let orchestrator = offered(ORCHESTRATOR_ROLE, &[]);
            assert!(worker.iter().any(|n| n == "plaza_bid"));
            assert!(!worker.iter().any(|n| n == "plaza_post_task"));
            assert!(orchestrator.iter().any(|n| n == "plaza_post_task"));
            // The order of a plan's tasks is the orchestrator's too: it posts the list and relinks.
            for tool in ["plaza_tasks_post", "plaza_task_link"] {
                assert!(orchestrator.iter().any(|n| n == tool), "{tool}");
                assert!(!worker.iter().any(|n| n == tool), "{tool}");
            }
            assert!(!orchestrator.iter().any(|n| n == "plaza_bid"));
            assert!(!worker.iter().any(|n| n == "plaza_vote"));
            let seated = offered("finance", &[]);
            assert!(seated.iter().any(|n| n == "plaza_vote"));
            assert!(seated.iter().any(|n| n == "plaza_propose"));
        }

        // Projects and harness tools go to their roles; everyone reads projects and critiques them.
        {
            let has = |role: &str, tool: &str| offered(role, &[]).iter().any(|n| n == tool);
            assert!(has(ORCHESTRATOR_ROLE, "plaza_project_open"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_project_advance"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_agent_control"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_loop"));
            assert!(has("planner", "plaza_project_advance"));
            assert!(has("mayor", "plaza_project_approve"));
            assert!(has("mayor", "plaza_agent_control"));
            assert!(!has("builder", "plaza_project_open"));
            assert!(!has("builder", "plaza_project_approve"));
            assert!(!has("builder", "plaza_agent_control"));
            // Everyone can read projects, attach to them and critique them.
            assert!(has("builder", "plaza_projects") && has("builder", "plaza_project_critique"));
        }

        // The laws decide the offer: a builder cannot post a task or approve a project, a mayor can.
        {
            assert!(
                !offered("builder", &[])
                    .iter()
                    .any(|n| n == "plaza_post_task")
            );
            assert!(
                !offered("builder", &[])
                    .iter()
                    .any(|n| n == "plaza_project_approve")
            );
            assert!(
                offered("mayor", &[])
                    .iter()
                    .any(|n| n == "plaza_project_approve")
            );
            assert!(refusal("builder", "plaza_post_task").is_some());
        }

        // A worker calling post_task is refused.
        {
            assert!(refusal("worker", "plaza_post_task").is_some());
            assert!(refusal(ORCHESTRATOR_ROLE, "plaza_post_task").is_none());
            assert!(refusal("builder", "plaza_task_link").is_some());
            assert!(refusal(ORCHESTRATOR_ROLE, "plaza_task_link").is_none());
            assert!(refusal("planner", "plaza_tasks_post").is_some());
            assert!(refusal("worker", "bash").is_some());
        }
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
    fn a_tool_task_takes_the_default_budget_and_refuses_below_the_minimum() {
        use apz_plaza::tasks::DEFAULT_TASK_BUDGET;
        let default = DEFAULT_TASK_BUDGET as u64;
        assert_eq!(budget_arg(&json!({})), Ok(default));
        assert_eq!(budget_arg(&json!({"budget": null})), Ok(default));
        assert_eq!(budget_arg(&json!({"budget": 200})), Ok(200));
        assert_eq!(budget_arg(&json!({"budget": 5000})), Ok(5000));
        for low in [100, 0] {
            let refusal = budget_arg(&json!({"budget": low})).expect_err("below the minimum");
            assert!(
                refusal.contains("minimum of 200"),
                "names the minimum: {refusal}"
            );
            assert!(refusal.contains("wallet is not debited"), "{refusal}");
        }
        assert!(budget_arg(&json!({"budget": -5})).is_err());
        assert!(budget_arg(&json!({"budget": "lots"})).is_err());
    }

    /// Agents never run anything on the host: the host-side tools are the
    /// plaza's data operations only (no processes, files or network).
    #[test]
    fn host_tools_are_plaza_data_operations_only() {
        let host: Vec<&str> = [
            "transcript_search",
            "submit_claim",
            "plaza_tasks",
            "plaza_bid",
            "plaza_board_read",
            "plaza_board_post",
            "plaza_board_comment",
            "plaza_send_message",
            "plaza_vote",
            "plaza_propose",
            "plaza_agents",
            "plaza_post_task",
            "plaza_assign",
            "plaza_cancel_task",
            "plaza_task_result",
            "plaza_get_file",
            "plaza_set_name",
            "plaza_pay",
            "plaza_verdict",
            "plaza_contacts",
            "plaza_rate",
            "plaza_note",
            "plaza_hire",
            "memory_write",
            "memory_read",
            "memory_search",
            "memory_list",
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
