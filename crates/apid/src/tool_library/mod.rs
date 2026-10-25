//! The tool library: every capability the plaza has, as one tree of entries with dotted paths. Agents explore it with
//! `tools_list`, `tools_describe` and `tools_search`, and run an entry with `tools_call`; the owner browses the same
//! tree in the web IDE (`routes.rs`).
//!
//! An entry is a tool the harness or the hub already runs (a host tool, an in-VM tool, a connector action), a hub
//! route answered in-process, or held text (a toolbox discovery, a recipe's outline). Sources add entries through
//! [`Source`]: the built-in ones (`sources.rs`) read the live registries at runtime, and a subsystem added later calls
//! [`register`] once at startup. A group's summary is the one a source gives it, else the one in [`GROUPS`], else what
//! is under it.
//!
//! A call goes through the path a native call takes: the laws refuse what they forbid, the owner's permission mode and
//! approvals gate it (a card is raised), then the entry's own runner runs it. A seat sees only what its laws and its
//! connectors allow; `tools_describe` says why an entry is unavailable.

pub mod routes;
pub mod sources;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, Once, PoisonError};

use apz_http::{Request, Response};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::agents::Agent;
use crate::approvals::{self, Gate, Held};
use crate::connectors;
use crate::laws;
use crate::state::App;
use crate::tools;

/// The tools an agent uses to explore and call the library. Served here: the hub runs them for the calling seat.
pub const TOOLS: &[&str] = &["tools_list", "tools_describe", "tools_search", "tools_call"];

/// The most entries one search answers with.
const MAX_HITS: usize = 12;

/// Where an entry runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runs {
    /// In the agent's VM: the harness runs it.
    Guest,
    /// On the hub, under the calling agent's identity.
    Hub,
}

impl Runs {
    fn as_str(self) -> &'static str {
        match self {
            Runs::Guest => "guest",
            Runs::Hub => "hub",
        }
    }
}

/// Who may use an entry beyond what its own tool or connector decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Any seat its tool, connector or laws allow.
    Any,
    /// Only the orchestrator: what changes a deploy, a release or the plaza's work as a whole.
    Operator,
}

/// How a call of an entry runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exec {
    /// A hub host tool, run by `tools::run_host` under the caller's identity.
    Host(String),
    /// An in-VM tool: the harness runs it (`apz_proto::chat::GUEST_TOOL_PATHS`); the hub only describes it.
    Guest(String),
    /// A connector action (`connectors::call`).
    Connector { connector: String, action: String },
    /// A hub route, answered in-process by the router with the call's arguments as its path, query or body.
    Route {
        method: &'static str,
        pattern: &'static str,
    },
    /// Held text: what a call returns (a toolbox discovery, a recipe's outline).
    Text(String),
}

impl Exec {
    fn label(&self) -> String {
        match self {
            Exec::Host(name) => format!("host tool {name}"),
            Exec::Guest(name) => format!("in-VM tool {name}"),
            Exec::Connector { connector, action } => {
                format!("connector {connector}, action {action}")
            }
            Exec::Route { method, pattern } => format!("route {method} {pattern}"),
            Exec::Text(_) => "held text".to_string(),
        }
    }
}

/// One entry of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The dotted path: `code.read`, `connectors.github.repos`, `deploy.rollback`.
    pub path: String,
    /// One line on what it does.
    pub summary: String,
    /// What it does, what it needs and what it returns.
    pub description: String,
    pub runs: Runs,
    /// The source that adds it: `harness`, `hub`, `connector`, `route`, `recipe`, `toolbox`.
    pub source: &'static str,
    pub input_schema: Value,
    pub output_schema: Value,
    pub access: Access,
    /// Whether the call waits for the owner: an irreversible action or one the owner's mode holds.
    pub approval: bool,
    /// A cost hint: `free`, `cheap` (a read), `metered` (a provider or a model is billed) or `slow` (minutes or more).
    pub cost: &'static str,
    pub exec: Exec,
}

/// A named group of the tree, with what is under it.
pub struct Group {
    pub path: &'static str,
    pub summary: &'static str,
}

