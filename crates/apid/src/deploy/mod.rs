//! The deploy recipe (PLAN 23, 11.1, 11.4): a qualified, published release goes to an environment, in stages, with checks after
//! each stage, and comes back when a check fails or the environment's health breaches.
//!
//! Only a release the release entity holds as published (`releases/entity.rs`: a qualified candidate is published, and
//! a yanked or unpublished one is never promoted) is deployed. An environment on the `releases` track deploys the newest
//! such release ([`published`]); an owner's or an approved deploy names one ([`start`]). An environment deploys to the platforms its variables name: a Cloudflare
//! Worker (`CLOUDFLARE_WORKER`) or Pages project (`CLOUDFLARE_PAGES`), an AWS target (below), or a Kubernetes Deployment; the
//! rest keep the deploy hook of `environments.rs`.
//!
//! A rollout (`recipes/builtin/deploy.toml`, step `rollout`, one item per target) takes each target in turn: it uploads the
//! release's artifact (the `web` target by default) as a new version, serves it at each stage's percentage (Workers:
//! 10, 50, 100 by default), and after each stage runs the smoke set and reads the environment's health for the window
//! ([`health`]). The smoke set is the project's confirmed user-test scenarios, run through a web target in a browser of
//! the release's commit (8.2, `user_test::smoke`). A failed smoke check, or a health breach (error signals, error ratio or
//! p95 latency past the environment's thresholds), restores every target it changed to the version it replaced: the deploy
//! is recorded as rolled back, a rollback deploy records the earlier release, and a breach opens an incident. Pages has
//! no percentages, so its one stage is the whole deploy. A protected environment deploys a release only after a passing
//! staging stage, and its deploy waits for the owner in Approvals as any protected deploy does. The deploy's status and
//! the environment's version follow the outcome, and the project's chat is told at each step.
//!
//! AWS (11.3, [`aws`]) is a platform of the same path: an environment names a Lambda alias (`AWS_LAMBDA_FUNCTION`,
//! `AWS_LAMBDA_ALIAS`), an ECS service (`AWS_ECS_CLUSTER`, `AWS_ECS_SERVICE`, `AWS_ECS_CONTAINER`, `AWS_ECS_FAMILY`,
//! `ECS_MIN_HEALTHY_PERCENT`, `ECS_MAX_PERCENT`), a static site (`AWS_S3_BUCKET`, `AWS_S3_PREFIX`,
//! `AWS_CLOUDFRONT_DISTRIBUTION`) or a CloudFormation stack (`AWS_CLOUDFORMATION_STACK`), in `AWS_REGION` (else the
//! connector's region). The artifact of each is named by `DEPLOY_ARTIFACT`.
//!
//! A Kubernetes environment (`KUBERNETES_DEPLOYMENT` set, 11.2, [`kubernetes`]) names one Deployment, which is its only
//! target: its release's manifests (or Helm chart) are applied and its rollout watched by the kubernetes module's own
//! engine, with a canary (`KUBERNETES_CANARY`) sharing each stage's replicas with the stable Deployment. Its address, CA and
//! token are the `kubernetes` connector's. The stages and the health watch are read the same way for every platform
//! ([`Settings::watch_of`]).
//!
//! Environment variables (not secret): `CLOUDFLARE_WORKER`, `CLOUDFLARE_PAGES`, `ROLLOUT_STAGES` (`10,50,100`),
//! `DEPLOY_WINDOW_SECS` (300), `DEPLOY_MAX_ERRORS` (0), `DEPLOY_ARTIFACT` (`web`), `HEALTH_MAX_ERROR_RATE` (0.05),
//! `HEALTH_MAX_P95_MS` (2000), `HEALTH_ERROR_QUERY`, `HEALTH_LATENCY_QUERY` (the monitoring queries, see [`health`]). The
//! Cloudflare account ID and API token are the Cloudflare connector's (`connectors::hub_values`), read on the hub only; the
//! AWS base credentials and role are the `aws` connector's, the same way; the monitoring source is the `prometheus`
//! connector's URL and optional token.

