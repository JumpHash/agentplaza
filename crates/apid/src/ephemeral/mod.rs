//! Ephemeral test environments (the work engine's deploys, extended): a lane's change or a release candidate is deployed
//! into a short-lived environment on a real target, tests run against its address, and it is torn down when they are done
//! or when its time is up.
//!
//! An environment is ephemeral when its variables set `EPHEMERAL=true`. Its target is the platform its deploy variables name
//! (the same ones a deploy reads, [`crate::deploy::Settings::of`]): a Kubernetes Deployment (`KUBERNETES_DEPLOYMENT`), a
//! Cloudflare Pages project (`CLOUDFLARE_PAGES`), a Worker (`CLOUDFLARE_WORKER`) or a CloudFormation stack
//! (`AWS_CLOUDFORMATION_STACK`). Each instance gets its own copy, named from a template (`platform`):
//!
//! - `EPHEMERAL_NAMESPACE` (Kubernetes, `apz-{project}-{subject}`): the instance's namespace. The release's (or the lane's)
//!   manifests are applied into it, with the environment's variables as a ConfigMap and its vault secrets as a Secret.
//! - `EPHEMERAL_BRANCH` (Pages, `apz-{subject}-{instance}`): the branch the instance's preview deployment is made on.
//! - `EPHEMERAL_WORKER` (Workers, `apz-{project}-{subject}`): the instance's own Worker script.
//! - `EPHEMERAL_STACK` (CloudFormation, `apz-{project}-{subject}`): the instance's own stack.
//! - `EPHEMERAL_URL`: the address pattern (`{name}`, `{project}`, `{environment}`, `{subject}`, `{instance}`). Without it the
//!   address is the one the platform gives: the Ingress host the manifests declare, the Pages preview URL, the stack's `Url`
//!   output.
//! - `EPHEMERAL_TTL_MINUTES` (60): how long an instance lives before the ticker tears it down (a request may ask for another).
//!
//! The subject's artifact is the release's own, or, for a lane, the lane's build through the builder releases use
//! ([`crate::releases::builder`]). Credentials are the connectors', read on the hub only: nothing a platform sees carries a
//! token, and an instance's record holds no secret values.
//!
//! Routes (owner, under `/v1`):
//! - `POST /projects/{id}/ephemeral {environment, subject: {lane} | {release}, ttl_minutes?}`: starts an instance (202, its
//!   state `creating`; the rollout runs in the background and its end is in `GET`)
//! - `GET /projects/{id}/ephemeral`: the project's instances, newest first
//! - `GET /ephemeral/{instance}`, `DELETE /ephemeral/{instance}` (tears it down)
//!
//! The CI recipe's `against` step (`against`) runs a change's tests against an instance of the project's first ephemeral
//! environment. Instances are kept in `plaza.db` (`store`).

pub mod against;
pub mod platform;
pub mod store;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use apz_http::{Params, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::deploy::{self, kubernetes};
use crate::environments::Environment;
use crate::releases::entity::Release;
use crate::state::App;

pub use store::Store;

/// The minutes an instance lives when neither the environment nor the request says.
pub const DEFAULT_TTL_MINUTES: u32 = 60;
/// The longest an instance may live: a week.
pub const MAX_TTL_MINUTES: u32 = 7 * 24 * 60;
/// How often the ticker looks for instances whose time is up.
const TICK: Duration = Duration::from_secs(60);
const MINUTE_MS: u64 = 60_000;
/// The variable that makes an environment ephemeral, and the ones that set its templates and address.
pub const EPHEMERAL: &str = "EPHEMERAL";
const TTL_VAR: &str = "EPHEMERAL_TTL_MINUTES";
const URL_VAR: &str = "EPHEMERAL_URL";
/// How many times the ticker tries to remove an expired instance whose platform is unreachable, and the longest it waits
/// between two tries (the first retry comes one tick after the failure, then the wait doubles up to this cap).
pub const MAX_TEARDOWN_ATTEMPTS: u32 = 12;
pub const TEARDOWN_WAIT_CAP_MS: u64 = 60 * MINUTE_MS;

/// What an instance is an instance of: a lane's change (its build), or a release candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Lane(String),
    Release(String),
}