/// The summaries of the groups the built-in sources fill. A group that has none gets a summary of what is under it.
pub const GROUPS: &[Group] = &[
    Group {
        path: "code",
        summary: "Read, edit, search and test code in your own VM.",
    },
    Group {
        path: "shell",
        summary: "Run commands and scripts in your own VM.",
    },
    Group {
        path: "helpers",
        summary: "Start helpers that work in parallel, and wait for them.",
    },
    Group {
        path: "files",
        summary: "Files you hand in, get, share or deliver.",
    },
    Group {
        path: "web",
        summary: "Fetch pages and download files from the internet through the plaza's proxy.",
    },
    Group {
        path: "packages",
        summary: "Install software in your own VM from the registries the plaza allows.",
    },
    Group {
        path: "todo",
        summary: "Your own checklist for the work in hand.",
    },
    Group {
        path: "artifacts",
        summary: "Documents, data and live pages you make for people to read or use.",
    },
    Group {
        path: "checkpoints",
        summary: "Checkpoints of your VM: list them, diff them, go back to one.",
    },
    Group {
        path: "tools",
        summary: "The tool library itself, and tools you package for the plaza.",
    },
    Group {
        path: "toolbox",
        summary: "What the plaza offers and what other agents found: tools, services, techniques.",
    },
    Group {
        path: "memory",
        summary: "The project's memory: search, read, propose and record what it has settled.",
    },
    Group {
        path: "journal",
        summary: "Your journal, your handover and the brief you start from.",
    },
    Group {
        path: "sessions",
        summary: "Sessions and their transcripts: what was done and said before.",
    },
    Group {
        path: "board",
        summary: "The plaza's board: post, read and comment.",
    },
    Group {
        path: "messages",
        summary: "Messages to other agents and seats.",
    },
    Group {
        path: "threads",
        summary: "Threads of a work item: read them and answer the owner.",
    },
    Group {
        path: "team",
        summary: "Your team's settings.",
    },
    Group {
        path: "tasks",
        summary: "Tasks: list, post, assign, link, cancel and record results.",
    },
    Group {
        path: "agents",
        summary: "Agents: list them, propose one, and control the ones you may.",
    },
    Group {
        path: "projects",
        summary: "Projects: open, plan, approve, advance, attach, critique, and their records.",
    },
    Group {
        path: "maintenance",
        summary: "A project's maintenance: its map, signals and proposed work.",
    },
    Group {
        path: "environments",
        summary: "A project's environments: where its versions run.",
    },
    Group {
        path: "environments.ephemeral",
        summary: "Ephemeral environments: short-lived instances of a version, up for a test and torn down at their TTL.",
    },
    Group {
        path: "deploy",
        summary: "Deploys and rollbacks: put a version in an environment, or take it back.",
    },
    Group {
        path: "workflows",
        summary: "Workflows: the CI/CD pipelines a project defines in .plaza/workflows, their runs, and imports from GitHub Actions.",
    },
    Group {
        path: "workflows.changes",
        summary: "The workflow runs of a change: its pipelines' test and build results.",
    },
    Group {
        path: "workflows.import",
        summary: "Import a GitHub Actions workflow as one of the project's workflows, through a change.",
    },
    Group {
        path: "releases",
        summary: "Releases: draft, candidate, checks, publish, promote, yank, notes and sync.",
    },
    Group {
        path: "qualify",
        summary: "Qualification of a release candidate: its depth, its stages and their reports.",
    },
    Group {
        path: "lab",
        summary: "The test lab: qualification runs, their stages, observations and tests.",
    },
    Group {
        path: "user_test",
        summary: "User testing: drive a built app the way a person would and report what breaks.",
    },
    Group {
        path: "incidents",
        summary: "Incidents: the alerts a project's environments raised, and their resolution.",
    },
    Group {
        path: "upgrades",
        summary: "Dependency upgrades: plan them, and start a fix for an advisory.",
    },
    Group {
        path: "cves",
        summary: "The CVE report of a project: its findings, their releases and environments.",
    },
    Group {
        path: "evals",
        summary: "Evaluations: the scores, costs, leaderboard and benchmark of step sessions and reviewers.",
    },
    Group {
        path: "recipes",
        summary: "Recipes: the review and CI recipes, their runs, and the triggers that start them.",
    },
    Group {
        path: "reviews",
        summary: "Review runs: their steps, scores, findings and verdicts.",
    },
    Group {
        path: "weft",
        summary: "Weft, the plaza's version control: changes, diffs, blame, change trees and bisects.",
    },
    Group {
        path: "tests",
        summary: "Tests: select the ones a change needs, and read a project's test history.",
    },
    Group {
        path: "observe",
        summary: "What the plaza observed: logs, metrics and traces, read back.",
    },
    Group {
        path: "connectors",
        summary: "Outside services, each as a connector with its actions: GitHub, Kubernetes, AWS, registries and more.",
    },
    Group {
        path: "stations",
        summary: "Workstations: lease one, act on its screen and shell, read its files and recordings.",
    },
    Group {
        path: "nodes",
        summary: "The plaza's machines: list them, read their stats, move work between them, stop or resume them.",
    },
    Group {
        path: "previews",
        summary: "Previews of a project's running apps, and the links that share them.",
    },
    Group {
        path: "triggers",
        summary: "Triggers: the events that start a recipe's run.",
    },
    Group {
        path: "loops",
        summary: "Scheduled loops that prompt an agent on a timetable.",
    },
    Group {
        path: "intake",
        summary: "Intake: the work items other systems hand the plaza, and their connections.",
    },
    Group {
        path: "laws",
        summary: "The laws the plaza enforces, and the owner's rules and overrides.",
    },
    Group {
        path: "metrics",
        summary: "Metrics: the catalog, queries, alerts and service-level objectives.",
    },
    Group {
        path: "skills",
        summary: "Skills: the instructions agents follow for a kind of work.",
    },
    Group {
        path: "prompts",
        summary: "Prompts: the texts the plaza's roles and harness read, with their versions.",
    },
    Group {
        path: "foundry",
        summary: "The foundry: datasets, training jobs, models and their hugging-face imports.",
    },
    Group {
        path: "usage",
        summary: "What the plaza spent: model calls, tokens and cost over time.",
    },
    Group {
        path: "providers",
        summary: "Model providers: which ones the plaza uses and whether their keys work.",
    },
    Group {
        path: "engines",
        summary: "Coding engines: the built-in and added ones, and their versions.",
    },
    Group {
        path: "hooks",
        summary: "Hooks: commands the plaza runs on a tool call.",
    },
];

