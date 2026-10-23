//! A project's settings: overrides of the plaza's settings for the project's agents, and the project's budget.
//!
//! Each field is optional. Unset, the plaza's value holds; set, it fills in before the plaza's (an agent's own choice
//! still wins). The resolvers are where the hub reads each setting:
//! - **Models:** the agent's override, then the project's model for the role, then the project's default, then the
//!   plaza's role model and default (`agents::resolved_model`, where `model_source` says `project`). A project's
//!   choices are held to the plaza's model policy, so a project narrows it and never widens it.
//! - **Effort and `max_tokens`:** the project's fill in before the plaza's (`internal::model_call`).
//! - **Limits:** field by field, the agent's, then the project's, then the plaza's (`agents::profile`).
//! - **A task's budget** when it names none (`task_budget`, in the project budget's unit), and the **review timeout** of
//!   its tasks.
//! - **The permission mode** of its agents that have none of their own (`approvals::gate`).
//! - **The hooks** its agents run around their edits and commands (`hooks_of_agent`, pushed in each agent's profile), and
//!   the change's land hooks before it joins the landing queue (`land_refusals`, in `work_loop`).
//!
//! The project's instructions and skills are the prompt and skills bound to `project:<id>`, beside `plaza` and
//! `role:<role>`. Connectors and MCP servers take a `projects` list beside their roles.
//!
//! Kept in plaza.db (table `project_setting`: a row per project that has overrides, its role models a child table).
//!
//! GET   /v1/projects/{id}/settings            → the project's overrides
//! PATCH /v1/projects/{id}/settings {…}        → the overrides (a field set to null goes back to the plaza's)
//! GET   /v1/projects/{id}/settings/effective  → every setting the project's agents get, and where it comes from

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use apz_proto::hooks::{Action, ActionKind, Hook, Mode as HookMode, On, default_hooks, findings};

use crate::agents::Agent;
use crate::approvals::Mode;
use crate::settings::{Tier, tier_of};
use crate::state::App;

/// One project's overrides. Every field is optional: unset, the plaza's value holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSettings {
    /// The model of the project's agents when their role has none here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// The model of each role's agents.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[table(map_value)]
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
    /// The amount a task reserves when it names none, in the project budget's unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_budget: Option<u64>,
    /// The permission mode of the project's agents that have none of their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<Mode>,
    /// How long a finished task waits for a reviewer before the owner is asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_timeout_minutes: Option<u32>,
    /// The minutes between the full suite's runs on main (the coverage map and the misses, PLAN 15.4). 0 turns the
    /// schedule off; [`crate::full_run::DEFAULT_MINUTES`] when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_suite_minutes: Option<u32>,
    /// The most tasks of the project that run at once (its team's maximum); [`DEFAULT_TEAM_MAX`] when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_max: Option<u32>,
    /// The team the planner aims for when the graph allows, never above the maximum; [`DEFAULT_TEAM_TARGET`] when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_target: Option<u32>,
    /// How many of the project's lanes the merge queue tests together (a batch, 4.6); [`DEFAULT_QUEUE_BATCH`] when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_batch: Option<u32>,
    /// Whether the owner approves each landing (a `land` approval, 4.6). Off by default: the plaza lands what passed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub land_approval: bool,
    /// Whether the project releases continuously: each landing starts the release recipe (22; a stub until then, 4.6).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub release_continuous: bool,
    /// How many hours a project's previews outlive their lane's landing or closing (PLAN 18.1). A preview target's own
    /// `grace_hours` overrides it; 24 when neither is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_grace_hours: Option<u64>,
    /// The words an intake item may have, title and text together, before it gets a spec and a plan rather than a task
    /// (planning.rs); [`crate::planning::DEFAULT_SPEC_OVER_WORDS`] when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_over_words: Option<u32>,
    /// The project's hooks: checks and formatters that run around its agents' edits and commands (`apz_proto::hooks`).
    /// The JSON names them as a list under `hooks`; `"hooks": "defaults"` sets `hook_defaults` instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(child)]
    pub hooks: Vec<HookRow>,
    /// Whether the project's hooks start from the defaults (`apz_proto::hooks::default_hooks`), before its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hook_defaults: bool,
    /// Whether the project's subscription calls take the policy's API-key route (`allowances::route`): the owner's
    /// "use an API key for this project" choice on an allowance card. Off: they wait on their allowance.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subscription_fallback: bool,
    /// The intake rules: which outside items (GitHub, Linear, Notion) become this project's tasks, by repository,
    /// label, team or database, and on which recipe. The JSON names them as a list under `intake`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(child)]
    pub intake: Vec<IntakeRule>,
    /// The project's limits on its performance comparisons with the last qualified stable release (PLAN 22.8): a
    /// metric's name (exact, or a prefix ending in `*`) and its largest change in tenths of a percent (150 is 15 percent;
    /// for an error rate, tenths of a point). The defaults (`apz_topology::compare`) hold for the metrics no row names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(child)]
    pub performance_limits: Vec<PerfLimit>,
}

