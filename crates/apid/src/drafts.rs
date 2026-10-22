//! A project's draft: the project the orchestrator sets up with the owner, which the owner starts. It lives in the
//! project's chat: the orchestrator opens the project (its id reserved, at "brief", where nothing runs) and the chat
//! with it, and writes the draft with `plaza_project_draft` as the owner's talk goes. The chat shows the draft and a
//! `start` card. The owner can edit the draft too (`PATCH …/draft`). Each edit bumps the revision: a write names the
//! revision it read, so neither overwrites a change it has not seen, and the orchestrator is told of the owner's edits
//! with the next message. Every revision withdraws the card and asks again.
//!
//! Start (the card's `start`, or `POST …/start {revision}`) takes the revision the owner looked at and checks the whole
//! draft. Then, all or nothing, it:
//! - opens the project: at "implementation", its plan the owner's, with its GitHub mode and disk quota;
//! - sets its budget (unit, cap and monthly cap);
//! - posts its tasks.
//!
//! The project's planner starts on the work loop's next tick, and each task's seats fill from there (`work_loop.rs`).
//! A step that fails undoes the ones before it: the tasks are withdrawn, and the project is archived, which ends its
//! agents' work. Afterwards the chat is the project's line to the orchestrator that drafted it: the project's notices
//! (it started, a task ended, tasks to replan) come there.
//!
//! GET   /v1/chats/{id}/draft                → the draft, and what still stops Start (`problems`)
//! PATCH /v1/chats/{id}/draft {revision, …}  → the draft one revision on
//! POST  /v1/chats/{id}/start {revision}     → the project started, and its tasks

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::budgets::Unit;
use crate::cards::{self, Card, NewCard};
use crate::chats::{Author, Chat, Kind, MessageKind, Post};
use crate::projects::Actor;
use crate::state::App;
use crate::task_plan::{self, Planned};
use crate::work_loop;

pub const TOOLS: &[&str] = &["plaza_project_draft"];
/// Who the owner's edits are by.
pub const OWNER: &str = "owner";
/// Who the orchestrator's writes are by.
pub const ORCHESTRATOR: &str = "orchestrator";
/// The share of a project's cap at which the owner is told, when the draft's budget starts it.
const WARN_AT_PCT: u8 = 80;
/// The smallest disk quota a draft may set, in bytes (zero means none).
const MIN_QUOTA_BYTES: u64 = 1 << 20;

/// A project's draft (a child of its chat's row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Draft {
    /// Bumped by every edit, the orchestrator's or the owner's.
    pub revision: u64,
    pub title: String,
    pub goal: String,
    /// `code` or `other`.
    pub kind: String,
    /// Where the work goes, in words (a repository, a branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The task list, as a plan's (`task_plan`): `after` names keys of the list.
    #[serde(default)]
    #[table(child)]
    pub tasks: Vec<Planned>,
    /// The project's budget: its unit, its cap and its monthly cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[table(child)]
    pub budget: Option<DraftBudget>,
    /// How the project meets GitHub: one of `projects::GITHUB_MODES`.
    #[serde(default = "crate::projects::github_none")]
    pub github: String,
    /// The disk the project's files may take, in bytes; none means no quota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_quota_bytes: Option<u64>,
    /// The project's settings overrides, when any were set: the project's own settings, as the plaza reads them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[table(child)]
    pub settings: Option<crate::project_settings::ProjectSettings>,
    /// `orchestrator` or `owner`: who made the last edit.
    pub updated_by: String,
    pub updated_ms: u64,
    /// When the owner started it; a started draft is not edited again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<u64>,
}

/// A project's budget as its draft sets it: the unit, the cap in it, and optionally the cap per month for the planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct DraftBudget {
    pub unit: Unit,
    /// In the unit: micro-dollars for `usd`, tokens for `tokens`. Never zero.
    pub cap: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_cap: Option<u64>,
}

/// An edit: the fields given replace the draft's.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    /// The revision the edit was made on (0, or absent, for the first).
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    tasks: Option<Vec<Planned>>,
    #[serde(default)]
    budget: Option<DraftBudget>,
    #[serde(default)]
    github: Option<String>,
    /// Zero goes back to no quota.
    #[serde(default)]
    disk_quota_bytes: Option<u64>,
    #[serde(default)]
    settings: Option<crate::project_settings::ProjectSettings>,
}