/// What an instance's teardown removes, and what its platform calls name. A Pages deployment's id is known once it is
/// uploaded (an empty id until then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum Target {
    Kubernetes {
        namespace: String,
        deployment: String,
    },
    CloudflarePages {
        project: String,
        deployment: String,
        branch: String,
    },
    CloudflareWorker {
        script: String,
    },
    AwsStack {
        stack: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
}

/// An instance's state: `creating` while its rollout runs, `ready` once its checks have what they need, `failed` with its
/// reason (its resources were removed, or the reason says what was left), `torn_down` once its target is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum State {
    Creating,
    Ready,
    Failed(String),
    TornDown,
}

/// One ephemeral instance, as the API and the callers read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// `eph-<hex>`.
    pub id: String,
    pub project: String,
    pub environment: String,
    pub subject: Subject,
    pub target: Target,
    /// The address the instance serves at, when its platform gives one.
    pub url: Option<String>,
    pub state: State,
    pub created_ms: u64,
    /// When the ticker tears it down.
    pub expires_ms: u64,
    /// The workflow run that brought the instance up: its teardown is the run's (a run's end takes down what it did not
    /// keep). None for an instance a caller started by hand or through the CI recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Whether the run keeps the instance past its end.
    #[serde(default, skip_serializing_if = "is_false")]
    pub keep: bool,
    /// The build's blobs the instance deploys, by artifact name (a run's build). Empty: the instance builds its own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub built: BTreeMap<String, String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// What a caller asks for: an environment of a project, the subject to put in it, and how long it may live. A run that
/// brings an instance up names itself (`run`) and whether it keeps the instance; `built` names a run's build artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpRequest {
    pub project: String,
    pub environment: String,
    pub subject: Subject,
    pub ttl_minutes: Option<u32>,
    pub run: Option<String>,
    pub keep: bool,
    pub built: BTreeMap<String, String>,
}

/// The hub's ports of an instance: the clock, the environments and releases of the project, the subject's artifacts,
/// the platform clients (their connectors' credentials, read on the hub), and the environment's vault secrets. Production
/// is [`platform::Production`]; the tests give fakes.
pub trait Ports: Send + Sync {
    fn now(&self) -> u64;
    /// The environment as the owner set it.
    fn environment(&self, project: &str, env: &str) -> Result<Environment, String>;
    /// The release a release subject names; None for a lane.
    fn release(&self, project: &str, subject: &Subject) -> Result<Option<Release>, String>;
    /// The bytes of the subject's artifact `artifact`: the lane's build, or the release's own artifact.
    fn artifact(&self, project: &str, subject: &Subject, artifact: &str)
    -> Result<Vec<u8>, String>;
    /// The bytes of a blob by its digest (a build a workflow run made and recorded in Weft).
    fn blob(&self, digest: &str) -> Result<Vec<u8>, String>;
    /// The Kubernetes client of the environment's cluster (the `kubernetes` connector).
    fn kubernetes(&self) -> Result<kubernetes::Client, String>;
    /// The hub's tools for a Kubernetes overlay (kustomize, helm).
    fn tools(&self) -> kubernetes::Tools;
    /// The Cloudflare client (the `cloudflare` connector).
    fn cloudflare(&self) -> Result<crate::deploy::cloudflare::Client, String>;
    /// The AWS client in `region` (the `aws` connector's role).
    fn aws(&self, region: Option<&str>) -> Result<crate::deploy::aws::Client, String>;
    /// The environment's secrets as name and value, read on the hub. They go to the instance only (a Secret, a stack's
    /// parameters are not used), never to a record.
    fn secrets(&self, project: &str, env: &str) -> Vec<(String, String)>;
}

