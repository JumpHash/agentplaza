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

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
pub const STAGES: &[&str] = &[
    "brief",
    "research",
    "proposal",
    "critique",
    "approval",
    "implementation",
    "review",
    "delivered",
];

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
    /// The owner paused it: its agents' work waits until it is resumed (`membership`).
    #[serde(default)]
    pub paused: bool,
    /// The owner archived it: its agents went back to the bench, and nothing more is done for it.
    #[serde(default)]
    pub archived: bool,
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
        Self {
            cid: 0,
            name: "owner".into(),
            role: "owner".into(),
        }
    }
}

/// The projects, kept in `<state-dir>/projects.json`.
pub struct Store {
    path: PathBuf,
    items: Mutex<Vec<Project>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The trimmed text, if it is between `min` and `max` characters long.
fn bounded(field: &str, value: &str, min: usize, max: usize) -> Result<String, String> {
    let v = value.trim();
    let n = v.chars().count();
    if n < min || n > max {
        return Err(format!("{field} must be {min} to {max} characters"));
    }
    Ok(v.to_string())
}

fn need(ok: bool, what: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("needs {what}"))
    }
}

fn event(by: &Actor, what: &str, note: &str, at_ms: u64) -> Event {
    Event {
        at_ms,
        by_cid: by.cid,
        by: by.name.clone(),
        what: what.into(),
        note: note.into(),
    }
}

/// The actor for the plaza's own bookkeeping, when no agent or owner acted.
fn system() -> Actor {
    Actor {
        cid: 0,
        name: "plaza".into(),
        role: "plaza".into(),
    }
}

fn find_mut<'a>(items: &'a mut [Project], id: &str) -> Result<&'a mut Project, String> {
    items
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no such project: {id}"))
}

fn is_plan_doc(kind: &str) -> bool {
    matches!(kind, "proposal" | "design" | "spec" | "tasks" | "revision")
}

/// The cid that wrote the newest plan document: the planner whose revision is up for critique.
fn plan_author(project: &Project) -> Option<u64> {
    project
        .docs
        .iter()
        .rev()
        .find(|d| is_plan_doc(&d.kind))
        .map(|d| d.by_cid)
}

impl Store {
    pub fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("projects.json");
        let items: Vec<Project> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    fn save(&self, items: &[Project]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Runs `change` on a copy of the projects and saves it; only then is the copy kept. A refusal
    /// or a failed save leaves the store as it was.
    fn mutate<T>(
        &self,
        change: impl FnOnce(&mut Vec<Project>) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut items = self
            .items
            .lock()
            .map_err(|_| "projects are unavailable".to_string())?;
        let mut next: Vec<Project> = items.clone();
        let out = change(&mut next)?;
        self.save(&next)?;
        *items = next;
        Ok(out)
    }

    pub fn list(&self) -> Vec<Project> {
        let mut out = self.items.lock().map(|i| i.clone()).unwrap_or_default();
        out.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
        out
    }

    pub fn get(&self, id: &str) -> Option<Project> {
        self.items.lock().ok()?.iter().find(|p| p.id == id).cloned()
    }

    /// Opens a project at stage "brief". `kind` is "code" or "other".
    pub fn open(
        &self,
        by: &Actor,
        title: &str,
        goal: &str,
        kind: &str,
        owner_approval: bool,
    ) -> Result<Project, String> {
        let title = bounded("title", title, 1, 120)?;
        let goal = bounded("goal", goal, 1, 8000)?;
        if kind != "code" && kind != "other" {
            return Err("kind must be code or other".into());
        }
        self.mutate(|items| {
            let now = now_ms();
            let seq = items
                .iter()
                .filter_map(|p| p.id.strip_prefix("pr")?.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            let project = Project {
                id: format!("pr{seq}"),
                title: title.clone(),
                goal: goal.clone(),
                kind: kind.into(),
                stage: "brief".into(),
                lead_cid: by.cid,
                owner_approval,
                repo: None,
                docs: Vec::new(),
                critiques: Vec::new(),
                revision: 0,
                approved_by: None,
                tasks: Vec::new(),
                history: vec![event(by, "opened", &title, now)],
                created_ms: now,
                updated_ms: now,
                paused: false,
                archived: false,
            };
            items.push(project.clone());
            Ok(project)
        })
    }

    /// Opens a project for the owner, its plan already theirs: it starts at "implementation", approved by the
    /// owner, so its tasks can be posted at once and its agents put on it. It has no orchestrator lead.
    pub fn open_for_owner(&self, title: &str, goal: &str, kind: &str) -> Result<Project, String> {
        let owner = Actor::owner();
        let opened = self.open(&owner, title, goal, kind, true)?;
        self.mutate(|items| {
            let p = find_mut(items, &opened.id)?;
            let now = now_ms();
            p.lead_cid = 0;
            p.stage = "implementation".into();
            p.approved_by = Some("owner".into());
            p.history.push(event(
                &owner,
                "approved",
                "opened by the owner, its plan theirs",
                now,
            ));
            p.updated_ms = now;
            Ok(p.clone())
        })
    }

    /// Makes agent `cid` project `id`'s lead: the orchestrator a conversation started it from, told of its tasks to
    /// replan.
    pub fn set_lead(&self, id: &str, cid: u64) -> Result<Project, String> {
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            p.lead_cid = cid;
            p.updated_ms = now_ms();
            Ok(p.clone())
        })
    }

