//! The IDE's view model: pure functions over the hub's answers (the roster, the projects, the tasks and an
//! agent's transcript). The grouping of the sidebar, the status dots, the collapsing of tool steps, the
//! activity split and the palette's filter are decided here, so they are tested without iced.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::console::roles;

// ---- The roster and the projects --------------------------------------------------------------

/// One agent as the sidebar shows it (`GET /v1/agents`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub cid: u64,
    /// The agent's label, the name it is shown by: its role and its cid (`roles::agent_label_of`).
    pub label: String,
    pub role_label: String,
    pub model: String,
    pub status: String,
    /// The prompt of the work it is doing now, if any.
    pub task: Option<String>,
    pub last_seen_ms: u64,
    pub tokens: u64,
    /// The project its live session is on (None: no live session, or the orchestrator).
    pub project: Option<String>,
    /// The orchestrator: it works on every project.
    pub orchestrator: bool,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Reads one roster entry; an entry without a cid is not an agent.
pub fn parse_agent(v: &Value) -> Option<AgentRow> {
    let cid = v.get("cid")?.as_u64()?;
    let role = text(v, "role");
    let role_label = match text(v, "role_label") {
        label if label.is_empty() => role,
        label => label,
    };
    let usage = |key: &str| {
        v.get("usage")
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let task = v
        .get("work")
        .filter(|w| w.is_object())
        .and_then(|w| w.get("prompt"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(AgentRow {
        cid,
        label: roles::agent_label_of(v),
        role_label,
        model: text(v, "model"),
        status: text(v, "status"),
        task,
        last_seen_ms: v.get("last_seen_ms").and_then(Value::as_u64).unwrap_or(0),
        tokens: usage("input_tokens") + usage("output_tokens"),
        project: v
            .get("project")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(str::to_string),
        orchestrator: text(v, "tier") == "orchestrator",
    })
}

/// One project as the sidebar groups by (`GET /v1/projects`): its repo, its tasks, and the agents that
/// worked on it (the authors of its documents, critiques and history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub title: String,
    pub stage: String,
    pub repo: Option<String>,
    pub tasks: Vec<String>,
    /// Its lead agent (the orchestrator of a project it opened): listed under it while it works on no project.
    pub agents: BTreeSet<u64>,
}

/// Reads one project; its lead (the orchestrator of a project it opened) is its agent on no project.
pub fn parse_project(v: &Value) -> Option<ProjectRow> {
    let id = v.get("id")?.as_str()?.to_string();
    let mut agents = BTreeSet::new();
    if let Some(lead) = v
        .get("lead_cid")
        .and_then(Value::as_u64)
        .filter(|c| *c != 0)
    {
        agents.insert(lead);
    }
    let tasks = v
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect();
    Some(ProjectRow {
        title: text(v, "title"),
        stage: text(v, "stage"),
        repo: v
            .get("repo")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty())
            .map(str::to_string),
        id,
        tasks,
        agents,
    })
}

/// The sidebar in project mode (the IDE opened under a project): that project's group alone, with its agents (the ones
/// working on it, and the orchestrator, which works on every project) and its repo. Empty until the project is listed.
pub fn project_group(
    agents: &[AgentRow],
    projects: &[ProjectRow],
    repos: &[(String, String)],
    project: &str,
) -> Vec<Group> {
    let Some(p) = projects.iter().find(|p| p.id == project) else {
        return Vec::new();
    };
    vec![Group {
        key: format!("project:{project}"),
        title: if p.title.is_empty() {
            p.id.clone()
        } else {
            p.title.clone()
        },
        project: Some(project.to_string()),
        agents: agents
            .iter()
            .filter(|a| a.project.as_deref() == Some(project) || a.orchestrator)
            .map(|a| a.cid)
            .collect(),
        repos: repos
            .iter()
            .filter(|(_, name)| p.repo.as_deref() == Some(name.as_str()))
            .map(|(id, _)| id.clone())
            .collect(),
        stations: Vec::new(),
    }]
}

/// One task as the Tasks tab counts it (`GET /v1/tasks`), and the agent that claimed it, if one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub goal: String,
    pub state: String,
    pub agent: Option<u64>,
}

pub fn parse_task(v: &Value) -> Option<TaskRow> {
    Some(TaskRow {
        id: v.get("id")?.as_str()?.to_string(),
        goal: text(v, "goal"),
        state: text(v, "state"),
        agent: v.get("agent_cid").and_then(Value::as_u64),
    })
}

/// The number in a task's id (`t12` is 12): tasks are numbered in the order they were made.
fn task_number(id: &str) -> u64 {
    id.trim_start_matches('t').parse().unwrap_or(0)
}