pub mod aws;
pub mod cloudflare;
pub mod health;
pub mod kubernetes;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use health::{Health, Prometheus};

use crate::environments::{self, Deploy, Environment};
use crate::recipes::graph::Report;
use crate::recipes::runs::Runs;
use crate::releases::entity::{self, Release, ReleaseState};
use crate::releases::flow::Ledger;
use crate::state::App;
use crate::user_test::scenario::Scenario;
use crate::work_loop;

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
/// Who a rollback's deploy record names.
pub const ROLLBACK_BY: &str = "rollback";

/// What a rollout deploys to: a Cloudflare target, an AWS one, or a Kubernetes Deployment. The first two are rolled out target
/// by target through [`Api`]; a Kubernetes Deployment is rolled out whole by [`kubernetes`], which keeps its own state (the
/// manifests, the canary's replicas and the revisions it undoes to), so it is the environment's only target.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Cloudflare(cloudflare::Target),
    Aws(aws::Target),
    Kubernetes(kubernetes::Settings),
}

impl Target {
    /// The target as the logs and the chat name it.
    pub fn label(&self) -> String {
        match self {
            Target::Cloudflare(target) => target.label(),
            Target::Aws(target) => target.label(),
            Target::Kubernetes(settings) => {
                format!("Kubernetes {}/{}", settings.namespace, settings.deployment)
            }
        }
    }

    /// Whether the target can serve several versions at once, at percentages: Workers' percentage deployments and Lambda's
    /// weighted aliases. The rest serve one version, so their first stage is the whole deploy.
    fn staged(&self) -> bool {
        matches!(
            self,
            Target::Cloudflare(cloudflare::Target::Workers(_))
                | Target::Aws(aws::Target::Lambda { .. })
        )
    }
}

/// The calls a rollout makes to its platforms, by target: `live` reads the version that serves now, `upload` makes a
/// version from the release's artifact, `split` serves versions at percentages, `restore` serves an earlier one again.
pub trait Api: Send + Sync {
    fn live(&self, target: &Target) -> Result<Option<String>, String>;
    fn upload(&self, target: &Target, bytes: &[u8]) -> Result<String, String>;
    fn split(&self, target: &Target, versions: &[(String, u32)]) -> Result<(), String>;
    fn restore(&self, target: &Target, to: &str) -> Result<(), String>;
    /// The Kubernetes client and the hub's tools, which a Kubernetes Deployment's rollout runs with ([`kubernetes::roll`]).
    fn kubernetes(&self) -> Result<(&kubernetes::Client, &kubernetes::Tools), String> {
        Err("no Kubernetes connector serves this rollout".to_string())
    }
}

/// The platforms a rollout reaches, each present when an environment's targets need it. The tools are the hub's, for a
/// Kubernetes Deployment's Helm chart and kustomize.
pub struct Platforms {
    pub cloudflare: Option<cloudflare::Client>,
    pub aws: Option<aws::Client>,
    pub kubernetes: Option<kubernetes::Client>,
    pub tools: kubernetes::Tools,
}

impl Platforms {
    fn cloudflare(&self) -> Result<&cloudflare::Client, String> {
        self.cloudflare
            .as_ref()
            .ok_or_else(|| "no Cloudflare connector serves this rollout".to_string())
    }

    fn aws(&self) -> Result<&aws::Client, String> {
        self.aws
            .as_ref()
            .ok_or_else(|| "no AWS connector serves this rollout".to_string())
    }
}

impl Api for Platforms {
    fn live(&self, target: &Target) -> Result<Option<String>, String> {
        match target {
            Target::Cloudflare(t) => self.cloudflare()?.live(t),
            Target::Aws(t) => self.aws()?.live(t),
            Target::Kubernetes(_) => Err(KUBERNETES_UNIT.to_string()),
        }
    }

