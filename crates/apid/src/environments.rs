//! Environments: where a project runs. A project has as many as it needs (production, staging, previews, a
//! developer's), each with its address, what it runs (`track`: every landing on main, the signed releases, or what
//! the owner deploys by hand), its settings and secrets, and the record of what was deployed there.
//!
//! A deploy records the version and, when the environment has a deploy hook (a CI's or a host's deploy URL, kept in
//! the vault), calls it with `{project, environment, version, repo, url, by}`. An environment that tracks main
//! deploys each landing, and one that tracks releases each signed release. A protected environment (production, by
//! default) never deploys on its own or for an agent: the agent's request waits in Approvals as a `deploy`, and a
//! tracked landing or release waits there too.
//!
//! Owner routes (`/v1/projects/{id}/environments…`):
//! - `GET` → [`Environment`] list (with the names of its secrets, never their values)
//! - `PUT /{env} {kind, url?, track?, protected?, variables?, notes?, hook?}`: creates or changes one (`hook` a URL
//!   puts it in the vault, null removes it, absent keeps it)
//! - `DELETE /{env}`
//! - `POST /{env}/deploy {version?}`: deploys now (the newest signed release or main's head when not given)
//! - `PUT /{env}/secrets/{name} {value}`, `DELETE /{env}/secrets/{name}`: the environment's secrets, in the vault
//!   bound to `project:<id>.<env>`
//!
//! Tools: `plaza_environments` (read) and `plaza_deploy`. Kept in plaza.db (table `project_environment`, one row per
//! project; its environments, deploys and variables are child tables).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use apz_cache::Generations;
use apz_http::{Params, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::state::App;

pub const TOOLS: &[&str] = &["plaza_environments", "plaza_deploy"];

pub const KINDS: &[&str] = &["production", "staging", "preview", "development", "other"];
/// What an environment runs: each landing on main, each signed release, or what is deployed by hand.
pub const TRACKS: &[&str] = &["main", "releases", "manual"];
/// The environments a project may have.
const MAX_ENVIRONMENTS: usize = 20;
/// The deploys an environment remembers.
const MAX_DEPLOYS: usize = 30;

/// One deploy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Deploy {
    /// `d<n>` in the environment.
    pub id: String,
    pub version: String,
    pub by: String,
    pub at_ms: u64,
    /// "recorded" (no hook), "sent" (the hook answered 2xx), "failed", "waiting" (for the owner), "refused".
    pub status: String,
    #[serde(default)]
    pub detail: String,
    /// The approval it waits for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

/// Where a project runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Environment {
    /// A slug, unique in the project: production, staging, pr-42.
    pub name: String,
    /// One of [`KINDS`].
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// One of [`TRACKS`].
    #[serde(default = "manual")]
    pub track: String,
    /// Deploys wait for the owner.
    #[serde(default)]
    pub protected: bool,
    /// Settings that are not secret.
    #[serde(default)]
    #[table(map_value = "environment_variable")]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
    /// What runs there now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub deployed_ms: u64,
    #[serde(default)]
    #[table(child = "deploy")]
    pub deploys: Vec<Deploy>,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
    /// Filled on read: the names of its secrets in the vault, and whether it has a deploy hook.
    #[serde(default, skip_deserializing)]
    #[table(skip)]
    pub secrets: Vec<String>,
    #[serde(default, skip_deserializing)]
    #[table(skip)]
    pub hook: bool,
}

fn manual() -> String {
    "manual".into()
}

/// One project's environments, as its row of `project_environment`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, apz_store::Table)]
#[table(name = "project_environment")]
struct ProjectEnvs {
    #[table(key)]
    project: String,
    #[serde(default)]
    #[table(child = "environment")]
    environments: Vec<Environment>,
    #[serde(default)]
    seq: u64,
}

/// The environments of every project.
#[derive(Default)]
pub struct Store {
    items: Mutex<Vec<ProjectEnvs>>,
    /// The hub's generations: a change to a project's environments bumps its `project:<id>`.
    gens: Arc<Generations<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    apz_plaza::now_ms()
}