/// How an agent's last finished task ended (`done` or `failed`), from the roster's tasks: the one it claimed with the
/// highest number. None while it has finished none. This is the agent's last work state for the sidebar's dot.
pub fn last_task_outcome(tasks: &[TaskRow], cid: u64) -> Option<String> {
    tasks
        .iter()
        .filter(|t| t.agent == Some(cid) && matches!(t.state.as_str(), "done" | "failed"))
        .max_by_key(|t| task_number(&t.id))
        .map(|t| t.state.clone())
}

/// A sidebar group: a project (with its agents and the repo workspaces it owns), the workstations, or the agents that
/// belong to no project. An agent is listed once, under the first project that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub key: String,
    pub title: String,
    pub project: Option<String>,
    pub agents: Vec<u64>,
    /// The repo workspaces' ids (`repo:<name>@<ref>`) the group owns.
    pub repos: Vec<String>,
    /// The workstations' ids (`station:<id>`) the group lists.
    pub stations: Vec<String>,
}

impl Group {
    /// The count the group's header shows: its agents, repos and workstations.
    pub fn count(&self) -> usize {
        self.agents.len() + self.repos.len() + self.stations.len()
    }
}

/// The sidebar's groups: each project that has an agent or a repo, in the order given; then the workstations (after
/// the projects, as the web IDE has them); then the agents no project names, under "Other agents". An agent is under
/// the project it works on, and only that one; an agent on none, under the project it leads. `repos` are (workspace
/// id, repo name) pairs; a repo no project owns goes under "Repositories", last. `stations` are the workstations'
/// workspace ids.
pub fn groups(
    agents: &[AgentRow],
    projects: &[ProjectRow],
    repos: &[(String, String)],
    stations: &[String],
) -> Vec<Group> {
    let mut placed: BTreeSet<u64> = BTreeSet::new();
    let mut placed_repos: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for project in projects {
        let listed: Vec<u64> = agents
            .iter()
            .filter(|a| match &a.project {
                Some(p) => *p == project.id,
                None => project.agents.contains(&a.cid),
            })
            .map(|a| a.cid)
            .filter(|cid| !placed.contains(cid))
            .collect();
        let mut owned: Vec<String> = Vec::new();
        for (id, name) in repos {
            if project.repo.as_deref() == Some(name.as_str()) && !placed_repos.contains(name) {
                placed_repos.insert(name.clone());
                owned.push(id.clone());
            }
        }
        if listed.is_empty() && owned.is_empty() {
            continue;
        }
        placed.extend(listed.iter().copied());
        out.push(Group {
            key: format!("project:{}", project.id),
            title: if project.title.is_empty() {
                project.id.clone()
            } else {
                project.title.clone()
            },
            project: Some(project.id.clone()),
            agents: listed,
            repos: owned,
            stations: Vec::new(),
        });
    }
    if !stations.is_empty() {
        out.push(Group {
            key: "workstations".into(),
            title: "Workstations".into(),
            project: None,
            agents: Vec::new(),
            repos: Vec::new(),
            stations: stations.to_vec(),
        });
    }
    let loose: Vec<u64> = agents
        .iter()
        .map(|a| a.cid)
        .filter(|cid| !placed.contains(cid))
        .collect();
    if !loose.is_empty() {
        out.push(Group {
            key: "other".into(),
            title: "Other agents".into(),
            project: None,
            agents: loose,
            repos: Vec::new(),
            stations: Vec::new(),
        });
    }
    let stray: Vec<String> = repos
        .iter()
        .filter(|(_, name)| !placed_repos.contains(name))
        .map(|(id, _)| id.clone())
        .collect();
    if !stray.is_empty() {
        out.push(Group {
            key: "repos".into(),
            title: "Repositories".into(),
            project: None,
            agents: Vec::new(),
            repos: stray,
            stations: Vec::new(),
        });
    }
    out
}

// ---- Workstations -----------------------------------------------------------------------------

/// One workstation's account, as the sidebar's row needs it (`GET /v1/stations`): when its agents act as the owner in
/// the owner's own account, that account's name (the row warns); a separate agent account has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationRow {
    pub id: String,
    pub owner_account: Option<String>,
}

/// Reads one station record. A record without an id is not a station.
pub fn parse_station(v: &Value) -> Option<StationRow> {
    let id = v.get("id")?.as_str()?.to_string();
    let owner_account = v
        .get("facts")
        .and_then(|f| f.get("account"))
        .filter(|a| text(a, "mode") == "owner")
        .map(|a| text(a, "name"));
    Some(StationRow { id, owner_account })
}

/// A workstation's OS as a short label, as the web IDE shows it. A missing OS reads as unknown; an OS this build does
/// not name is shown as it came.
pub fn os_label(os: Option<&str>) -> String {
    match os {
        Some("macos") => "macOS".into(),
        Some("windows") => "Windows".into(),
        Some("linux") => "Linux".into(),
        Some(other) if !other.is_empty() => other.into(),
        _ => "Unknown OS".into(),
    }
}