/// One limit on a performance comparison: the metric's name and the largest change it may make, in tenths of a
/// percent (an integer, so the row keeps its equality).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default, deny_unknown_fields)]
pub struct PerfLimit {
    pub metric: String,
    pub max_tenths: u32,
}

/// One intake rule: an outside item whose `field` is `value` becomes a task of the project on `recipe`. The field is
/// a repository or a label (GitHub), a team's key or a label (Linear), a database's id (Notion). An item from a public
/// repository starts work only when `allow_public` is set; otherwise it waits for the owner (`held`). The Notion fields
/// name the database's status property and its options (empty: `Status`, `In progress`, `Done`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default, deny_unknown_fields)]
pub struct IntakeRule {
    pub source: String,
    pub field: String,
    pub value: String,
    pub recipe: String,
    pub allow_public: bool,
    pub status_property: String,
    pub started: String,
    pub done: String,
}

/// The longest grace a preview may keep after its lane ends: a year.
pub const MAX_PREVIEW_GRACE_HOURS: u64 = 8760;

/// Every project's overrides, by project id.
#[derive(Default)]
pub struct Store {
    by_project: Mutex<BTreeMap<String, ProjectSettings>>,
}

impl Store {
    fn guard(&self) -> MutexGuard<'_, BTreeMap<String, ProjectSettings>> {
        self.by_project
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Every project's overrides, by project id (the intake routes read every project's rules).
    pub fn all(&self) -> Vec<(String, ProjectSettings)> {
        self.guard()
            .iter()
            .map(|(id, settings)| (id.clone(), settings.clone()))
            .collect()
    }

    /// Project `id`'s overrides (none set when it has none).
    pub fn get(&self, id: &str) -> ProjectSettings {
        self.guard().get(id).cloned().unwrap_or_default()
    }

    /// Replaces project `id`'s overrides; a project with none keeps no entry. The change is kept in memory and the
    /// saver writes it to plaza.db, so this does not fail.
    pub fn set(&self, id: &str, settings: ProjectSettings) -> std::io::Result<()> {
        let mut by_project = self.guard();
        if settings == ProjectSettings::default() {
            by_project.remove(id);
        } else {
            by_project.insert(id.to_string(), settings);
        }
        Ok(())
    }

    /// The overrides' rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let rows: Vec<SettingsRow> = self
            .guard()
            .iter()
            .map(|(id, settings)| SettingsRow::of(id, settings))
            .collect();
        RowSet::list(def(), &rows)
    }

    /// Replaces the overrides with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<SettingsRow> = apz_store::table::load_list(conn, def())?;
        *self.guard() = stored.into_iter().map(SettingsRow::into_entry).collect();
        Ok(())
    }
}

/// One project's overrides as a row of `project_setting`: the project is the key and each setting a column (the role
/// models are a child table).
#[derive(Debug, Clone, PartialEq, Eq, apz_store::Table)]
#[table(name = "project_setting")]
struct SettingsRow {
    #[table(key)]
    project: String,
    default_model: Option<String>,
    effort: Option<String>,
    max_tokens: Option<u32>,
    max_turns: Option<u32>,
    compress_at: Option<u64>,
    keep_recent: Option<u32>,
    task_budget: Option<u64>,
    permission_mode: Option<Mode>,
    review_timeout_minutes: Option<u32>,
    full_suite_minutes: Option<u32>,
    team_max: Option<u32>,
    team_target: Option<u32>,
    queue_batch: Option<u32>,
    spec_over_words: Option<u32>,
    land_approval: bool,
    release_continuous: bool,
    preview_grace_hours: Option<u64>,
    hook_defaults: bool,
    subscription_fallback: bool,
    #[table(map_value)]
    role_models: BTreeMap<String, String>,
    #[table(child)]
    hooks: Vec<HookRow>,
    #[table(child)]
    intake: Vec<IntakeRule>,
    #[table(child)]
    performance_limits: Vec<PerfLimit>,
}

