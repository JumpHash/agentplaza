//! Projects: the flow every goal goes through before anything is built.
//!
//! brief → research → proposal → critique → approval → implementation → review → delivered.
//!
//! The orchestrator opens a project from the operator's goal and writes the brief. Researchers
//! attach what they found (approaches, references, tools). The planner writes the proposal —
//! for code an OpenSpec change (proposal, design, spec deltas, tasks) in the project's repo,
//! validated; for anything else the same shape without spec deltas: proposal (directions),
//! design (the chosen direction in detail), tasks. Agents other than the author critique it and
//! the planner revises. The mayor approves (the owner can approve or veto at any stage, and a
//! project marked `owner_approval` waits for the owner). Only then does the orchestrator post
//! tasks, each linked to the project and carrying its acceptance from the plan.
//!
//! Documents are files in the plaza's file store (by id) or paths in the project's repo.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::Agent;
use crate::state::App;

/// The host tools this module serves.
pub const TOOLS: &[&str] = &[
    "plaza_project_open",
    "plaza_projects",
    "plaza_project_attach",
    "plaza_project_critique",
    "plaza_project_advance",
    "plaza_project_approve",
];

/// The stages, in order.
pub const STAGES: &[&str] = &["brief", "research", "proposal", "critique", "approval", "implementation", "review", "delivered"];

/// The kinds of document a project holds, and the stage that produces each.
pub const DOC_KINDS: &[(&str, &str)] = &[
    ("brief", "brief"),
    ("research", "research"),
    ("proposal", "proposal"),
    ("design", "proposal"),
    ("spec", "proposal"),
    ("tasks", "proposal"),
    ("revision", "critique"),
    ("deliverable", "review"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    /// One of DOC_KINDS' names.
    pub kind: String,
    pub title: String,
    /// A file in the plaza's store, by id ...
    #[serde(default)]
    pub file: Option<String>,
    /// ... or a path in the project's repo (with the commit, when known).
    #[serde(default)]
    pub repo_path: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    /// A one-paragraph summary, so a reader knows what is inside without opening it.
    pub summary: String,
    pub by_cid: u64,
    pub by: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Critique {
    pub by_cid: u64,
    pub by: String,
    /// "approve" (good to go) | "revise" (must change first) | "comment".
    pub verdict: String,
    pub text: String,
    /// The proposal revision it reviewed (Project::revision at the time).
    pub revision: u32,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub at_ms: u64,
    /// cid, or 0 for the owner.
    pub by_cid: u64,
    pub by: String,
    /// "opened" | "attached" | "critiqued" | "advanced" | "approved" | "vetoed" | "reopened".
    pub what: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// `pr<seq>`.
    pub id: String,
    pub title: String,
    /// The operator's goal, verbatim.
    pub goal: String,
    /// "code" | "other".
    pub kind: String,
    /// One of STAGES.
    pub stage: String,
    /// The orchestrator that opened it.
    pub lead_cid: u64,
    /// When true only the owner can approve the plan.
    #[serde(default)]
    pub owner_approval: bool,
    /// The project's repo on the plaza's git host, once it has one.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub docs: Vec<Doc>,
    #[serde(default)]
    pub critiques: Vec<Critique>,
    /// Bumped each time a proposal/design/spec/tasks/revision doc is attached at or after `proposal`.
    #[serde(default)]
    pub revision: u32,
    /// Who approved the plan ("owner" or the mayor's name), once approved.
    #[serde(default)]
    pub approved_by: Option<String>,
    /// Tasks posted for it (ids in the task ledger).
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub history: Vec<Event>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

/// Who is acting: an agent (cid, name, role) or the owner (cid 0, role "owner").
#[derive(Debug, Clone)]
pub struct Actor {
    pub cid: u64,
    pub name: String,
    pub role: String,
}

impl Actor {
    pub fn owner() -> Self {
        Self { cid: 0, name: "owner".into(), role: "owner".into() }
    }
}

/// The projects, kept in `<state-dir>/projects.json`.
pub struct Store {
    path: PathBuf,
    items: Mutex<Vec<Project>>,
}

impl Store {
    pub fn load(state_dir: &Path) -> Self {
        // contract: P1 reads the file (missing or bad → empty).
        Self { path: state_dir.join("projects.json"), items: Mutex::new(Vec::new()) }
    }

    pub fn list(&self) -> Vec<Project> {
        // contract: P1 (newest first)
        let _ = &self.path;
        self.items.lock().map(|i| i.clone()).unwrap_or_default()
    }

    pub fn get(&self, _id: &str) -> Option<Project> {
        None // contract: P1
    }

    /// Opens a project at stage "brief". `kind` is "code" or "other".
    pub fn open(&self, _by: &Actor, _title: &str, _goal: &str, _kind: &str, _owner_approval: bool) -> Result<Project, String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    /// Attaches a document. Its kind must be in DOC_KINDS; exactly one of file / repo_path.
    pub fn attach(&self, _id: &str, _by: &Actor, _doc: Doc) -> Result<Project, String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    /// Records a critique of the current revision. The author of the latest proposal-stage doc
    /// may not critique their own revision.
    pub fn critique(&self, _id: &str, _by: &Actor, _verdict: &str, _text: &str) -> Result<Project, String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    /// Moves to the next stage when its gate holds (see `gate`), else says what is missing.
    pub fn advance(&self, _id: &str, _by: &Actor, _note: &str) -> Result<Project, String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    /// Approves the plan (stage "approval" → "implementation"): the mayor or the owner, or only
    /// the owner when `owner_approval`. The owner may also approve from "critique".
    pub fn approve(&self, _id: &str, _by: &Actor, _note: &str) -> Result<Project, String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    /// The owner sends a project back to `stage` (any earlier stage) with a reason.
    pub fn veto(&self, _id: &str, _stage: &str, _reason: &str) -> Result<Project, String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    /// Links a posted task to the project (stage must be "implementation" or later).
    pub fn link_task(&self, _id: &str, _task: &str) -> Result<(), String> {
        Err("projects are not available yet".into()) // contract: P1
    }

    pub fn set_repo(&self, _id: &str, _repo: &str) -> Result<(), String> {
        Err("projects are not available yet".into()) // contract: P1
    }
}

/// What the move out of `project.stage` needs, or `Ok(())`:
/// brief → research: a "brief" doc.
/// research → proposal: at least one "research" doc.
/// proposal → critique: "proposal" and "tasks" docs, plus "spec" when kind == "code".
/// critique → approval: at least one critique of the current revision by an agent other than the
///   proposal's author, and no "revise" critique of the current revision.
/// approval → implementation: only through `approve`.
/// implementation → review: every linked task is done (checked by the caller, which passes `tasks_done`).
/// review → delivered: a "deliverable" doc.
pub fn gate(_project: &Project, _tasks_done: bool) -> Result<(), String> {
    Err("projects are not available yet".into()) // contract: P1
}

/// The tools' specs, in the same shape as `society::spec`.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_project_open" => (
            "Open a project for an operator goal (orchestrator). Every goal goes brief → research → proposal → critique → approval → implementation → review → delivered; no implementation task is posted before approval. kind: code (the plan is an OpenSpec change) or other. Write and attach the brief next: the goal restated, who it is for, what a professional result includes, constraints the operator actually stated, open questions only the operator can answer.",
            obj(json!({"title": {"type": "string"}, "goal": {"type": "string"}, "kind": {"type": "string", "enum": ["code", "other"]}}), &["title", "goal", "kind"]),
        ),
        "plaza_projects" => (
            "List projects, or show one in full (stage, documents with summaries, critiques, approval, tasks, history) when `id` is given.",
            obj(json!({"id": {"type": "string"}}), &[]),
        ),
        "plaza_project_attach" => (
            "Attach a document to a project: brief, research, proposal, design, spec (OpenSpec change, code projects), tasks, revision or deliverable. Give a shared file id (share_file) or a repo path, and a one-paragraph summary.",
            obj(json!({"id": {"type": "string"}, "kind": {"type": "string", "enum": ["brief", "research", "proposal", "design", "spec", "tasks", "revision", "deliverable"]}, "title": {"type": "string"}, "file": {"type": "string"}, "repo_path": {"type": "string"}, "commit": {"type": "string"}, "summary": {"type": "string"}}), &["id", "kind", "title", "summary"]),
        ),
        "plaza_project_critique" => (
            "Critique a project's current plan: approve (good to go), revise (must change first: say exactly what and why) or comment. Judge it against what a professional result would include. You cannot critique your own revision.",
            obj(json!({"id": {"type": "string"}, "verdict": {"type": "string", "enum": ["approve", "revise", "comment"]}, "text": {"type": "string"}}), &["id", "verdict", "text"]),
        ),
        "plaza_project_advance" => (
            "Move a project to its next stage. Refused, with what is missing, until the stage's gate holds.",
            obj(json!({"id": {"type": "string"}, "note": {"type": "string"}}), &["id"]),
        ),
        "plaza_project_approve" => (
            "Approve a project's plan so implementation can start (the mayor; the owner can too).",
            obj(json!({"id": {"type": "string"}, "note": {"type": "string"}}), &["id"]),
        ),
        _ => return None,
    })
}