/// Where a group is listed: a static summary, else one from its source, else one that says what is under it.
fn static_summary(path: &str) -> Option<&'static str> {
    GROUPS.iter().find(|g| g.path == path).map(|g| g.summary)
}

/// A source of entries. Each subsystem that owns capabilities implements it and registers once (see [`register`]).
pub trait Source: Sync {
    /// The entries this source adds, read from its live registry. `app` is `None` where there is no hub state (the
    /// catalog's own tests); a source that reads hub state adds none then.
    fn entries(&self, app: Option<&App>) -> Vec<Entry>;

    /// Summaries of the groups this source owns, when they are its own to describe (a connector's own description).
    fn groups(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// Every registered source, in the order they were registered. The built-in ones register first, once, through the same
/// hook a subsystem uses.
static REGISTERED: Mutex<Vec<&'static dyn Source>> = Mutex::new(Vec::new());
static BUILTIN: Once = Once::new();

/// Adds a source to the library: its entries and group summaries appear in every catalog built after this call.
pub fn register(source: &'static dyn Source) {
    REGISTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(source);
}

fn all_sources() -> Vec<&'static dyn Source> {
    BUILTIN.call_once(|| {
        for source in sources::builtin() {
            register(source);
        }
    });
    REGISTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// One child of a group, as the tree shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub path: String,
    /// `group` or `entry`.
    pub kind: &'static str,
    pub summary: String,
    /// The entries under a group (0 for an entry).
    pub count: usize,
}

/// The whole tree, built from every source at once.
pub struct Catalog {
    entries: Vec<Entry>,
    groups: BTreeMap<String, String>,
}

impl Catalog {
    /// The catalog of every source. Entries are sorted by path; a path two sources both add keeps the first (a test
    /// holds that no two do).
    pub fn build(app: Option<&App>) -> Catalog {
        let mut entries: Vec<Entry> = Vec::new();
        let mut dynamic: BTreeMap<String, String> = BTreeMap::new();
        for source in all_sources() {
            entries.extend(source.entries(app));
            dynamic.extend(source.groups());
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);
        let mut groups: BTreeMap<String, String> = BTreeMap::new();
        for entry in &entries {
            for group in ancestors(&entry.path) {
                groups.entry(group).or_default();
            }
        }
        for group in groups.keys().cloned().collect::<Vec<_>>() {
            let summary = dynamic
                .get(&group)
                .cloned()
                .or_else(|| static_summary(&group).map(str::to_string))
                .unwrap_or_else(|| generated_summary(&entries, &group));
            groups.insert(group, summary);
        }
        Catalog { entries, groups }
    }