    fn upload(&self, target: &Target, bytes: &[u8]) -> Result<String, String> {
        match target {
            Target::Cloudflare(t) => self.cloudflare()?.upload(t, bytes),
            Target::Aws(t) => self.aws()?.upload(t, bytes),
            Target::Kubernetes(_) => Err(KUBERNETES_UNIT.to_string()),
        }
    }

    fn split(&self, target: &Target, versions: &[(String, u32)]) -> Result<(), String> {
        match target {
            Target::Cloudflare(t) => self.cloudflare()?.split(t, versions),
            Target::Aws(t) => self.aws()?.split(t, versions),
            Target::Kubernetes(_) => Err(KUBERNETES_UNIT.to_string()),
        }
    }

    fn restore(&self, target: &Target, to: &str) -> Result<(), String> {
        match target {
            Target::Cloudflare(t) => self.cloudflare()?.restore(t, to),
            Target::Aws(t) => self.aws()?.restore(t, to),
            Target::Kubernetes(_) => Err(KUBERNETES_UNIT.to_string()),
        }
    }

    fn kubernetes(&self) -> Result<(&kubernetes::Client, &kubernetes::Tools), String> {
        let client = self
            .kubernetes
            .as_ref()
            .ok_or_else(|| "no Kubernetes connector serves this rollout".to_string())?;
        Ok((client, &self.tools))
    }
}

/// The staged calls refuse a Kubernetes target: its Deployment is rolled out whole, by [`kubernetes::roll`].
const KUBERNETES_UNIT: &str = "a Kubernetes Deployment is rolled out whole, by its own engine";

/// An environment's rollout settings, read from its variables.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub targets: Vec<Target>,
    /// The AWS region the environment deploys in (`AWS_REGION`), when it names one; else the connector's.
    pub aws_region: Option<String>,
    pub stages: Vec<u32>,
    pub window: Duration,
    pub max_errors: u32,
    pub artifact: String,
    /// The error ratio a stage may show, from 0 to 1.
    pub max_error_rate: f64,
    /// The p95 latency a stage may show, in milliseconds.
    pub max_p95_ms: f64,
    pub error_query: String,
    pub latency_query: String,
}

impl Settings {
    /// The settings of an environment, or None when it names no platform target (it keeps its deploy hook).
    pub fn of(env: &Environment) -> Result<Option<Settings>, String> {
        let var = |key: &str| variable(env, key);
        let mut targets = Vec::new();
        if let Some(name) = var("CLOUDFLARE_WORKER") {
            targets.push(Target::Cloudflare(cloudflare::Target::Workers(name)));
        }
        if let Some(name) = var("CLOUDFLARE_PAGES") {
            targets.push(Target::Cloudflare(cloudflare::Target::Pages(name)));
        }
        if let Some(function) = var("AWS_LAMBDA_FUNCTION") {
            targets.push(Target::Aws(aws::Target::Lambda {
                function,
                alias: var("AWS_LAMBDA_ALIAS").unwrap_or_else(|| "live".to_string()),
            }));
        }
        if var("AWS_ECS_CLUSTER").is_some() || var("AWS_ECS_SERVICE").is_some() {
            let (Some(cluster), Some(service), Some(container)) = (
                var("AWS_ECS_CLUSTER"),
                var("AWS_ECS_SERVICE"),
                var("AWS_ECS_CONTAINER"),
            ) else {
                return Err(
                    "an ECS rollout names AWS_ECS_CLUSTER, AWS_ECS_SERVICE and AWS_ECS_CONTAINER"
                        .into(),
                );
            };
            targets.push(Target::Aws(aws::Target::Ecs {
                family: var("AWS_ECS_FAMILY").unwrap_or_else(|| service.clone()),
                cluster,
                service,
                container,
                min_healthy: var_u32(&var, "ECS_MIN_HEALTHY_PERCENT", 100)?,
                max_percent: var_u32(&var, "ECS_MAX_PERCENT", 200)?,
            }));
        }
        if let Some(bucket) = var("AWS_S3_BUCKET") {
            let prefix = var("AWS_S3_PREFIX").unwrap_or_default();
            let prefix = match prefix.trim_matches('/') {
                "" => String::new(),
                trimmed => format!("{trimmed}/"),
            };
            targets.push(Target::Aws(aws::Target::Site {
                bucket,
                prefix,
                distribution: var("AWS_CLOUDFRONT_DISTRIBUTION"),
            }));
        }
        if let Some(name) = var("AWS_CLOUDFORMATION_STACK") {
            targets.push(Target::Aws(aws::Target::Stack { name }));
        }
        if let Some(settings) = kubernetes::Settings::of(env)? {
            targets.push(Target::Kubernetes(settings));
        }
        if targets.is_empty() {
            return Ok(None);
        }
        if targets.len() > 1 && targets.iter().any(|t| matches!(t, Target::Kubernetes(_))) {
            return Err(format!(
                "{} names a Kubernetes Deployment and other targets: the Deployment is the environment's only target",
                env.name
            ));
        }
        Ok(Some(Settings {
            targets,
            ..Settings::watch_of(env)?
        }))
    }