/// `edit` applied to `current` (none: a first draft) by `by`, at `now`: the next revision, checked for shape
/// ([`shape`]). A stale revision, or an edit of a started draft, is refused (409); a malformed one (400).
fn apply(current: Option<&Draft>, edit: Edit, by: &str, now: u64) -> Result<Draft, (u16, String)> {
    let revision = current.map_or(0, |draft| draft.revision);
    if let Some(draft) = current
        && draft.started_ms.is_some()
    {
        return Err((
            409,
            "the project is started: its draft is not edited any more".into(),
        ));
    }
    if edit.revision.unwrap_or(0) != revision {
        return Err((
            409,
            format!(
                "the draft is at revision {revision} now, not {}: read it again, then edit that revision",
                edit.revision.unwrap_or(0)
            ),
        ));
    }
    let base = current.cloned();
    let pick = |given: Option<String>, old: Option<&String>| given.or_else(|| old.cloned());
    let draft = Draft {
        revision: revision + 1,
        title: pick(edit.title, base.as_ref().map(|d| &d.title)).unwrap_or_default(),
        goal: pick(edit.goal, base.as_ref().map(|d| &d.goal)).unwrap_or_default(),
        kind: pick(edit.kind, base.as_ref().map(|d| &d.kind)).unwrap_or_else(|| "code".into()),
        repo: match edit.repo {
            Some(repo) if repo.trim().is_empty() => None,
            Some(repo) => Some(repo),
            None => base.as_ref().and_then(|d| d.repo.clone()),
        },
        tasks: edit
            .tasks
            .or_else(|| base.as_ref().map(|d| d.tasks.clone()))
            .unwrap_or_default(),
        budget: edit
            .budget
            .or_else(|| base.as_ref().and_then(|d| d.budget.clone())),
        github: pick(edit.github, base.as_ref().map(|d| &d.github))
            .unwrap_or_else(crate::projects::github_none),
        disk_quota_bytes: match edit.disk_quota_bytes {
            Some(0) => None,
            Some(quota) => Some(quota),
            None => base.as_ref().and_then(|d| d.disk_quota_bytes),
        },
        settings: edit
            .settings
            .or_else(|| base.as_ref().and_then(|d| d.settings.clone())),
        updated_by: by.to_string(),
        updated_ms: now,
        started_ms: None,
    };
    shape(&draft).map_err(|e| (400, e))?;
    Ok(draft)
}

/// Whether a draft is well formed, whatever the plaza holds: its text within bounds, its budget and quota positive, its
/// GitHub mode one of the modes, and its tasks a list that can be posted (keys once, links inside the list, no cycle).
pub fn shape(draft: &Draft) -> Result<(), String> {
    let chars = |text: &str| text.trim().chars().count();
    if !(1..=120).contains(&chars(&draft.title)) {
        return Err("a draft's title is 1 to 120 characters".into());
    }
    if !(1..=8000).contains(&chars(&draft.goal)) {
        return Err("a draft's goal is 1 to 8000 characters".into());
    }
    if draft.kind != "code" && draft.kind != "other" {
        return Err("a draft's kind is code or other".into());
    }
    if draft
        .repo
        .as_deref()
        .is_some_and(|repo| repo.chars().count() > 200)
    {
        return Err("a draft's repo is at most 200 characters".into());
    }
    if let Some(budget) = &draft.budget {
        if budget.cap == 0 {
            return Err("a draft's budget cap is at least 1".into());
        }
        if budget.monthly_cap == Some(0) {
            return Err("a draft's monthly cap is at least 1 (leave it out for none)".into());
        }
    }
    if !crate::projects::GITHUB_MODES.contains(&draft.github.as_str()) {
        return Err(format!(
            "a draft's github is one of {}",
            crate::projects::GITHUB_MODES.join(", ")
        ));
    }
    if draft
        .disk_quota_bytes
        .is_some_and(|quota| quota < MIN_QUOTA_BYTES)
    {
        return Err("a draft's disk quota is at least 1 MiB (zero means none)".into());
    }
    if !draft.tasks.is_empty() {
        task_plan::check(&draft.tasks, |_| false)?;
    }
    Ok(())
}

/// What still stops Start, as the plaza is now: no tasks; a task for a role the plaza does not have; no budget; the
/// project's settings, which the plaza refuses.
pub fn problems(app: &App, draft: &Draft) -> Vec<String> {
    let mut found = Vec::new();
    if draft.tasks.is_empty() {
        found.push("the draft has no tasks yet".to_string());
    }
    for task in &draft.tasks {
        if let Some(role) = task.role.as_deref().filter(|r| !r.is_empty())
            && !crate::roles_custom::exists(app, role)
        {
            found.push(format!(
                "task {} is for a {role}, and the plaza has no such role",
                task.key.trim()
            ));
        }
    }
    if draft.budget.is_none() {
        found.push("the draft has no budget yet: its unit and cap".to_string());
    }
    if let Err(why) = crate::project_settings::check(app, &settings_of(draft)) {
        found.push(format!("the settings: {why}"));
    }
    found
}

/// The draft's settings as a project's overrides (none set when it has none).
fn settings_of(draft: &Draft) -> crate::project_settings::ProjectSettings {
    draft.settings.clone().unwrap_or_default()
}

/// The project's goal: the draft's goal, and where the work goes after it.
fn goal_of(draft: &Draft) -> String {
    match &draft.repo {
        Some(repo) => format!("{}\n\nWhere the work goes: {repo}", draft.goal.trim()),
        None => draft.goal.trim().to_string(),
    }
}