    /// Every entry, sorted by path, whatever the seat.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// The summary of group `path`, when it is one.
    pub fn group(&self, path: &str) -> Option<&str> {
        self.groups.get(path).map(String::as_str)
    }

    /// The children of `path` (the root with ""), as `seat` may see them: `None` seat is the owner's view, which sees
    /// everything. A group shows only when a child of it is visible.
    pub fn children(&self, app: Option<&App>, seat: Option<&str>, path: &str) -> Vec<Node> {
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}.")
        };
        let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
        for entry in &self.entries {
            let Some(rest) = entry.path.strip_prefix(&prefix) else {
                continue;
            };
            if !prefix.is_empty() && rest.is_empty() {
                continue;
            }
            if let Some(role) = seat
                && visible(app, role, entry).is_err()
            {
                continue;
            }
            let segment = rest.split('.').next().unwrap_or(rest);
            let child = format!("{prefix}{segment}");
            if rest == segment {
                nodes.insert(
                    child.clone(),
                    Node {
                        path: child,
                        kind: "entry",
                        summary: entry.summary.clone(),
                        count: 0,
                    },
                );
            } else {
                let node = nodes.entry(child.clone()).or_insert_with(|| Node {
                    path: child.clone(),
                    kind: "group",
                    summary: self.group(&child).unwrap_or_default().to_string(),
                    count: 0,
                });
                node.count += 1;
            }
        }
        nodes.into_values().collect()
    }
}

/// Every proper ancestor of a dotted path: `a.b.c` has `a` and `a.b`.
fn ancestors(path: &str) -> Vec<String> {
    let segments: Vec<&str> = path.split('.').collect();
    (1..segments.len())
        .map(|n| segments[..n].join("."))
        .collect()
}

/// What a group with no summary of its own says: how many entries are under it and the first of its children.
fn generated_summary(entries: &[Entry], group: &str) -> String {
    let prefix = format!("{group}.");
    let under: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.path.starts_with(&prefix))
        .collect();
    let mut children: BTreeSet<&str> = BTreeSet::new();
    for entry in &under {
        if let Some(rest) = entry.path.strip_prefix(&prefix) {
            children.insert(rest.split('.').next().unwrap_or(rest));
        }
    }
    let shown: Vec<&str> = children.into_iter().take(6).collect();
    format!("{} entries: {}", under.len(), shown.join(", "))
}

