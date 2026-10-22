//! A conversation's project draft: the project the orchestrator sets up with the owner, which the owner starts.
//!
//! The orchestrator writes it with `plaza_project_draft` as the conversation goes, and the owner can edit it too
//! (`PATCH …/draft`). Each edit bumps the revision: a write names the revision it read, so neither overwrites a change
//! it has not seen, and the orchestrator is told of the owner's edits with the next message. Start
//! (`POST …/start {revision}`) takes the revision the owner looked at and checks the whole draft. Then, all or
//! nothing, it:
//! - opens the project (its plan the owner's, the orchestrator its lead);
//! - posts its tasks.
//!
//! The project's planner starts on the work loop's next tick, and each task's seats fill from there (`work_loop.rs`).
//! A step that fails undoes the ones before it: the tasks are withdrawn, and the project is archived, which ends its
//! agents' work. Afterwards the conversation is the project's line to the orchestrator: it is told there that the
//! project started, and the project's notices (a task ended, tasks to replan) come there too.
//!
//! GET   /v1/owner/conversations/{id}/draft               → the draft, and what still stops Start (`problems`)
//! PATCH /v1/owner/conversations/{id}/draft {revision, …} → the conversation, its draft one revision on
//! POST  /v1/owner/conversations/{id}/start {revision}    → the conversation, with its project, and what Start did

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::conversations::{self, Conversation};
use crate::state::App;
use crate::task_plan::{self, Planned};

pub const TOOLS: &[&str] = &["plaza_project_draft"];

/// A conversation's project draft (a child of the conversation's row).
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
    /// The project's budget, in credits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u64>,
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
    budget: Option<u64>,
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
        budget: edit.budget.or_else(|| base.as_ref().and_then(|d| d.budget)),
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

/// Whether a draft is well formed, whatever the plaza holds: its text within bounds, and its tasks a list that can be
/// posted (keys once, links inside the list, no cycle).
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
    if !draft.tasks.is_empty() {
        task_plan::check(&draft.tasks, |_| false)?;
    }
    Ok(())
}

/// What still stops Start, as the plaza is now: no tasks; a task for a role the plaza does not have; the project's
/// settings, which the plaza refuses; a budget below what the tasks need, or more than the plaza's budget holds.
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
    let settings = settings_of(draft);
    if let Err(why) = crate::project_settings::check(app, &settings) {
        found.push(format!("the settings: {why}"));
    }
    let task_budget = settings.task_budget;
    let needed: u64 = draft
        .tasks
        .iter()
        .map(|task| apz_plaza::tasks::resolve_budget(task.budget.or(task_budget)).unwrap_or(0))
        .sum();
    if let Some(budget) = draft.budget
        && budget < needed
    {
        found.push(format!(
            "the budget is {budget} credits, and the tasks alone need {needed}"
        ));
    }
    if let Some(budget) = draft.budget
        && i128::from(budget) > app.plaza.treasury.plaza_budget_balance()
    {
        found.push(format!(
            "the budget is {budget} credits, more than the plaza's budget holds"
        ));
    }
    found
}

/// The draft's settings as a project's overrides (none set when it has none).
fn settings_of(draft: &Draft) -> crate::project_settings::ProjectSettings {
    draft.settings.clone().unwrap_or_default()
}

/// The draft as the API shows it: its fields, and what still stops Start.
pub fn view(app: &App, draft: &Draft) -> Value {
    let mut value = serde_json::to_value(draft).unwrap_or(Value::Null);
    value["problems"] = json!(problems(app, draft));
    value
}