// ---- Status ----------------------------------------------------------------------------------

/// What an agent's status dot says: working (accent pulse), idle (hollow), sleeping (its VM is down), or, for an
/// idle agent whose last work the transcript shows, done or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Working,
    Idle,
    Sleeping,
    Done,
    Failed,
}

impl Dot {
    pub fn label(self) -> &'static str {
        match self {
            Dot::Working => "working",
            Dot::Idle => "idle",
            Dot::Sleeping => "sleeping",
            Dot::Done => "done",
            Dot::Failed => "failed",
        }
    }
}

/// The dot for an agent from the hub's status (`working`, `idle`, `asleep`, `dozing`, `offline`) and the outcome of its
/// last work (`done`, `failed`, `error`, or none when the transcript does not say).
pub fn dot(status: &str, last_outcome: Option<&str>) -> Dot {
    match status {
        "working" => Dot::Working,
        "offline" | "asleep" | "dozing" => Dot::Sleeping,
        _ => match last_outcome {
            Some("failed" | "error") => Dot::Failed,
            Some("done") => Dot::Done,
            _ => Dot::Idle,
        },
    }
}

/// The outcome of the transcript's last finished work, if it has one (`work_end`'s status).
pub fn last_outcome(entries: &[Value]) -> Option<String> {
    entries
        .iter()
        .rev()
        .find(|e| e.get("kind").and_then(Value::as_str) == Some("work_end"))
        .and_then(|e| e.get("data")?.get("status")?.as_str().map(str::to_string))
}

/// "Working for 4 min" style age for a last-seen time, against `now_ms`.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    if then_ms == 0 {
        return "never seen".to_string();
    }
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

// ---- The transcript: steps and activity ------------------------------------------------------

/// A row of the chat: one transcript entry, a run of tool steps collapsed into one line, or a coding engine's
/// call drawn as one block (see `engine_view`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Entry(usize),
    /// The run is the entries `first..end`.
    Steps {
        first: usize,
        end: usize,
        count: usize,
        summary: String,
    },
    Engine {
        first: usize,
        call: String,
    },
}

/// Whether an entry is part of a tool step: a call, its decision or result, or a model turn that only asks for
/// tools (no text).
pub fn is_step(entry: &Value) -> bool {
    match entry.get("kind").and_then(Value::as_str) {
        Some("tool_request" | "tool_decision" | "tool_result") => true,
        Some("model_response") => {
            let blocks = entry
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(Value::as_array);
            blocks.is_some_and(|b| {
                !b.is_empty()
                    && b.iter()
                        .all(|x| x.get("type").and_then(Value::as_str) == Some("tool_use"))
            })
        }
        _ => false,
    }
}

/// The tools a model turn calls: its tool_use blocks, by name.
fn calls_of(entry: &Value) -> Vec<String> {
    entry
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|b| b.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// "3 steps · run, edit": the step line's text. A run of one reads "1 step".
/// One tool call in a run of steps, as the web's chat lists it: its name, the argument that names what it acted on,
/// its outcome ("running", "ok", "failed" or "denied") and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    pub name: String,
    pub detail: String,
    pub outcome: &'static str,
    pub ms: Option<u64>,
}

/// The first line of a value, trimmed and clipped to 80 characters.
fn clip_line(s: &str) -> String {
    let line = s.split('\n').next().unwrap_or("").trim();
    line.chars().take(80).collect()
}