    /// Pauses or resumes a project: while paused, its agents' work waits (`membership`).
    pub fn set_paused(&self, id: &str, paused: bool) -> Result<Project, String> {
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            if p.archived {
                return Err(format!("project {id} is archived"));
            }
            if p.paused != paused {
                let now = now_ms();
                p.paused = paused;
                let what = if paused { "paused" } else { "resumed" };
                p.history.push(event(&Actor::owner(), what, "", now));
                p.updated_ms = now;
            }
            Ok(p.clone())
        })
    }

    /// Archives a project: nothing more is done for it (its agents are sent back to the bench by the caller).
    pub fn archive(&self, id: &str) -> Result<Project, String> {
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            if !p.archived {
                let now = now_ms();
                p.archived = true;
                p.paused = false;
                p.history.push(event(&Actor::owner(), "archived", "", now));
                p.updated_ms = now;
            }
            Ok(p.clone())
        })
    }

    /// Attaches a document. Its kind must be in DOC_KINDS; exactly one of file / repo_path.
    pub fn attach(&self, id: &str, by: &Actor, doc: Doc) -> Result<Project, String> {
        if !DOC_KINDS.iter().any(|(k, _)| *k == doc.kind) {
            return Err(format!("unknown document kind: {}", doc.kind));
        }
        if doc.file.is_some() == doc.repo_path.is_some() {
            return Err("a document needs exactly one of file or repo_path".into());
        }
        let title = bounded("title", &doc.title, 1, 160)?;
        let summary = bounded("summary", &doc.summary, 1, 2000)?;
        let now = now_ms();
        let doc = Doc {
            title,
            summary,
            by_cid: by.cid,
            by: by.name.clone(),
            at_ms: now,
            ..doc
        };
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            if p.stage == "delivered" {
                return Err("the project is delivered".into());
            }
            // One revision per set of plan documents: the first plan document after the current
            // revision was critiqued (or the very first one) opens the next revision.
            if is_plan_doc(&doc.kind)
                && matches!(p.stage.as_str(), "proposal" | "critique" | "approval")
            {
                let critiqued = p.critiques.iter().any(|c| c.revision == p.revision);
                if p.revision == 0 || critiqued {
                    p.revision += 1;
                }
            }
            if doc.kind == "revision" && p.stage == "approval" {
                p.stage = "critique".into();
            }
            let note = format!("{}: {}", doc.kind, doc.title);
            p.history.push(event(by, "attached", &note, now));
            p.docs.push(doc);
            p.updated_ms = now;
            Ok(p.clone())
        })
    }

    /// Records a critique of the current revision. The author of the latest proposal-stage doc
    /// may not critique their own revision.
    pub fn critique(
        &self,
        id: &str,
        by: &Actor,
        verdict: &str,
        text: &str,
    ) -> Result<Project, String> {
        if !matches!(verdict, "approve" | "revise" | "comment") {
            return Err("verdict must be approve, revise or comment".into());
        }
        let body = bounded("text", text, 1, 4000)?;
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            if !matches!(p.stage.as_str(), "proposal" | "critique" | "approval") {
                return Err(format!(
                    "a plan is critiqued at proposal, critique or approval, not at {}",
                    p.stage
                ));
            }
            if plan_author(p) == Some(by.cid) {
                return Err("you cannot critique your own revision".into());
            }
            let now = now_ms();
            let revision = p.revision;
            p.critiques.push(Critique {
                by_cid: by.cid,
                by: by.name.clone(),
                verdict: verdict.into(),
                text: body.clone(),
                revision,
                at_ms: now,
            });
            p.history.push(event(
                by,
                "critiqued",
                &format!("{verdict} on revision {revision}"),
                now,
            ));
            p.updated_ms = now;
            Ok(p.clone())
        })
    }

    /// Moves to the next stage when its gate holds (see `gate`), else says what is missing.
    pub fn advance(&self, id: &str, by: &Actor, note: &str) -> Result<Project, String> {
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            match p.stage.as_str() {
                "approval" => return Err("the plan needs approval: plaza_project_approve".into()),
                "delivered" => return Err("the project is already delivered".into()),
                _ => {}
            }
            let idx = STAGES
                .iter()
                .position(|s| *s == p.stage)
                .ok_or_else(|| format!("unknown stage: {}", p.stage))?;
            let next = *STAGES
                .get(idx + 1)
                .ok_or_else(|| "the project is already delivered".to_string())?;
            gate(p, p.stage == "implementation")?;
            let now = now_ms();
            let from = std::mem::replace(&mut p.stage, next.into());
            let detail = match note.trim() {
                "" => format!("{from} to {next}"),
                n => format!("{from} to {next}: {n}"),
            };
            p.history.push(event(by, "advanced", &detail, now));
            p.updated_ms = now;
            Ok(p.clone())
        })
    }

    /// Approves the plan (stage "approval" → "implementation"): the mayor or the owner, or only
    /// the owner when `owner_approval`. The owner may also approve from "critique".
    pub fn approve(&self, id: &str, by: &Actor, note: &str) -> Result<Project, String> {
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            let owner = by.role == "owner";
            // The mayor decides once the plan has been critiqued: approval does not wait for every
            // reviewer to agree. Open "revise" items are recorded with the approval and carried
            // into the tasks' acceptance.
            let awaiting = matches!(p.stage.as_str(), "approval" | "critique");
            if !awaiting {
                return Err(format!("the plan is at {}, not awaiting approval", p.stage));
            }
            if p.owner_approval && !owner {
                return Err("this project waits for the owner's approval".into());
            }
            if !owner && by.role != "mayor" && by.role != "mayor-office" {
                return Err("only the mayor or the owner can approve a plan".into());
            }
            let author = p
                .docs
                .iter()
                .rev()
                .find(|d| is_plan_doc(&d.kind))
                .map(|d| d.by_cid);
            if !owner && !p.critiques.iter().any(|c| Some(c.by_cid) != author) {
                return Err(
                    "the plan needs at least one critique by an agent other than its author first"
                        .into(),
                );
            }
            let open: Vec<String> = p
                .critiques
                .iter()
                .filter(|c| c.revision == p.revision && c.verdict == "revise")
                .map(|c| format!("{}: {}", c.by, c.text.chars().take(300).collect::<String>()))
                .collect();
            let mut record = note.trim().to_string();
            if !open.is_empty() {
                record.push_str(&format!(
                    "\nOpen items carried into the tasks' acceptance:\n- {}",
                    open.join("\n- ")
                ));
            }
            let now = now_ms();
            p.stage = "implementation".into();
            p.approved_by = Some(by.name.clone());
            p.history.push(event(by, "approved", record.trim(), now));
            p.updated_ms = now;
            Ok(p.clone())
        })
    }

    /// The owner sends a project back to `stage` (any earlier stage) with a reason.
    pub fn veto(&self, id: &str, stage: &str, reason: &str) -> Result<Project, String> {
        let target = STAGES
            .iter()
            .position(|s| *s == stage)
            .ok_or_else(|| format!("unknown stage: {stage}"))?;
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            let current = STAGES.iter().position(|s| *s == p.stage).unwrap_or(0);
            if target >= current {
                return Err(format!(
                    "a veto sends a project to an earlier stage than {}",
                    p.stage
                ));
            }
            let implementation = STAGES
                .iter()
                .position(|s| *s == "implementation")
                .unwrap_or(usize::MAX);
            if target < implementation {
                p.approved_by = None;
            }
            let now = now_ms();
            p.stage = stage.into();
            p.history.push(event(
                &Actor::owner(),
                "vetoed",
                &format!("back to {stage}: {}", reason.trim()),
                now,
            ));
            p.updated_ms = now;
            Ok(p.clone())
        })
    }

    /// Links a posted task to the project (stage must be "implementation" or later).
    pub fn link_task(&self, id: &str, task: &str) -> Result<(), String> {
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            need(
                matches!(p.stage.as_str(), "implementation" | "review" | "delivered"),
                "an approved plan before tasks are linked",
            )?;
            if p.tasks.iter().any(|t| t == task) {
                return Ok(());
            }
            let now = now_ms();
            p.tasks.push(task.to_string());
            p.history
                .push(event(&system(), "attached", &format!("task {task}"), now));
            p.updated_ms = now;
            Ok(())
        })
    }

    pub fn set_repo(&self, id: &str, repo: &str) -> Result<(), String> {
        let repo = bounded("repo", repo, 1, 512)?;
        self.mutate(|items| {
            let p = find_mut(items, id)?;
            let now = now_ms();
            p.history
                .push(event(&system(), "attached", &format!("repo {repo}"), now));
            p.repo = Some(repo.clone());
            p.updated_ms = now;
            Ok(())
        })
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
pub fn gate(project: &Project, tasks_done: bool) -> Result<(), String> {
    let has = |kind: &str| project.docs.iter().any(|d| d.kind == kind);
    match project.stage.as_str() {
        "brief" => need(has("brief"), "a brief document"),
        "research" => need(has("research"), "a research document"),
        "proposal" => {
            need(has("proposal"), "a proposal document")?;
            need(has("tasks"), "a tasks document")?;
            need(
                project.kind != "code" || has("spec"),
                "a spec document (the OpenSpec change)",
            )
        }
        "critique" => {
            let author = plan_author(project);
            let current: Vec<&Critique> = project
                .critiques
                .iter()
                .filter(|c| c.revision == project.revision)
                .collect();
            if current.iter().any(|c| c.verdict == "revise") {
                return Err(
                    "needs a revised plan: a critic asked for changes to this revision".into(),
                );
            }
            need(
                current.iter().any(|c| Some(c.by_cid) != author),
                "a critique of this revision by another agent",
            )
        }
        "approval" => Err("the plan needs approval: plaza_project_approve".into()),
        "implementation" => need(tasks_done, "every linked task done"),
        "review" => need(has("deliverable"), "a deliverable document"),
        "delivered" => Err("the project is already delivered".into()),
        other => Err(format!("unknown stage: {other}")),
    }
}

/// The tools' specs, in the same shape as `society::spec`.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_project_open" => (
            "Open a project for an operator goal (orchestrator). Every goal goes brief → research → proposal → critique → approval → implementation → review → delivered; no implementation task is posted before approval. kind: code (the plan is an OpenSpec change) or other. Write and attach the brief next: the goal restated, who it is for, what a professional result includes, constraints the operator actually stated, open questions only the operator can answer.",
            obj(
                json!({"title": {"type": "string"}, "goal": {"type": "string"}, "kind": {"type": "string", "enum": ["code", "other"]}}),
                &["title", "goal", "kind"],
            ),
        ),
        "plaza_projects" => (
            "List projects, or show one in full (stage, documents with summaries, critiques, approval, tasks, history) when `id` is given.",
            obj(json!({"id": {"type": "string"}}), &[]),
        ),
        "plaza_project_attach" => (
            "Attach a document to a project: brief, research, proposal, design, spec (OpenSpec change, code projects), tasks, revision or deliverable. Give a shared file id (share_file) or a repo path, and a one-paragraph summary. A tasks document's list is checked as it is attached: write each task as `- [ ] 1.3 <goal> (after: 1.1, 1.2)`, every number once, every after naming a task of the list, and no task waiting on itself through others.",
            obj(
                json!({"id": {"type": "string"}, "kind": {"type": "string", "enum": ["brief", "research", "proposal", "design", "spec", "tasks", "revision", "deliverable"]}, "title": {"type": "string"}, "file": {"type": "string"}, "repo_path": {"type": "string"}, "commit": {"type": "string"}, "summary": {"type": "string"}}),
                &["id", "kind", "title", "summary"],
            ),
        ),
        "plaza_project_critique" => (
            "Critique a project's current plan: approve (good to go), revise (must change first: say exactly what and why) or comment. Judge it against what a professional result would include. You cannot critique your own revision.",
            obj(
                json!({"id": {"type": "string"}, "verdict": {"type": "string", "enum": ["approve", "revise", "comment"]}, "text": {"type": "string"}}),
                &["id", "verdict", "text"],
            ),
        ),
        "plaza_project_advance" => (
            "Move a project to its next stage. Refused, with what is missing, until the stage's gate holds.",
            obj(
                json!({"id": {"type": "string"}, "note": {"type": "string"}}),
                &["id"],
            ),
        ),
        "plaza_project_approve" => (
            "Approve a project's plan so implementation can start (the mayor; the owner can too).",
            obj(
                json!({"id": {"type": "string"}, "note": {"type": "string"}}),
                &["id"],
            ),
        ),
        _ => return None,
    })
}