/// Whether `seat` may see `entry`, or why not. The owner's view passes no seat and sees everything.
pub fn visible(app: Option<&App>, seat: &str, entry: &Entry) -> Result<(), String> {
    if entry.access == Access::Operator && seat != crate::settings::ORCHESTRATOR_ROLE {
        return Err(format!(
            "{} is the orchestrator's: the {seat} seat may not use it",
            entry.path
        ));
    }
    match &entry.exec {
        Exec::Host(name) | Exec::Guest(name) => {
            if laws::global().permits(seat, name) {
                Ok(())
            } else {
                Err(format!("the laws do not let the {seat} seat use {name}"))
            }
        }
        Exec::Connector { connector, action } => {
            let tool = connectors::catalog()
                .into_iter()
                .find(|c| c.id == connector)
                .and_then(|c| {
                    c.actions
                        .iter()
                        .find(|a| a.name == action)
                        .map(|a| connectors::tool_name(c, a))
                })
                .unwrap_or_default();
            let offered = app.is_some_and(|app| {
                connectors::specs_for(app, seat, None)
                    .iter()
                    .any(|spec| spec.name == tool)
            });
            if offered {
                Ok(())
            } else {
                Err(format!(
                    "connector {connector} is off, not configured, or not open to the {seat} seat"
                ))
            }
        }
        Exec::Route { .. } => {
            if laws::global().permits(seat, &entry.path) {
                Ok(())
            } else {
                Err(format!(
                    "the laws do not let the {seat} seat use {}",
                    entry.path
                ))
            }
        }
        Exec::Text(_) => Ok(()),
    }
}

/// An entry as the tree describes it, with the reason it is unavailable to the seat it was described for (if any).
fn entry_json(entry: &Entry, reason: Option<&str>) -> Value {
    json!({
        "path": entry.path,
        "summary": entry.summary,
        "description": entry.description,
        "runs": entry.runs.as_str(),
        "source": entry.source,
        "input_schema": entry.input_schema,
        "output_schema": entry.output_schema,
        "access": entry.access,
        "approval": entry.approval,
        "cost": entry.cost,
        "exec": entry.exec.label(),
        "available": reason.is_none(),
        "reason": reason,
    })
}

/// The owner's and the seat's view of a path: a group's children, or an entry's full description.
pub fn describe(app: Option<&App>, seat: Option<&str>, path: &str) -> Result<Value, String> {
    let catalog = Catalog::build(app);
    if let Some(entry) = catalog.entry(path) {
        let reason = seat.and_then(|role| visible(app, role, entry).err());
        return Ok(entry_json(entry, reason.as_deref()));
    }
    if catalog.group(path).is_some() {
        return Ok(json!({
            "path": path,
            "kind": "group",
            "summary": catalog.group(path),
            "children": catalog.children(app, seat, path),
        }));
    }
    Err(format!(
        "no tool or group at {path}: list the root with tools_list, or search with tools_search"
    ))
}

/// The entries that match `query`, best first: a word in a path segment counts most, then in the summary, then in the
/// description. `under` narrows the search to a group.
pub fn search(
    app: Option<&App>,
    seat: Option<&str>,
    query: &str,
    under: &str,
) -> Vec<(Entry, usize)> {
    let catalog = Catalog::build(app);
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2)
        .map(str::to_lowercase)
        .collect();
    let scope = if under.is_empty() {
        String::new()
    } else {
        format!("{under}.")
    };
    let mut hits: Vec<(Entry, usize)> = catalog
        .entries()
        .iter()
        .filter(|e| e.path.starts_with(&scope))
        .filter(|e| seat.is_none_or(|role| visible(app, role, e).is_ok()))
        .filter_map(|entry| {
            let score = score(entry, &words);
            (score > 0).then(|| (entry.clone(), score))
        })
        .collect();
    hits.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(MAX_HITS);
    hits
}

fn score(entry: &Entry, words: &[String]) -> usize {
    let segments: Vec<String> = entry.path.split('.').map(str::to_lowercase).collect();
    let summary = entry.summary.to_lowercase();
    let description = entry.description.to_lowercase();
    words
        .iter()
        .map(|word| {
            let mut points = 0;
            if segments.iter().any(|s| s == word) {
                points += 6;
            } else if segments.iter().any(|s| s.contains(word.as_str())) {
                points += 3;
            }
            if summary.contains(word.as_str()) {
                points += 3;
            }
            if description.contains(word.as_str()) {
                points += 1;
            }
            points
        })
        .sum()
}