/// Saves `draft` as conversation `id`'s and publishes the conversation. `seen`: the orchestrator made the edit, so it
/// has seen this revision.
fn save(app: &App, id: &str, draft: Draft, seen: bool) -> Result<Conversation, (u16, String)> {
    let saved = app.conversations.update(id, |c| {
        if seen {
            c.draft_seen = draft.revision;
        }
        c.updated_ms = draft.updated_ms;
        c.draft = Some(draft);
        Ok(())
    })?;
    // The draft's edit is written at once, not on the saver's next pass.
    crate::store::saver::save_now(app, "conversations");
    app.bus
        .publish("owner_conversation", &conversations::view(app, &saved));
    Ok(saved)
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
            "Keep the project draft of the owner's conversation you are in current: the project you set up with the owner, which they start (you never start it). `read` returns the draft, its revision and what still stops Start. `write` changes it: give the revision you read (the owner may edit it too), and only the fields that change: title, goal, kind (code|other), repo (where the work goes, in words), tasks [{key, goal, acceptance, role, budget, after}] (after: keys of tasks of this list it runs after), budget (credits for the whole project) and settings (the project's overrides).",
            json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["read", "write"]},
                    "revision": {"type": "integer", "minimum": 0},
                    "title": {"type": "string"}, "goal": {"type": "string"},
                    "kind": {"type": "string", "enum": ["code", "other"]},
                    "repo": {"type": "string"},
                    "tasks": {"type": "array", "items": task, "maxItems": task_plan::MAX_PLANNED},
                    "budget": {"type": "integer", "minimum": 0},
                    "settings": {"type": "object"},
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    })
}

/// Runs `plaza_project_draft` for the orchestrator, in the conversation its current work item is in.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let id = app
        .work
        .current(agent.cid)
        .and_then(|item| item.origin.conversation)
        .ok_or(
            "plaza_project_draft works in the owner's conversations, and this work is not one",
        )?;
    let conversation = app
        .conversations
        .get(&id)
        .ok_or_else(|| format!("no conversation {id}"))?;
    match input.get("action").and_then(Value::as_str) {
        Some("read") => {
            app.conversations
                .update(&id, |c| {
                    c.draft_seen = c.draft.as_ref().map_or(0, |d| d.revision);
                    Ok(())
                })
                .map_err(|(_, e)| e)?;
            Ok(json!({
                "conversation": id,
                "project": conversation.project,
                "draft": conversation.draft.as_ref().map(|draft| view(app, draft)),
            })
            .to_string())
        }
        Some("write") => {
            let mut fields = input.clone();
            if let Some(object) = fields.as_object_mut() {
                object.remove("action");
            }
            let edit: Edit =
                serde_json::from_value(fields).map_err(|e| format!("a draft edit: {e}"))?;
            let draft = apply(
                conversation.draft.as_ref(),
                edit,
                conversations::ORCHESTRATOR,
                apz_plaza::now_ms(),
            )
            .map_err(|(_, e)| e)?;
            let found = problems(app, &draft);
            let revision = draft.revision;
            save(app, &id, draft, true).map_err(|(_, e)| e)?;
            let next = if found.is_empty() {
                "Nothing stops Start: tell the owner it is ready, and that they start it."
                    .to_string()
            } else {
                format!("Before the owner can start it: {}.", found.join("; "))
            };
            Ok(format!(
                "Draft revision {revision} saved; the owner sees it beside the conversation. {next}"
            ))
        }
        _ => Err("action is read or write".into()),
    }
}

// ---- The owner's routes -------------------------------------------------------------------------------------------

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/owner/conversations/{id}/draft", get_route)
        .doc(RouteDoc::new("Read a conversation's project draft").errors(&[404]))
        .route("PATCH", "/v1/owner/conversations/{id}/draft", patch_route)
        .doc(RouteDoc::new("Edit a conversation's project draft").errors(&[400, 404, 409, 500]))
        .post("/v1/owner/conversations/{id}/start", start_route)
        .doc(
            RouteDoc::new("Start the project a conversation drafted")
                .errors(&[400, 404, 409, 500, 502]),
        )
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let Some(conversation) = app.conversations.get(p.get("id")) else {
        return Response::error(404, "no such conversation");
    };
    match &conversation.draft {
        Some(draft) => Response::json(200, &view(app, draft)),
        None => Response::error(404, "the conversation has no draft yet"),
    }
}