impl Store {
    /// Counts this store's changes in `gens` (the hub's generations).
    pub fn with_generations(mut self, gens: Arc<Generations<String>>) -> Self {
        self.gens = gens;
        self
    }

    pub(crate) fn of(&self, project: &str) -> Vec<Environment> {
        lock(&self.items)
            .iter()
            .find(|p| p.project == project)
            .map(|p| p.environments.clone())
            .unwrap_or_default()
    }

    fn mutate<T>(
        &self,
        project: &str,
        change: impl FnOnce(&mut ProjectEnvs) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut items = lock(&self.items);
        let mut next = items.clone();
        let at = match next.iter().position(|p| p.project == project) {
            Some(at) => at,
            None => {
                next.push(ProjectEnvs {
                    project: project.to_string(),
                    ..ProjectEnvs::default()
                });
                next.len() - 1
            }
        };
        let out = change(&mut next[at])?;
        *items = next;
        crate::sources::bump(&self.gens, &crate::sources::project(project));
        Ok(out)
    }

    /// The rows of every project's environments, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(def(), lock(&self.items).iter())
    }

    /// Replaces the environments with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<ProjectEnvs> = apz_store::table::load_list(conn, def())?;
        *lock(&self.items) = stored;
        Ok(())
    }
}

/// The table of the environments: `project_environment`, one row per project.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(ProjectEnvs)
}

/// The vault binding an environment's secrets are read under.
pub fn binding(project: &str, env: &str) -> String {
    format!("project:{project}.{env}")
}