/// The argument that best names what a tool call acted on: a path, a command, a query.
fn detail_of(input: Option<&Value>) -> String {
    match input {
        Some(Value::String(s)) => clip_line(s),
        Some(v @ Value::Object(_)) => {
            let direct = [
                "path",
                "file_path",
                "file",
                "command",
                "query",
                "pattern",
                "url",
                "title",
                "name",
            ]
            .iter()
            .map(|key| text_of(v, key))
            .find(|s| !s.is_empty());
            // The edit tools name their file in the first entry of their list.
            let listed = ["files", "create", "edits"].iter().find_map(|key| {
                let path = text_of(v.get(key)?.as_array()?.first()?, "path");
                (!path.is_empty()).then_some(path)
            });
            direct.or(listed).map(clip_line).unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// The steps of a run of entries, built as the web's tool tracker builds them: a tool_request opens a step; a
/// result, or a denying decision, closes the matching open step (by id when both carry one, else the oldest open
/// step). A result with no open step to close is a step of its own.
pub fn steps_of(entries: &[Value]) -> Vec<StepRow> {
    // Each step with its call id and the time its request came in; `open` holds the steps not yet closed.
    let mut steps: Vec<(StepRow, Option<String>, u64)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for entry in entries {
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
        let data = entry.get("data").unwrap_or(&Value::Null);
        let at = entry.get("at_ms").and_then(Value::as_u64).unwrap_or(0);
        let id = ["id", "tool_use_id", "call_id"]
            .iter()
            .find_map(|key| data.get(*key))
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string));
        let name = |d: &Value| {
            let n = text_of(d, "name");
            if n.is_empty() {
                text_of(d, "tool").to_string()
            } else {
                n.to_string()
            }
        };
        match kind {
            "tool_request" => {
                let row = StepRow {
                    name: name(data),
                    detail: detail_of(data.get("input")),
                    outcome: "running",
                    ms: None,
                };
                steps.push((row, id, at));
                open.push(steps.len() - 1);
            }
            "tool_result" | "tool_decision" => {
                let denied = kind == "tool_decision";
                if denied && text_of(data, "decision") != "deny" {
                    continue;
                }
                let outcome = if denied {
                    "denied"
                } else if data.get("is_error").and_then(Value::as_bool) == Some(true) {
                    "failed"
                } else {
                    "ok"
                };
                let matched = id
                    .as_ref()
                    .and_then(|want| open.iter().position(|&i| steps[i].1.as_ref() == Some(want)))
                    .or_else(|| (!open.is_empty()).then_some(0));
                match matched {
                    Some(pos) => {
                        let i = open.remove(pos);
                        let (row, _, req_at) = &mut steps[i];
                        row.outcome = outcome;
                        row.ms = Some(at.saturating_sub(*req_at));
                    }
                    None => steps.push((
                        StepRow {
                            name: name(data),
                            detail: detail_of(data.get("input")),
                            outcome,
                            ms: None,
                        },
                        id,
                        at,
                    )),
                }
            }
            _ => {}
        }
    }
    steps.into_iter().map(|(row, _, _)| row).collect()
}

/// A step's line: what it did to what, by the tool's name (the web's `stepLabel`).
pub fn step_label(step: &StepRow) -> String {
    let n = step.name.to_lowercase();
    let d = step.detail.as_str();
    let verb = |word: &str, bare: &str| {
        if d.is_empty() {
            bare.to_string()
        } else {
            format!("{word} {d}")
        }
    };
    if n.contains("board") && n.contains("read") {
        return "Read board message".into();
    }
    if n.contains("board") && (n.contains("post") || n.contains("write")) {
        return "Posted to board".into();
    }
    if n == "run"
        || n == "shell"
        || n.contains("command")
        || n.contains("exec")
        || n.contains("run")
    {
        return "Ran command".into();
    }
    if n.starts_with("read") || n.starts_with("view") || n.starts_with("open") {
        return verb("Read", "Read file");
    }
    if ["edit", "patch", "replace"].iter().any(|w| n.contains(w)) {
        return verb("Edited", "Edited file");
    }
    if n.contains("write") || n.contains("create") {
        return verb("Wrote", "Wrote file");
    }
    if ["grep", "search", "glob", "find"]
        .iter()
        .any(|w| n.contains(w))
    {
        return verb("Searched", "Searched files");
    }
    format!("Used {}", step.name)
}

/// A run's outcome word: "running" while a step runs, "done" when none failed, "failed" when all did, else "mixed".
pub fn group_outcome(steps: &[StepRow]) -> &'static str {
    if steps.iter().any(|s| s.outcome == "running") {
        return "running";
    }
    let bad = steps
        .iter()
        .filter(|s| s.outcome == "failed" || s.outcome == "denied")
        .count();
    match bad {
        0 => "done",
        n if n == steps.len() => "failed",
        _ => "mixed",
    }
}

/// A duration as the web shows it: "420 ms", "1.2 s", "2 min 5 s".
pub fn format_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        let total = (ms as f64 / 1000.0).round() as u64;
        format!("{} min {} s", total / 60, total % 60)
    }
}

pub fn steps_label(count: usize, names: &[String]) -> String {
    let mut distinct: Vec<&str> = Vec::new();
    for name in names {
        if !distinct.contains(&name.as_str()) {
            distinct.push(name);
        }
    }
    let noun = if count == 1 { "step" } else { "steps" };
    let tools = distinct
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if tools.is_empty() {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun} · {tools}")
    }
}

/// The call a `code_engine` request makes; None for any other entry.
fn engine_start(entry: &Value) -> Option<String> {
    let data = entry.get("data")?;
    if entry.get("kind")?.as_str()? != "tool_request"
        || data.get("name")?.as_str()? != "code_engine"
    {
        return None;
    }
    data.get("call")?.as_str().map(str::to_string)
}

/// The code_engine call an entry's call id belongs to: a nested check's id is `<call>.e<n>`, its engine's call
/// with the check's number after it.
fn engine_of(call: &str) -> &str {
    match call.rsplit_once(".e") {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => call,
    }
}