/// One hook of a project as its row: the action is a kind and, for a rule, the rule's id, and its paths and languages
/// are child lists (the tables `project_setting_hooks` and its lists). The JSON is the hook's own
/// ([`apz_proto::hooks::Hook`]), so the consoles and the wire see the hook, never its row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(from = "Hook", into = "Hook")]
pub struct HookRow {
    on: On,
    action: ActionKind,
    rule_id: Option<String>,
    mode: HookMode,
    #[table(list)]
    paths: Vec<String>,
    #[table(list)]
    languages: Vec<String>,
}

impl From<Hook> for HookRow {
    fn from(hook: Hook) -> Self {
        let (action, rule) = hook.action.split();
        HookRow {
            on: hook.on,
            action,
            rule_id: rule.map(str::to_string),
            mode: hook.mode,
            paths: hook.paths,
            languages: hook.languages,
        }
    }
}

impl From<HookRow> for Hook {
    fn from(row: HookRow) -> Self {
        Hook {
            on: row.on,
            paths: row.paths,
            languages: row.languages,
            // A stored rule always has its id (the check names one); an empty id is reported as not in the catalogue.
            action: Action::join(row.action, row.rule_id)
                .unwrap_or(Action::Rule { id: String::new() }),
            mode: row.mode,
        }
    }
}

impl SettingsRow {
    fn of(project: &str, settings: &ProjectSettings) -> Self {
        SettingsRow {
            project: project.to_string(),
            default_model: settings.default_model.clone(),
            effort: settings.effort.clone(),
            max_tokens: settings.max_tokens,
            max_turns: settings.max_turns,
            compress_at: settings.compress_at,
            keep_recent: settings.keep_recent,
            task_budget: settings.task_budget,
            permission_mode: settings.permission_mode,
            review_timeout_minutes: settings.review_timeout_minutes,
            full_suite_minutes: settings.full_suite_minutes,
            team_max: settings.team_max,
            team_target: settings.team_target,
            queue_batch: settings.queue_batch,
            spec_over_words: settings.spec_over_words,
            land_approval: settings.land_approval,
            release_continuous: settings.release_continuous,
            preview_grace_hours: settings.preview_grace_hours,
            hook_defaults: settings.hook_defaults,
            subscription_fallback: settings.subscription_fallback,
            role_models: settings.role_models.clone(),
            hooks: settings.hooks.clone(),
            intake: settings.intake.clone(),
            performance_limits: settings.performance_limits.clone(),
        }
    }

    fn into_entry(self) -> (String, ProjectSettings) {
        let settings = ProjectSettings {
            default_model: self.default_model,
            role_models: self.role_models,
            effort: self.effort,
            max_tokens: self.max_tokens,
            max_turns: self.max_turns,
            compress_at: self.compress_at,
            keep_recent: self.keep_recent,
            task_budget: self.task_budget,
            permission_mode: self.permission_mode,
            review_timeout_minutes: self.review_timeout_minutes,
            full_suite_minutes: self.full_suite_minutes,
            team_max: self.team_max,
            team_target: self.team_target,
            queue_batch: self.queue_batch,
            spec_over_words: self.spec_over_words,
            land_approval: self.land_approval,
            release_continuous: self.release_continuous,
            preview_grace_hours: self.preview_grace_hours,
            hooks: self.hooks,
            intake: self.intake,
            performance_limits: self.performance_limits,
            hook_defaults: self.hook_defaults,
            subscription_fallback: self.subscription_fallback,
        };
        (self.project, settings)
    }
}

/// The table of the projects' overrides: `project_setting`.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(SettingsRow)
}