/// The draft as the API shows it: its fields, and what still stops Start.
pub fn view(app: &App, draft: &Draft) -> Value {
    let mut value = serde_json::to_value(draft).unwrap_or(Value::Null);
    value["problems"] = json!(problems(app, draft));
    value
}

/// The project draft chat of project `pid` (archived ones too): the chat that holds its draft, if it has one.
pub fn chat_of(app: &App, pid: &str) -> Option<Chat> {
    app.chats
        .list(Some(pid), true)
        .into_iter()
        .find(|c| c.kind == Kind::Project && c.draft.is_some())
}

/// Saves `draft` as the draft of project `pid`'s chat `chat`, and keeps the project's fields and the chat's start card
/// current. `seen`: the orchestrator made the edit, so it has seen this revision.
fn save(
    app: &App,
    chat: &str,
    pid: &str,
    draft: Draft,
    seen: bool,
) -> Result<Draft, (u16, String)> {
    app.projects
        .set_draft(
            pid,
            draft.title.trim(),
            &goal_of(&draft),
            &draft.kind,
            &draft.github,
            draft.disk_quota_bytes,
        )
        .map_err(|e| (409, e))?;
    let now = draft.updated_ms;
    let saved = app.chats.update(chat, |c| {
        if seen {
            c.draft_seen = draft.revision;
        }
        c.updated_ms = now;
        c.draft = Some(draft.clone());
        Ok(())
    })?;
    ask_to_start(app, chat, &draft, pid, now);
    crate::store::saver::save_now(app, "projects");
    crate::store::saver::save_now(app, "chats");
    app.bus.publish(
        "chat",
        &json!({ "chat": saved.id, "draft": view(app, &draft) }),
    );
    Ok(draft)
}

/// The subject of a project's start card: one open card per draft.
fn start_subject(pid: &str) -> String {
    format!("draft:{pid}")
}

/// Withdraws the start card of the draft's revision before, and asks the owner to start the draft in `chat` (unless it
/// has started). The card's two choices: start it, or keep drafting.
fn ask_to_start(app: &App, chat: &str, draft: &Draft, pid: &str, now: u64) {
    let subject = start_subject(pid);
    for card in app.cards.withdraw(cards::Kind::Start, &subject, now) {
        app.bus.publish("card", &card);
    }
    if draft.started_ms.is_some() {
        return;
    }
    let title = format!("Start project {pid} \"{}\"?", draft.title.trim());
    let (card, created) = app.cards.open(
        NewCard {
            kind: cards::Kind::Start,
            chat: Some(chat.to_string()),
            project: Some(pid.to_string()),
            task: None,
            session: None,
            subject,
            title: title.clone(),
            body: Some(summary(draft)),
            choices: vec![
                cards::Choice {
                    id: "start".into(),
                    label: "Start".into(),
                    detail: Some("open the project with this budget and post its tasks".into()),
                    takes_amount: false,
                },
                cards::Choice {
                    id: "edit-later".into(),
                    label: "Keep drafting".into(),
                    detail: Some("the draft stays here until it is started".into()),
                    takes_amount: false,
                },
            ],
        },
        now,
    );
    if created {
        let posted = app.chats.post(
            chat,
            Post {
                author: Author::System,
                session: None,
                seat: None,
                kind: MessageKind::Question,
                text: format!("{title}\n\n{}", summary(draft)),
                card: Some(card.id.clone()),
                attachments: Vec::new(),
            },
            now,
        );
        app.bus.publish("card", &card);
        if let Some(message) = posted {
            app.bus
                .publish("chat", &json!({ "chat": chat, "message": message }));
        }
    }
}

/// What the start card says of a draft: its tasks, budget, disk quota and GitHub mode, in words.
fn summary(draft: &Draft) -> String {
    let tasks = if draft.tasks.is_empty() {
        "none yet".to_string()
    } else {
        draft
            .tasks
            .iter()
            .map(|task| format!("- {}: {}", task.key.trim(), task.goal.trim()))
            .collect::<Vec<String>>()
            .join("\n")
    };
    let budget = draft.budget.as_ref().map_or_else(
        || "No budget yet".to_string(),
        |b| {
            let cap = work_loop::amount(b.cap, b.unit);
            match b.monthly_cap {
                Some(monthly) => format!(
                    "{cap}, {} a month for the planner",
                    work_loop::amount(monthly, b.unit)
                ),
                None => cap,
            }
        },
    );
    let quota = draft.disk_quota_bytes.map_or_else(
        || "No disk quota".to_string(),
        |q| format!("Disk quota {} MiB", q >> 20),
    );
    format!(
        "{}\n\nTasks:\n{tasks}\n\nBudget: {budget}. {quota}. GitHub: {}.",
        draft.goal.trim(),
        draft.github
    )
}

