//! The deploy recipe (PLAN 23, 11.1): a qualified, published release goes to an environment, in stages, with checks after
//! each stage, and comes back when a check fails.
//!
//! Only a release the release entity holds as published (`releases/entity.rs`: a qualified candidate is published, and
//! a yanked or unpublished one is never promoted) is deployed. An environment on the `releases` track deploys the newest
//! such release ([`published`]); an owner's or an approved deploy names one ([`start`]). An environment is a Cloudflare
//! one when its variables name a Worker (`CLOUDFLARE_WORKER`) or a Pages project (`CLOUDFLARE_PAGES`); the rest keep the
//! deploy hook of `environments.rs`.
//!
//! A rollout (`recipes/builtin/deploy.toml`, step `rollout`, one item per target) takes each target in turn: it uploads the
//! release's artifact (the `web` target by default) as a new version, serves it at each stage's percentage (Workers:
//! 10, 50, 100 by default), and after each stage runs the smoke checks and counts the environment's error signals over
//! the project's window. A failed check restores every target it changed to the version it replaced; Pages has no
//! percentages, so its one stage is the whole deploy. A protected environment deploys a release only after a staging
//! environment passed that same version, and its deploy waits for the owner in Approvals as any protected deploy does.
//! The deploy's status and the environment's version follow the outcome, and the project's chat is told.
//!
//! Environment variables (not secret): `CLOUDFLARE_WORKER`, `CLOUDFLARE_PAGES`, `ROLLOUT_STAGES` (`10,50,100`),
//! `DEPLOY_WINDOW_SECS` (300), `DEPLOY_MAX_ERRORS` (0), `DEPLOY_ARTIFACT` (`web`). The account ID and the API token are the
//! Cloudflare connector's (`connectors::hub_values`), read on the hub only.

pub mod cloudflare;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use cloudflare::{Api, Target};

use crate::chats::{Author, MessageKind, Post};
use crate::environments::{self, Deploy, Environment};
use crate::recipes::graph::Report;
use crate::recipes::runs::Runs;
use crate::releases::entity::{self, Release, ReleaseState};
use crate::releases::flow::Ledger;
use crate::state::App;
use crate::work_loop;
use std::collections::BTreeMap;

/// The recipe a rollout runs as, and its one step.
pub const RECIPE: &str = "deploy";
const TRIGGER: &str = "deploy";
const STEP: &str = "rollout";
/// The release artifact a Cloudflare target is built from, unless the environment names another.
const ARTIFACT: &str = "web";
/// The percentages a Workers target is served at, in order, ending at 100.
pub const DEFAULT_STAGES: [u32; 3] = [10, 50, 100];
const DEFAULT_WINDOW_SECS: u64 = 300;
/// The longest window a project may set: an hour.
const MAX_WINDOW_SECS: u64 = 3600;

/// An environment's rollout settings, read from its variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub targets: Vec<Target>,
    pub stages: Vec<u32>,
    pub window: Duration,
    pub max_errors: u32,
    pub artifact: String,
}

