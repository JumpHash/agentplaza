//! A project's settings: overrides of the plaza's settings for the project's agents, and the project's budget.
//!
//! Each field is optional. Unset, the plaza's value holds; set, it fills in before the plaza's (an agent's own choice
//! still wins). The resolvers are where the hub reads each setting:
//! - **Models:** the agent's override, then the project's model for the role, then the project's default, then the
//!   plaza's role model and default (`agents::resolved_model`, where `model_source` says `project`). A project's
//!   choices are held to the plaza's model policy, so a project narrows it and never widens it.
//! - **Effort and `max_tokens`:** the project's fill in before the plaza's (`internal::model_call`).
//! - **Limits:** field by field, the agent's, then the project's, then the plaza's (`agents::profile`).
//! - **A task's budget** when it names none (`task_budget`), and the **review timeout** of its tasks.
//! - **The permission mode** of its agents that have none of their own (`approvals::gate`).
//!
//! The project's instructions and skills are the prompt and skills bound to `project:<id>`, beside `plaza` and
//! `role:<role>`. Connectors and MCP servers take a `projects` list beside their roles.
//!
//! **The budget** is a `town_budget` account of the project's own (`apz_plaza::treasury`). Setting credits aside moves
//! them from the plaza's budget as the owner's operation, the project's tasks are funded from it (and refunded to it),
//! and when the project is over the rest goes back to the plaza.
//!
//! Kept in `project-settings.json` under the state dir, and with the plaza state (component `project_settings`).
//!
//! GET   /v1/projects/{id}/settings            → the project's overrides
//! PATCH /v1/projects/{id}/settings {…}        → the overrides (a field set to null goes back to the plaza's)
//! GET   /v1/projects/{id}/settings/effective  → every setting the project's agents get, and where it comes from
//! GET   /v1/projects/{id}/budget              → its budget: balance, and the plaza's
//! POST  /v1/projects/{id}/budget {credits}    → sets more aside (or, negative, gives some back)

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_plaza::treasury::PLAZA_OWNER;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::agents::Agent;
use crate::approvals::Mode;
use crate::settings::{Tier, tier_of};
use crate::state::App;

/// One project's overrides. Every field is optional: unset, the plaza's value holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSettings {
    /// The model of the project's agents when their role has none here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// The model of each role's agents.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub role_models: BTreeMap<String, String>,
    /// The reasoning effort of their model calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The longest answer of a model call, in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// The harness limits: turns per work item, the size a conversation is compressed at, the turns kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compress_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_recent: Option<u32>,
    /// A task's budget when it names none, in credits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_budget: Option<u64>,
    /// The permission mode of the project's agents that have none of their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<Mode>,
    /// How long a finished task waits for a reviewer before the owner is asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_timeout_minutes: Option<u32>,
}

/// Every project's overrides, by project id.
pub struct Store {
    path: PathBuf,
    by_project: Mutex<BTreeMap<String, ProjectSettings>>,
}