// ---- The orchestrator's tool -------------------------------------------------------------------------------------

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let task = json!({
        "type": "object",
        "properties": {
            "key": {"type": "string"}, "goal": {"type": "string"}, "acceptance": {"type": "string"},
            "role": {"type": "string"}, "budget": {"type": "integer", "minimum": 0},
            "after": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["key", "goal"],
    });
    (name == "plaza_project_draft").then(|| {
        (
            "Keep a project draft current: the project you set up with the owner, in its own chat, which they start (you never start it). `read` with `project` returns its draft, the revision and what still stops Start; without `project` it lists the drafts not started yet. `write` without `project` opens a new draft (its project and chat); with `project` it changes that draft: give the revision you read (the owner may edit it too), and only the fields that change: title, goal, kind (code|other), repo (where the work goes, in words), tasks [{key, goal, acceptance, role, budget, after}] (after: keys of tasks of this list it runs after), budget {unit: usd|tokens, cap, monthly_cap?} (the project's budget), github (none|mirror|ci), disk_quota_bytes (0 for none) and settings (the project's overrides).",
            json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["read", "write"]},
                    "project": {"type": "string"},
                    "revision": {"type": "integer", "minimum": 0},
                    "title": {"type": "string"}, "goal": {"type": "string"},
                    "kind": {"type": "string", "enum": ["code", "other"]},
                    "repo": {"type": "string"},
                    "tasks": {"type": "array", "items": task, "maxItems": task_plan::MAX_PLANNED},
                    "budget": {
                        "type": "object",
                        "properties": {
                            "unit": {"type": "string", "enum": ["usd", "tokens"]},
                            "cap": {"type": "integer", "minimum": 1},
                            "monthly_cap": {"type": "integer", "minimum": 1},
                        },
                        "required": ["unit", "cap"],
                        "additionalProperties": false,
                    },
                    "github": {"type": "string", "enum": crate::projects::GITHUB_MODES},
                    "disk_quota_bytes": {"type": "integer", "minimum": 0},
                    "settings": {"type": "object"},
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    })
}

/// Runs `plaza_project_draft` for the orchestrator.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    match input.get("action").and_then(Value::as_str) {
        Some("read") => read(app, input),
        Some("write") => write(app, agent, input),
        _ => Err("action is read or write".into()),
    }
}

/// `read`: the draft of `project`, marked seen; without a project, the drafts not started.
fn read(app: &App, input: &Value) -> Result<String, String> {
    let Some(pid) = input.get("project").and_then(Value::as_str) else {
        let open: Vec<Value> = app
            .chats
            .list(None, false)
            .into_iter()
            .filter(|c| c.kind == Kind::Project)
            .filter_map(|c| {
                let draft = c.draft.as_ref().filter(|d| d.started_ms.is_none())?;
                Some(json!({
                    "project": c.project,
                    "chat": c.id,
                    "revision": draft.revision,
                    "title": draft.title,
                }))
            })
            .collect();
        return Ok(json!({ "drafts": open }).to_string());
    };
    let chat = chat_of(app, pid).ok_or_else(|| format!("no draft for project {pid}"))?;
    let Some(draft) = chat.draft.clone() else {
        return Err(format!("no draft for project {pid}"));
    };
    app.chats
        .update(&chat.id, |c| {
            c.draft_seen = draft.revision;
            Ok(())
        })
        .map_err(|(_, e)| e)?;
    Ok(json!({
        "project": pid,
        "chat": chat.id,
        "draft": view(app, &draft),
    })
    .to_string())
}

/// `write`: a new draft (without `project`), or the next revision of the draft of `project`.
fn write(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let mut fields = input.clone();
    if let Some(object) = fields.as_object_mut() {
        object.remove("action");
        object.remove("project");
    }
    let edit: Edit = serde_json::from_value(fields).map_err(|e| format!("a draft edit: {e}"))?;
    let now = apz_plaza::now_ms();
    let (pid, chat, draft) = match input.get("project").and_then(Value::as_str) {
        Some(pid) => {
            let chat = chat_of(app, pid).ok_or_else(|| format!("no draft for project {pid}"))?;
            let draft = apply(chat.draft.as_ref(), edit, ORCHESTRATOR, now).map_err(|(_, e)| e)?;
            let saved = save(app, &chat.id, pid, draft, true).map_err(|(_, e)| e)?;
            (pid.to_string(), chat.id, saved)
        }
        None => {
            let draft = apply(None, edit, ORCHESTRATOR, now).map_err(|(_, e)| e)?;
            let lead = Actor {
                cid: agent.cid,
                name: agent.label(),
                role: agent.role.clone(),
            };
            let project = app.projects.open(
                &lead,
                draft.title.trim(),
                &goal_of(&draft),
                &draft.kind,
                true,
            )?;
            let chat = app.chats.open_project(&project.id, draft.title.trim(), now);
            let saved = save(app, &chat.id, &project.id, draft, true).map_err(|(_, e)| e)?;
            // The main chat is told where the draft is.
            app.chats.main(now);
            work_loop::notice(
                app,
                crate::chats::MAIN,
                &format!(
                    "Draft {} \"{}\" is in chat {}: the owner starts it there.",
                    project.id,
                    saved.title.trim(),
                    chat.id
                ),
                now,
            );
            (project.id, chat.id, saved)
        }
    };
    let found = problems(app, &draft);
    let next = if found.is_empty() {
        "Nothing stops Start: tell the owner it is ready, and that they start it.".to_string()
    } else {
        format!("Before the owner can start it: {}.", found.join("; "))
    };
    Ok(format!(
        "Draft revision {} saved as project {pid} (chat {chat}); the owner sees it there. {next}",
        draft.revision
    ))
}