/// Runs one of TOOLS for `agent`.
pub fn run(_app: &App, _agent: &Agent, _name: &str, _input: &Value) -> Result<String, String> {
    Err("projects are not available yet".into()) // contract: P2
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects", |app, _, _| Response::json(200, &app.projects.list()))
        .get("/v1/projects/{id}", get_route)
        .post("/v1/projects/{id}/approve", approve_route)
        .post("/v1/projects/{id}/veto", veto_route)
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.projects.get(p.get("id")) {
        Some(project) => Response::json(200, &project),
        None => Response::error(404, "no such project"),
    }
}

/// Body: `{note?}`.
fn approve_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let note = body.get("note").and_then(Value::as_str).unwrap_or("");
    reply(app, app.projects.approve(p.get("id"), &Actor::owner(), note))
}

/// Body: `{stage, reason}`.
fn veto_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let stage = body.get("stage").and_then(Value::as_str).unwrap_or("proposal");
    let reason = body.get("reason").and_then(Value::as_str).unwrap_or("");
    reply(app, app.projects.veto(p.get("id"), stage, reason))
}

fn reply(app: &App, result: Result<Project, String>) -> Response {
    match result {
        Ok(project) => {
            app.bus.publish("project", &project);
            Response::json(200, &project)
        }
        Err(e) => Response::error(409, &e),
    }
}