impl Settings {
    /// The settings of an environment, or None when it names no Cloudflare target (it keeps its deploy hook).
    pub fn of(env: &Environment) -> Result<Option<Settings>, String> {
        let var = |key: &str| {
            env.variables
                .get(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut targets = Vec::new();
        if let Some(name) = var("CLOUDFLARE_WORKER") {
            targets.push(Target::Workers(name));
        }
        if let Some(name) = var("CLOUDFLARE_PAGES") {
            targets.push(Target::Pages(name));
        }
        if targets.is_empty() {
            return Ok(None);
        }
        let stages = match var("ROLLOUT_STAGES") {
            Some(text) => parse_stages(&text)?,
            None => DEFAULT_STAGES.to_vec(),
        };
        let window = match var("DEPLOY_WINDOW_SECS") {
            Some(text) => text
                .parse::<u64>()
                .ok()
                .filter(|s| *s <= MAX_WINDOW_SECS)
                .ok_or_else(|| {
                    format!("DEPLOY_WINDOW_SECS is a number of seconds, at most {MAX_WINDOW_SECS}")
                })?,
            None => DEFAULT_WINDOW_SECS,
        };
        let max_errors = match var("DEPLOY_MAX_ERRORS") {
            Some(text) => text
                .parse::<u32>()
                .map_err(|_| "DEPLOY_MAX_ERRORS is a number".to_string())?,
            None => 0,
        };
        Ok(Some(Settings {
            targets,
            stages,
            window: Duration::from_secs(window),
            max_errors,
            artifact: var("DEPLOY_ARTIFACT").unwrap_or_else(|| ARTIFACT.to_string()),
        }))
    }
}

/// `10,50,100`: increasing percentages from 1 to 100, ending at 100.
fn parse_stages(text: &str) -> Result<Vec<u32>, String> {
    const HELP: &str =
        "ROLLOUT_STAGES is increasing percentages from 1 to 100 ending at 100, e.g. 10,50,100";
    let stages: Vec<u32> = text
        .split(',')
        .map(|s| s.trim().parse::<u32>().map_err(|_| HELP.to_string()))
        .collect::<Result<_, _>>()?;
    let increasing = stages.windows(2).all(|w| w[0] < w[1]);
    if stages.is_empty()
        || !increasing
        || stages.iter().any(|s| !(1..=100).contains(s))
        || stages.last() != Some(&100)
    {
        return Err(HELP.to_string());
    }
    Ok(stages)
}

/// Whether a release's state lets an environment take it: published (qualified first, 22.2), promoted, or superseded by a
/// newer one.
fn takes(state: ReleaseState) -> bool {
    matches!(
        state,
        ReleaseState::Published | ReleaseState::Promoted | ReleaseState::Superseded
    )
}

/// The release `version` of `project`, when it is qualified and published.
pub fn eligible(releases: &entity::Store, project: &str, version: &str) -> Result<Release, String> {
    releases
        .list(project)
        .into_iter()
        .find(|r| r.version == version && takes(r.state))
        .ok_or_else(|| format!("{version} is not a qualified, published release of this project"))
}

/// The newest release of `project` that is published (and not superseded): what a track deploys.
pub fn newest(app: &App, project: &str) -> Option<Release> {
    app.releases
        .list(project)
        .into_iter()
        .find(|r| matches!(r.state, ReleaseState::Published | ReleaseState::Promoted))
}

/// Whether a protected environment's deploy of `version` may go ahead: a staging environment passed that version.
fn staging_passed(envs: &[Environment], version: &str) -> bool {
    envs.iter().filter(|e| e.kind == "staging").any(|e| {
        e.deploys
            .iter()
            .any(|d| d.version == version && d.status == "passed")
    })
}

/// The gate a deploy of `version` to `env` passes before it is recorded or waits for approval: the release is
/// qualified and published, and a protected Cloudflare environment has a passing staging deploy of it. Returns the
/// environment's settings, or None when it is not a Cloudflare environment (no gate here).
pub fn check_gate(
    releases: &entity::Store,
    envs: &[Environment],
    project: &str,
    env: &Environment,
    version: &str,
) -> Result<Option<Settings>, String> {
    let Some(settings) = Settings::of(env)? else {
        return Ok(None);
    };
    eligible(releases, project, version)?;
    if env.protected && !staging_passed(envs, version) {
        return Err(format!(
            "{} is protected: it deploys {version} only after a staging environment passed that version",
            env.name
        ));
    }
    Ok(Some(settings))
}

/// [`check_gate`] over the hub's stores.
pub fn gate(
    app: &App,
    project: &str,
    env: &Environment,
    version: &str,
) -> Result<Option<Settings>, String> {
    check_gate(
        &app.releases,
        &app.environments.of(project),
        project,
        env,
        version,
    )
}

// ---- Rollouts -------------------------------------------------------------------------------------------------------

/// The ports a rollout uses: Cloudflare, the release's bytes, the checks, and the recipe run that records it.
pub struct Ports<'a> {
    pub api: &'a dyn Api,
    pub artifacts: &'a dyn Artifacts,
    pub checks: &'a dyn Checks,
    pub runs: &'a Runs,
}

/// One release artifact's bytes, by the release and the target it was built for.
pub trait Artifacts: Send + Sync {
    fn bytes(&self, release: &Release, target: &str) -> Result<Vec<u8>, String>;
}

/// The checks after each stage.
pub trait Checks: Send + Sync {
    /// The smoke scenarios: the environment's address answers, and the project's scenarios pass. Fails when either does
    /// not, or when the project has scenarios and no target can run them.
    fn smoke(&self, project: &str, env: &Environment) -> Result<(), String>;
    /// The monitoring signals of the environment since `since_ms`, once `window` has passed: how many are errors.
    fn errors(
        &self,
        project: &str,
        env: &Environment,
        since_ms: u64,
        window: Duration,
    ) -> Result<u32, String>;
}

/// What a rollout did: `passed`, `rolled_back`, or `failed` (a restore that could not run, so a target is left as it is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rolled {
    pub status: &'static str,
    pub detail: String,
}