/// The vault name of an environment's secret `name`: `PR3_PRODUCTION_DATABASE_URL`.
fn secret_name(project: &str, env: &str, name: &str) -> String {
    format!("{project}_{env}_{name}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

const HOOK: &str = "DEPLOY_HOOK";

/// A secret's name as the owner gives it: letters, digits and underscores, starting with a letter.
fn valid_secret(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// The project's environments, with their secrets' names and whether each has a deploy hook.
pub fn list(app: &App, project: &str) -> Vec<Environment> {
    let entries = app.vault.list().unwrap_or_default();
    app.environments
        .of(project)
        .into_iter()
        .map(|mut env| {
            let bind = binding(project, &env.name);
            let prefix = secret_name(project, &env.name, "");
            let hook = secret_name(project, &env.name, HOOK);
            for e in &entries {
                let bound = e["bindings"]
                    .as_array()
                    .is_some_and(|b| b.iter().any(|x| x.as_str() == Some(bind.as_str())));
                let name = e["name"].as_str().unwrap_or("");
                if !bound {
                    continue;
                }
                if name == hook {
                    env.hook = true;
                } else if let Some(short) = name.strip_prefix(&prefix) {
                    env.secrets.push(short.to_string());
                }
            }
            env.secrets.sort();
            env
        })
        .collect()
}

fn put_secret(app: &App, project: &str, env: &str, name: &str, value: &str) -> Result<(), String> {
    app.vault
        .put_plaza(
            &secret_name(project, env, name),
            &binding(project, env),
            &format!("{project} {env}: {name}"),
            &SecretString::from(value.to_string()),
            "owner",
            now_ms(),
        )
        .map(|_| ())
        .map_err(|e| format!("the vault: {}", e.message()))
}

fn forget_secret(app: &App, project: &str, env: &str, name: &str) -> bool {
    let mut gone = false;
    for e in app
        .vault
        .entries_named(&secret_name(project, env, name))
        .unwrap_or_default()
    {
        gone |= app.vault.delete(&e.id, "owner", now_ms()).is_ok();
    }
    gone
}

fn hook_url(app: &App, project: &str, env: &str) -> Option<String> {
    app.vault.value_for(
        &secret_name(project, env, HOOK),
        &binding(project, env),
        &crate::vault::OnBehalf::Hub,
    )
}

// ---- Changing them -------------------------------------------------------------------------------------------------

/// What the owner sends to make or change an environment.
#[derive(Debug, Deserialize)]
struct EnvIn {
    kind: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    protected: Option<bool>,
    #[serde(default)]
    variables: Option<BTreeMap<String, String>>,
    #[serde(default)]
    notes: Option<String>,
}

fn check_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2000 {
        return Err("a URL starts with http:// or https:// (at most 2000 characters)".into());
    }
    Ok(url.to_string())
}

/// Creates or changes environment `name` of `project`; a hook URL in `hook` goes into the vault.
fn put(app: &App, project: &str, name: &str, body: &Value) -> Result<Environment, (u16, String)> {
    if !valid_name(name) {
        return Err((
            400,
            "an environment's name is lower-case letters, digits and dashes, starting with a letter".into(),
        ));
    }
    let input: EnvIn =
        serde_json::from_value(body.clone()).map_err(|e| (400, format!("bad body: {e}")))?;
    if !KINDS.contains(&input.kind.as_str()) {
        return Err((400, format!("kind must be one of {}", KINDS.join(", "))));
    }
    let track = input.track.unwrap_or_else(|| {
        match input.kind.as_str() {
            "production" => "releases",
            "staging" | "preview" => "main",
            _ => "manual",
        }
        .to_string()
    });
    if !TRACKS.contains(&track.as_str()) {
        return Err((400, format!("track must be one of {}", TRACKS.join(", "))));
    }
    let url = match input.url.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(url) => Some(check_url(url).map_err(|e| (400, e))?),
    };
    let variables = input.variables.unwrap_or_default();
    if variables.len() > 100
        || variables
            .iter()
            .any(|(k, v)| !valid_secret(k) || v.chars().count() > 4000)
    {
        return Err((
            400,
            "variables: at most 100, each NAME (letters, digits, _) with a value of at most 4000 characters".into(),
        ));
    }
    let notes: String = input
        .notes
        .unwrap_or_default()
        .trim()
        .chars()
        .take(2000)
        .collect();
    match body.get("hook") {
        Some(Value::String(hook)) if !hook.trim().is_empty() => {
            let hook = check_url(hook).map_err(|e| (400, format!("hook: {e}")))?;
            put_secret(app, project, name, HOOK, &hook).map_err(|e| (503, e))?;
        }
        Some(Value::Null) => {
            forget_secret(app, project, name, HOOK);
        }
        _ => {}
    }
    let kind = input.kind;
    let protected = input.protected;
    let now = now_ms();
    let name = name.to_string();
    app.environments
        .mutate(project, |p| {
            match p.environments.iter_mut().find(|e| e.name == name) {
                Some(env) => {
                    env.kind = kind;
                    env.url = url;
                    env.track = track;
                    if let Some(protected) = protected {
                        env.protected = protected;
                    }
                    env.variables = variables;
                    env.notes = notes;
                    env.updated_ms = now;
                }
                None => {
                    if p.environments.len() >= MAX_ENVIRONMENTS {
                        return Err(format!(
                            "a project has at most {MAX_ENVIRONMENTS} environments"
                        ));
                    }
                    p.environments.push(Environment {
                        protected: protected.unwrap_or(kind == "production"),
                        name: name.clone(),
                        kind,
                        url,
                        track,
                        variables,
                        notes,
                        version: None,
                        deployed_ms: 0,
                        deploys: Vec::new(),
                        created_ms: now,
                        updated_ms: now,
                        secrets: Vec::new(),
                        hook: false,
                    });
                }
            }
            Ok(())
        })
        .map_err(|e| (409, e))?;
    list(app, project)
        .into_iter()
        .find(|e| e.name == name)
        .ok_or((500, "the environment was not saved".into()))
}

/// Makes or changes an environment for the owner (a project adopted with its environments).
pub fn put_for_owner(
    app: &App,
    project: &str,
    name: &str,
    body: &Value,
) -> Result<Environment, String> {
    put(app, project, name, body).map_err(|(_, e)| e)
}

// ---- Deploys -------------------------------------------------------------------------------------------------------

/// The version to deploy when none is given: the newest signed (or imported) release, or main's head.
fn default_version(app: &App, project: &str, track: &str) -> Option<String> {
    let repo = app.projects.get(project)?.repo?;
    if track == "releases" {
        let releases = app.weft.get(&format!("/repos/{repo}/releases")).ok()?;
        let newest = releases["releases"]
            .as_array()?
            .iter()
            // A signed release, or one imported with the project's history (it was released before the plaza).
            .filter(|r| !r["signed"].is_null() || r["imported"] == true)
            .max_by_key(|r| r["proposed_seq"].as_u64().unwrap_or(0))?;
        return newest["version"].as_str().map(str::to_string);
    }
    let detail = app.weft.get(&format!("/repos/{repo}")).ok()?;
    detail["refs"]["main"].as_str().map(str::to_string)
}

/// Records a deploy of `version` to `env` and calls its hook (in the background). With `approval`, the deploy waits
/// for it instead.
pub(crate) fn record(
    app: &App,
    project: &str,
    env: &str,
    version: &str,
    by: &str,
    approval: Option<String>,
) -> Result<Deploy, String> {
    let has_hook = hook_url(app, project, env).is_some();
    let status = if approval.is_some() {
        "waiting"
    } else if has_hook {
        "sending"
    } else {
        "recorded"
    };
    let now = now_ms();
    let deploy = app.environments.mutate(project, |p| {
        p.seq += 1;
        let id = format!("d{}", p.seq);
        let e = p
            .environments
            .iter_mut()
            .find(|e| e.name == env)
            .ok_or_else(|| format!("no environment {env}"))?;
        let deploy = Deploy {
            id,
            version: version.to_string(),
            by: by.to_string(),
            at_ms: now,
            status: status.into(),
            detail: String::new(),
            approval,
        };
        if deploy.approval.is_none() {
            e.version = Some(version.to_string());
            e.deployed_ms = now;
        }
        e.deploys.push(deploy.clone());
        if e.deploys.len() > MAX_DEPLOYS {
            let extra = e.deploys.len() - MAX_DEPLOYS;
            e.deploys.drain(..extra);
        }
        Ok(deploy)
    })?;
    publish(app, project, env, &deploy);
    Ok(deploy)
}

fn publish(app: &App, project: &str, env: &str, deploy: &Deploy) {
    app.bus.publish(
        "environment",
        &json!({"project": project, "environment": env, "deploy": deploy}),
    );
}

/// Settles a deploy's outcome: its status and what it says, and the version the environment runs now when `running`
/// says so (a rollout that rolled back leaves the previous one).
pub(crate) fn settle_deploy(
    app: &App,
    project: &str,
    env: &str,
    id: &str,
    status: &str,
    detail: &str,
    running: Option<Option<String>>,
) {
    let now = now_ms();
    let changed = app.environments.mutate(project, |p| {
        let e = p
            .environments
            .iter_mut()
            .find(|e| e.name == env)
            .ok_or("gone")?;
        if let Some(version) = running {
            e.version = version;
            e.deployed_ms = now;
        }
        let d = e.deploys.iter_mut().find(|d| d.id == id).ok_or("gone")?;
        d.status = status.to_string();
        d.detail = detail.chars().take(500).collect();
        Ok(d.clone())
    });
    if let Ok(deploy) = changed {
        publish(app, project, env, &deploy);
    }
}

fn set_status(app: &App, project: &str, env: &str, id: &str, status: &str, detail: &str) {
    let changed = app.environments.mutate(project, |p| {
        let e = p
            .environments
            .iter_mut()
            .find(|e| e.name == env)
            .ok_or("gone")?;
        let d = e.deploys.iter_mut().find(|d| d.id == id).ok_or("gone")?;
        d.status = status.to_string();
        d.detail = detail.chars().take(500).collect();
        Ok(d.clone())
    });
    if let Ok(deploy) = changed {
        publish(app, project, env, &deploy);
    }
}

/// Calls the hook of a recorded deploy, in the background: `{project, environment, version, repo, url, by}`.
fn send(app: &Arc<App>, project: &str, env: &str, deploy: &Deploy) {
    let Some(hook) = hook_url(app, project, env) else {
        return;
    };
    let repo = app.projects.get(project).and_then(|p| p.repo);
    let url = app
        .environments
        .of(project)
        .into_iter()
        .find(|e| e.name == env)
        .and_then(|e| e.url);
    let body = json!({
        "project": project, "environment": env, "version": deploy.version, "repo": repo, "url": url, "by": deploy.by,
    });
    let (app, project, env, id) = (
        Arc::clone(app),
        project.to_string(),
        env.to_string(),
        deploy.id.clone(),
    );
    std::thread::spawn(move || match call_hook(&hook, &body) {
        Ok(code) if (200..300).contains(&code) => set_status(
            &app,
            &project,
            &env,
            &id,
            "sent",
            &format!("the hook answered {code}"),
        ),
        Ok(code) => set_status(
            &app,
            &project,
            &env,
            &id,
            "failed",
            &format!("the hook answered {code}"),
        ),
        Err(e) => set_status(&app, &project, &env, &id, "failed", &e),
    });
}

/// POSTs `body` to a deploy hook; its HTTP status.
fn call_hook(hook: &str, body: &Value) -> Result<u16, String> {
    let request = ureq::http::Request::builder()
        .method("POST")
        .uri(hook)
        .header("User-Agent", "agentplaza")
        .header("Content-Type", "application/json")
        .body(body.to_string().into_bytes())
        .map_err(|e| format!("bad hook: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build();
    let response = ureq::Agent::new_with_config(config)
        .run(request)
        .map_err(|e| format!("the hook did not answer: {e}"))?;
    Ok(response.status().as_u16())
}

/// Deploys `version` (or the default for its track) to `env` now, as `by`.
pub fn deploy_now(
    app: &Arc<App>,
    project: &str,
    env: &str,
    version: Option<&str>,
    by: &str,
) -> Result<Deploy, (u16, String)> {
    let e = app
        .environments
        .of(project)
        .into_iter()
        .find(|e| e.name == env)
        .ok_or((404, format!("no environment {env}")))?;
    // A Cloudflare or Kubernetes environment deploys a qualified, published release through the deploy recipe (23).
    if let Some(plan) = crate::deploy::Plan::of(&e).map_err(|e| (409, e))? {
        let version = match version.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => v.to_string(),
            None => crate::deploy::newest(app, project)
                .map(|r| r.version)
                .ok_or((
                    409,
                    "no qualified, published release yet: name a version".to_string(),
                ))?,
        };
        crate::deploy::gate(app, project, &e, &version).map_err(|m| (409, m))?;
        return crate::deploy::start(app, project, &e, &version, by, plan).map_err(|m| (409, m));
    }
    let version = match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => v.chars().take(120).collect(),
        None => default_version(app, project, &e.track).ok_or((
            409,
            "no version to deploy: the project has no signed release or landing yet; name one"
                .to_string(),
        ))?,
    };
    let deploy = record(app, project, env, &version, by, None).map_err(|e| (409, e))?;
    send(app, project, env, &deploy);
    Ok(deploy)
}

/// Asks to deploy: at once to an open environment, through the owner's Approvals to a protected one.
pub(crate) fn request(
    app: &Arc<App>,
    project: &str,
    env: &str,
    version: &str,
    by_cid: u64,
    by: &str,
    why: &str,
) -> Result<Deploy, String> {
    let e = app
        .environments
        .of(project)
        .into_iter()
        .find(|e| e.name == env)
        .ok_or_else(|| format!("no environment {env}"))?;
    // A protected deploy to a platform asks only for a release that passed staging (23).
    if e.protected {
        crate::deploy::gate(app, project, &e, version)?;
    }
    if !e.protected {
        return deploy_now(app, project, env, Some(version), by).map_err(|(_, e)| e);
    }
    let approval = crate::approvals::submit(
        app,
        "deploy",
        &format!("Deploy {version} to {project} {env}"),
        json!({
            "cid": by_cid, "project": project, "environment": env, "version": version, "by": by, "why": why,
        }),
    );
    let deploy = record(app, project, env, version, by, Some(approval.id))?;
    let now = now_ms();
    let chat = crate::work_loop::project_chat(app, project, now);
    crate::work_loop::notice(
        app,
        &chat,
        &format!("Deploying {version} to {env} waits for the owner's approval in Approvals."),
        now,
    );
    Ok(deploy)
}

/// The owner's decision on a `deploy` approval: approved, the deploy goes out; denied, it is refused.
pub fn decided(app: &App, item: &crate::approvals::Approval, approve: bool) -> Result<(), String> {
    let detail = &item.detail;
    let (Some(project), Some(env)) = (detail["project"].as_str(), detail["environment"].as_str())
    else {
        return Err("the approval names no environment".into());
    };
    let found = app
        .environments
        .of(project)
        .into_iter()
        .find(|e| e.name == env);
    let waiting = found.as_ref().and_then(|e| {
        e.deploys
            .iter()
            .find(|d| d.approval.as_deref() == Some(item.id.as_str()))
            .cloned()
    });
    let Some(deploy) = waiting else {
        return Err(format!("no deploy waits for approval {}", item.id));
    };
    if !approve {
        set_status(
            app,
            project,
            env,
            &deploy.id,
            "refused",
            "the owner declined",
        );
        let now = now_ms();
        let chat = crate::work_loop::project_chat(app, project, now);
        crate::work_loop::notice(
            app,
            &chat,
            &format!(
                "The owner declined the deploy of {} to {env}.",
                deploy.version
            ),
            now,
        );
        return Ok(());
    }
    let now = now_ms();
    let has_hook = hook_url(app, project, env).is_some();
    let prior = found.as_ref().and_then(|e| e.version.clone());
    let sent = app.environments.mutate(project, |p| {
        let e = p
            .environments
            .iter_mut()
            .find(|e| e.name == env)
            .ok_or("gone")?;
        e.version = Some(deploy.version.clone());
        e.deployed_ms = now;
        let d = e
            .deploys
            .iter_mut()
            .find(|d| d.id == deploy.id)
            .ok_or("gone")?;
        d.status = if has_hook { "sending" } else { "recorded" }.into();
        d.detail = "approved by the owner".into();
        Ok(d.clone())
    })?;
    publish(app, project, env, &sent);
    // A Cloudflare environment runs its rollout instead of the hook.
    if let Some(arc) = app.handle().upgrade() {
        if let Some(e) = found.as_ref()
            && let Ok(Some(plan)) = crate::deploy::Plan::of(e)
        {
            crate::deploy::approved(&arc, project, e, plan, &sent, prior);
            return Ok(());
        }
        send(&arc, project, env, &sent);
    }
    Ok(())
}

/// After a landing on `repo`'s main, or a signed release: each environment of its project that tracks it deploys
/// (a protected one asks the owner first).
pub fn tracked(app: &Arc<App>, repo: &str, track: &str, version: &str) {
    let Some(project) = app
        .projects
        .list()
        .into_iter()
        .find(|p| p.repo.as_deref() == Some(repo) && !p.archived)
    else {
        return;
    };
    for e in app.environments.of(&project.id) {
        if e.track != track || e.version.as_deref() == Some(version) {
            continue;
        }
        // A Cloudflare environment that tracks releases deploys once the release is published (23, `published`).
        if track == "releases" && matches!(crate::deploy::Plan::of(&e), Ok(Some(_))) {
            continue;
        }
        let result = request(
            app,
            &project.id,
            &e.name,
            version,
            0,
            "the plaza",
            &format!("it tracks {track}"),
        );
        if let Err(err) = result {
            eprintln!(
                "apid: deploying {version} to {} {}: {err}",
                project.id, e.name
            );
        }
    }
}

// ---- Tools ---------------------------------------------------------------------------------------------------------

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_environments" => (
            "Your project's environments: each one's kind, address, what it runs (track), its settings, the names of \
             its secrets (never their values), and its recent deploys.",
            obj(json!({}), &[]),
        ),
        "plaza_deploy" => (
            "Deploy a version (a release or a change id) of your project to one of its environments. A protected one \
             (production) waits for the owner's approval.",
            obj(
                json!({
                    "environment": {"type": "string", "maxLength": 40},
                    "version": {"type": "string", "maxLength": 120},
                    "why": {"type": "string", "maxLength": 1000}
                }),
                &["environment", "version", "why"],
            ),
        ),
        _ => return None,
    })
}

pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    let project = crate::projects::project_of(app, agent.cid).ok_or("you are on no project")?;
    match name {
        "plaza_environments" => {
            let envs: Vec<Value> = list(app, &project)
                .into_iter()
                .map(|e| {
                    json!({
                        "name": e.name, "kind": e.kind, "url": e.url, "track": e.track, "protected": e.protected,
                        "variables": e.variables, "secrets": e.secrets, "version": e.version,
                        "deploys": e.deploys.iter().rev().take(5).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({"project": project, "environments": envs}).to_string())
        }
        "plaza_deploy" => {
            let arc = app.handle().upgrade().ok_or("the hub is not ready")?;
            let env = input
                .get("environment")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = input
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let why = input.get("why").and_then(Value::as_str).unwrap_or("");
            if version.is_empty() {
                return Err("name the version to deploy".into());
            }
            let deploy = request(&arc, &project, env, version, agent.cid, &agent.label(), why)?;
            Ok(match deploy.status.as_str() {
                "waiting" => format!(
                    "{env} is protected: the deploy of {version} waits for the owner's approval"
                ),
                status => format!("deployed {version} to {env} ({status})"),
            })
        }
        other => Err(format!("unknown tool {other}")),
    }
}

// ---- Routes --------------------------------------------------------------------------------------------------------

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/environments", |app, _, p| {
            with_project(app, p, |app, id| Response::json(200, &list(app, id)))
        })
        .doc(RouteDoc::new("List a project's environments").errors(&[404]))
        .route(
            "PUT",
            "/v1/projects/{id}/environments/{env}",
            |app, req, p| {
                with_project(app, p, |app, id| {
                    let body: Value = match serde_json::from_slice(&req.body) {
                        Ok(body) => body,
                        Err(e) => return Response::error(400, &format!("bad body: {e}")),
                    };
                    match put(app, id, p.get("env"), &body) {
                        Ok(env) => {
                            app.bus.publish(
                                "environment",
                                &json!({"project": id, "environment": env.name}),
                            );
                            Response::json(200, &env)
                        }
                        Err((code, e)) => Response::error(code, &e),
                    }
                })
            },
        )
        .doc(
            RouteDoc::new("Create or change a project's environment").errors(&[400, 404, 409, 503]),
        )
        .delete("/v1/projects/{id}/environments/{env}", |app, _, p| {
            with_project(app, p, |app, id| {
                let env = p.get("env").to_string();
                let secrets = list(app, id)
                    .into_iter()
                    .find(|e| e.name == env)
                    .map(|e| e.secrets)
                    .unwrap_or_default();
                match app.environments.mutate(id, |p| {
                    let before = p.environments.len();
                    p.environments.retain(|e| e.name != env);
                    Ok(before != p.environments.len())
                }) {
                    Ok(true) => {
                        forget_secret(app, id, &env, HOOK);
                        for s in secrets {
                            forget_secret(app, id, &env, &s);
                        }
                        app.bus.publish(
                            "environment",
                            &json!({"project": id, "environment": env, "removed": true}),
                        );
                        Response::json(200, &json!({"deleted": true}))
                    }
                    Ok(false) => Response::error(404, "no such environment"),
                    Err(e) => Response::error(500, &e),
                }
            })
        })
        .doc(RouteDoc::new("Remove a project's environment and its secrets").errors(&[404]))
        .post(
            "/v1/projects/{id}/environments/{env}/deploy",
            |app, req, p| {
                with_project(app, p, |app, id| {
                    let Some(arc) = app.handle().upgrade() else {
                        return Response::error(503, "the hub is not ready");
                    };
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let version = body.get("version").and_then(Value::as_str);
                    match deploy_now(&arc, id, p.get("env"), version, "owner") {
                        Ok(deploy) => Response::json(200, &deploy),
                        Err((code, e)) => Response::error(code, &e),
                    }
                })
            },
        )
        .doc(RouteDoc::new("Deploy a version to an environment now").errors(&[404, 409, 503]))
        .route(
            "PUT",
            "/v1/projects/{id}/environments/{env}/secrets/{name}",
            |app, req, p| {
                with_env(app, p, |app, id, env| {
                    let name = p.get("name");
                    if !valid_secret(name) || name == HOOK {
                        return Response::error(
                            400,
                            "a secret's name is letters, digits and _, starting with a letter",
                        );
                    }
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let Some(value) = body
                        .get("value")
                        .and_then(Value::as_str)
                        .filter(|v| !v.is_empty())
                    else {
                        return Response::error(400, "missing value");
                    };
                    match put_secret(app, id, env, name, value) {
                        Ok(()) => {
                            Response::json(200, &json!({"name": name, "binding": binding(id, env)}))
                        }
                        Err(e) => Response::error(503, &e),
                    }
                })
            },
        )
        .doc(
            RouteDoc::new("Set an environment's secret (kept in the vault)")
                .errors(&[400, 404, 503]),
        )
        .delete(
            "/v1/projects/{id}/environments/{env}/secrets/{name}",
            |app, _, p| {
                with_env(app, p, |app, id, env| {
                    if forget_secret(app, id, env, p.get("name")) {
                        Response::json(200, &json!({"deleted": true}))
                    } else {
                        Response::error(404, "no such secret")
                    }
                })
            },
        )
        .doc(RouteDoc::new("Remove an environment's secret").errors(&[404]))
}