/// Checks a project's overrides against the plaza: models the roles may run (the plaza's model policy), roles it
/// has, an effort it knows, and limits within the harness's bounds.
pub fn check(app: &App, settings: &ProjectSettings) -> Result<(), String> {
    if let Some(batch) = settings.queue_batch
        && !(1..=16).contains(&batch)
    {
        return Err("queue_batch: a batch holds from 1 to 16 lanes".into());
    }
    crate::intake::check_rules(&settings.intake)?;
    for limit in &settings.performance_limits {
        if limit.metric.trim().is_empty() || limit.max_tenths == 0 {
            return Err(
                "performance_limits: each names a metric and a largest change above zero (tenths of a percent)".into(),
            );
        }
    }
    if settings
        .preview_grace_hours
        .is_some_and(|hours| hours > MAX_PREVIEW_GRACE_HOURS)
    {
        return Err(format!(
            "preview_grace_hours: a preview outlives its lane by a year at most ({MAX_PREVIEW_GRACE_HOURS} hours)"
        ));
    }
    if let Some(model) = &settings.default_model {
        app.settings
            .allowed(Tier::Worker, model)
            .map_err(|e| format!("default_model: {e}"))?;
    }
    for (role, model) in &settings.role_models {
        if !crate::roles_custom::exists(app, role) {
            return Err(format!("role_models: the plaza has no role {role}"));
        }
        if crate::settings::plaza_wide(role) {
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
    if settings.task_budget == Some(0) {
        return Err("task_budget is at least 1 (unset it for the plaza's)".into());
    }
    if let Some(minutes) = settings.review_timeout_minutes
        && !(1..=10_080).contains(&minutes)
    {
        return Err("review_timeout_minutes must be 1 to 10080 (a week)".into());
    }
    if let Some(minutes) = settings.full_suite_minutes
        && minutes > 10_080
    {
        return Err("full_suite_minutes must be 0 (off) to 10080 (a week)".into());
    }
    check_team(settings)?;
    let hooks: Vec<Hook> = settings.hooks.iter().cloned().map(Hook::from).collect();
    check_hooks(&hooks)
}

/// A project's team sizes are each 1 to [`TEAM_LIMIT`] (pure). The target is kept within the maximum where it is read
/// ([`team_target`]), so the two need not be checked against each other.
fn check_team(settings: &ProjectSettings) -> Result<(), String> {
    if let Some(max) = settings.team_max
        && !(1..=TEAM_LIMIT).contains(&max)
    {
        return Err(format!("team_max must be 1 to {TEAM_LIMIT}"));
    }
    if let Some(target) = settings.team_target
        && !(1..=TEAM_LIMIT).contains(&target)
    {
        return Err(format!("team_target must be 1 to {TEAM_LIMIT}"));
    }
    Ok(())
}

/// The most hooks a project declares (its defaults are not counted: they are the plaza's own list).
const MAX_HOOKS: usize = 64;

/// Checks a project's hooks: a rule the catalogue has, languages the hooks name, a format or a lint only on an edit,
/// and paths that are not empty.
fn check_hooks(hooks: &[Hook]) -> Result<(), String> {
    if hooks.len() > MAX_HOOKS {
        return Err(format!("hooks: at most {MAX_HOOKS} hooks a project"));
    }
    for hook in hooks {
        let name = hook.action.name();
        if let Action::Rule { id } = &hook.action
            && !apz_proto::hooks::RULES.iter().any(|(known, _)| known == id)
        {
            return Err(format!("hooks: rule {id:?} is not in the catalogue"));
        }
        if matches!(hook.action, Action::Format | Action::Lint) && hook.on != On::Edit {
            return Err(format!("hooks: {name} runs on edit only"));
        }
        if let Some(language) = hook
            .languages
            .iter()
            .find(|l| !apz_proto::hooks::LANGUAGES.contains(&l.as_str()))
        {
            return Err(format!("hooks: unknown language {language:?}"));
        }
        if hook.paths.iter().any(|p| p.is_empty()) {
            return Err("hooks: a path pattern must not be empty".into());
        }
    }
    Ok(())
}

/// The hooks a project's agents run: its defaults when it takes them, then its own.
pub fn effective_hooks(settings: &ProjectSettings) -> Vec<Hook> {
    let mut hooks = if settings.hook_defaults {
        default_hooks()
    } else {
        Vec::new()
    };
    hooks.extend(settings.hooks.iter().cloned().map(Hook::from));
    hooks
}

/// The findings a change's landing refuses on, from the hooks of its project that run on `land`: each finding of a
/// block-mode hook, on the lines the change's diff adds. Report-mode findings do not refuse, so they are not returned.
pub fn land_refusals(hooks: &[Hook], diff: &str) -> Vec<String> {
    let added = apz_proto::hooks::added_lines(diff);
    hooks
        .iter()
        .filter(|hook| hook.on == On::Land && hook.mode == HookMode::Block)
        .flat_map(|hook| {
            let name = hook.action.name();
            findings(&hook.action, &added)
                .into_iter()
                .map(move |line| format!("{name}: {line}"))
        })
        .collect()
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
        if key == "hooks" && value.as_str() == Some("defaults") {
            base.insert("hook_defaults".into(), Value::Bool(true));
        } else if key == "role_models" && value.is_object() {
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
    // `hooks` is a list in the JSON: a patch that names it "defaults" sets hook_defaults, and a null patch unsets it.
    if let Some(value) = base.get("hooks")
        && !value.is_array()
        && !value.is_null()
    {
        return Err("hooks is a list of hooks, or \"defaults\"".into());
    }
    serde_json::from_value(Value::Object(base)).map_err(|e| e.to_string())
}

// ---- Resolvers ----------------------------------------------------------------------------------------------------

/// The overrides of the project agent `cid` works on, if it works on one.
pub fn of_agent(app: &App, cid: u64) -> Option<ProjectSettings> {
    crate::projects::project_of(app, cid).map(|project| app.project_settings.get(&project))
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

/// The minutes between a project's full suite runs: its setting, or the default (0 turns the schedule off).
pub fn full_suite_minutes(app: &App, pid: &str) -> u32 {
    app.project_settings
        .get(pid)
        .full_suite_minutes
        .unwrap_or(crate::full_run::DEFAULT_MINUTES)
}

/// The amount (in the project budget's unit) a task of project `pid` reserves when it names none.
pub fn task_budget(app: &App, pid: Option<&str>) -> Option<u64> {
    pid.and_then(|pid| app.project_settings.get(pid).task_budget)
}

/// The most tasks a project runs at once when it sets no maximum, and the team its planner aims for when it sets no
/// target (PLAN 7.1, W25). The largest either may be set to is [`TEAM_LIMIT`].
pub const DEFAULT_TEAM_MAX: u32 = 20;
/// The lanes the merge queue tests together when the project sets no batch size (4.6).
pub const DEFAULT_QUEUE_BATCH: u32 = 4;
pub const DEFAULT_TEAM_TARGET: u32 = 10;
pub const TEAM_LIMIT: u32 = 100;

/// The most tasks of project `pid` run at once: its team's maximum.
pub fn team_max(app: &App, pid: &str) -> usize {
    app.project_settings
        .get(pid)
        .team_max
        .unwrap_or(DEFAULT_TEAM_MAX)
        .try_into()
        .unwrap_or(usize::MAX)
}

/// The team the planner of project `pid` aims for: its target, never above its maximum.
pub fn team_target(app: &App, pid: &str) -> u32 {
    let settings = app.project_settings.get(pid);
    settings
        .team_target
        .unwrap_or(DEFAULT_TEAM_TARGET)
        .min(settings.team_max.unwrap_or(DEFAULT_TEAM_MAX))
}

/// How long a finished task of project `pid` waits for a reviewer, in milliseconds.
pub fn review_timeout_ms(app: &App, pid: Option<&str>) -> u64 {
    let minutes = pid
        .and_then(|pid| app.project_settings.get(pid).review_timeout_minutes)
        .unwrap_or(app.settings.get().review_timeout_minutes);
    u64::from(minutes) * 60_000
}

/// The hooks agent `cid` runs: its project's effective hooks (none when it works on no project).
pub fn hooks_of_agent(app: &App, cid: u64) -> Vec<Hook> {
    of_agent(app, cid).map_or_else(Vec::new, |settings| effective_hooks(&settings))
}

/// The permission mode of agent `cid` when it has none of its own: its project's, else autonomous.
pub fn permission_mode(app: &App, cid: u64) -> Mode {
    of_agent(app, cid)
        .and_then(|settings| settings.permission_mode)
        .unwrap_or_default()
}

/// After a change to project `pid`'s overrides, its agents get their new profiles at once.
fn push_agents(app: &App, pid: &str) {
    for cid in crate::projects::agents_of(app, pid) {
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
    push_agents(app, id);
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
    let hooks = effective_hooks(&own);
    json!({
        "project": id,
        "settings": {
            "hooks": pick((!hooks.is_empty()).then(|| json!(hooks)), json!([])),
            "default_model": pick(own.default_model.clone().map(Value::from), json!(plaza.default_model)),
            "effort": pick(own.effort.clone().map(Value::from), json!(plaza.effort)),
            "max_tokens": pick(own.max_tokens.map(Value::from), json!(plaza.max_tokens)),
            "max_turns": pick(own.max_turns.map(Value::from), json!(plaza.max_turns)),
            "compress_at": pick(own.compress_at.map(Value::from), json!(crate::harness_control::DEFAULT_COMPRESS_AT)),
            "keep_recent": pick(own.keep_recent.map(Value::from), json!(crate::harness_control::DEFAULT_KEEP_RECENT)),
            "task_budget": match own.task_budget {
                Some(budget) => json!({"value": budget, "source": "project"}),
                None => json!({"value": null, "source": "default"}),
            },
            "permission_mode": pick(own.permission_mode.map(|m| json!(m)), json!(Mode::default())),
            "review_timeout_minutes": pick(own.review_timeout_minutes.map(Value::from), json!(plaza.review_timeout_minutes)),
            "full_suite_minutes": pick(own.full_suite_minutes.map(Value::from), json!(crate::full_run::DEFAULT_MINUTES)),
            "team_max": pick(own.team_max.map(Value::from), json!(DEFAULT_TEAM_MAX)),
            "team_target": pick(own.team_target.map(Value::from), json!(DEFAULT_TEAM_TARGET)),
        },
        "role_models": role_models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_grace_is_a_project_setting_that_a_patch_sets_and_clears() {
        let set = merged(
            &ProjectSettings::default(),
            &json!({"preview_grace_hours": 48}),
        )
        .expect("merges");
        assert_eq!(set.preview_grace_hours, Some(48));
        let cleared = merged(&set, &json!({"preview_grace_hours": null})).expect("merges");
        assert_eq!(cleared.preview_grace_hours, None);
    }

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
    fn a_team_is_sized_between_one_and_the_limit() {
        let sized = |max: Option<u32>, target: Option<u32>| {
            check_team(&ProjectSettings {
                team_max: max,
                team_target: target,
                ..ProjectSettings::default()
            })
        };
        assert!(sized(None, None).is_ok(), "unset, the defaults hold");
        assert!(sized(Some(1), Some(1)).is_ok());
        assert!(sized(Some(TEAM_LIMIT), Some(TEAM_LIMIT)).is_ok());
        assert!(sized(Some(0), None).is_err());
        assert!(sized(Some(TEAM_LIMIT + 1), None).is_err());
        assert!(sized(None, Some(0)).is_err());
        assert!(sized(None, Some(TEAM_LIMIT + 1)).is_err());
        // The target above a lowered maximum is read as the maximum, so it is not refused here.
        assert!(sized(Some(4), Some(10)).is_ok());
    }

    #[test]
    fn a_team_patch_sets_and_unsets_its_sizes() {
        let current = ProjectSettings {
            team_max: Some(4),
            ..ProjectSettings::default()
        };
        let next = merged(&current, &json!({"team_max": null, "team_target": 6})).expect("a patch");
        assert_eq!((next.team_max, next.team_target), (None, Some(6)));
    }

    #[test]
    fn settings_come_back_from_their_table_and_an_empty_one_is_dropped() {
        let store = Store::default();
        let settings = ProjectSettings {
            task_budget: Some(3000),
            permission_mode: Some(Mode::ReadOnly),
            team_max: Some(6),
            team_target: Some(3),
            role_models: BTreeMap::from([("builder".into(), "anthropic/claude-sonnet-5-5".into())]),
            ..ProjectSettings::default()
        };
        store.set("pr1", settings.clone()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let loaded = Store::default();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.get("pr1"), settings);
        store.set("pr1", ProjectSettings::default()).unwrap();
        assert!(
            store.rows().unwrap().tables["project_setting"].is_empty(),
            "a project with no overrides keeps no row"
        );
    }

    fn secret_hook(on: On, mode: HookMode) -> Hook {
        Hook {
            on,
            paths: Vec::new(),
            languages: Vec::new(),
            action: Action::SecretScan,
            mode,
        }
    }

    #[test]
    fn a_patch_takes_a_list_of_hooks_or_the_defaults() {
        let current = ProjectSettings::default();
        let next = merged(
            &current,
            &json!({"hooks": [{"on": "edit", "action": {"kind": "rule", "id": "conflict-markers"}, "paths": ["src/**"]}]}),
        )
        .expect("a list of hooks");
        assert_eq!(next.hooks.len(), 1);
        assert_eq!(next.hooks[0].on, On::Edit);
        assert!(!next.hook_defaults);
        let defaults = merged(&next, &json!({"hooks": "defaults"})).expect("the defaults");
        assert!(defaults.hook_defaults);
        assert_eq!(
            defaults.hooks, next.hooks,
            "the list stays when the defaults are named"
        );
        assert!(merged(&current, &json!({"hooks": "sometimes"})).is_err());
        let unset = merged(&next, &json!({"hooks": null})).expect("unset");
        assert!(unset.hooks.is_empty());
    }

    #[test]
    fn a_hook_is_checked_before_it_is_kept() {
        let rule = |id: &str| Hook {
            action: Action::Rule { id: id.into() },
            ..secret_hook(On::Edit, HookMode::Report)
        };
        assert!(check_hooks(&[rule("conflict-markers")]).is_ok());
        assert!(check_hooks(&[rule("made-up")]).is_err(), "an unknown rule");
        let mut klingon = secret_hook(On::Edit, HookMode::Report);
        klingon.languages = vec!["klingon".into()];
        assert!(check_hooks(&[klingon]).is_err(), "an unknown language");
        let format_on_run = Hook {
            action: Action::Format,
            ..secret_hook(On::Run, HookMode::Report)
        };
        assert!(
            check_hooks(&[format_on_run]).is_err(),
            "a format runs on edit"
        );
        let mut blank = secret_hook(On::Run, HookMode::Block);
        blank.paths = vec![String::new()];
        assert!(check_hooks(&[blank]).is_err(), "an empty path");
    }

    #[test]
    fn hooks_come_back_from_their_tables_and_the_defaults_come_first() {
        let store = Store::default();
        let settings = ProjectSettings {
            hooks: vec![
                Hook {
                    on: On::Edit,
                    paths: vec!["src/**".into()],
                    languages: vec!["rust".into()],
                    action: Action::Format,
                    mode: HookMode::Report,
                }
                .into(),
                Hook {
                    on: On::Land,
                    paths: Vec::new(),
                    languages: Vec::new(),
                    action: Action::Rule {
                        id: "conflict-markers".into(),
                    },
                    mode: HookMode::Block,
                }
                .into(),
            ],
            hook_defaults: true,
            ..ProjectSettings::default()
        };
        store.set("pr1", settings.clone()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let loaded = Store::default();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.get("pr1"), settings);
        let effective = effective_hooks(&settings);
        assert_eq!(effective.len(), default_hooks().len() + 2);
        assert_eq!(
            effective[effective.len() - 1],
            Hook::from(settings.hooks[1].clone())
        );
    }

    #[test]
    fn a_land_hook_refuses_on_the_lines_a_change_adds_only() {
        let key = format!("sk-ant-{}", "a1".repeat(12));
        let diff = format!("--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-old {key}\n+fine\n+key = {key}\n");
        let refused = land_refusals(&[secret_hook(On::Land, HookMode::Block)], &diff);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(!refused[0].contains(&key));
        assert!(land_refusals(&[secret_hook(On::Land, HookMode::Report)], &diff).is_empty());
        assert!(land_refusals(&[secret_hook(On::Edit, HookMode::Block)], &diff).is_empty());
    }
}