/// Whether an entry belongs to one of the `engines` calls: its request, decision, result or stream item. Such
/// entries are drawn in the engine's block, never as tool steps.
fn in_engine(entry: &Value, engines: &BTreeSet<String>) -> bool {
    let kind = entry.get("kind").and_then(Value::as_str);
    if !matches!(
        kind,
        Some("tool_request" | "tool_decision" | "tool_result" | "engine_update")
    ) {
        return false;
    }
    entry
        .get("data")
        .and_then(|d| d.get("call"))
        .and_then(Value::as_str)
        .is_some_and(|call| engines.contains(engine_of(call)))
}

/// The chat's rows: each entry as it is, except that a run of tool steps becomes one `Steps` row, and a coding
/// engine's call (its request and everything that belongs to it) one `Engine` row. The run is counted by the calls
/// its model turns make; a run that names no tool (only requests) is counted by its entries.
pub fn rows(entries: &[Value]) -> Vec<Row> {
    let engines: BTreeSet<String> = entries.iter().filter_map(engine_start).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        if let Some(call) = engine_start(&entries[i]) {
            out.push(Row::Engine { first: i, call });
            i += 1;
            continue;
        }
        if in_engine(&entries[i], &engines) {
            i += 1;
            continue;
        }
        if !is_step(&entries[i]) {
            out.push(Row::Entry(i));
            i += 1;
            continue;
        }
        let first = i;
        let mut calls = Vec::new();
        let mut requests = Vec::new();
        while i < entries.len() && is_step(&entries[i]) && !in_engine(&entries[i], &engines) {
            match entries[i].get("kind").and_then(Value::as_str) {
                Some("model_response") => calls.extend(calls_of(&entries[i])),
                Some("tool_request") => requests.extend(
                    entries[i]
                        .get("data")
                        .and_then(|d| d.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ),
                _ => {}
            }
            i += 1;
        }
        // A turn that names its tools and the requests for them are the same calls: the turns count.
        let names = if calls.is_empty() { requests } else { calls };
        let count = if names.is_empty() {
            i - first
        } else {
            names.len()
        };
        out.push(Row::Steps {
            first,
            end: i,
            count,
            summary: steps_label(count, &names),
        });
    }
    out
}

/// One tool a coding engine has called, as its stream reports it. `id` is the engine's tool call id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineTool {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    /// Where the tool works: `path` or `path:line`.
    pub locations: Vec<String>,
}

/// One step of the engine's plan, and whether it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePlanItem {
    pub content: String,
    pub done: bool,
}

/// A check the engine holds for the owner's approval: what it is, and the approval's id (what Approve and Deny
/// decide). The id is empty when the decision did not name one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePending {
    pub title: String,
    pub approval: String,
}

/// A code_engine call as the chat draws it: the engine and session it runs, its status, what it has said, the
/// tools and plan it reports, and the checks waiting for the owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineView {
    pub engine: String,
    pub session: String,
    /// "running", "waiting for approval", "done" or "failed".
    pub status: String,
    /// The engine's messages, in order.
    pub message: String,
    pub tools: Vec<EngineTool>,
    pub plan: Vec<EnginePlanItem>,
    pub pending: Vec<EnginePending>,
    /// "claude · 3f9a2c1b · done": the header line.
    pub summary: String,
}

/// A nested check of an engine call: its request's title and its latest decision.
#[derive(Debug, Default)]
struct Check {
    call: String,
    title: String,
    decision: String,
    output: String,
}

/// The text at `key` of a value, or "".
fn text_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Sets `slot` to the string at `key` of `v`, when there is one: an update changes only what it names.
fn put(slot: &mut String, v: &Value, key: &str) {
    if let Some(s) = v.get(key).and_then(Value::as_str) {
        *slot = s.to_string();
    }
}

/// The approval id a pending decision names: "waiting for the owner's approval (ap_1)".
fn approval_id(output: &str) -> String {
    match (output.rfind('('), output.rfind(')')) {
        (Some(open), Some(close)) if open < close => output[open + 1..close].trim().to_string(),
        _ => String::new(),
    }
}

/// A location as the view shows it: a bare path, or `path:line`.
fn location(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    let path = v.get("path")?.as_str()?;
    Some(match v.get("line").and_then(Value::as_u64) {
        Some(line) => format!("{path}:{line}"),
        None => path.to_string(),
    })
}