fn with_project(app: &App, p: &Params<'_>, then: impl FnOnce(&App, &str) -> Response) -> Response {
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    then(app, id)
}

fn with_env(
    app: &App,
    p: &Params<'_>,
    then: impl FnOnce(&App, &str, &str) -> Response,
) -> Response {
    with_project(app, p, |app, id| {
        let env = p.get("env");
        if !app.environments.of(id).iter().any(|e| e.name == env) {
            return Response::error(404, "no such environment");
        }
        then(app, id, env)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_secret_names_are_checked() {
        assert!(valid_name("production") && valid_name("pr-42"));
        assert!(!valid_name("Prod") && !valid_name("-x") && !valid_name(""));
        assert!(valid_secret("DATABASE_URL") && !valid_secret("1X") && !valid_secret("a-b"));
        assert_eq!(secret_name("pr3", "pr-42", "API_KEY"), "PR3_PR_42_API_KEY");
        assert_eq!(binding("pr3", "staging"), "project:pr3.staging");
    }

    #[test]
    fn the_store_keeps_each_projects_environments_apart() {
        let store = Store::default();
        for (project, name) in [
            ("pr1", "production"),
            ("pr1", "staging"),
            ("pr2", "production"),
        ] {
            store
                .mutate(project, |p| {
                    p.environments.push(Environment {
                        name: name.into(),
                        kind: "production".into(),
                        url: None,
                        track: "manual".into(),
                        protected: true,
                        variables: BTreeMap::new(),
                        notes: String::new(),
                        version: None,
                        deployed_ms: 0,
                        deploys: Vec::new(),
                        created_ms: 1,
                        updated_ms: 1,
                        secrets: Vec::new(),
                        hook: false,
                    });
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(store.of("pr1").len(), 2);
        assert_eq!(store.of("pr2").len(), 1);
        store
            .mutate("pr1", |p| {
                let env = &mut p.environments[0];
                env.variables.insert("REGION".into(), "eu".into());
                env.deploys.push(Deploy {
                    id: "d1".into(),
                    version: "v1".into(),
                    by: "owner".into(),
                    at_ms: 2,
                    status: "recorded".into(),
                    detail: String::new(),
                    approval: None,
                });
                Ok(())
            })
            .unwrap();
        assert!(store.of("pr9").is_empty());
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let again = Store::default();
        again.load(&conn).unwrap();
        assert_eq!(again.of("pr1"), store.of("pr1"));
        assert_eq!(again.of("pr1")[0].variables["REGION"], "eu");
    }

    #[test]
    fn a_change_to_a_projects_environments_moves_its_source() {
        let gens: std::sync::Arc<apz_cache::Generations<String>> = Default::default();
        let store = Store::default().with_generations(std::sync::Arc::clone(&gens));
        let key = crate::sources::project("app");
        store
            .mutate("app", |_| Ok::<(), String>(()))
            .expect("a change");
        assert!(
            gens.get(&key) > 0,
            "a change to the environments moves the project"
        );
        assert_eq!(gens.get(&crate::sources::project("other")), 0);
    }
}