/// Runs one of TOOLS for `agent`.
/// Gives project `project` its repo in Weft (`<id>-<slug of the title>`) and records it; None when Weft is not
/// running (the project works without one).
pub(crate) fn attach_repo(app: &App, project: &Project) -> Option<String> {
    repo_for(app, project)
}

fn repo_for(app: &App, project: &Project) -> Option<String> {
    if !app.weft.running() {
        return None;
    }
    let slug: String = project
        .title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let name = format!("{}-{}", project.id, slug);
    // The project's repo in Weft (created by the owner's console when it is not there yet).
    app.projects.set_repo(&project.id, &name).ok()?;
    crate::weft_hub::ensure_repo(app, &project.id).ok()
}

pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    let actor = Actor {
        cid: agent.cid,
        name: if agent.name.is_empty() {
            agent.role.clone()
        } else {
            agent.name.clone()
        },
        role: agent.role.clone(),
    };
    // A project's agent works on its own project only; the orchestrator runs them all.
    if let Some(id) = opt_str(input, "id")
        && !crate::membership::plaza_wide(&agent.role)
        && app.members.project_of(agent.cid).as_deref() != Some(id.as_str())
    {
        return Err(format!(
            "you work on {}, not on project {id}",
            app.members
                .project_of(agent.cid)
                .map_or("no project (the bench)".to_string(), |own| format!(
                    "project {own}"
                ))
        ));
    }
    match name {
        "plaza_project_open" => {
            if !matches!(
                crate::settings::tier_of(&agent.role),
                crate::settings::Tier::Orchestrator
            ) {
                return Err("only the orchestrator opens projects".into());
            }
            let project = app.projects.open(
                &actor,
                str_arg(input, "title")?,
                str_arg(input, "goal")?,
                str_arg(input, "kind")?,
                false,
            )?;
            // Every project gets a repo on the plaza's git host, for its plan and its code.
            let project = match repo_for(app, &project) {
                Some(repo) => app
                    .projects
                    .get(&project.id)
                    .filter(|p| p.repo.as_deref() == Some(repo.as_str()))
                    .unwrap_or(project),
                None => project,
            };
            answer(
                app,
                &project,
                "write the brief (the goal restated, who it is for, what a professional result includes, constraints the operator stated, questions only the operator can answer), share it with share_file and attach it with plaza_project_attach kind=brief, then plaza_project_advance",
            )
        }
        "plaza_projects" => match opt_str(input, "id") {
            Some(id) => {
                let project = app
                    .projects
                    .get(&id)
                    .ok_or_else(|| format!("no project {id}"))?;
                serde_json::to_string_pretty(&project).map_err(|e| e.to_string())
            }
            None => {
                let projects = app.projects.list();
                if projects.is_empty() {
                    return Ok("no projects".into());
                }
                Ok(projects
                    .iter()
                    .map(project_line)
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        },
        "plaza_project_attach" => {
            let id = str_arg(input, "id")?;
            let file = opt_str(input, "file");
            if let Some(file) = &file {
                if app.deliverables.meta(file).is_none() {
                    return Err(format!("no file {file}"));
                }
                if !app.deliverables.can_read(file, agent.cid) {
                    return Err(format!("you cannot read file {file}"));
                }
            }
            let doc = Doc {
                kind: str_arg(input, "kind")?.to_string(),
                title: str_arg(input, "title")?.to_string(),
                file,
                repo_path: opt_str(input, "repo_path"),
                commit: opt_str(input, "commit"),
                summary: str_arg(input, "summary")?.to_string(),
                // Filled by the store: the author and the time.
                by_cid: 0,
                by: String::new(),
                at_ms: 0,
            };
            // A tasks list is checked as a graph before it is attached, so the planner fixes its
            // order before the critique.
            let checked = if doc.kind == "tasks" {
                Some(check_tasks_doc(app, id, &doc)?)
            } else {
                None
            };
            let project = app.projects.attach(id, &actor, doc)?;
            let hint = stage_hint(&project.stage);
            match checked {
                Some(found) => answer(app, &project, &format!("{found}. {hint}")),
                None => answer(app, &project, hint),
            }
        }
        "plaza_project_critique" => {
            let project = app.projects.critique(
                str_arg(input, "id")?,
                &actor,
                str_arg(input, "verdict")?,
                str_arg(input, "text")?,
            )?;
            answer(app, &project, stage_hint(&project.stage))
        }
        "plaza_project_advance" => {
            let id = str_arg(input, "id")?;
            let before = app
                .projects
                .get(id)
                .ok_or_else(|| format!("no project {id}"))?;
            if before.stage == "implementation" {
                let unfinished = unfinished_tasks(app, &before);
                if !unfinished.is_empty() {
                    return Err(format!("tasks not done: {}", unfinished.join(", ")));
                }
            }
            let note = opt_str(input, "note").unwrap_or_default();
            let project = app.projects.advance(id, &actor, &note)?;
            // Delivered: the project's agents go back to the bench, and the rest of its budget to the plaza.
            if project.stage == "delivered" {
                crate::membership::release_project(app, &project.id);
                crate::project_settings::return_rest(app, &project.id);
            }
            answer(app, &project, stage_hint(&project.stage))
        }
        "plaza_project_approve" => {
            let id = str_arg(input, "id")?;
            let note = opt_str(input, "note").unwrap_or_default();
            let project = app.projects.approve(id, &actor, &note)?;
            answer(app, &project, stage_hint(&project.stage))
        }
        other => Err(format!("not a project tool: {other}")),
    }
}

/// How a tasks list marks its order, for the refusals and hints that ask for it.
const TASKS_FORMAT: &str = "write each task as `- [ ] 1.3 <goal> (after: 1.1, 1.2)`: every number once, every after naming a task of the list, and no task waiting on itself through others";

/// Checks a `tasks` document's list before it is attached. A list that does not hold together
/// refuses the attach, with what is wrong. Otherwise the answer says what was found, or why the
/// list could not be read; neither blocks the attach, since posting checks the list again.
fn check_tasks_doc(app: &App, id: &str, doc: &Doc) -> Result<String, String> {
    let project = app
        .projects
        .get(id)
        .ok_or_else(|| format!("no project {id}"))?;
    let text = match crate::task_plan::read_doc(
        app,
        &project,
        doc.file.as_deref(),
        doc.repo_path.as_deref(),
        doc.commit.as_deref(),
    ) {
        Ok(text) => text,
        Err(why) => return Ok(format!("The tasks list was not checked ({why})")),
    };
    let tasks = crate::task_plan::parse(&text)
        .map_err(|why| format!("the tasks document's list does not read: {why}; {TASKS_FORMAT}"))?;
    if tasks.is_empty() {
        return Ok(format!(
            "No task list was found in it, so its order was not checked; {TASKS_FORMAT}"
        ));
    }
    crate::task_plan::check(&tasks, |_| false)
        .map_err(|why| format!("the tasks list does not hold together: {why}; {TASKS_FORMAT}"))?;
    Ok(crate::task_plan::summary(&tasks))
}

/// A required, non-blank string argument.
fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing or empty string `{key}`"))
}