/// Body: `{revision, title?, goal?, kind?, repo?, tasks?, budget?, settings?}`.
fn patch_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let edit: Edit = match req.json() {
        Ok(edit) => edit,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let id = p.get("id");
    let Some(conversation) = app.conversations.get(id) else {
        return Response::error(404, "no such conversation");
    };
    let draft = match apply(
        conversation.draft.as_ref(),
        edit,
        conversations::OWNER,
        apz_plaza::now_ms(),
    ) {
        Ok(draft) => draft,
        Err((code, e)) => return Response::error(code, &e),
    };
    match save(app, id, draft, false) {
        Ok(conversation) => Response::json(200, &conversations::view(app, &conversation)),
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
        Ok((conversation, done)) => {
            let mut value = conversations::view(app, &conversation);
            value["started"] = done;
            Response::json(200, &value)
        }
        Err((code, e)) => Response::error(code, &e),
    }
}

// ---- Start --------------------------------------------------------------------------------------------------------

/// Starts conversation `id`'s draft at `revision`: checks it, then opens the project and posts its tasks, undoing what
/// was done when a step fails. Returns the conversation and what Start did.
pub fn start(app: &App, id: &str, revision: u64) -> Result<(Conversation, Value), (u16, String)> {
    let conversation = app
        .conversations
        .get(id)
        .ok_or_else(|| (404, format!("no conversation {id}")))?;
    if conversation.archived {
        return Err((409, format!("conversation {id} is archived")));
    }
    if let Some(project) = &conversation.project {
        return Err((
            409,
            format!("this conversation started project {project} already"),
        ));
    }
    let draft = conversation
        .draft
        .clone()
        .ok_or_else(|| (409, "the conversation has no draft yet".to_string()))?;
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

    // 1. The project, its plan the owner's.
    let goal = match &draft.repo {
        Some(repo) => format!("{}\n\nWhere the work goes: {repo}", draft.goal.trim()),
        None => draft.goal.trim().to_string(),
    };
    let project = app
        .projects
        .open_for_owner(draft.title.trim(), &goal, &draft.kind)
        .map_err(|e| (400, e))?;
    let pid = project.id.clone();
    crate::projects::attach_repo(app, &project);
    if let Err(e) = app.projects.set_lead(&pid, conversation.cid) {
        undo(app, &pid, &[]);
        return Err((500, e));
    }

    // 2. Its settings, and its budget set aside: its tasks are funded from it.
    let settings = settings_of(&draft);
    let task_budget = settings.task_budget;
    if let Err(e) = app.project_settings.set(&pid, settings) {
        undo(app, &pid, &[]);
        return Err((500, format!("the project's settings were not saved: {e}")));
    }
    if let Some(budget) = draft.budget
        && let Err(e) = crate::project_settings::fund(app, &pid, budget)
    {
        undo(app, &pid, &[]);
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
            undo(app, &pid, &[]);
            return Err((409, format!("the tasks were not posted: {e}")));
        }
    };
    let task_ids: Vec<String> = posted.iter().map(|task| task.id.clone()).collect();

    // The orchestrator leads it from this conversation: it hears when each task ends.
    for task in &task_ids {
        app.work.note_poster(task, conversation.cid);
    }
    let now = apz_plaza::now_ms();
    let started = app.conversations.update(id, |c| {
        c.project = Some(pid.clone());
        if let Some(draft) = c.draft.as_mut() {
            draft.started_ms = Some(now);
        }
        c.updated_ms = now;
        Ok(())
    })?;
    if let Some(project) = app.projects.get(&pid) {
        app.bus.publish("project", &project);
    }
    app.bus
        .publish("owner_conversation", &conversations::view(app, &started));
    let posted_in_words = posted_words(task_ids.len());
    let notice = format!(
        "[plaza] The owner started project {pid} \"{}\" from this conversation's draft (revision {revision}): {posted_in_words} (tasks {}). You lead it from here: the plaza hands each task out once everything it runs after is done. Follow the tasks with plaza_task_result, replan blocked ones, post new tasks to {pid} when the owner asks, and answer the owner in this conversation.",
        draft.title.trim(),
        task_ids.join(", "),
    );
    let shown = format!(
        "You started project {pid} \"{}\": {posted_in_words}.",
        draft.title.trim()
    );
    conversations::notify(app, id, &shown, &notice);
    let done = json!({
        "project": pid,
        "tasks": posted,
    });
    Ok((app.conversations.get(id).unwrap_or(started), done))
}