/// Whether an environment is ephemeral: its `EPHEMERAL` variable is `true`.
pub fn is_ephemeral(env: &Environment) -> bool {
    deploy::variable(env, EPHEMERAL).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// The minutes an instance of `env` lives: the request's, else the environment's `EPHEMERAL_TTL_MINUTES`, else the default.
pub fn ttl_minutes(env: &Environment, asked: Option<u32>) -> Result<u32, String> {
    let ttl = match (asked, deploy::variable(env, TTL_VAR)) {
        (Some(minutes), _) => minutes,
        (None, Some(text)) => text
            .parse::<u32>()
            .map_err(|_| format!("{TTL_VAR} is a number of minutes"))?,
        (None, None) => DEFAULT_TTL_MINUTES,
    };
    if (1..=MAX_TTL_MINUTES).contains(&ttl) {
        Ok(ttl)
    } else {
        Err(format!(
            "an instance lives from 1 minute to {MAX_TTL_MINUTES} minutes"
        ))
    }
}

/// A new instance's id: `eph-` and 16 random hex characters.
fn new_id() -> String {
    let mut bytes = [0u8; 8];
    apz_os::random::fill(&mut bytes).expect("ephemeral ids need OS randomness");
    format!(
        "eph-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

/// Makes an instance (its row, in Creating) for `req`: the environment must be ephemeral, the subject a name the
/// platforms can take, and the target its platform's names. Nothing is deployed yet.
pub fn reserve(store: &Store, ports: &dyn Ports, req: &UpRequest) -> Result<Instance, String> {
    if !valid_id(&req.project) || !valid_id(&req.environment) {
        return Err("a project and an environment are named by their slugs".into());
    }
    let subject_id = match &req.subject {
        Subject::Lane(id) | Subject::Release(id) => id,
    };
    if !valid_id(subject_id) {
        return Err("a subject is a change's or a release's id".into());
    }
    let env = ports.environment(&req.project, &req.environment)?;
    if !is_ephemeral(&env) {
        return Err(format!(
            "{} is not an ephemeral environment: its {EPHEMERAL} variable is not true",
            env.name
        ));
    }
    let ttl = ttl_minutes(&env, req.ttl_minutes)?;
    let id = new_id();
    let target = platform::plan(&env, &req.project, &req.subject, &id)?;
    let now = ports.now();
    let instance = Instance {
        id,
        project: req.project.clone(),
        environment: req.environment.clone(),
        subject: req.subject.clone(),
        target,
        url: None,
        state: State::Creating,
        created_ms: now,
        expires_ms: now + u64::from(ttl) * MINUTE_MS,
        run: req.run.clone(),
        keep: req.keep,
        built: req.built.clone(),
    };
    store.insert(&instance)?;
    Ok(instance)
}

/// A slug an id or a name may be: letters, digits, dashes, dots and underscores (what Weft and the stores name things with).
fn valid_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 80
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Deploys a reserved instance and waits for its checks' inputs (the rollout's readiness). Ready on success, with its
/// address. A failed deploy removes what it made, and the instance is Failed with the reason; the error says it.
pub fn execute(store: &Store, ports: &dyn Ports, instance: &Instance) -> Result<Instance, String> {
    let mut target = instance.target.clone();
    let outcome = match ports.environment(&instance.project, &instance.environment) {
        Ok(env) => platform::deploy(
            ports,
            &env,
            &instance.project,
            &instance.subject,
            &instance.id,
            &instance.built,
            &mut target,
        ),
        Err(why) => Err(why),
    };
    if target != instance.target {
        store.set_target(&instance.id, &target)?;
    }
    match outcome {
        Ok(url) => {
            store.set_url(&instance.id, url.clone());
            store.set_state(&instance.id, &State::Ready, Some("creating"))?;
            Ok(Instance {
                url,
                state: State::Ready,
                target,
                ..instance.clone()
            })
        }
        Err(why) => {
            let reason = match platform::teardown(ports, &target) {
                Ok(()) => why,
                Err(left) => format!("{why}; not removed: {left}"),
            };
            store.set_state(
                &instance.id,
                &State::Failed(reason.clone()),
                Some("creating"),
            )?;
            Err(reason)
        }
    }
}

/// Starts an instance now: reserved here, deployed by [`execute`] on a background thread. Returns it in Creating.
pub fn start(app: &Arc<App>, req: UpRequest) -> Result<Instance, String> {
    let instance = reserve(&app.ephemeral, &platform::Production { app }, &req)?;
    let app = Arc::clone(app);
    let for_thread = instance.clone();
    std::thread::spawn(move || {
        let ports = platform::Production { app: &app };
        if let Err(why) = execute(&app.ephemeral, &ports, &for_thread) {
            eprintln!("apid: ephemeral {}: {why}", for_thread.id);
        }
    });
    Ok(instance)
}

/// Deploys an instance of `req` and returns it once it is Ready (its rollout and checks are done). An error is the reason
/// the deploy failed; the instance is then Failed and its target is removed.
pub fn up(app: &Arc<App>, req: UpRequest) -> Result<Instance, String> {
    up_with(&app.ephemeral, &platform::Production { app }, req)
}

/// [`up`] over the ports and the store.
pub fn up_with(store: &Store, ports: &dyn Ports, req: UpRequest) -> Result<Instance, String> {
    let instance = reserve(store, ports, &req)?;
    execute(store, ports, &instance)
}

/// Tears an instance down: its target is removed, and the instance is torn down. One already torn down is not an error;
/// a target that could not be removed is an error, and the instance stays as it was for the ticker to try again.
pub fn down(app: &Arc<App>, instance: &str) -> Result<(), String> {
    down_with(&app.ephemeral, &platform::Production { app }, instance)
}

/// [`down`] over the ports.
pub fn down_with(store: &Store, ports: &dyn Ports, instance: &str) -> Result<(), String> {
    let found = store
        .get(instance)
        .ok_or_else(|| format!("no instance {instance}"))?;
    if found.state == State::TornDown {
        return Ok(());
    }
    platform::teardown(ports, &found.target)?;
    store.clear_retry(instance);
    store.set_state(instance, &State::TornDown, None)
}

/// An instance by its id.
pub fn get(app: &App, instance: &str) -> Option<Instance> {
    app.ephemeral.get(instance)
}

/// The project's instances, newest first.
pub fn list(app: &App, project: &str) -> Vec<Instance> {
    app.ephemeral.list(project)
}

/// One pass of the ticker over `store`: every instance whose time is up (not creating, not torn down) is torn down. A
/// removal that fails (its platform unreachable) is retried on a backoff: one tick after the failure, the wait doubling
/// up to [`TEARDOWN_WAIT_CAP_MS`], and at most [`MAX_TEARDOWN_ATTEMPTS`] tries, after which the ticker leaves it for the
/// owner's `DELETE`. The notes say what was torn down, what could not be, and when it is tried again.
pub fn sweep(store: &Store, ports: &dyn Ports, now: u64) -> Vec<String> {
    let mut notes = Vec::new();
    for instance in store.live() {
        if instance.expires_ms > now || instance.state == State::Creating {
            continue;
        }
        let waiting = store
            .retry_of(&instance.id)
            .is_some_and(|r| r.attempts >= MAX_TEARDOWN_ATTEMPTS || now < r.next_ms);
        if waiting {
            continue;
        }
        match down_with(store, ports, &instance.id) {
            Ok(()) => {
                store.clear_retry(&instance.id);
                notes.push(format!("ephemeral {}: torn down at its time", instance.id));
            }
            Err(why) => {
                let retry = store.failed_teardown(&instance.id, now);
                if retry.attempts >= MAX_TEARDOWN_ATTEMPTS {
                    notes.push(format!(
                        "ephemeral {}: gave up after {} tries, not torn down: {why} (DELETE the instance to try again)",
                        instance.id, retry.attempts
                    ));
                } else {
                    notes.push(format!(
                        "ephemeral {}: not torn down (try {} of {}), again at {}: {why}",
                        instance.id, retry.attempts, MAX_TEARDOWN_ATTEMPTS, retry.next_ms
                    ));
                }
            }
        }
    }
    notes
}

/// The hub's ticker: one [`sweep`] every minute.
pub fn spawn_ticker(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(TICK);
            let ports = platform::Production { app: &app };
            for note in sweep(&app.ephemeral, &ports, apz_plaza::now_ms()) {
                eprintln!("apid: {note}");
            }
        }
    });
}