/// An optional, non-blank string argument.
fn opt_str(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// One line of the project listing.
fn project_line(p: &Project) -> String {
    format!(
        "{} [{}] {} — lead {}, rev {}, {} docs, {} critiques",
        p.id,
        p.stage,
        p.title,
        p.lead_cid,
        p.revision,
        p.docs.len(),
        p.critiques.len()
    )
}

/// Publishes the changed project and returns it as compact JSON, with the next step.
fn answer(app: &App, project: &Project, hint: &str) -> Result<String, String> {
    app.bus.publish("project", project);
    let json = serde_json::to_string(project).map_err(|e| e.to_string())?;
    Ok(if hint.is_empty() {
        json
    } else {
        format!("{json}\nNext: {hint}")
    })
}

/// What the agents of a stage do next.
fn stage_hint(stage: &str) -> &'static str {
    match stage {
        "brief" => "plaza_project_advance moves it to research",
        "research" => "researchers attach findings",
        "proposal" => {
            "the planner writes proposal, design, tasks (and for code an OpenSpec change: openspec/changes/<id>/ with proposal.md, design.md, tasks.md, specs/; validate it with the openspec CLI in your VM). In the tasks list, end a task that needs others' results with (after: 1.1, 1.2); tasks without it run side by side"
        }
        "critique" => "agents other than the author critique with plaza_project_critique",
        "approval" => "the mayor approves with plaza_project_approve",
        "implementation" => {
            "the orchestrator posts the tasks list whole with plaza_tasks_post (each task's key, goal, acceptance and after): each task goes to a worker as soon as everything it runs after is done"
        }
        "review" => "QA and reviewers check the deliverables; attach a deliverable doc",
        "delivered" => "report to the operator",
        _ => "",
    }
}