/// Folds one `engine_update` into the view. A message item's text is appended; a plan item replaces the plan; a
/// tool call or status item sets the tool it names. The payload is the entry's `update`, or the entry itself.
fn apply_update(
    data: &Value,
    text: &mut String,
    plan: &mut Vec<EnginePlanItem>,
    tools: &mut Vec<EngineTool>,
) {
    let upd = data.get("update").filter(|u| !u.is_null()).unwrap_or(data);
    match text_of(data, "item") {
        "message" => text.push_str(text_of(upd, "text")),
        "plan" => {
            *plan = upd
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| EnginePlanItem {
                    content: text_of(e, "content").to_string(),
                    done: text_of(e, "status") == "completed",
                })
                .collect();
        }
        "tool_call" | "tool_status" => {
            let id = text_of(upd, "toolCallId");
            let i = match tools.iter().position(|t| t.id == id) {
                Some(i) => i,
                None => {
                    tools.push(EngineTool {
                        id: id.to_string(),
                        ..Default::default()
                    });
                    tools.len() - 1
                }
            };
            let tool = &mut tools[i];
            put(&mut tool.title, upd, "title");
            put(&mut tool.kind, upd, "kind");
            put(&mut tool.status, upd, "status");
            if let Some(locs) = upd.get("locations").and_then(Value::as_array) {
                tool.locations = locs.iter().filter_map(location).collect();
            }
        }
        _ => {}
    }
}

/// The short form of a session id, for the header line.
fn short_session(session: &str) -> String {
    session.chars().take(8).collect()
}

/// What a code_engine call shows, folded from the transcript: its request (the engine and session), its nested
/// checks (the ones still pending are the approvals it waits for), its stream (`engine_update`), and its result.
/// Entries of other calls are ignored.
pub fn engine_view(entries: &[Value], call: &str) -> EngineView {
    let mut view = EngineView::default();
    let mut result: Option<bool> = None;
    let mut checks: Vec<Check> = Vec::new();
    let mut tools: Vec<EngineTool> = Vec::new();
    for entry in entries {
        let data = entry.get("data").unwrap_or(&Value::Null);
        let own = text_of(data, "call");
        if engine_of(own) != call {
            continue;
        }
        let nested = own != call;
        let kind = text_of(entry, "kind");
        let index = |checks: &mut Vec<Check>| match checks.iter().position(|c| c.call == own) {
            Some(i) => i,
            None => {
                checks.push(Check {
                    call: own.to_string(),
                    ..Default::default()
                });
                checks.len() - 1
            }
        };
        match kind {
            "tool_request" if !nested => {
                let input = data.get("input").unwrap_or(&Value::Null);
                view.engine = text_of(input, "engine").to_string();
                view.session = text_of(input, "session").to_string();
            }
            "tool_request" => {
                let i = index(&mut checks);
                let input = data.get("input").unwrap_or(&Value::Null);
                checks[i].title = text_of(input, "title").to_string();
            }
            "tool_decision" if nested => {
                let i = index(&mut checks);
                checks[i].decision = text_of(data, "decision").to_string();
                checks[i].output = text_of(data, "output").to_string();
            }
            "tool_result" if !nested => {
                result = Some(data.get("ok").and_then(Value::as_bool).unwrap_or(false));
            }
            "engine_update" => apply_update(data, &mut view.message, &mut view.plan, &mut tools),
            _ => {}
        }
    }
    view.pending = checks
        .iter()
        .filter(|c| c.decision == "pending")
        .map(|c| EnginePending {
            title: if c.title.is_empty() {
                "a check".to_string()
            } else {
                c.title.clone()
            },
            approval: approval_id(&c.output),
        })
        .collect();
    view.tools = tools;
    view.status = match result {
        Some(true) => "done",
        Some(false) => "failed",
        None if !view.pending.is_empty() => "waiting for approval",
        None => "running",
    }
    .to_string();
    let short = short_session(&view.session);
    view.summary = [view.engine.as_str(), short.as_str(), view.status.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    view
}

/// Where an agent's time went: thinking (the model's turns) and tools, with each tool call and its duration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub thinking_ms: u64,
    pub tools_ms: u64,
    pub calls: Vec<Call>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    pub ms: u64,
}

/// Splits the transcript's time. Thinking runs from the work's start, or from the last tool result, to the model's
/// reply. A tool runs from its request to its decision or result, matched by the call's id.
pub fn activity(entries: &[Value]) -> Activity {
    let mut out = Activity::default();
    let mut since: Option<u64> = None;
    let mut open: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for entry in entries {
        let at = entry.get("at_ms").and_then(Value::as_u64).unwrap_or(0);
        let data = entry.get("data").unwrap_or(&Value::Null);
        match entry.get("kind").and_then(Value::as_str).unwrap_or("") {
            "work_start" => since = Some(at),
            "model_response" => {
                if let Some(start) = since.take() {
                    out.thinking_ms += at.saturating_sub(start);
                }
            }
            "tool_request" => {
                let call = data
                    .get("call")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                open.insert(call, (name, at));
            }
            "tool_decision" | "tool_result" => {
                let call = data.get("call").and_then(Value::as_str).unwrap_or_default();
                if let Some((name, start)) = open.remove(call) {
                    let ms = at.saturating_sub(start);
                    out.tools_ms += ms;
                    out.calls.push(Call { name, ms });
                }
                since = Some(at);
            }
            "work_end" => since = None,
            _ => {}
        }
    }
    out
}