    /// The stages and the health watch an environment's variables set (`ROLLOUT_STAGES`, `DEPLOY_WINDOW_SECS`, the
    /// thresholds, the queries) and its AWS region. Every platform reads them; the targets are the platform's own.
    pub fn watch_of(env: &Environment) -> Result<Settings, String> {
        let var = |key: &str| variable(env, key);
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
        let max_error_rate = match var("HEALTH_MAX_ERROR_RATE") {
            Some(text) => text
                .parse::<f64>()
                .ok()
                .filter(|r| (0.0..=1.0).contains(r))
                .ok_or("HEALTH_MAX_ERROR_RATE is a ratio from 0 to 1")?,
            None => health::DEFAULT_MAX_ERROR_RATE,
        };
        let max_p95_ms = match var("HEALTH_MAX_P95_MS") {
            Some(text) => text
                .parse::<f64>()
                .ok()
                .filter(|m| m.is_finite() && *m >= 0.0)
                .ok_or("HEALTH_MAX_P95_MS is a number of milliseconds")?,
            None => health::DEFAULT_MAX_P95_MS,
        };
        Ok(Settings {
            targets: Vec::new(),
            aws_region: var("AWS_REGION"),
            stages,
            window: Duration::from_secs(window),
            max_errors,
            artifact: var("DEPLOY_ARTIFACT").unwrap_or_else(|| ARTIFACT.to_string()),
            max_error_rate,
            max_p95_ms,
            error_query: var("HEALTH_ERROR_QUERY")
                .unwrap_or_else(|| health::ERROR_QUERY.to_string()),
            latency_query: var("HEALTH_LATENCY_QUERY")
                .unwrap_or_else(|| health::LATENCY_QUERY.to_string()),
        })
    }

    /// The Kubernetes Deployment the environment rolls out to, when it names one (it is then its only target).
    pub fn kubernetes(&self) -> Option<&kubernetes::Settings> {
        self.targets.iter().find_map(|target| match target {
            Target::Kubernetes(settings) => Some(settings),
            _ => None,
        })
    }
}