/// Rolls `release` out to each target of `env` in stages. The recipe run's step gets one item per target: it passes when
/// every target passes its stages and checks, and fails at the first that does not (the walker's rule), after which every
/// target that changed is restored.
pub fn roll(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &Settings,
    release: &Release,
    run: &str,
) -> Rolled {
    let mut changed: Vec<(Target, Option<String>)> = Vec::new();
    let items = settings.targets.len();
    match rollout_all(ports, project, env, settings, release, &mut changed) {
        Ok(()) => {
            for _ in 0..items {
                ports.runs.settle(run, STEP, &Report::verdict(true), now());
            }
            Rolled {
                status: "passed",
                detail: format!(
                    "{} served at {} for {} target(s); smoke and signals passed at each stage",
                    release.version,
                    stages_text(&settings.stages),
                    items
                ),
            }
        }
        Err(why) => {
            ports.runs.settle(run, STEP, &Report::verdict(false), now());
            let mut notes = Vec::new();
            let mut restored = true;
            for (target, previous) in changed.iter().rev() {
                let outcome = match previous {
                    Some(to) => ports.api.restore(target, to),
                    None => Err("no earlier version to return to".to_string()),
                };
                if let Err(e) = outcome {
                    restored = false;
                    notes.push(format!("{}: {e}", target.label()));
                }
            }
            let detail = if notes.is_empty() {
                format!("{why}; restored the previous version")
            } else {
                format!("{why}; not restored: {}", notes.join("; "))
            };
            Rolled {
                status: if restored { "rolled_back" } else { "failed" },
                detail,
            }
        }
    }
}

