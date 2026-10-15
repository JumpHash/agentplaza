//! The IDE's view model: pure functions over the hub's answers (the roster, the projects, the tasks and an
//! agent's transcript). The grouping of the sidebar, the status dots, the collapsing of tool steps, the
//! activity split and the palette's filter are decided here, so they are tested without iced.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

// ---- The roster and the projects --------------------------------------------------------------

/// One agent as the sidebar shows it (`GET /v1/agents`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub cid: u64,
    pub name: String,
    pub role_label: String,
    pub model: String,
    pub status: String,
    /// The prompt of the work it is doing now, if any.
    pub task: Option<String>,
    pub last_seen_ms: u64,
    pub tokens: u64,
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
        name: text(v, "name"),
        role_label,
        model: text(v, "model"),
        status: text(v, "status"),
        task,
        last_seen_ms: v.get("last_seen_ms").and_then(Value::as_u64).unwrap_or(0),
        tokens: usage("input_tokens") + usage("output_tokens"),
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
    pub members: BTreeSet<u64>,
}

/// Reads one project; the agents are the `by_cid` of its documents, critiques and history.
pub fn parse_project(v: &Value) -> Option<ProjectRow> {
    let id = v.get("id")?.as_str()?.to_string();
    let mut members = BTreeSet::new();
    for key in ["docs", "critiques", "history"] {
        for item in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            if let Some(cid) = item.get("by_cid").and_then(Value::as_u64) {
                members.insert(cid);
            }
        }
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
        members,
    })
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
/// the projects, as the web IDE has them); then the agents no project names, under "Other agents". `repos` are
/// (workspace id, repo name) pairs; a repo no project owns goes under "Repositories", last. `stations` are the
/// workstations' workspace ids.
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
        let members: Vec<u64> = agents
            .iter()
            .map(|a| a.cid)
            .filter(|cid| project.members.contains(cid) && !placed.contains(cid))
            .collect();
        let mut owned: Vec<String> = Vec::new();
        for (id, name) in repos {
            if project.repo.as_deref() == Some(name.as_str()) && !placed_repos.contains(name) {
                placed_repos.insert(name.clone());
                owned.push(id.clone());
            }
        }
        if members.is_empty() && owned.is_empty() {
            continue;
        }
        placed.extend(members.iter().copied());
        out.push(Group {
            key: format!("project:{}", project.id),
            title: if project.title.is_empty() {
                project.id.clone()
            } else {
                project.title.clone()
            },
            project: Some(project.id.clone()),
            agents: members,
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

/// The dot for an agent from the hub's status (`working`, `idle`, `asleep`, `offline`) and the outcome of its last work
/// (`done`, `failed`, `error`, or none when the transcript does not say).
pub fn dot(status: &str, last_outcome: Option<&str>) -> Dot {
    match status {
        "working" => Dot::Working,
        "offline" | "asleep" => Dot::Sleeping,
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

/// A row of the chat: one transcript entry, or a run of tool steps collapsed into one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Entry(usize),
    Steps {
        first: usize,
        count: usize,
        summary: String,
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

/// "3 steps · bash, edit_file": the step line's text. A run of one reads "1 step".
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

/// The chat's rows: each entry as it is, except that a run of tool steps becomes one `Steps` row. The run is
/// counted by the calls its model turns make; a run that names no tool (only requests) is counted by its entries.
pub fn rows(entries: &[Value]) -> Vec<Row> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        if !is_step(&entries[i]) {
            out.push(Row::Entry(i));
            i += 1;
            continue;
        }
        let first = i;
        let mut calls = Vec::new();
        let mut requests = Vec::new();
        while i < entries.len() && is_step(&entries[i]) {
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
            count,
            summary: steps_label(count, &names),
        });
    }
    out
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

    fn agent(cid: u64, status: &str) -> AgentRow {
        AgentRow {
            cid,
            name: format!("a{cid}"),
            role_label: String::new(),
            model: String::new(),
            status: status.into(),
            task: None,
            last_seen_ms: 0,
            tokens: 0,
        }
    }

    fn project(id: &str, members: &[u64], repo: Option<&str>) -> ProjectRow {
        ProjectRow {
            id: id.into(),
            title: format!("P {id}"),
            stage: String::new(),
            repo: repo.map(str::to_string),
            tasks: Vec::new(),
            members: members.iter().copied().collect(),
        }
    }

    #[test]
    fn an_agent_joins_the_first_project_that_names_it_and_the_rest_are_other_agents() {
        let agents = vec![agent(1, "idle"), agent(2, "idle"), agent(3, "idle")];
        let projects = vec![project("a", &[1, 2], None), project("b", &[2, 3], None)];
        let groups = groups(&agents, &projects, &[], &[]);
        assert_eq!(
            groups.len(),
            2,
            "project b keeps only agent 3 and no other agent is left over"
        );
        assert_eq!(groups[0].agents, vec![1, 2]);
        assert_eq!(groups[1].agents, vec![3]);
        assert_eq!(groups[1].count(), 1);
    }

    #[test]
    fn agents_no_project_names_go_under_other_agents_and_empty_projects_are_left_out() {
        let agents = vec![agent(1, "idle"), agent(9, "working")];
        let projects = vec![project("a", &[1], None), project("empty", &[], None)];
        let groups = groups(&agents, &projects, &[], &[]);
        assert_eq!(
            groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(),
            vec!["project:a", "other"]
        );
        assert_eq!(groups[1].agents, vec![9]);
        assert_eq!(groups[1].title, "Other agents");
    }

    #[test]
    fn a_repo_belongs_to_the_project_of_that_name_or_to_repositories() {
        let repos = vec![
            ("repo:web@main".to_string(), "web".to_string()),
            ("repo:tools@main".to_string(), "tools".to_string()),
        ];
        let projects = vec![project("p", &[], Some("web"))];
        let groups = groups(&[], &projects, &repos, &[]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].repos, vec!["repo:web@main".to_string()]);
        assert_eq!(groups[1].key, "repos");
        assert_eq!(groups[1].repos, vec!["repo:tools@main".to_string()]);
    }

    #[test]
    fn the_workstations_come_after_the_projects_and_before_other_agents() {
        let agents = vec![agent(1, "idle"), agent(9, "working")];
        let projects = vec![project("a", &[1], None)];
        let stations = vec!["station:w3".to_string(), "station:w4".to_string()];
        let groups = groups(&agents, &projects, &[], &stations);
        assert_eq!(
            groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(),
            vec!["project:a", "workstations", "other"]
        );
        assert_eq!(groups[1].title, "Workstations");
        assert_eq!(groups[1].stations, stations);
        assert_eq!(groups[1].count(), 2, "a workstation counts in its header");
        assert!(
            groups[1].agents.is_empty(),
            "no agent is filed under a workstation"
        );
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
    fn a_station_os_reads_as_a_short_label() {
        assert_eq!(os_label(Some("macos")), "macOS");
        assert_eq!(os_label(Some("windows")), "Windows");
        assert_eq!(os_label(Some("linux")), "Linux");
        assert_eq!(os_label(None), "Unknown OS");
        assert_eq!(os_label(Some("plan9")), "plan9");
    }

    #[test]
    fn the_status_dot_follows_the_hub_and_the_last_outcome() {
        assert_eq!(dot("working", Some("failed")), Dot::Working);
        assert_eq!(dot("offline", None), Dot::Sleeping);
        assert_eq!(dot("idle", None), Dot::Idle);
        assert_eq!(dot("idle", Some("done")), Dot::Done);
        assert_eq!(dot("idle", Some("error")), Dot::Failed);
        assert_eq!(dot("idle", Some("cancelled")), Dot::Idle);
        assert_eq!(Dot::Sleeping.label(), "sleeping");
    }

    #[test]
    fn the_last_outcome_is_the_latest_work_end() {
        let entries = vec![
            json!({"kind": "work_end", "data": {"status": "done"}}),
            json!({"kind": "model_response"}),
            json!({"kind": "work_end", "data": {"status": "failed"}}),
        ];
        assert_eq!(last_outcome(&entries).as_deref(), Some("failed"));
        assert_eq!(last_outcome(&entries[..1]).as_deref(), Some("done"));
        assert_eq!(last_outcome(&[]), None);
    }

    #[test]
    fn a_roster_entry_reads_its_work_and_its_usage() {
        let v = json!({
            "cid": 4, "name": "Ada", "role": "dev", "role_label": "Developer", "status": "working",
            "model": "m1", "last_seen_ms": 5000, "usage": {"input_tokens": 10, "output_tokens": 5},
            "work": {"kind": "task", "prompt": "Fix the parser"}
        });
        let row = parse_agent(&v).expect("an agent");
        assert_eq!(
            (row.cid, row.role_label.as_str(), row.tokens),
            (4, "Developer", 15)
        );
        assert_eq!(row.task.as_deref(), Some("Fix the parser"));
        assert!(parse_agent(&json!({"name": "no cid"})).is_none());
    }

    #[test]
    fn a_project_lists_its_authors_its_repo_and_its_tasks() {
        let v = json!({
            "id": "p1", "title": "Plaza", "stage": "build", "repo": "plaza", "tasks": ["t1", "t2"],
            "docs": [{"by_cid": 3}], "critiques": [{"by_cid": 5}], "history": [{"by_cid": 3}, {"by": "owner"}]
        });
        let row = parse_project(&v).expect("a project");
        assert_eq!(row.members.iter().copied().collect::<Vec<_>>(), vec![3, 5]);
        assert_eq!(row.repo.as_deref(), Some("plaza"));
        assert_eq!(row.tasks.len(), 2);
    }

    #[test]
    fn a_run_of_tool_calls_is_one_step_line_and_text_breaks_it() {
        let entries = vec![
            json!({"kind": "work_start", "data": {"prompt": "Read the board"}}),
            json!({"kind": "model_response", "data": {"content": [{"type": "tool_use", "name": "board_read"}]}}),
            json!({"kind": "tool_request", "data": {"call": "c1", "name": "board_read"}}),
            json!({"kind": "tool_decision", "data": {"call": "c1"}}),
            json!({"kind": "model_response", "data": {"content": [{"type": "tool_use", "name": "bash"}, {"type": "tool_use", "name": "bash"}]}}),
            json!({"kind": "model_response", "data": {"content": [{"type": "text", "text": "Done."}]}}),
        ];
        let rows = rows(&entries);
        assert_eq!(
            rows,
            vec![
                Row::Entry(0),
                Row::Steps {
                    first: 1,
                    count: 3,
                    summary: "3 steps · board_read, bash".into()
                },
                Row::Entry(5),
            ]
        );
    }

    #[test]
    fn the_step_label_counts_and_names_the_tools() {
        assert_eq!(steps_label(1, &["bash".into()]), "1 step · bash");
        assert_eq!(
            steps_label(
                2,
                &[
                    "bash".into(),
                    "bash".into(),
                    "grep".into(),
                    "edit".into(),
                    "x".into()
                ]
            ),
            "2 steps · bash, grep, edit"
        );
        assert_eq!(steps_label(0, &[]), "0 steps");
    }

    #[test]
    fn activity_splits_thinking_from_tools_and_times_each_call() {
        let entries = vec![
            json!({"at_ms": 1000, "kind": "work_start", "data": {}}),
            json!({"at_ms": 3000, "kind": "model_response", "data": {"content": []}}),
            json!({"at_ms": 3000, "kind": "tool_request", "data": {"call": "c1", "name": "bash"}}),
            json!({"at_ms": 3600, "kind": "tool_decision", "data": {"call": "c1"}}),
            json!({"at_ms": 4000, "kind": "model_response", "data": {"content": []}}),
            json!({"at_ms": 4500, "kind": "work_end", "data": {"status": "done"}}),
        ];
        let activity = activity(&entries);
        assert_eq!(activity.thinking_ms, 2000 + 400);
        assert_eq!(activity.tools_ms, 600);
        assert_eq!(
            activity.calls,
            vec![Call {
                name: "bash".into(),
                ms: 600
            }]
        );
    }

    #[test]
    fn the_palette_matches_every_word_ignoring_case() {
        let labels: Vec<String> = [
            "Agent Ada",
            "src/main.rs",
            "Fix the parser task",
            "Toggle drawer",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(filter(&labels, ""), vec![0, 1, 2, 3]);
        assert_eq!(filter(&labels, "ADA"), vec![0]);
        assert_eq!(filter(&labels, "fix parser"), vec![2]);
        assert_eq!(filter(&labels, "fix drawer"), Vec::<usize>::new());
    }

    #[test]
    fn ages_read_in_minutes_hours_and_days() {
        assert_eq!(ago(10_000, 0), "never seen");
        assert_eq!(ago(10_000, 5_000), "just now");
        assert_eq!(ago(601_000, 1_000), "10 min ago");
        assert_eq!(ago(3 * 3_600_000 + 1_000, 1_000), "3 h ago");
        assert_eq!(ago(2 * 86_400_000 + 1_000, 1_000), "2 d ago");
    }
}