// ---- Routes ----------------------------------------------------------------------------------------------------------

/// The request a start takes: the environment, the subject as `{"lane": id}` or `{"release": id}`, and the minutes.
fn parse_subject(value: &Value) -> Result<Subject, String> {
    if let Some(id) = value.get("lane").and_then(Value::as_str) {
        return Ok(Subject::Lane(id.to_string()));
    }
    if let Some(id) = value.get("release").and_then(Value::as_str) {
        return Ok(Subject::Release(id.to_string()));
    }
    Err("subject is {\"lane\": change id} or {\"release\": release id}".into())
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/projects/{id}/ephemeral", |app, req, p| {
            with_project(app, p, |app, id| {
                let Some(arc) = app.handle().upgrade() else {
                    return Response::error(503, "the hub is not ready");
                };
                let body: Value = match serde_json::from_slice(&req.body) {
                    Ok(body) => body,
                    Err(e) => return Response::error(400, &format!("bad body: {e}")),
                };
                let Some(environment) = body.get("environment").and_then(Value::as_str) else {
                    return Response::error(400, "name the environment");
                };
                let subject = match body.get("subject").map(parse_subject) {
                    Some(Ok(subject)) => subject,
                    Some(Err(e)) => return Response::error(400, &e),
                    None => return Response::error(400, "name the subject"),
                };
                let ttl_minutes = body
                    .get("ttl_minutes")
                    .and_then(Value::as_u64)
                    .and_then(|m| u32::try_from(m).ok());
                let req = UpRequest {
                    project: id.to_string(),
                    environment: environment.to_string(),
                    subject,
                    ttl_minutes,
                    run: None,
                    keep: false,
                    built: BTreeMap::new(),
                };
                match start(&arc, req) {
                    Ok(instance) => Response::json(202, &instance),
                    Err(e) => Response::error(409, &e),
                }
            })
        })
        .doc(
            RouteDoc::new("Start an ephemeral instance of a lane or a release")
                .errors(&[400, 404, 409, 503]),
        )
        .post("/v1/projects/{id}/ephemeral/against", |app, req, p| {
            with_project(app, p, |app, id| {
                let Some(arc) = app.handle().upgrade() else {
                    return Response::error(503, "the hub is not ready");
                };
                let body: Value = match serde_json::from_slice(&req.body) {
                    Ok(body) => body,
                    Err(e) => return Response::error(400, &format!("bad body: {e}")),
                };
                let Some(environment) = body.get("environment").and_then(Value::as_str) else {
                    return Response::error(400, "name the environment");
                };
                let subject = match body.get("subject").map(parse_subject) {
                    Some(Ok(subject)) => subject,
                    Some(Err(e)) => return Response::error(400, &e),
                    None => return Response::error(400, "name the subject"),
                };
                let paths = body
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                let request = against::Request {
                    project: id.to_string(),
                    environment: environment.to_string(),
                    subject,
                    paths,
                    keep: body.get("keep").and_then(Value::as_bool).unwrap_or(false),
                    ttl_minutes: body
                        .get("ttl_minutes")
                        .and_then(Value::as_u64)
                        .and_then(|m| u32::try_from(m).ok()),
                };
                match against::check(&arc, &request) {
                    Ok(verdict) => Response::json(200, &verdict),
                    Err(e) => Response::error(409, &e),
                }
            })
        })
        .doc(
            RouteDoc::new(
                "Deploy an ephemeral instance, probe its paths and run the project's user-test scenarios against it, then tear it down unless kept",
            )
            .errors(&[400, 404, 409, 503]),
        )
        .get("/v1/projects/{id}/ephemeral", |app, _, p| {
            with_project(app, p, |app, id| Response::json(200, &list(app, id)))
        })
        .doc(RouteDoc::new("List a project's ephemeral instances").errors(&[404]))
        .get("/v1/ephemeral/{instance}", |app, _, p| {
            match get(app, p.get("instance")) {
                Some(instance) => Response::json(200, &instance),
                None => Response::error(404, "no such instance"),
            }
        })
        .doc(RouteDoc::new("An ephemeral instance, with its state and address").errors(&[404]))
        .delete("/v1/ephemeral/{instance}", |app, _, p| {
            let Some(arc) = app.handle().upgrade() else {
                return Response::error(503, "the hub is not ready");
            };
            match down(&arc, p.get("instance")) {
                Ok(()) => Response::json(200, &json!({"torn_down": true})),
                Err(e) if e.starts_with("no instance") => Response::error(404, &e),
                Err(e) => Response::error(409, &e),
            }
        })
        .doc(RouteDoc::new("Tear an ephemeral instance down now").errors(&[404, 409, 503]))
}

fn with_project(app: &App, p: &Params<'_>, then: impl FnOnce(&App, &str) -> Response) -> Response {
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    then(app, id)
}