/// An environment variable's value, trimmed, when it is set and not empty.
fn variable(env: &Environment, key: &str) -> Option<String> {
    env.variables
        .get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A whole-number variable, or its default when the environment does not name it.
fn var_u32(var: &impl Fn(&str) -> Option<String>, key: &str, default: u32) -> Result<u32, String> {
    match var(key) {
        Some(text) => text
            .parse::<u32>()
            .map_err(|_| format!("{key} is a whole number")),
        None => Ok(default),
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
/// qualified and published, and a protected environment has a passing staging deploy of it, whichever platform it deploys
/// to. Returns the environment's settings, or None when it names no platform (no gate here).
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

/// The ports a rollout uses: the platforms, the release's bytes, the checks, the incidents, and the recipe run that records it.
pub struct Ports<'a> {
    pub api: &'a dyn Api,
    pub artifacts: &'a dyn Artifacts,
    pub checks: &'a dyn Checks,
    pub incidents: &'a dyn Incidents,
    pub runs: &'a Runs,
}

/// One release artifact's bytes, by the release and the target it was built for.
pub trait Artifacts: Send + Sync {
    fn bytes(&self, release: &Release, target: &str) -> Result<Vec<u8>, String>;
}

/// The checks after each stage.
pub trait Checks: Send + Sync {
    /// The smoke set (8.2): the project's scenarios run through a web target at the environment's address, in a browser of
    /// the release's commit. Fails at the first scenario that does not hold.
    fn smoke(&self, project: &str, env: &Environment, release: &Release) -> Result<(), String>;
    /// The environment's health once the window has passed: the error signals since `since_ms`, and the monitoring
    /// source's error ratio and p95 latency when it is connected. The thresholds are in `settings`.
    fn health(
        &self,
        project: &str,
        env: &Environment,
        since_ms: u64,
        settings: &Settings,
    ) -> Result<Health, String>;
}

/// The incidents a health breach opens (12.1 fills the incident recipe; this records the incident).
pub trait Incidents: Send + Sync {
    fn open(&self, project: &str, env: &str, version: &str, detail: &str);
}

/// What a rollout did: `passed`, `rolled_back`, or `failed` (a restore that could not run, so a target is left as it is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rolled {
    pub status: &'static str,
    pub detail: String,
}

/// Why a stage stopped: a check failed (rolled back), or the health breached (rolled back, and an incident is opened).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    Check(String),
    Breach(String),
}

impl Stop {
    fn why(&self) -> &str {
        match self {
            Stop::Check(why) | Stop::Breach(why) => why,
        }
    }