/// The project's linked tasks that are not done, as `id (state)`.
fn unfinished_tasks(app: &App, project: &Project) -> Vec<String> {
    let tasks = app.plaza.tasks.list();
    project
        .tasks
        .iter()
        .filter_map(|id| {
            let state = tasks
                .iter()
                .find(|t| t.id() == id.as_str())
                .map_or("missing", |t| t.state());
            (state != "done").then(|| format!("{id} ({state})"))
        })
        .collect()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects", |app, _, _| {
            Response::json(200, &app.projects.list())
        })
        .doc(RouteDoc::new("List the projects"))
        .get("/v1/projects/{id}", get_route)
        .doc(RouteDoc::new("Get a project").errors(&[404]))
        .post("/v1/projects", create_route)
        .doc(
            RouteDoc::new("Open a project for the owner, its plan approved")
                .status(201)
                .errors(&[400]),
        )
        .get("/v1/projects/{id}/graph", graph_route)
        .doc(RouteDoc::new("Get a project's task graph").errors(&[404]))
        .post("/v1/projects/{id}/pause", |app, _, p| {
            lifecycle(app, p.get("id"), Lifecycle::Pause)
        })
        .doc(RouteDoc::new("Pause a project: its agents' work waits").errors(&[404, 409]))
        .post("/v1/projects/{id}/resume", |app, _, p| {
            lifecycle(app, p.get("id"), Lifecycle::Resume)
        })
        .doc(RouteDoc::new("Resume a paused project").errors(&[404, 409]))
        .post("/v1/projects/{id}/archive", |app, _, p| {
            lifecycle(app, p.get("id"), Lifecycle::Archive)
        })
        .doc(
            RouteDoc::new("Archive a project: its agents go back to the bench").errors(&[404, 409]),
        )
        .post("/v1/projects/{id}/approve", approve_route)
        .doc(RouteDoc::new("Approve a project's current stage").errors(&[409]))
        .post("/v1/projects/{id}/veto", veto_route)
        .doc(RouteDoc::new("Veto a project at a stage").errors(&[409]))
}