// ---- The owner's routes -------------------------------------------------------------------------------------------

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/chats/{id}/draft", get_route)
        .doc(RouteDoc::new("Read a project chat's draft").errors(&[404]))
        .route("PATCH", "/v1/chats/{id}/draft", patch_route)
        .doc(RouteDoc::new("Edit a project chat's draft").errors(&[400, 404, 409, 500]))
        .post("/v1/chats/{id}/start", start_route)
        .doc(
            RouteDoc::new("Start the project a chat's draft describes")
                .errors(&[400, 404, 409, 500]),
        )
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.chats.get(p.get("id")).and_then(|chat| chat.draft) {
        Some(draft) => Response::json(200, &view(app, &draft)),
        None => Response::error(404, "the chat has no draft"),
    }
}

/// Body: `{revision, title?, goal?, kind?, repo?, tasks?, budget?, github?, disk_quota_bytes?, settings?}`.
fn patch_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let edit: Edit = match req.json() {
        Ok(edit) => edit,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let id = p.get("id");
    let Some(chat) = app.chats.get(id) else {
        return Response::error(404, &format!("no chat {id}"));
    };
    let Some(pid) = chat.project.clone() else {
        return Response::error(409, "the chat has no project");
    };
    let draft = match apply(chat.draft.as_ref(), edit, OWNER, apz_plaza::now_ms()) {
        Ok(draft) => draft,
        Err((code, e)) => return Response::error(code, &e),
    };
    match save(app, id, &pid, draft, false) {
        Ok(saved) => Response::json(200, &view(app, &saved)),
        Err((code, e)) => Response::error(code, &e),
    }
}

#[derive(Deserialize)]
struct StartBody {
    revision: u64,
}

/// Body: `{revision}`, the draft revision the owner looked at.
fn start_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: StartBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    match start(app, p.get("id"), body.revision) {
        Ok(done) => Response::json(200, &done),
        Err((code, e)) => Response::error(code, &e),
    }
}

// ---- Start --------------------------------------------------------------------------------------------------------

/// Starts chat `id`'s draft at `revision`: checks it, then opens the project and posts its tasks, undoing what was
/// done when a step fails. Returns what Start did: the project and its tasks.
pub fn start(app: &App, id: &str, revision: u64) -> Result<Value, (u16, String)> {
    let chat = app
        .chats
        .get(id)
        .ok_or_else(|| (404, format!("no chat {id}")))?;
    if chat.archived {
        return Err((409, format!("chat {id} is archived")));
    }
    let draft = chat
        .draft
        .clone()
        .ok_or_else(|| (409, "the chat has no draft yet".to_string()))?;
    let pid = chat
        .project
        .clone()
        .ok_or_else(|| (409, "the chat has no project".to_string()))?;
    if draft.started_ms.is_some() {
        return Err((409, format!("this draft started project {pid} already")));
    }
    if draft.revision != revision {
        return Err((
            409,
            format!(
                "the draft is at revision {} now, not {revision}: look at it again before you start",
                draft.revision
            ),
        ));
    }
    shape(&draft).map_err(|e| (400, e))?;
    let found = problems(app, &draft);
    if !found.is_empty() {
        return Err((
            409,
            format!("the draft cannot start yet: {}", found.join("; ")),
        ));
    }
    let budget = draft
        .budget
        .clone()
        .ok_or_else(|| (409, "the draft has no budget yet".to_string()))?;

    // 1. The project, at implementation: its plan the owner's.
    let project = app.projects.start_draft(&pid).map_err(|e| (409, e))?;
    crate::projects::attach_repo(app, &project);

    // 2. Its settings, and its budget: its tasks are funded from it.
    let settings = settings_of(&draft);
    let task_budget = settings.task_budget;
    if let Err(e) = app.project_settings.set(&pid, settings) {
        undo(app, &pid);
        return Err((500, format!("the project's settings were not saved: {e}")));
    }
    if let Err(e) = app.budgets.set_project(
        &pid,
        budget.unit,
        Some(budget.cap),
        WARN_AT_PCT,
        budget.monthly_cap,
        apz_plaza::now_ms(),
    ) {
        undo(app, &pid);
        return Err((409, e));
    }

    // 3. Its tasks: the ledger creates them all or none.
    let posted = match task_plan::post(
        &app.projects,
        &app.plaza.tasks,
        &pid,
        &draft.tasks,
        task_budget,
    ) {
        Ok(posted) => posted,
        Err(e) => {
            undo(app, &pid);
            return Err((409, format!("the tasks were not posted: {e}")));
        }
    };
    let task_ids: Vec<String> = posted.iter().map(|task| task.id.clone()).collect();
    // The orchestrator that drafted it leads it: it hears when each task ends.
    for task in &task_ids {
        app.work.note_poster(task, project.lead_cid);
    }

    // 4. The chat: the draft is started, its start card is settled, and the project's lead is told.
    let now = apz_plaza::now_ms();
    let started = app.chats.update(id, |c| {
        if let Some(draft) = c.draft.as_mut() {
            draft.started_ms = Some(now);
        }
        c.updated_ms = now;
        Ok(())
    })?;
    if let Some(draft) = &started.draft {
        ask_to_start(app, id, draft, &pid, now);
    }
    crate::store::saver::save_now(app, "projects");
    crate::store::saver::save_now(app, "chats");
    if let Some(project) = app.projects.get(&pid) {
        app.bus.publish("project", &project);
    }
    let posted_in_words = posted_words(task_ids.len());
    let shown = format!(
        "Project {pid} \"{}\" started: {posted_in_words}, budget {}. Its planner starts on the next tick.",
        draft.title.trim(),
        work_loop::amount(budget.cap, budget.unit)
    );
    let prompt = format!(
        "[plaza] The owner started project {pid} \"{}\" from this chat's draft (revision {revision}): {posted_in_words} (tasks {}). You lead it from here: the plaza hands each task out once everything it runs after is done. Follow the tasks with plaza_task_result, replan blocked ones, post new tasks to {pid} when the owner asks, and answer the owner in this chat.",
        draft.title.trim(),
        task_ids.join(", "),
    );
    work_loop::tell_chat(app, id, project.lead_cid, &shown, &prompt);
    Ok(json!({
        "chat": id,
        "project": pid,
        "tasks": posted,
    }))
}