/// What Start did, in words: `2 tasks posted` (a project always has a task).
fn posted_words(tasks: usize) -> String {
    match tasks {
        1 => "1 task posted".to_string(),
        n => format!("{n} tasks posted"),
    }
}

/// Undoes a Start that failed after opening project `pid`: its tasks are withdrawn (their escrow back in its budget),
/// the project is archived, which ends its agents' work, and its budget goes back to the plaza.
fn undo(app: &App, pid: &str, tasks: &[String]) {
    for id in tasks {
        if let Some(task) = app.plaza.tasks.find(id) {
            let _ = task.withdraw();
        }
    }
    if let Err(e) = app.projects.archive(pid) {
        eprintln!("apid: project {pid} of a failed start not archived: {e}");
    }
    crate::work_loop::end_project(app, pid, "the project did not start");
    crate::project_settings::return_rest(app, pid);
    if let Some(project) = app.projects.get(pid) {
        app.bus.publish("project", &project);
    }
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
        let draft = apply(None, first(), "orchestrator", 10).expect("a first draft");
        assert_eq!(draft.revision, 1);
        assert_eq!(draft.kind, "code", "kind defaults to code");
        // The owner changes the budget on revision 1: the rest is kept.
        let edit = Edit {
            revision: Some(1),
            budget: Some(15_000),
            ..Edit::default()
        };
        let owned = apply(Some(&draft), edit, "owner", 20).expect("the owner's edit");
        assert_eq!((owned.revision, owned.budget), (2, Some(15_000)));
        assert_eq!(owned.tasks, draft.tasks);
        assert_eq!(owned.updated_by, "owner");
        // The orchestrator still writing on revision 1 is told to read again.
        let stale = Edit {
            revision: Some(1),
            title: Some("Release 0.9.1".into()),
            ..Edit::default()
        };
        let (code, why) = apply(Some(&owned), stale, "orchestrator", 30).unwrap_err();
        assert_eq!(code, 409);
        assert!(why.contains("revision 2"), "{why}");
        // A started draft is not edited again.
        let mut started = owned.clone();
        started.started_ms = Some(40);
        let edit = Edit {
            revision: Some(2),
            ..Edit::default()
        };
        assert_eq!(apply(Some(&started), edit, "owner", 50).unwrap_err().0, 409);
    }

    #[test]
    fn a_malformed_draft_is_refused_with_what_is_wrong() {
        let bad = |edit: Edit| apply(None, edit, "orchestrator", 1).unwrap_err();
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
        // The settings are the project's own, typed: an unknown one is refused when the edit is read.
        assert!(
            serde_json::from_value::<Edit>(json!({"settings": {"max_turns": 5, "owner": "me"}}))
                .is_err()
        );
        assert!(serde_json::from_value::<Edit>(json!({"settings": {"max_turns": 5}})).is_ok());
        // An edit with a field the draft does not have is not one: a draft has no team any more.
        assert!(serde_json::from_value::<Edit>(json!({"title": "x", "owner": "me"})).is_err());
        assert!(serde_json::from_value::<Edit>(json!({"team": []})).is_err());
    }
}