/// `POST /v1/projects` `{title, goal, kind?}`: the owner opens a project with its plan already theirs
/// (see [`Store::open_for_owner`]); `kind` is `code` (the default) or `other`. Its repo is made as for an
/// orchestrator's project.
fn create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let text = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or("");
    let kind = match text("kind") {
        "" => "code",
        kind => kind,
    };
    let project = match app
        .projects
        .open_for_owner(text("title"), text("goal"), kind)
    {
        Ok(project) => project,
        Err(e) => return Response::error(400, &e),
    };
    let project = match repo_for(app, &project) {
        Some(_) => app.projects.get(&project.id).unwrap_or(project),
        None => project,
    };
    app.bus.publish("project", &project);
    Response::json(201, &project)
}

/// A change the owner makes to a project's life.
enum Lifecycle {
    Pause,
    Resume,
    Archive,
}

/// Pauses, resumes or archives project `id`; an archive sends its agents back to the bench.
fn lifecycle(app: &App, id: &str, change: Lifecycle) -> Response {
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    let result = match change {
        Lifecycle::Pause => app.projects.set_paused(id, true),
        Lifecycle::Resume => app.projects.set_paused(id, false),
        Lifecycle::Archive => app.projects.archive(id),
    };
    let project = match result {
        Ok(project) => project,
        Err(e) => return Response::error(409, &e),
    };
    if project.archived {
        // Nothing more is done for it: its agents go back to the bench (the tasks waiting in their queues open
        // again), then its open tasks are withdrawn (their escrow back in its budget), and the rest of its budget
        // goes to the plaza.
        crate::membership::release_project(app, id);
        for task in &project.tasks {
            if let Some(task) = app.plaza.tasks.find(task)
                && task.is_open()
            {
                let _ = task.withdraw();
            }
        }
        crate::project_settings::return_rest(app, id);
    } else if !project.paused {
        // Resumed: its agents' waiting work goes out.
        app.work.dispatch_idle(app);
    }
    app.bus.publish("project", &project);
    Response::json(200, &project)
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.projects.get(p.get("id")) {
        Some(project) => Response::json(200, &project),
        None => Response::error(404, "no such project"),
    }
}