/// The owner answered a draft's start card: `start` starts the project, `edit-later` keeps it a draft. Returns what
/// it did, in words; a Start that fails says why in the chat, and asks again.
pub fn answered(app: &App, card: &Card) -> String {
    let Some(chat) = card.chat.clone() else {
        return "recorded".into();
    };
    let now = apz_plaza::now_ms();
    if card.answer.as_deref() != Some("start") {
        work_loop::notice(
            app,
            &chat,
            "The draft stays a draft: edit it, then start it when it looks right.",
            now,
        );
        return "the draft stays a draft".into();
    }
    let revision = app
        .chats
        .get(&chat)
        .and_then(|c| c.draft)
        .map_or(0, |draft| draft.revision);
    match start(app, &chat, revision) {
        Ok(done) => format!(
            "project {} started",
            done["project"].as_str().unwrap_or_default()
        ),
        Err((_, why)) => {
            work_loop::notice(app, &chat, &format!("The draft did not start: {why}."), now);
            if let Some(found) = app.chats.get(&chat)
                && let (Some(draft), Some(pid)) = (found.draft.as_ref(), found.project.as_deref())
            {
                ask_to_start(app, &chat, draft, pid, now);
            }
            format!("not started: {why}")
        }
    }
}

/// The owner's draft edits the orchestrator has not read are pointed out with the owner's next message in the chat:
/// the note the message's prompt starts with. None while there is nothing new (or the draft started).
pub fn edit_note(chats: &crate::chats::Chats, chat: &str) -> Option<String> {
    let mut revision = None;
    let _ = chats.update(chat, |c| {
        if let Some(draft) = &c.draft
            && draft.revision > c.draft_seen
            && draft.updated_by == OWNER
            && draft.started_ms.is_none()
        {
            revision = Some(draft.revision);
        }
        if let Some(seen) = revision {
            c.draft_seen = seen;
        }
        Ok(())
    });
    revision.map(|revision| {
        format!(
            "[plaza] The owner edited the project draft: it is at revision {revision} now. Read it with plaza_project_draft before you change it."
        )
    })
}

/// Undoes a Start that failed after opening project `pid`: its tasks never posted, so the project is archived, which
/// ends its agents' work.
fn undo(app: &App, pid: &str) {
    if let Err(e) = app.projects.archive(pid) {
        eprintln!("apid: project {pid} of a failed start not archived: {e}");
    }
    work_loop::end_project(app, pid, "the project did not start");
    if let Some(project) = app.projects.get(pid) {
        app.bus.publish("project", &project);
    }
}

/// What the tasks count is, in words: `2 tasks` (a draft always has a task to start).
fn tasks_words(tasks: usize) -> String {
    match tasks {
        1 => "1 task".to_string(),
        n => format!("{n} tasks"),
    }
}