/// The text the entries under `prefix` answer for a search of `query`, as the agent reads it.
fn search_text(hits: &[(Entry, usize)]) -> String {
    if hits.is_empty() {
        return "no tool matches: list the groups with tools_list and try other words".to_string();
    }
    hits.iter()
        .map(|(entry, _)| json!({"path": entry.path, "summary": entry.summary, "runs": entry.runs.as_str()}))
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The system-prompt paragraph that maps the library's top-level groups. It depends on the static groups alone, so the
/// same bytes go out on every turn (a stable block before the cache breakpoint).
pub fn prompt_map() -> String {
    let mut lines = vec![
        "## Tool library".to_string(),
        String::new(),
        "Everything the plaza lets you use is one tree of tools by dotted path. Its top-level groups:".to_string(),
    ];
    for group in GROUPS.iter().filter(|g| !g.path.contains('.')) {
        lines.push(format!("- {}: {}", group.path, group.summary));
    }
    lines.push(String::new());
    lines.push(
        "Explore it before you say something cannot be done: tools_list with a group's path lists what is under it, \
         tools_search with words finds tools, tools_describe with a path gives an entry's schema and whether you may \
         use it, and tools_call with a path and args runs it. Schemas stay deferred until you describe or call a tool."
            .to_string(),
    );
    lines.join("\n")
}

/// The string argument `key` of `input`.
fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("invalid input: {key} is required"))
}

/// The arguments of a call: `args`, an object (none is an empty one).
fn args_arg(input: &Value) -> Result<Value, String> {
    match input.get("args") {
        None | Some(Value::Null) => Ok(json!({})),
        Some(value @ Value::Object(_)) => Ok(value.clone()),
        Some(_) => Err("invalid input: args must be an object".to_string()),
    }
}

/// Runs one of the library's tools for `agent`: `(is_error, output)`, as the hub answers a host tool.
pub fn run(
    app: &App,
    agent: &Agent,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    let seat = agent.role.as_str();
    match name {
        "tools_list" => {
            let path = input.get("path").and_then(Value::as_str).unwrap_or("");
            let catalog = Catalog::build(Some(app));
            if path.is_empty() || catalog.group(path).is_some() {
                let children = catalog.children(Some(app), Some(seat), path);
                Ok(json!({"path": path, "children": children}).to_string())
            } else {
                Err(format!(
                    "{path} is not a group: describe it with tools_describe"
                ))
            }
        }
        "tools_describe" => {
            let path = str_arg(input, "path")?;
            describe(Some(app), Some(seat), path).map(|v| v.to_string())
        }
        "tools_search" => {
            let query = str_arg(input, "query")?;
            let under = input.get("under").and_then(Value::as_str).unwrap_or("");
            Ok(search_text(&search(Some(app), Some(seat), query, under)))
        }
        "tools_call" => call(app, agent, work, input),
        other => Err(format!("{other} is not a tool library tool")),
    }
}

/// `tools_call`: checks the seat may use the entry, then runs it the way a native call runs: the laws, the owner's
/// permission mode and approvals (a card is raised when one holds the call), then the entry's runner.
fn call(app: &App, agent: &Agent, work: u64, input: &Value) -> Result<String, String> {
    let path = str_arg(input, "path")?;
    let args = args_arg(input)?;
    let catalog = Catalog::build(Some(app));
    let entry = catalog.entry(path).ok_or_else(|| {
        format!("no tool at {path}: find it with tools_search, or list its group with tools_list")
    })?;
    visible(Some(app), &agent.role, entry)?;
    let cid = agent.cid;
    let role = agent.role.as_str();
    match &entry.exec {
        Exec::Text(text) => return Ok(text.clone()),
        Exec::Guest(tool) => {
            return Err(format!(
                "{path} runs in your VM as {tool}: the harness runs it when you call it through tools_call from your VM"
            ));
        }
        _ => {}
    }
    // The tool the laws and the approvals see: the host tool's or connector action's own name, else the entry's path.
    let (tool, held) = match &entry.exec {
        Exec::Host(name) => (name.clone(), true),
        Exec::Connector { connector, action } => (
            connectors::catalog()
                .into_iter()
                .find(|c| c.id == connector)
                .and_then(|c| {
                    c.actions
                        .iter()
                        .find(|a| a.name == action)
                        .map(|a| connectors::tool_name(c, a))
                })
                .unwrap_or_else(|| format!("{connector}_{action}")),
            true,
        ),
        Exec::Route { .. } => (path.to_string(), true),
        _ => (path.to_string(), false),
    };
    if held && let Some(why) = app.laws.refusal(cid, role, &tool, &args) {
        return Err(why);
    }
    let held_call = Held {
        thread: None,
        waiting: None,
    };
    match approvals::gate(app, cid, &tool, &args, &held_call) {
        Gate::Run => {}
        Gate::Refused(reason) => return Err(reason),
        Gate::Held { refusal, .. } => return Err(refusal),
    }
    match &entry.exec {
        Exec::Host(name) => {
            let (is_error, output) = tools::run_host(app, agent, work, name, &args);
            if is_error { Err(output) } else { Ok(output) }
        }
        Exec::Connector { connector, action } => connectors::call(
            app,
            Some((cid, &agent.label())),
            role,
            connector,
            action,
            &args,
        ),
        Exec::Route { method, pattern } => route_call(app, method, pattern, &args),
        Exec::Guest(_) | Exec::Text(_) => Err("not a hub entry".to_string()),
    }
}