fn stages_text(stages: &[u32]) -> String {
    stages
        .iter()
        .map(|s| format!("{s}%"))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Takes each target in turn. `changed` records a target once its new version is up, with the version it replaced.
fn rollout_all(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &Settings,
    release: &Release,
    changed: &mut Vec<(Target, Option<String>)>,
) -> Result<(), String> {
    let bytes = ports.artifacts.bytes(release, &settings.artifact)?;
    for target in &settings.targets {
        let label = target.label();
        let previous = ports
            .api
            .live(target)
            .map_err(|e| format!("{label}: {e}"))?;
        let version = ports
            .api
            .upload(target, &bytes)
            .map_err(|e| format!("{label}: {e}"))?;
        changed.push((target.clone(), previous.clone()));
        // Pages has no percentages, and a first deploy has nothing to split with: both go to 100% at once.
        let stages: Vec<u32> = match (target, &previous) {
            (Target::Workers(_), Some(_)) => settings.stages.clone(),
            _ => vec![100],
        };
        for pct in stages {
            let versions = match &previous {
                Some(prev) if pct < 100 => vec![(version.clone(), pct), (prev.clone(), 100 - pct)],
                _ => vec![(version.clone(), 100)],
            };
            ports
                .api
                .split(target, &versions)
                .map_err(|e| format!("{label} at {pct}%: {e}"))?;
            checks(ports, project, env, settings).map_err(|e| format!("{label} at {pct}%: {e}"))?;
        }
    }
    Ok(())
}

/// The checks after a stage: the smoke scenarios, then the environment's error signals over the window.
fn checks(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &Settings,
) -> Result<(), String> {
    let since = now();
    ports.checks.smoke(project, env)?;
    let errors = ports.checks.errors(project, env, since, settings.window)?;
    if errors > settings.max_errors {
        return Err(format!(
            "{errors} error signals in the {}s window (at most {})",
            settings.window.as_secs(),
            settings.max_errors
        ));
    }
    Ok(())
}

/// The chat's line about a rollout's end.
pub fn telling(env: &str, version: &str, rolled: &Rolled) -> String {
    match rolled.status {
        "passed" => format!("Deployed {version} to {env}: {}.", rolled.detail),
        "rolled_back" => format!(
            "Rolled {env} back after a failed check on {version}: {}.",
            rolled.detail
        ),
        _ => format!(
            "{version} failed on {env} and is not restored: {}. The owner needs to look.",
            rolled.detail
        ),
    }
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

// ---- Starting them -------------------------------------------------------------------------------------------------

/// Deploys `version` to a Cloudflare environment now: the gate passes, the deploy is recorded, and its rollout starts in
/// the background. The deploy comes back with status `recorded` (its outcome follows).
pub fn start(
    app: &Arc<App>,
    project: &str,
    env: &Environment,
    version: &str,
    by: &str,
    settings: Settings,
) -> Result<Deploy, String> {
    let release = eligible(&app.releases, project, version)?;
    let prior = env.version.clone();
    let deploy = environments::record(app, project, &env.name, version, by, None)?;
    spawn(app, project, env, settings, release, &deploy, prior);
    Ok(deploy)
}

/// A protected deploy the owner approved: its recorded deploy goes on with its rollout.
pub fn approved(
    app: &Arc<App>,
    project: &str,
    env: &Environment,
    settings: Settings,
    deploy: &Deploy,
    prior: Option<String>,
) {
    match eligible(&app.releases, project, &deploy.version) {
        Ok(release) => spawn(app, project, env, settings, release, deploy, prior),
        Err(why) => {
            environments::settle_deploy(app, project, &env.name, &deploy.id, "failed", &why, None)
        }
    }
}

/// After a release is published: each environment on the `releases` track that deploys to Cloudflare deploys the newest
/// published release (a protected one asks the owner first, as any protected deploy does).
pub fn published(app: &Arc<App>, project: &str) {
    let Some(release) = newest(app, project) else {
        return;
    };
    for env in app.environments.of(project) {
        if env.track != "releases" || env.version.as_deref() == Some(release.version.as_str()) {
            continue;
        }
        if !matches!(Settings::of(&env), Ok(Some(_))) {
            continue;
        }
        if let Err(why) = environments::request(
            app,
            project,
            &env.name,
            &release.version,
            0,
            "the plaza",
            "it tracks releases",
        ) {
            eprintln!(
                "apid: deploying {} to {project} {}: {why}",
                release.version, env.name
            );
        }
    }
}

/// Starts the rollout of `release` as deploy `deploy` in the background. The recipe run is started here, so its step
/// shows at once; the deploy's status and the environment's version follow the outcome, and the chat is told.
fn spawn(
    app: &Arc<App>,
    project: &str,
    env: &Environment,
    settings: Settings,
    release: Release,
    deploy: &Deploy,
    prior: Option<String>,
) {
    let client = cloudflare_client(app);
    let client = match client {
        Ok(client) => client,
        Err(why) => {
            environments::settle_deploy(app, project, &env.name, &deploy.id, "failed", &why, None);
            return;
        }
    };
    let items = BTreeMap::from([(STEP.to_string(), settings.targets.len().max(1) as u32)]);
    let subject = format!("{}@{}", env.name, deploy.version);
    let run = app
        .recipe_runs
        .start_engine_items(
            RECIPE,
            &items,
            TRIGGER,
            None,
            Some(project),
            Some(&subject),
            now(),
        )
        .unwrap_or_default();
    environments::settle_deploy(
        app,
        project,
        &env.name,
        &deploy.id,
        "rolling",
        "the rollout started",
        None,
    );
    let (app, project, env, deploy_id, version) = (
        Arc::clone(app),
        project.to_string(),
        env.clone(),
        deploy.id.clone(),
        deploy.version.clone(),
    );
    std::thread::spawn(move || {
        let artifacts = AppArtifacts {
            app: Arc::clone(&app),
        };
        let checks = AppChecks {
            app: Arc::clone(&app),
        };
        let ports = Ports {
            api: &client,
            artifacts: &artifacts,
            checks: &checks,
            runs: &app.recipe_runs,
        };
        let rolled = roll(&ports, &project, &env, &settings, &release, &run);
        let running = match rolled.status {
            "passed" => Some(Some(version.clone())),
            "rolled_back" => Some(prior),
            _ => None,
        };
        environments::settle_deploy(
            &app,
            &project,
            &env.name,
            &deploy_id,
            rolled.status,
            &rolled.detail,
            running,
        );
        tell(&app, &project, &telling(&env.name, &version, &rolled));
    });
}

/// The Cloudflare client for the environment's account, with the connector's token read on the hub.
fn cloudflare_client(app: &App) -> Result<cloudflare::Client, String> {
    let values = crate::connectors::hub_values(app, "cloudflare")?;
    let account = values.get("account_id").map(String::as_str).unwrap_or("");
    let token = values.get("api_token").map(String::as_str).unwrap_or("");
    cloudflare::Client::new(
        Arc::new(crate::platforms::http::Ureq),
        &cloudflare::Client::base_from_env(),
        account,
        token,
    )
}

/// Writes one notice to the project's chat.
fn tell(app: &App, project: &str, text: &str) {
    let now = now();
    let chat = work_loop::project_chat(app, project, now);
    let _ = app.chats.post(
        &chat,
        Post {
            author: Author::System,
            session: None,
            seat: None,
            kind: MessageKind::Notice,
            text: text.to_string(),
            card: None,
            attachments: Vec::new(),
        },
        now,
    );
}

// ---- The production ports -------------------------------------------------------------------------------------------

/// The release's artifact bytes, from Weft's blobs (the digest the release recorded).
struct AppArtifacts {
    app: Arc<App>,
}

impl Artifacts for AppArtifacts {
    fn bytes(&self, release: &Release, target: &str) -> Result<Vec<u8>, String> {
        let artifact = release
            .artifacts
            .iter()
            .find(|a| a.target == target)
            .ok_or_else(|| format!("{} has no {target} artifact to deploy", release.version))?;
        let hash = artifact
            .digest
            .parse::<weft::hash::Hash>()
            .map_err(|e| format!("the {target} artifact's digest is not a hash: {e}"))?;
        self.app.weft.blob(&hash)
    }
}

/// The smoke scenarios and the error signals, from the environment's address and the project's maintenance book.
struct AppChecks {
    app: Arc<App>,
}

impl Checks for AppChecks {
    fn smoke(&self, project: &str, env: &Environment) -> Result<(), String> {
        if let Some(url) = &env.url {
            let status = http_status(url)?;
            if !(200..300).contains(&status) {
                return Err(format!("{url} answered {status}"));
            }
        }
        if !self.app.reviews.confirmed_scenarios(project).is_empty() {
            return Err(
                "the project has smoke scenarios, and no web target runs them yet (PLAN 8.2)"
                    .into(),
            );
        }
        Ok(())
    }

    fn errors(
        &self,
        project: &str,
        env: &Environment,
        since_ms: u64,
        window: Duration,
    ) -> Result<u32, String> {
        std::thread::sleep(window);
        let book = self.app.maintenance.get(project);
        let count = book
            .signals
            .iter()
            .filter(|s| {
                s.kind == "error"
                    && s.environment.as_deref() == Some(env.name.as_str())
                    && s.at_ms >= since_ms
            })
            .count();
        Ok(count as u32)
    }
}

/// The environment's address answers with its status (a GET, the hub's own request).
fn http_status(url: &str) -> Result<u16, String> {
    let request = ureq::http::Request::builder()
        .method("GET")
        .uri(url)
        .header("User-Agent", "agentplaza")
        .body(Vec::new())
        .map_err(|e| format!("bad address {url}: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build();
    let response = ureq::Agent::new_with_config(config)
        .run(request)
        .map_err(|e| format!("{url} did not answer: {e}"))?;
    Ok(response.status().as_u16())
}