// ---- The palette's filter --------------------------------------------------------------------

/// The indices of the labels the query matches: every word of the query must appear in the label, ignoring case.
/// An empty query matches all, in order.
pub fn filter(labels: &[String], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    labels
        .iter()
        .enumerate()
        .filter(|(_, label)| {
            let label = label.to_lowercase();
            words.iter().all(|w| label.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_dozing_agent_has_the_sleeping_dot() {
        assert!(matches!(dot("dozing", None), Dot::Sleeping));
        assert!(matches!(dot("asleep", None), Dot::Sleeping));
        assert!(matches!(dot("idle", None), Dot::Idle));
    }

    /// A code_engine call as the plaza records it: a request, a check held for approval, the engine's stream,
    /// then its result and a closing message.
    fn engine_run(decision: &str) -> Vec<Value> {
        vec![
            json!({"kind": "model_response", "data": {"content": [{"type": "tool_use", "name": "code_engine"}]}}),
            json!({"kind": "tool_request", "data": {"call": "c1", "name": "code_engine",
                "input": {"engine": "claude", "prompt": "fix it", "session": "sess-0123456789"}}}),
            json!({"kind": "engine_update", "data": {"call": "c1", "item": "message", "text": "Looking"}}),
            json!({"kind": "tool_request", "data": {"call": "c1.e1", "name": "engine.edit",
                "input": {"engine": "claude", "title": "Edit lib.rs", "locations": [], "raw": {}}}}),
            json!({"kind": "tool_decision", "data": {"call": "c1.e1", "decision": decision,
                "output": "waiting for the owner's approval (ap_9)"}}),
            json!({"kind": "engine_update", "data": {"call": "c1", "item": "tool_call",
                "update": {"toolCallId": "t1", "title": "Read main.rs", "kind": "read", "status": "pending",
                    "locations": [{"path": "src/main.rs", "line": 3}]}}}),
            json!({"kind": "engine_update", "data": {"call": "c1", "item": "tool_status",
                "update": {"toolCallId": "t1", "status": "completed"}}}),
            json!({"kind": "engine_update", "data": {"call": "c1", "item": "plan",
                "update": [{"content": "Read", "status": "completed"}, {"content": "Edit", "status": "in_progress"}]}}),
            json!({"kind": "tool_result", "data": {"call": "c1", "ok": true, "output": "done"}}),
            json!({"kind": "model_response", "data": {"content": [{"type": "text", "text": "Done."}]}}),
        ]
    }

    #[test]
    fn a_finished_engine_view_has_its_tools_plan_message_and_result() {
        let view = engine_view(&engine_run("allow"), "c1");
        assert_eq!(view.engine, "claude");
        assert_eq!(view.session, "sess-0123456789");
        assert_eq!(view.status, "done");
        assert_eq!(view.message, "Looking");
        assert_eq!(view.summary, "claude · sess-012 · done");
        assert_eq!(
            view.tools,
            vec![EngineTool {
                id: "t1".into(),
                title: "Read main.rs".into(),
                kind: "read".into(),
                status: "completed".into(),
                locations: vec!["src/main.rs:3".into()],
            }]
        );
        assert_eq!(
            view.plan,
            vec![
                EnginePlanItem {
                    content: "Read".into(),
                    done: true,
                },
                EnginePlanItem {
                    content: "Edit".into(),
                    done: false,
                },
            ]
        );
        assert!(view.pending.is_empty(), "an allowed check is not waiting");
    }

    #[test]
    fn a_check_still_pending_is_the_approval_the_engine_waits_for() {
        let entries = engine_run("pending");
        // Before the result the engine is waiting; the pending check names the approval to decide.
        let view = engine_view(&entries[..8], "c1");
        assert_eq!(view.status, "waiting for approval");
        assert_eq!(
            view.pending,
            vec![EnginePending {
                title: "Edit lib.rs".into(),
                approval: "ap_9".into(),
            }]
        );
        // The engine's result ends the wait even when the check was never decided in the chat.
        assert_eq!(engine_view(&entries, "c1").status, "done");
    }

    #[test]
    fn a_call_id_belongs_to_its_engine_only_through_a_numbered_check_suffix() {
        assert_eq!(engine_of("c1.e2"), "c1");
        assert_eq!(engine_of("c1.e"), "c1.e", "no check number, no owner");
        assert_eq!(engine_of("c1.ex"), "c1.ex");
        assert_eq!(engine_of("c10"), "c10");
    }

    #[test]
    fn a_station_record_reads_its_account_and_only_the_owners_gets_a_warning() {
        let owner = serde_json::json!({ "id": "w3", "facts": { "account": { "mode": "owner", "name": "ana" } } });
        let separate = serde_json::json!({ "id": "w4", "facts": { "account": { "mode": "agent_user", "name": "agent" } } });
        assert_eq!(
            parse_station(&owner),
            Some(StationRow {
                id: "w3".into(),
                owner_account: Some("ana".into())
            })
        );
        assert_eq!(parse_station(&separate).and_then(|s| s.owner_account), None);
        assert_eq!(
            parse_station(&serde_json::json!({ "facts": {} })),
            None,
            "a record without an id is no station"
        );
    }

    #[test]
    fn a_roster_entry_reads_its_work_and_its_usage() {
        let v = json!({
            "cid": 4, "role": "builder", "role_label": "Builder", "status": "working",
            "model": "m1", "last_seen_ms": 5000, "usage": {"input_tokens": 10, "output_tokens": 5},
            "work": {"kind": "task", "prompt": "Fix the parser"}
        });
        let row = parse_agent(&v).expect("an agent");
        assert_eq!(
            (row.cid, row.label.as_str(), row.tokens),
            (4, "Builder #4", 15)
        );
        assert_eq!(row.role_label, "Builder");
        assert_eq!(row.task.as_deref(), Some("Fix the parser"));
        assert!(parse_agent(&json!({"status": "working"})).is_none());
    }

    #[test]
    fn an_agent_is_under_the_project_it_works_on_and_project_mode_shows_that_one() {
        let agent = |cid: u64, project: Option<&str>, orchestrator: bool| {
            let mut a = parse_agent(&json!({ "cid": cid, "project": project })).expect("an agent");
            a.orchestrator = orchestrator;
            a
        };
        // Agent 1 works on p2; agent 2 works on no project, though it wrote p1's brief; agent 9, the
        // orchestrator, leads p2.
        let agents = vec![
            agent(1, Some("p2"), false),
            agent(2, None, false),
            agent(9, None, true),
        ];
        let p1 = parse_project(&json!({ "id": "p1", "title": "One", "docs": [{ "by_cid": 2 }] }))
            .expect("p1");
        let p2 =
            parse_project(&json!({ "id": "p2", "title": "Two", "repo": "two", "lead_cid": 9 }))
                .expect("p2");
        let projects = vec![p1, p2];
        let repos = vec![("repo:two@main".to_string(), "two".to_string())];
        let g = groups(&agents, &projects, &repos, &[]);
        assert_eq!(
            g[0].agents,
            vec![1, 9],
            "p2: its agent, and its lead on no project"
        );
        assert_eq!(g[0].repos, vec!["repo:two@main".to_string()]);
        assert_eq!(g[1].key, "other");
        assert_eq!(
            g[1].agents,
            vec![2],
            "an author of p1's brief is under no project"
        );
        let mode = project_group(&agents, &projects, &repos, "p2");
        assert_eq!(mode.len(), 1);
        assert_eq!(mode[0].agents, vec![1, 9], "its agent and the orchestrator");
        assert_eq!(mode[0].repos, vec!["repo:two@main".to_string()]);
        assert!(project_group(&agents, &projects, &repos, "p9").is_empty());
    }

    #[test]
    fn a_project_lists_its_lead_its_repo_and_its_tasks() {
        let v = json!({
            "id": "p1", "title": "Plaza", "stage": "build", "repo": "plaza", "tasks": ["t1", "t2"], "lead_cid": 7,
            "docs": [{"by_cid": 3}], "critiques": [{"by_cid": 5}], "history": [{"by_cid": 3}, {"by": "owner"}]
        });
        let row = parse_project(&v).expect("a project");
        assert_eq!(
            row.agents.iter().copied().collect::<Vec<_>>(),
            vec![7],
            "its authors may work elsewhere now"
        );
        assert_eq!(row.repo.as_deref(), Some("plaza"));
        assert_eq!(row.tasks.len(), 2);
    }
}

#[cfg(test)]
mod step_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_run_of_steps_is_labelled_by_its_last_step_and_the_outcome_of_all() {
        let entries = vec![
            json!({"kind": "tool_request", "at_ms": 0, "data": {"id": "a", "name": "read", "input": {"files": [{"path": "src/main.rs"}]}}}),
            json!({"kind": "tool_result", "at_ms": 420, "data": {"id": "a"}}),
            json!({"kind": "tool_request", "at_ms": 500, "data": {"id": "b", "name": "run", "input": {"command": "cargo test\nmore"}}}),
            json!({"kind": "tool_result", "at_ms": 2600, "data": {"id": "b", "is_error": true}}),
        ];
        let steps = steps_of(&entries);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].ms, Some(420));
        assert_eq!(step_label(&steps[1]), "Ran command");
        assert_eq!(steps[1].detail, "cargo test");
        assert_eq!(group_outcome(&steps), "mixed");
        assert_eq!(format_ms(2100), "2.1 s");
    }
}