/// Answers a hub route in-process: the arguments fill the pattern's `{name}` segments, and the rest are the query (a
/// GET) or the JSON body.
fn route_call(app: &App, method: &str, pattern: &str, args: &Value) -> Result<String, String> {
    let mut rest: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
    let mut path = String::new();
    for segment in pattern.split('/').filter(|s| !s.is_empty()) {
        path.push('/');
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(name) => {
                let value = rest
                    .remove(name)
                    .ok_or_else(|| format!("invalid input: {name} is required"))?;
                let text = match &value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => {
                        return Err(format!(
                            "invalid input: {name} must be a string or a number"
                        ));
                    }
                };
                path.push_str(&crate::connectors::encode(&text));
            }
            None => path.push_str(segment),
        }
    }
    if path.is_empty() {
        path.push('/');
    }
    let (query, body) = if method == "GET" {
        let query: Vec<String> = rest
            .iter()
            .map(|(k, v)| {
                let text = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!(
                    "{}={}",
                    crate::connectors::encode(k),
                    crate::connectors::encode(&text)
                )
            })
            .collect();
        (query.join("&"), Vec::new())
    } else {
        (String::new(), Value::Object(rest).to_string().into_bytes())
    };
    let request = Request {
        method: method.to_string(),
        path,
        query,
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body,
        peer: None,
    };
    let response: Response = crate::api::router().handle(app, &request);
    let text = String::from_utf8_lossy(response.bytes()).into_owned();
    if response.status >= 400 {
        Err(format!("{}: {text}", response.status))
    } else {
        Ok(text)
    }
}

/// The description and input schema of a library tool, for the tool list.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let object = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "tools_list" => (
            "List the children of a group of the tool library (no path: the top-level groups). Each child is a group with its summary and how many entries are under it, or an entry you can call. Only what you may use is listed.",
            object(json!({"path": {"type": "string"}}), &[]),
        ),
        "tools_describe" => (
            "Describe a path of the tool library: an entry's summary, description, input schema, where it runs, whether it needs the owner's approval, its cost hint, and whether you may use it (with the reason when you may not). A group describes its children.",
            object(
                json!({"path": {"type": "string", "minLength": 1}}),
                &["path"],
            ),
        ),
        "tools_search" => (
            "Search the tool library by words: names, summaries and descriptions. Returns the best matches as path, summary and where they run. Use it when you do not know the group.",
            object(
                json!({"query": {"type": "string", "minLength": 1}, "under": {"type": "string", "description": "a group to search within"}}),
                &["query"],
            ),
        ),
        "tools_call" => (
            "Run an entry of the tool library by its path, with args (an object of the entry's input). It runs the way a tool of its own runs: the owner's laws and permission mode apply, and a call that needs the owner's approval waits for a card. Describe the entry first for its input schema.",
            object(
                json!({"path": {"type": "string", "minLength": 1}, "args": {"type": "object"}}),
                &["path"],
            ),
        ),
        _ => return None,
    })
}