/// `GET /v1/projects/{id}/graph`: the project's tasks in the order their links let them run
/// ([`crate::task_graph`]).
fn graph_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match crate::task_graph::of_project(app, p.get("id")) {
        Some(graph) => Response::json(200, &graph),
        None => Response::error(404, "no such project"),
    }
}

/// Body: `{note?}`.
fn approve_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let note = body.get("note").and_then(Value::as_str).unwrap_or("");
    reply(
        app,
        app.projects.approve(p.get("id"), &Actor::owner(), note),
    )
}

/// Body: `{stage, reason}`.
fn veto_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let stage = body
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or("proposal");
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

impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let Ok(items) = self.items.lock() else {
            return String::new();
        };
        serde_json::to_string_pretty(&*items).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let next: Vec<Project> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut items = self
            .items
            .lock()
            .map_err(|_| "projects are unavailable".to_string())?;
        self.save(&next)?;
        *items = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_restores_into_a_fresh_store() {
        let (_dir, a) = fresh("snapshot");
        a.open(
            &lead(),
            "Ship the thing",
            "Make the thing work",
            "other",
            false,
        )
        .unwrap();
        let (_dir2, b) = fresh("snapshot-restored");
        b.restore(&a.snapshot()).unwrap();
        assert_eq!(b.list(), a.list());
        assert!(b.restore("not json").is_err());
        assert_eq!(b.list(), a.list(), "a refused restore changes nothing");
    }

    fn fresh(tag: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!(
            "apid-projects-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(&dir);
        (dir, store)
    }

    fn who(cid: u64, name: &str, role: &str) -> Actor {
        Actor {
            cid,
            name: name.into(),
            role: role.into(),
        }
    }

    fn lead() -> Actor {
        who(1, "lead", "orchestrator")
    }

    fn researcher() -> Actor {
        who(2, "scout", "researcher")
    }

    fn planner() -> Actor {
        who(3, "planner", "planner")
    }

    fn critic() -> Actor {
        who(4, "critic", "critic")
    }

    fn mayor() -> Actor {
        who(5, "mayor", "mayor")
    }

    fn doc(kind: &str, title: &str) -> Doc {
        Doc {
            kind: kind.into(),
            title: title.into(),
            file: Some("file-1".into()),
            repo_path: None,
            commit: None,
            summary: format!("the {kind}"),
            by_cid: 0,
            by: String::new(),
            at_ms: 0,
        }
    }

    /// Walks a project from brief to critique, with no critique yet.
    fn to_critique(store: &Store, kind: &str, owner_approval: bool) -> String {
        let p = store
            .open(
                &lead(),
                "Ship the thing",
                "Make the thing work",
                kind,
                owner_approval,
            )
            .unwrap();
        store.attach(&p.id, &lead(), doc("brief", "Brief")).unwrap();
        store.advance(&p.id, &lead(), "").unwrap();
        store
            .attach(&p.id, &researcher(), doc("research", "Research"))
            .unwrap();
        store.advance(&p.id, &lead(), "").unwrap();
        store
            .attach(&p.id, &planner(), doc("proposal", "Proposal"))
            .unwrap();
        store
            .attach(&p.id, &planner(), doc("tasks", "Tasks"))
            .unwrap();
        if kind == "code" {
            store
                .attach(&p.id, &planner(), doc("spec", "Spec"))
                .unwrap();
        }
        store.advance(&p.id, &lead(), "").unwrap();
        p.id
    }

    #[test]
    fn open_attach_and_advance_to_research() {
        let (_dir, store) = fresh("open");
        let p = store
            .open(
                &lead(),
                "Ship the thing",
                "Make the thing work",
                "other",
                false,
            )
            .unwrap();
        assert_eq!(p.id, "pr1");
        assert_eq!(p.stage, "brief");
        assert_eq!(p.lead_cid, 1);
        let p = store.attach(&p.id, &lead(), doc("brief", "Brief")).unwrap();
        assert_eq!(p.docs[0].by, "lead");
        let p = store.advance(&p.id, &lead(), "").unwrap();
        assert_eq!(p.stage, "research");
        assert_eq!(p.history.last().unwrap().what, "advanced");
    }

    #[test]
    fn advance_gates_hold_a_project_until_its_documents_are_in() {
        // Advance refused without research.
        {
            let (_dir, store) = fresh("gate");
            let p = store.open(&lead(), "Ship", "Goal", "other", false).unwrap();
            store.attach(&p.id, &lead(), doc("brief", "Brief")).unwrap();
            store.advance(&p.id, &lead(), "").unwrap();
            let err = store.advance(&p.id, &lead(), "").unwrap_err();
            assert!(err.contains("research"), "{err}");
            assert_eq!(store.get(&p.id).unwrap().stage, "research");
        }

        // Revise blocks until revision is approved.
        {
            let (_dir, store) = fresh("revise");
            let id = to_critique(&store, "other", false);
            store
                .critique(&id, &critic(), "revise", "The tasks miss the tests.")
                .unwrap();
            assert!(store.advance(&id, &lead(), "").is_err());
            store
                .attach(&id, &planner(), doc("revision", "Revision"))
                .unwrap();
            assert!(store.advance(&id, &lead(), "").is_err());
            store.critique(&id, &critic(), "approve", "Fixed.").unwrap();
            assert_eq!(store.advance(&id, &lead(), "").unwrap().stage, "approval");
            // A revision attached at approval sends the plan back to critique.
            let p = store
                .attach(&id, &planner(), doc("revision", "Second revision"))
                .unwrap();
            assert_eq!(p.stage, "critique");
        }

        // Code needs a spec to leave proposal.
        {
            let (_dir, store) = fresh("code");
            let p = store
                .open(&lead(), "Code thing", "Goal", "code", false)
                .unwrap();
            store.attach(&p.id, &lead(), doc("brief", "Brief")).unwrap();
            store.advance(&p.id, &lead(), "").unwrap();
            store
                .attach(&p.id, &researcher(), doc("research", "Research"))
                .unwrap();
            store.advance(&p.id, &lead(), "").unwrap();
            store
                .attach(&p.id, &planner(), doc("proposal", "Proposal"))
                .unwrap();
            store
                .attach(&p.id, &planner(), doc("tasks", "Tasks"))
                .unwrap();
            let err = store.advance(&p.id, &lead(), "").unwrap_err();
            assert!(err.contains("spec"), "{err}");
            store
                .attach(&p.id, &planner(), doc("spec", "Spec"))
                .unwrap();
            assert_eq!(store.advance(&p.id, &lead(), "").unwrap().stage, "critique");
        }
    }

    #[test]
    fn other_project_runs_to_implementation() {
        let (_dir, store) = fresh("full");
        let id = to_critique(&store, "other", false);
        store
            .critique(&id, &critic(), "approve", "Good to go.")
            .unwrap();
        assert_eq!(store.advance(&id, &lead(), "").unwrap().stage, "approval");
        let err = store.advance(&id, &lead(), "").unwrap_err();
        assert!(err.contains("plaza_project_approve"), "{err}");
        let p = store.approve(&id, &mayor(), "").unwrap();
        assert_eq!(p.stage, "implementation");
        assert_eq!(p.approved_by.as_deref(), Some("mayor"));
    }

    #[test]
    fn author_cannot_critique_own_revision() {
        let (_dir, store) = fresh("author");
        let id = to_critique(&store, "other", false);
        let err = store
            .critique(&id, &planner(), "approve", "I like it")
            .unwrap_err();
        assert!(err.contains("own revision"), "{err}");
    }

    #[test]
    fn owner_approval_refuses_a_mayor() {
        let (_dir, store) = fresh("owner");
        let id = to_critique(&store, "other", true);
        store.critique(&id, &critic(), "approve", "Fine.").unwrap();
        store.advance(&id, &lead(), "").unwrap();
        let err = store.approve(&id, &mayor(), "").unwrap_err();
        assert!(err.contains("owner's approval"), "{err}");
        assert_eq!(
            store.approve(&id, &Actor::owner(), "").unwrap().stage,
            "implementation"
        );
    }

    #[test]
    fn projects_persist_across_load() {
        let (dir, store) = fresh("persist");
        let p = store
            .open(&lead(), "Keep me", "Goal", "other", false)
            .unwrap();
        store.attach(&p.id, &lead(), doc("brief", "Brief")).unwrap();
        drop(store);
        let reloaded = Store::load(&dir);
        let all = reloaded.list();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Keep me");
        assert_eq!(all[0].docs.len(), 1);
        assert_eq!(
            reloaded
                .open(&lead(), "Second", "Goal", "other", false)
                .unwrap()
                .id,
            "pr2"
        );
    }
}