    /// The same stop, named by the place it happened (a target and its stage).
    fn at(self, place: &str) -> Stop {
        match self {
            Stop::Check(why) => Stop::Check(format!("{place}: {why}")),
            Stop::Breach(why) => Stop::Breach(format!("{place}: {why}")),
        }
    }
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
    if let Some(kube) = settings.kubernetes() {
        return roll_kubernetes(ports, project, env, kube, release, run);
    }
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
                    "{} served at {} for {} target(s); smoke and health passed at each stage",
                    release.version,
                    stages_text(&settings.stages),
                    items
                ),
            }
        }
        Err(stop) => {
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
            let why = stop.why().to_string();
            if matches!(stop, Stop::Breach(_)) {
                ports
                    .incidents
                    .open(project, &env.name, &release.version, &why);
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

/// A Kubernetes Deployment's rollout: the kubernetes module's engine runs it with the same recipe run, checks and incidents.
/// The engine's client and tools come from the platforms; without them the step fails before anything is touched.
fn roll_kubernetes(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &kubernetes::Settings,
    release: &Release,
    run: &str,
) -> Rolled {
    let (client, tools) = match ports.api.kubernetes() {
        Ok(pair) => pair,
        Err(why) => {
            ports.runs.settle(run, STEP, &Report::verdict(false), now());
            return Rolled {
                status: "failed",
                detail: why,
            };
        }
    };
    let kube = kubernetes::Ports {
        client,
        tools,
        artifacts: ports.artifacts,
        checks: ports.checks,
        incidents: ports.incidents,
        runs: ports.runs,
    };
    kubernetes::roll(&kube, project, env, settings, release, run)
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
) -> Result<(), Stop> {
    let bytes = ports
        .artifacts
        .bytes(release, &settings.artifact)
        .map_err(Stop::Check)?;
    for target in &settings.targets {
        let label = target.label();
        let previous = ports
            .api
            .live(target)
            .map_err(|e| Stop::Check(format!("{label}: {e}")))?;
        let version = ports
            .api
            .upload(target, &bytes)
            .map_err(|e| Stop::Check(format!("{label}: {e}")))?;
        changed.push((target.clone(), previous.clone()));
        // Pages, a site, a service and a stack have no percentages, and a first deploy has nothing to split with: both go
        // to 100% at once.
        let stages: Vec<u32> = match &previous {
            Some(_) if target.staged() => settings.stages.clone(),
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
                .map_err(|e| Stop::Check(format!("{label} at {pct}%: {e}")))?;
            checks(ports.checks, project, env, settings, release)
                .map_err(|stop| stop.at(&format!("{label} at {pct}%")))?;
        }
    }
    Ok(())
}

/// The checks after a stage: the smoke set, then the environment's health over the window.
fn checks(
    checks: &dyn Checks,
    project: &str,
    env: &Environment,
    settings: &Settings,
    release: &Release,
) -> Result<(), Stop> {
    let since = now();
    checks
        .smoke(project, env, release)
        .map_err(|e| Stop::Check(format!("smoke: {e}")))?;
    let health = checks
        .health(project, env, since, settings)
        .map_err(Stop::Check)?;
    match breach(&health, settings) {
        Some(why) => Err(Stop::Breach(why)),
        None => Ok(()),
    }
}

/// The breach a stage's health shows, in a line with its numbers, or none.
pub fn breach(health: &Health, settings: &Settings) -> Option<String> {
    let window = settings.window.as_secs();
    if health.signals > settings.max_errors {
        return Some(format!(
            "{} error signals in the {window}s window (at most {})",
            health.signals, settings.max_errors
        ));
    }
    let mut over = Vec::new();
    if let Some(rate) = health.error_rate.filter(|r| *r > settings.max_error_rate) {
        over.push(format!(
            "error rate {:.1}% (at most {:.1}%)",
            rate * 100.0,
            settings.max_error_rate * 100.0
        ));
    }
    if let Some(p95) = health.p95_ms.filter(|p| *p > settings.max_p95_ms) {
        over.push(format!(
            "p95 latency {p95:.0} ms (at most {:.0} ms)",
            settings.max_p95_ms
        ));
    }
    (!over.is_empty()).then(|| format!("{} in the {window}s window", over.join(", ")))
}

/// The chat's line when a rollout starts.
pub fn starting(env: &str, version: &str, settings: &Settings) -> String {
    format!(
        "Deploying {version} to {env}: rolling out at {}, with the smoke set and a health check after each stage.",
        stages_text(&settings.stages)
    )
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

/// Deploys `version` to an environment now: the gate passes, the deploy is recorded, and its rollout starts in
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

/// After a release is published: each environment on the `releases` track that deploys to a platform deploys the newest
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
    let client = match platforms(app, &settings) {
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
    tell(
        app,
        project,
        &starting(&env.name, &deploy.version, &settings),
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
        let incidents = AppIncidents {
            app: Arc::clone(&app),
        };
        let ports = Ports {
            api: &client,
            artifacts: &artifacts,
            checks: &checks,
            incidents: &incidents,
            runs: &app.recipe_runs,
        };
        let rolled = roll(&ports, &project, &env, &settings, &release, &run);
        let running = match rolled.status {
            "passed" => Some(Some(version.clone())),
            "rolled_back" => Some(prior.clone()),
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
        if rolled.status == "rolled_back" {
            record_rollback(&app, &project, &env.name, prior.as_deref(), &version);
        }
    });
}

/// A rollback is a deploy of its own (23): the environment records the earlier release it went back to, by the health
/// watch, so its deploys show the rollback as one.
fn record_rollback(app: &App, project: &str, env: &str, to: Option<&str>, from: &str) {
    let Some(to) = to else {
        return;
    };
    match environments::record(app, project, env, to, ROLLBACK_BY, None) {
        Ok(deploy) => environments::settle_deploy(
            app,
            project,
            env,
            &deploy.id,
            "rollback",
            &format!("back on {to}; {from} was rolled back"),
            None,
        ),
        Err(why) => eprintln!("apid: recording the rollback of {project} {env}: {why}"),
    }
}

/// The clients the environment's targets need: Cloudflare's when a Worker or Pages target is named, AWS's when an AWS one is,
/// Kubernetes's when a Deployment is. Each is built from its connector on the hub, so a missing connector refuses the deploy
/// before anything is called.
fn platforms(app: &App, settings: &Settings) -> Result<Platforms, String> {
    let cloudflare_targets = settings
        .targets
        .iter()
        .any(|t| matches!(t, Target::Cloudflare(_)));
    let aws_targets = settings.targets.iter().any(|t| matches!(t, Target::Aws(_)));
    Ok(Platforms {
        cloudflare: cloudflare_targets
            .then(|| cloudflare_client(app))
            .transpose()?,
        aws: aws_targets
            .then(|| aws_client(app, settings.aws_region.as_deref()))
            .transpose()?,
        kubernetes: settings
            .kubernetes()
            .is_some()
            .then(|| kubernetes_client(app))
            .transpose()?,
        tools: kubernetes::Tools::from_env(),
    })
}

/// The Kubernetes client for the environment's cluster: its address, CA and token from the connector, read on the hub.
fn kubernetes_client(app: &App) -> Result<kubernetes::Client, String> {
    let values = crate::connectors::hub_values(app, "kubernetes")?;
    let url = values.get("url").map(String::as_str).unwrap_or("");
    let token = values.get("token").map(String::as_str).unwrap_or("");
    let http: Arc<dyn crate::platforms::http::Http> =
        match values.get("ca").map(|c| c.trim()).filter(|c| !c.is_empty()) {
            Some(pem) => Arc::new(crate::platforms::http::PrivateCa(pem.to_string())),
            None => Arc::new(crate::platforms::http::Ureq),
        };
    Ok(kubernetes::Client::new(http, url, token)?.with_ca(
        values
            .get("ca")
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty()),
    ))
}

/// The AWS client: the `aws` connector's role and base credentials (or web identity token), read on the hub. The region is
/// the environment's `AWS_REGION`, else the connector's, else us-east-1.
fn aws_client(app: &App, region: Option<&str>) -> Result<aws::Client, String> {
    let values = crate::connectors::hub_values(app, "aws")?;
    let value = |key: &str| {
        values
            .get(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let region = region
        .map(str::to_string)
        .or_else(|| value("region"))
        .unwrap_or_else(|| aws::DEFAULT_REGION.to_string());
    let base = match (
        value("web_identity_token"),
        value("access_key_id"),
        value("secret_access_key"),
    ) {
        (Some(token), _, _) => aws::Base::WebIdentity { token },
        (None, Some(access_key), Some(secret_key)) => aws::Base::Keys {
            access_key,
            secret_key,
        },
        _ => {
            return Err(
                "the AWS connector holds neither a web identity token nor an access key and secret"
                    .into(),
            );
        }
    };
    let role = value("role_arn").ok_or("the AWS connector has no role ARN")?;
    aws::Client::new(
        Arc::new(crate::platforms::http::Ureq),
        aws::Endpoints::from_env(&region),
        &region,
        &role,
        base,
        aws::system_clock(),
        aws::Wait {
            interval: Duration::from_secs(15),
            attempts: 80,
        },
    )
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

/// Writes one notice to the project's chat (the loop's notice path).
fn tell(app: &App, project: &str, text: &str) {
    let now = now();
    let chat = work_loop::project_chat(app, project, now);
    work_loop::notice(app, &chat, text, now);
}

/// The monitoring source: the `prometheus` connector's URL and token, when the connector is enabled and has a URL. None
/// when it is not connected (the watch then counts the environment's error signals only).
fn monitor(app: &App) -> Result<Option<Prometheus>, String> {
    let Ok(values) = crate::connectors::hub_values(app, "prometheus") else {
        return Ok(None);
    };
    let Some(url) = values
        .get("url")
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    Prometheus::new(
        Arc::new(crate::platforms::http::Ureq),
        url,
        values.get("token").map(String::as_str),
    )
    .map(Some)
}

/// The project's smoke set (8.2): its confirmed user-test scenarios, each once.
fn smoke_set(app: &App, project: &str) -> Vec<Scenario> {
    let mut set: Vec<Scenario> = Vec::new();
    for json in app.reviews.confirmed_scenarios(project) {
        if let Ok(scenario) = serde_json::from_str::<Scenario>(&json)
            && scenario.check().is_ok()
            && !set.iter().any(|s| s.id == scenario.id)
        {
            set.push(scenario);
        }
    }
    set
}

// ---- The production ports -------------------------------------------------------------------------------------------

/// The release's artifact bytes, from Weft's blobs (the digest the release recorded). An environment's target is the
/// artifact of that name; a release with one artifact deploys that one.
struct AppArtifacts {
    app: Arc<App>,
}

impl Artifacts for AppArtifacts {
    fn bytes(&self, release: &Release, target: &str) -> Result<Vec<u8>, String> {
        let artifact = release
            .artifacts
            .iter()
            .find(|a| a.target == target)
            .or_else(|| (release.artifacts.len() == 1).then(|| &release.artifacts[0]))
            .ok_or_else(|| format!("{} has no {target} artifact to deploy", release.version))?;
        let hash = artifact
            .digest
            .parse::<weft::hash::Hash>()
            .map_err(|e| format!("the {target} artifact's digest is not a hash: {e}"))?;
        self.app.weft.blob(&hash)
    }
}

/// The smoke set and the health, from the project's user tests, the environment's maintenance signals and its monitoring.
struct AppChecks {
    app: Arc<App>,
}

impl Checks for AppChecks {
    fn smoke(&self, project: &str, env: &Environment, release: &Release) -> Result<(), String> {
        let scenarios = smoke_set(&self.app, project);
        if scenarios.is_empty() {
            return Ok(());
        }
        let url = env
            .url
            .as_deref()
            .ok_or_else(|| format!("{} has no address its smoke scenarios can open", env.name))?;
        crate::user_test::smoke(&self.app, project, url, &release.from, &scenarios)
    }

    fn health(
        &self,
        project: &str,
        env: &Environment,
        since_ms: u64,
        settings: &Settings,
    ) -> Result<Health, String> {
        std::thread::sleep(settings.window);
        let book = self.app.maintenance.get(project);
        let signals = book
            .signals
            .iter()
            .filter(|s| {
                s.kind == "error"
                    && s.environment.as_deref() == Some(env.name.as_str())
                    && s.at_ms >= since_ms
            })
            .count() as u32;
        let mut health = Health {
            signals,
            ..Health::default()
        };
        if let Some(monitor) = monitor(&self.app)? {
            let reading = health::read(
                &monitor,
                &env.name,
                settings.window,
                &settings.error_query,
                &settings.latency_query,
            )?;
            health.error_rate = reading.error_rate;
            health.p95_ms = reading.p95_ms;
        }
        Ok(health)
    }
}

/// Records an incident for a breach (12.1): the incident recipe runs on it, and its chat card carries the breach's numbers.
struct AppIncidents {
    app: Arc<App>,
}

impl Incidents for AppIncidents {
    fn open(&self, project: &str, env: &str, version: &str, detail: &str) {
        crate::incidents::breach(&self.app, project, env, version, detail);
    }
}