impl Store {
    /// Loads the store; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let by_project = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            by_project: Mutex::new(by_project),
        }
    }

    fn guard(&self) -> MutexGuard<'_, BTreeMap<String, ProjectSettings>> {
        self.by_project
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Project `id`'s overrides (none set when it has none).
    pub fn get(&self, id: &str) -> ProjectSettings {
        self.guard().get(id).cloned().unwrap_or_default()
    }

    /// Replaces project `id`'s overrides, and writes the file; on failure nothing changes.
    pub fn set(&self, id: &str, settings: ProjectSettings) -> std::io::Result<()> {
        let mut by_project = self.guard();
        let mut next = by_project.clone();
        if settings == ProjectSettings::default() {
            next.remove(id);
        } else {
            next.insert(id.to_string(), settings);
        }
        self.save(&next)?;
        *by_project = next;
        Ok(())
    }

    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let by_project = self.guard();
        if by_project.is_empty() {
            return String::new();
        }
        serde_json::to_string(&*by_project).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`] and writes the file; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: BTreeMap<String, ProjectSettings> =
            serde_json::from_str(json).map_err(|e| format!("project_settings: {e}"))?;
        let mut by_project = self.guard();
        self.save(&restored)
            .map_err(|e| format!("project_settings: {e}"))?;
        *by_project = restored;
        Ok(())
    }

    fn save(&self, by_project: &BTreeMap<String, ProjectSettings>) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(by_project).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// Checks a project's overrides against the plaza: models the roles may run (the plaza's model policy), roles it
/// has, an effort it knows, and limits within the harness's bounds.
pub fn check(app: &App, settings: &ProjectSettings) -> Result<(), String> {
    if let Some(model) = &settings.default_model {
        app.settings
            .allowed(Tier::Worker, model)
            .map_err(|e| format!("default_model: {e}"))?;
    }
    for (role, model) in &settings.role_models {
        if !crate::roles_custom::exists(app, role) {
            return Err(format!("role_models: the plaza has no role {role}"));
        }
        if crate::membership::plaza_wide(role) {
            return Err(format!(
                "role_models: {role} works on every project, so a project sets no model for it"
            ));
        }
        app.settings
            .allowed(tier_of(role), model)
            .map_err(|e| format!("role_models.{role}: {e}"))?;
    }
    if let Some(effort) = &settings.effort
        && !crate::settings::EFFORT_LEVELS.contains(&effort.as_str())
    {
        return Err(format!(
            "unknown effort {effort:?}; expected one of {}",
            crate::settings::EFFORT_LEVELS.join(", ")
        ));
    }
    if settings.max_tokens == Some(0) {
        return Err("max_tokens is at least 1 (unset it for the plaza's)".into());
    }
    if let Some(v) = settings.max_turns
        && !(1..=500).contains(&v)
    {
        return Err("max_turns must be 1 to 500".into());
    }
    if let Some(v) = settings.compress_at
        && v != 0
        && !(20_000..=4_000_000).contains(&v)
    {
        return Err("compress_at must be 0 (never) or 20000 to 4000000 bytes".into());
    }
    if let Some(v) = settings.keep_recent
        && !(1..=50).contains(&v)
    {
        return Err("keep_recent must be 1 to 50".into());
    }
    if let Some(budget) = settings.task_budget {
        apz_plaza::tasks::resolve_budget(Some(budget)).map_err(|e| format!("task_budget: {e}"))?;
    }
    if let Some(minutes) = settings.review_timeout_minutes
        && !(1..=10_080).contains(&minutes)
    {
        return Err("review_timeout_minutes must be 1 to 10080 (a week)".into());
    }
    Ok(())
}

/// `current` with `patch` applied: a field given replaces it, `null` unsets it (back to the plaza's), and `role_models`
/// merges role by role (a role set to `null` goes). A field settings do not have is refused.
pub fn merged(current: &ProjectSettings, patch: &Value) -> Result<ProjectSettings, String> {
    let Some(patch) = patch.as_object() else {
        return Err("the body is an object of settings".into());
    };
    let mut base = match serde_json::to_value(current) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for (key, value) in patch {
        if key == "role_models" && value.is_object() {
            let mut roles = base
                .remove("role_models")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            for (role, model) in value.as_object().into_iter().flatten() {
                if model.is_null() {
                    roles.remove(role);
                } else {
                    roles.insert(role.clone(), model.clone());
                }
            }
            base.insert(key.clone(), Value::Object(roles));
        } else if value.is_null() {
            base.remove(key);
        } else {
            base.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(Value::Object(base)).map_err(|e| e.to_string())
}

// ---- Resolvers ----------------------------------------------------------------------------------------------------

/// The overrides of the project agent `cid` works on, if it works on one.
pub fn of_agent(app: &App, cid: u64) -> Option<ProjectSettings> {
    app.members
        .project_of(cid)
        .map(|project| app.project_settings.get(&project))
}

/// The model agent `agent` runs, from the project's overrides (its role's, else the project's default); `None` when
/// the project sets none (or the agent is on none).
pub fn model_for(app: &App, agent: &Agent) -> Option<String> {
    let settings = of_agent(app, agent.cid)?;
    settings
        .role_models
        .get(&agent.role)
        .cloned()
        .or(settings.default_model)
}

/// The budget a task of project `pid` gets when it names none.
pub fn task_budget(app: &App, pid: Option<&str>) -> Option<u64> {
    pid.and_then(|pid| app.project_settings.get(pid).task_budget)
}

/// How long a finished task of project `pid` waits for a reviewer, in milliseconds.
pub fn review_timeout_ms(app: &App, pid: Option<&str>) -> u64 {
    let minutes = pid
        .and_then(|pid| app.project_settings.get(pid).review_timeout_minutes)
        .unwrap_or(app.settings.get().review_timeout_minutes);
    u64::from(minutes) * 60_000
}

/// The permission mode of agent `cid` when it has none of its own: its project's, else autonomous.
pub fn permission_mode(app: &App, cid: u64) -> Mode {
    of_agent(app, cid)
        .and_then(|settings| settings.permission_mode)
        .unwrap_or_default()
}

// ---- The budget ---------------------------------------------------------------------------------------------------

/// Sets `credits` of the plaza's budget aside for project `pid` (the owner's operation).
pub fn fund(app: &App, pid: &str, credits: u64) -> Result<(), String> {
    let amount = i64::try_from(credits).map_err(|_| "too many credits".to_string())?;
    app.plaza
        .treasury
        .move_budget(PLAZA_OWNER, pid, amount)
        .map(|_| ())
        .map_err(|e| format!("the plaza's budget could not set {credits} credits aside: {e}"))
}

/// Gives the rest of project `pid`'s budget back to the plaza (it is over); nothing when it has none.
pub fn return_rest(app: &App, pid: &str) {
    if let Err(e) = app.plaza.treasury.return_town_budget(pid) {
        eprintln!("apid: project {pid}'s budget not returned to the plaza: {e}");
    }
}

/// Project `pid`'s budget as the API shows it.
fn budget_view(app: &App, pid: &str) -> Value {
    let treasury = &app.plaza.treasury;
    json!({
        "project": pid,
        "own": treasury.has_town_budget(pid),
        "balance": i64::try_from(treasury.town_budget_balance(pid)).unwrap_or(i64::MAX),
        "plaza": i64::try_from(treasury.plaza_budget_balance()).unwrap_or(i64::MAX),
    })
}

/// After a change to project `pid`'s overrides, its agents get their new profiles at once.
fn push_members(app: &App, pid: &str) {
    for cid in app.members.members(pid) {
        if let Some(agent) = app.agents.get(cid)
            && agent.status != crate::agents::Status::Offline
        {
            crate::agents::push_profile(app, &agent);
        }
    }
}

// ---- Routes -------------------------------------------------------------------------------------------------------

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/settings", get_route)
        .doc(RouteDoc::new("Read a project's settings overrides").errors(&[404]))
        .route("PATCH", "/v1/projects/{id}/settings", patch_route)
        .doc(RouteDoc::new("Change a project's settings overrides").errors(&[400, 404, 500]))
        .get("/v1/projects/{id}/settings/effective", effective_route)
        .doc(
            RouteDoc::new("Show the settings a project's agents get, and where each comes from")
                .errors(&[404]),
        )
        // GET /v1/projects/{id}/budget is the work engine's budget (work_routes.rs); this POST moves credits until
        // the treasury goes (Engine 0.5).
        .post("/v1/projects/{id}/budget", budget_route)
        .doc(
            RouteDoc::new("Set credits aside for a project, or give some back")
                .errors(&[400, 404, 409]),
        )
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    if app.projects.get(p.get("id")).is_none() {
        return Response::error(404, "no such project");
    }
    Response::json(200, &app.project_settings.get(p.get("id")))
}

/// Body: the settings to change; `null` sets one back to the plaza's.
fn patch_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    let patch: Value = match req.json() {
        Ok(patch) => patch,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let next = match merged(&app.project_settings.get(id), &patch) {
        Ok(next) => next,
        Err(e) => return Response::error(400, &e),
    };
    if let Err(e) = check(app, &next) {
        return Response::error(400, &e);
    }
    if let Err(e) = app.project_settings.set(id, next.clone()) {
        return Response::error(500, &format!("settings not saved: {e}"));
    }
    push_members(app, id);
    if let Some(project) = app.projects.get(id) {
        app.bus.publish("project", &project);
    }
    Response::json(200, &next)
}

/// Every setting the project's agents get: `{value, source}`, the source `project` or `plaza`.
fn effective_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    Response::json(200, &effective(app, id))
}

/// The settings project `id`'s agents get, each with where it comes from.
pub fn effective(app: &App, id: &str) -> Value {
    let own = app.project_settings.get(id);
    let plaza = app.settings.get();
    let pick = |project: Option<Value>, fallback: Value| match project {
        Some(value) => json!({"value": value, "source": "project"}),
        None => json!({"value": fallback, "source": "plaza"}),
    };
    let mut role_models: BTreeMap<String, Value> = plaza
        .role_models
        .iter()
        .map(|(role, model)| (role.clone(), json!({"value": model, "source": "plaza"})))
        .collect();
    for (role, model) in &own.role_models {
        role_models.insert(role.clone(), json!({"value": model, "source": "project"}));
    }
    json!({
        "project": id,
        "settings": {
            "default_model": pick(own.default_model.clone().map(Value::from), json!(plaza.default_model)),
            "effort": pick(own.effort.clone().map(Value::from), json!(plaza.effort)),
            "max_tokens": pick(own.max_tokens.map(Value::from), json!(plaza.max_tokens)),
            "max_turns": pick(own.max_turns.map(Value::from), json!(plaza.max_turns)),
            "compress_at": pick(own.compress_at.map(Value::from), json!(crate::harness_control::DEFAULT_COMPRESS_AT)),
            "keep_recent": pick(own.keep_recent.map(Value::from), json!(crate::harness_control::DEFAULT_KEEP_RECENT)),
            "task_budget": pick(own.task_budget.map(Value::from), json!(apz_plaza::tasks::DEFAULT_TASK_BUDGET)),
            "permission_mode": pick(own.permission_mode.map(|m| json!(m)), json!(Mode::default())),
            "review_timeout_minutes": pick(own.review_timeout_minutes.map(Value::from), json!(plaza.review_timeout_minutes)),
        },
        "role_models": role_models,
        "budget": budget_view(app, id),
    })
}

#[derive(Deserialize)]
struct BudgetBody {
    credits: i64,
}

/// Body: `{credits}`: positive sets that much more aside from the plaza's budget, negative gives that much back.
fn budget_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Some(project) = app.projects.get(p.get("id")) else {
        return Response::error(404, "no such project");
    };
    let body: BudgetBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    // A delivered project in maintenance still funds the work the owner picks.
    if project.archived || (project.stage == "delivered" && !project.in_maintenance()) {
        return Response::error(
            409,
            &format!(
                "project {} is over: its budget went back to the plaza",
                project.id
            ),
        );
    }
    let moved = match body.credits {
        0 => return Response::error(400, "credits is not 0"),
        credits if credits > 0 => app
            .plaza
            .treasury
            .move_budget(PLAZA_OWNER, &project.id, credits),
        credits => {
            app.plaza
                .treasury
                .move_budget(&project.id, PLAZA_OWNER, credits.saturating_neg())
        }
    };
    match moved {
        Ok(_) => {
            app.bus.publish("project", &project);
            Response::json(200, &budget_view(app, &project.id))
        }
        Err(e) => Response::error(409, &format!("the budget did not move: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_sets_unsets_and_merges_role_models() {
        let current = ProjectSettings {
            effort: Some("high".into()),
            role_models: BTreeMap::from([
                ("builder".into(), "anthropic/claude-sonnet-5-5".into()),
                ("qa".into(), "openai/gpt-6-luna".into()),
            ]),
            ..ProjectSettings::default()
        };
        let next = merged(
            &current,
            &json!({"effort": null, "max_turns": 40, "role_models": {"qa": null, "release": "xai/grok-4.7"}}),
        )
        .expect("a patch");
        assert_eq!(next.effort, None, "null goes back to the plaza's");
        assert_eq!(next.max_turns, Some(40));
        assert_eq!(
            next.role_models,
            BTreeMap::from([
                ("builder".into(), "anthropic/claude-sonnet-5-5".into()),
                ("release".into(), "xai/grok-4.7".into()),
            ])
        );
        assert!(
            merged(&current, &json!({"colour": "red"})).is_err(),
            "no such setting"
        );
        assert!(merged(&current, &json!([1])).is_err());
        let mode = merged(&current, &json!({"permission_mode": "ask"})).expect("a mode");
        assert_eq!(mode.permission_mode, Some(Mode::Ask));
        assert!(merged(&current, &json!({"permission_mode": "sometimes"})).is_err());
    }

    #[test]
    fn settings_survive_a_restart_and_a_snapshot_and_an_empty_one_is_dropped() {
        let dir =
            std::env::temp_dir().join(format!("apid-project-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(dir.join("project-settings.json"));
        let settings = ProjectSettings {
            task_budget: Some(3000),
            ..ProjectSettings::default()
        };
        store.set("pr1", settings.clone()).unwrap();
        assert_eq!(
            Store::load(dir.join("project-settings.json")).get("pr1"),
            settings
        );
        let other = Store::load(dir.join("other.json"));
        other.restore(&store.snapshot()).unwrap();
        assert_eq!(other.get("pr1").task_budget, Some(3000));
        store.set("pr1", ProjectSettings::default()).unwrap();
        assert!(
            store.snapshot().is_empty(),
            "a project with no overrides keeps no entry"
        );
        assert!(other.restore("not json").is_err());
    }
}