/// What Start did, in words: `2 tasks posted`.
fn posted_words(tasks: usize) -> String {
    format!("{} posted", tasks_words(tasks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_says_what_it_did_in_words() {
        assert_eq!(posted_words(7), "7 tasks posted");
        assert_eq!(posted_words(1), "1 task posted");
    }

    fn task(key: &str, role: &str, after: &[&str]) -> Planned {
        Planned {
            key: key.into(),
            goal: format!("goal {key}"),
            acceptance: None,
            role: Some(role.into()),
            budget: None,
            after: after.iter().map(|k| k.to_string()).collect(),
        }
    }

    fn usd(cap: u64) -> DraftBudget {
        DraftBudget {
            unit: Unit::Usd,
            cap,
            monthly_cap: None,
        }
    }

    fn first() -> Edit {
        Edit {
            title: Some("Release 0.9".into()),
            goal: Some("Fix the flaky scheduler tests and tag 0.9.".into()),
            tasks: Some(vec![task("1", "qa", &[]), task("2", "builder", &["1"])]),
            ..Edit::default()
        }
    }

    #[test]
    fn edits_go_revision_by_revision_and_a_stale_one_is_refused() {
        let draft = apply(None, first(), ORCHESTRATOR, 10).expect("a first draft");
        assert_eq!(draft.revision, 1);
        assert_eq!(draft.kind, "code", "kind defaults to code");
        assert_eq!(draft.github, "none", "GitHub defaults to none");
        assert_eq!(draft.budget, None, "a draft has no budget until one is set");
        // The owner sets the budget and the quota on revision 1: the rest is kept.
        let edit = Edit {
            revision: Some(1),
            budget: Some(usd(15_000_000)),
            disk_quota_bytes: Some(5 << 30),
            github: Some("mirror".into()),
            ..Edit::default()
        };
        let owned = apply(Some(&draft), edit, OWNER, 20).expect("the owner's edit");
        assert_eq!(owned.revision, 2);
        assert_eq!(owned.budget, Some(usd(15_000_000)));
        assert_eq!(owned.disk_quota_bytes, Some(5 << 30));
        assert_eq!(owned.github, "mirror");
        assert_eq!(owned.tasks, draft.tasks);
        assert_eq!(owned.updated_by, OWNER);
        // The orchestrator still writing on revision 1 is told to read again.
        let stale = Edit {
            revision: Some(1),
            title: Some("Release 0.9.1".into()),
            ..Edit::default()
        };
        let (code, why) = apply(Some(&owned), stale, ORCHESTRATOR, 30).unwrap_err();
        assert_eq!(code, 409);
        assert!(why.contains("revision 2"), "{why}");
        // A started draft is not edited again.
        let mut started = owned.clone();
        started.started_ms = Some(40);
        let edit = Edit {
            revision: Some(2),
            ..Edit::default()
        };
        assert_eq!(apply(Some(&started), edit, OWNER, 50).unwrap_err().0, 409);
    }

    #[test]
    fn a_zero_quota_goes_back_to_none_and_the_rest_is_kept() {
        let draft = apply(None, first(), ORCHESTRATOR, 1).expect("a first draft");
        let quota = |current: &Draft, bytes: u64| Edit {
            revision: Some(current.revision),
            disk_quota_bytes: Some(bytes),
            ..Edit::default()
        };
        let set = apply(Some(&draft), quota(&draft, 2 << 30), OWNER, 2).expect("a quota");
        assert_eq!(set.disk_quota_bytes, Some(2 << 30));
        let cleared = apply(Some(&set), quota(&set, 0), OWNER, 3).expect("no quota");
        assert_eq!(cleared.disk_quota_bytes, None);
        assert_eq!(cleared.title, "Release 0.9", "the title is kept");
    }

    #[test]
    fn the_owners_edits_are_pointed_out_to_the_orchestrator_once() {
        use crate::chats::Chats;
        let chats = Chats::default();
        let chat = chats.open_project("pr1", "Release 0.9", 1);
        let first_draft = apply(None, first(), ORCHESTRATOR, 2).expect("a first draft");
        let owned = apply(
            Some(&first_draft),
            Edit {
                revision: Some(1),
                budget: Some(usd(5)),
                ..Edit::default()
            },
            OWNER,
            3,
        )
        .expect("the owner's edit");
        chats
            .update(&chat.id, |c| {
                c.draft = Some(first_draft.clone());
                c.draft_seen = 1;
                Ok(())
            })
            .expect("the draft is kept");
        chats
            .update(&chat.id, |c| {
                c.draft = Some(owned.clone());
                Ok(())
            })
            .expect("the owner's draft is kept");
        let note = edit_note(&chats, &chat.id).expect("the owner edited the draft");
        assert!(note.contains("revision 2"), "{note}");
        assert_eq!(edit_note(&chats, &chat.id), None, "pointed out once");
        // The orchestrator's own write is seen when it is made: nothing is pointed out.
        let written = apply(
            Some(&owned),
            Edit {
                revision: Some(2),
                title: Some("Release 0.9.1".into()),
                ..Edit::default()
            },
            ORCHESTRATOR,
            4,
        )
        .expect("the orchestrator's write");
        chats
            .update(&chat.id, |c| {
                c.draft = Some(written.clone());
                c.draft_seen = written.revision;
                Ok(())
            })
            .expect("the write is kept");
        assert_eq!(edit_note(&chats, &chat.id), None);
    }

    #[test]
    fn a_malformed_draft_is_refused_with_what_is_wrong() {
        let bad = |edit: Edit| apply(None, edit, ORCHESTRATOR, 1).unwrap_err();
        let (code, why) = bad(Edit {
            title: Some(" ".into()),
            ..first()
        });
        assert_eq!(code, 400);
        assert!(why.contains("title"), "{why}");
        let (_, why) = bad(Edit {
            tasks: Some(vec![task("1", "qa", &["2"]), task("2", "qa", &["1"])]),
            ..first()
        });
        assert!(why.contains("cycle") || why.contains("itself"), "{why}");
        let (_, why) = bad(Edit {
            kind: Some("film".into()),
            ..first()
        });
        assert!(why.contains("kind"), "{why}");
        let (_, why) = bad(Edit {
            budget: Some(usd(0)),
            ..first()
        });
        assert!(why.contains("cap"), "{why}");
        let (_, why) = bad(Edit {
            budget: Some(DraftBudget {
                monthly_cap: Some(0),
                ..usd(5)
            }),
            ..first()
        });
        assert!(why.contains("monthly"), "{why}");
        let (_, why) = bad(Edit {
            github: Some("gitlab".into()),
            ..first()
        });
        assert!(why.contains("github"), "{why}");
        let (_, why) = bad(Edit {
            disk_quota_bytes: Some(1024),
            ..first()
        });
        assert!(why.contains("disk quota"), "{why}");
        // The settings are the project's own, typed: an unknown one is refused when the edit is read.
        assert!(
            serde_json::from_value::<Edit>(json!({"settings": {"max_turns": 5, "owner": "me"}}))
                .is_err()
        );
        assert!(serde_json::from_value::<Edit>(json!({"settings": {"max_turns": 5}})).is_ok());
        // The budget names its unit: a budget in credits is not one.
        assert!(
            serde_json::from_value::<Edit>(json!({"budget": 9000})).is_err(),
            "a budget is an object"
        );
        assert!(
            serde_json::from_value::<Edit>(json!({"budget": {"unit": "credits", "cap": 1}}))
                .is_err()
        );
        // An edit with a field the draft does not have is not one: a draft has no team any more.
        assert!(serde_json::from_value::<Edit>(json!({"title": "x", "owner": "me"})).is_err());
        assert!(serde_json::from_value::<Edit>(json!({"team": []})).is_err());
    }

    #[test]
    fn the_start_card_says_the_draft_and_what_starting_costs() {
        let mut draft = apply(None, first(), ORCHESTRATOR, 1).expect("a first draft");
        draft.budget = Some(DraftBudget {
            unit: Unit::Usd,
            cap: 15_000_000,
            monthly_cap: Some(4_000_000),
        });
        draft.github = "ci".into();
        draft.disk_quota_bytes = Some(3 << 30);
        assert_eq!(
            summary(&draft),
            "Fix the flaky scheduler tests and tag 0.9.\n\nTasks:\n- 1: goal 1\n- 2: goal 2\n\nBudget: $15.00, $4.00 a month for the planner. Disk quota 3072 MiB. GitHub: ci."
        );
        draft.tasks.clear();
        draft.budget = None;
        draft.disk_quota_bytes = None;
        assert_eq!(
            summary(&draft),
            "Fix the flaky scheduler tests and tag 0.9.\n\nTasks:\nnone yet\n\nBudget: No budget yet. No disk quota. GitHub: ci."
        );
    }

    #[test]
    fn the_draft_keeps_its_budget_mode_and_quota_through_the_store() {
        use crate::chats::Chats;
        let chats = Chats::default();
        let chat = chats.open_project("pr1", "Release 0.9", 1);
        let mut draft = apply(None, first(), ORCHESTRATOR, 2).expect("a first draft");
        draft.budget = Some(DraftBudget {
            unit: Unit::Tokens,
            cap: 9_000,
            monthly_cap: None,
        });
        draft.github = "mirror".into();
        draft.disk_quota_bytes = Some(1 << 30);
        let saved = chats
            .update(&chat.id, |c| {
                c.draft = Some(draft.clone());
                Ok(())
            })
            .expect("the draft is kept");
        assert_eq!(saved.draft.as_ref(), Some(&draft));

        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::chats::def().ddl()).unwrap();
        apz_store::table::replace(&conn, crate::chats::def(), &chats.rows().unwrap()).unwrap();
        let loaded = Chats::default();
        loaded.load(&conn).unwrap();
        let back = loaded.get(&chat.id).unwrap().draft.unwrap();
        assert_eq!(back, draft);
        assert_eq!(back.budget.unwrap().unit, Unit::Tokens);
    }
}
