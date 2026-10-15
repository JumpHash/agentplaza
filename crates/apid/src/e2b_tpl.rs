//! E2B template builds: the SDKs' `Template.build` (the v2 build API), served by running each step
//! in a plaza sandbox and keeping the result as a memory snapshot. A sandbox created from the
//! template (by its id, its name, or `name:tag`) is a fork of that snapshot, so it starts warm with
//! what the steps installed and what the start command left running.
//!
//! POST   /e2b/v3/templates {name, tags?, cpuCount?, memoryMB?}      → 202 TemplateRequestResponseV3
//! GET    /e2b/templates/{id}/files/{hash}                           → 201 {present, url?, headers?}
//! PUT    /e2b/templates/{id}/files/{hash}   (the upload link; its one-time token is a header)
//! POST   /e2b/v2/templates/{id}/builds/{build} TemplateBuildStartV2 → 202
//! GET    /e2b/templates/{id}/builds/{build}/status?logsOffset=&limit= → TemplateBuildInfo
//! GET    /e2b/templates/aliases/{name}                              → {templateID, public}
//! GET    /e2b/templates/{id}/tags                                   → [TemplateTag]
//! POST   /e2b/templates/tags {target, tags}                         → 201 {buildID, tags}
//! DELETE /e2b/templates/tags {name, tags}                           → 204
//! DELETE /e2b/templates/{id}                                        → 204 (routed in `crate::e2b`)
//!
//! A build runs in a sandbox of its own: of the plaza's base image (`fromImage` `e2bdev/base`; no
//! other image is pulled) or of another built template (`fromTemplate`). Its steps run in order:
//! `RUN` (`/bin/bash -l -c`, as the step's user or the current one), `COPY` (the uploaded archive,
//! unpacked in the sandbox and copied by Docker's rules, owned by the step's user or the current
//! one), `ENV`, `WORKDIR` and `USER` (`user` or `root`, the guest's two users). Then the start
//! command is started in the background (its output goes to `/tmp/e2b-start-cmd.log`), the ready
//! command is retried until it succeeds, the daemon is given the build's environment, user and
//! working directory, and a memory snapshot is taken. The build sandbox is then killed and kept,
//! retired, for that snapshot. There is no layer cache: every build runs every step.
//!
//! When a build is ready the tags it asked for (`default` when it named none) move to it, and an
//! older ready build that holds no tag any more is deleted with its snapshot. A template's name
//! without a tag is its `default` build, or its newest ready build when no build has that tag.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use apz_http::{Request, Response, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::e2b::{self, Refused, Sandbox, Spec};
use crate::state::App;

const TEMPLATES_FILE: &str = "e2b-templates.json";
/// The uploaded archives, by their files hash (`<state>/e2b-template-files/<hash>`).
const FILES_DIR: &str = "e2b-template-files";
/// The images a build may start from: the plaza's own base image, by E2B's name for it.
const BASE_IMAGES: &[&str] = &["e2bdev/base", "e2bdev/base:latest", "base"];
const DEFAULT_TAG: &str = "default";
/// Builds that may run at once; the next trigger is refused with 429.
const MAX_RUNNING: usize = 2;
const MAX_STEPS: usize = 256;
const MAX_ARG: usize = 64 << 10;
const MAX_TAGS: usize = 32;
/// The builds a template keeps; the oldest failed ones go first.
const MAX_BUILDS: usize = 20;
const VCPUS: std::ops::RangeInclusive<u32> = 1..=8;
const MEMORY_MIB: std::ops::RangeInclusive<u32> = 128..=16_384;
/// How long one step (or the start command's launch) may run.
const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long the ready command is retried, and how often.
const READY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const READY_EVERY: Duration = Duration::from_secs(1);
const READY_CALL: Duration = Duration::from_secs(60);
/// The build sandbox's time to live: the reaper ends one a hub restart left behind.
const BUILD_TTL_S: u32 = 3 * 3600;
const UPLOAD_TTL_MS: u64 = 3_600_000;
const UPLOAD_HEADER: &str = "x-plaza-upload";
const MAX_LOG_LINES: usize = 10_000;
const MAX_LOG_LINE: usize = 4096;
/// The most log entries one status answer carries (E2B's page).
const STATUS_PAGE: usize = 100;
const BUILD_USER: &str = "user";
const BUILD_HOME: &str = "/home/user";
/// Where a `COPY` archive is put in the build sandbox before it is unpacked.
const COPY_STAGING: &str = "/tmp/.e2b-build";
const START_LOG: &str = "/tmp/e2b-start-cmd.log";
/// The most a daemon answer that is not a process stream may be (an error's text).
const MAX_ANSWER: usize = 64 << 10;
const MAX_ENVELOPE: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Waiting,
    Building,
    Ready,
    Error,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::Building => "building",
            Status::Ready => "ready",
            Status::Error => "error",
        }
    }
}

/// Why a build failed, and the step it failed in (`base`, a step's number from 1, or `finalize`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reason {
    pub message: String,
    pub step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub tag: String,
    pub assigned_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    /// A UUID, as E2B's build ids are.
    pub id: String,
    pub status: Status,
    pub created_ms: u64,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    pub vcpus: u32,
    pub memory_mib: u32,
    /// The tags the request named; they move to the build when it is ready.
    pub wanted_tags: Vec<String>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    /// The snapshot (`<sandbox>:<checkpoint>`) sandboxes of this build fork.
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub reason: Option<Reason>,
    /// The user and working directory the build ended with: a build from this template starts there.
    #[serde(default = "build_user")]
    pub user: String,
    #[serde(default = "build_home")]
    pub workdir: String,
}

fn build_user() -> String {
    BUILD_USER.to_string()
}

fn build_home() -> String {
    BUILD_HOME.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// Twenty hex digits.
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub builds: Vec<Build>,
}

impl Template {
    fn build(&self, id: &str) -> Option<&Build> {
        self.builds.iter().find(|b| b.id == id)
    }

    /// The ready build `tag` names; with no tag, the `default` one, else the newest ready one.
    fn pick(&self, tag: Option<&str>) -> Option<&Build> {
        let ready = || {
            self.builds
                .iter()
                .filter(|b| b.status == Status::Ready && b.snapshot.is_some())
        };
        let tagged = |tag: &str| ready().find(|b| b.tags.iter().any(|t| t.tag == tag));
        match tag {
            Some(tag) => tagged(tag),
            None => tagged(DEFAULT_TAG).or_else(|| ready().max_by_key(|b| b.created_ms)),
        }
    }
}

/// The templates, saved to `<state>/e2b-templates.json` on every change.
pub struct Templates {
    path: PathBuf,
    unreadable: Option<String>,
    set: Mutex<Vec<Template>>,
}

impl Templates {
    fn load(state_dir: &Path) -> (Templates, Vec<String>) {
        let path = state_dir.join(TEMPLATES_FILE);
        let (mut templates, unreadable) = e2b::read_list::<Template>(&path);
        if let Some(why) = &unreadable {
            eprintln!("apid: the E2B templates are unreadable ({why})");
        }
        // A build the hub was running when it stopped did not finish.
        let mut interrupted = Vec::new();
        for build in templates.iter_mut().flat_map(|t| t.builds.iter_mut()) {
            if matches!(build.status, Status::Waiting | Status::Building) {
                build.status = Status::Error;
                build.reason = Some(Reason {
                    message: "the hub stopped during the build".into(),
                    step: "base".into(),
                });
                interrupted.push(build.id.clone());
            }
        }
        let store = Templates {
            path,
            unreadable,
            set: Mutex::new(templates),
        };
        if !interrupted.is_empty() {
            let set = e2b::lock(&store.set).clone();
            if let Err(refused) = e2b::persist(&store.path, store.unreadable.as_deref(), &set) {
                eprintln!(
                    "apid: the E2B templates were not saved: {}",
                    refused.message()
                );
            }
        }
        (store, interrupted)
    }

    fn list(&self) -> Vec<Template> {
        e2b::lock(&self.set).clone()
    }

    /// Changes the templates and saves them; nothing changes when `change` refuses.
    fn edit<R>(
        &self,
        change: impl FnOnce(&mut Vec<Template>) -> Result<R, Refused>,
    ) -> Result<R, Refused> {
        let mut set = e2b::lock(&self.set);
        let mut next = set.clone();
        let result = change(&mut next)?;
        e2b::persist(&self.path, self.unreadable.as_deref(), &next)?;
        *set = next;
        Ok(result)
    }

    fn edit_build(
        &self,
        template: &str,
        build: &str,
        change: impl FnOnce(&mut Build),
    ) -> Result<(), Refused> {
        self.edit(|set| {
            let found = set
                .iter_mut()
                .find(|t| t.id == template)
                .and_then(|t| {
                    t.updated_ms = apz_plaza::now_ms();
                    t.builds.iter_mut().find(|b| b.id == build)
                })
                .ok_or_else(|| build_not_found(build))?;
            change(found);
            Ok(())
        })
    }
}

static TEMPLATES: OnceLock<Templates> = OnceLock::new();
/// Each build's log, kept in memory (a hub restart forgets it).
static LOGS: Mutex<BTreeMap<String, Vec<LogLine>>> = Mutex::new(BTreeMap::new());
/// The upload links handed out and not used yet, by token.
static UPLOADS: Mutex<BTreeMap<String, Upload>> = Mutex::new(BTreeMap::new());

struct Upload {
    template: String,
    hash: String,
    expires_ms: u64,
}

#[derive(Debug, Clone)]
struct LogLine {
    ms: u64,
    level: &'static str,
    message: String,
}

/// The hub's templates, loaded on first use. Build sandboxes an interrupted build left are ended then.
fn templates(app: &App) -> &'static Templates {
    TEMPLATES.get_or_init(|| {
        let (store, interrupted) = Templates::load(&app.config.state_dir);
        for sandbox in e2b::sandboxes(app).list() {
            let leftover = sandbox
                .metadata
                .get(BUILD_METADATA)
                .is_some_and(|build| interrupted.contains(build));
            if leftover && !sandbox.retired {
                let _ = e2b::destroy(app, &sandbox, "its template build was interrupted", "hub");
            }
        }
        store
    })
}

/// The metadata key that marks a build's sandbox (its value is the build's id).
const BUILD_METADATA: &str = "e2b.templateBuild";

fn template_not_found(id: &str) -> Refused {
    Refused::new(404, "template_not_found", format!("no template {id}"))
}

fn build_not_found(id: &str) -> Refused {
    Refused::new(404, "template_build_not_found", format!("no build {id}"))
}

fn log(build: &str, level: &'static str, message: impl Into<String>) {
    let mut message: String = message.into();
    if message.len() > MAX_LOG_LINE {
        let mut cut = MAX_LOG_LINE;
        while !message.is_char_boundary(cut) {
            cut -= 1;
        }
        message.truncate(cut);
    }
    let mut logs = e2b::lock(&LOGS);
    let lines = logs.entry(build.to_string()).or_default();
    if lines.len() < MAX_LOG_LINES {
        lines.push(LogLine {
            ms: apz_plaza::now_ms(),
            level,
            message,
        });
    }
}

/// The template a reference names (its id or its name, then an optional `:tag`) and the snapshot of
/// the build it picks: what a sandbox created from it forks. `None` when it names no ready build.
pub(crate) fn resolve(app: &App, reference: &str) -> Option<(String, String)> {
    let (name, tag) = split_reference(reference);
    templates(app)
        .list()
        .iter()
        .find(|t| t.id == name || t.name == name)
        .and_then(|t| {
            let build = t.pick(tag)?;
            Some((t.id.clone(), build.snapshot.clone()?))
        })
}

/// `name` and `tag` of `name:tag`; a reference with no `:` has no tag.
fn split_reference(reference: &str) -> (&str, Option<&str>) {
    match reference.rsplit_once(':') {
        Some((name, tag)) => (name, Some(tag)),
        None => (reference, None),
    }
}

fn check_name(name: &str) -> Result<(), Refused> {
    let ok = (1..=128).contains(&name.len())
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
    if !ok {
        return Err(e2b::invalid(format!(
            "template name {name:?}: up to 128 letters, digits, '-', '_', '.' and '/', starting with a letter or digit"
        )));
    }
    if name == "base" {
        return Err(e2b::invalid("base names the plaza's base image"));
    }
    Ok(())
}

fn check_tag(tag: &str) -> Result<(), Refused> {
    let ok = (1..=128).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(e2b::invalid(format!(
            "tag {tag:?}: up to 128 letters, digits, '-', '_' and '.'"
        )))
    }
}

fn uuid_v4() -> String {
    let mut bytes = apz_trust::random::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = e2b::hex_of(&bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildRequest {
    name: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
    cpu_count: Option<u32>,
    #[serde(rename = "memoryMB")]
    memory_mb: Option<u32>,
    #[serde(default)]
    #[allow(dead_code)]
    min_free_disk_mb: Option<u64>,
}

/// `POST /e2b/v3/templates`: a new build of the template the name names (a new template when none
/// has that name), waiting for its steps.
fn request_build(app: &App, req: &Request) -> Result<Response, Refused> {
    let body: BuildRequest = e2b::json_body(req)?;
    let (name, tag) = split_reference(body.name.trim());
    check_name(name)?;
    let mut wanted: Vec<String> = Vec::new();
    for tag in tag
        .into_iter()
        .chain(body.tags.iter().flatten().map(String::as_str))
    {
        check_tag(tag)?;
        if !wanted.iter().any(|t| t == tag) {
            wanted.push(tag.to_string());
        }
    }
    if wanted.len() > MAX_TAGS {
        return Err(e2b::invalid(format!("at most {MAX_TAGS} tags")));
    }
    if wanted.is_empty() {
        wanted.push(DEFAULT_TAG.into());
    }
    let vcpus = body.cpu_count.unwrap_or(e2b::SANDBOX_VCPUS);
    let memory_mib = body.memory_mb.unwrap_or(e2b::SANDBOX_MEMORY_MIB);
    if !VCPUS.contains(&vcpus) {
        return Err(e2b::invalid(format!(
            "cpuCount {vcpus}: {}..={}",
            VCPUS.start(),
            VCPUS.end()
        )));
    }
    if !MEMORY_MIB.contains(&memory_mib) {
        return Err(e2b::invalid(format!(
            "memoryMB {memory_mib}: {}..={}",
            MEMORY_MIB.start(),
            MEMORY_MIB.end()
        )));
    }
    let now = apz_plaza::now_ms();
    let build = Build {
        id: uuid_v4(),
        status: Status::Waiting,
        created_ms: now,
        finished_ms: None,
        vcpus,
        memory_mib,
        wanted_tags: wanted.clone(),
        tags: Vec::new(),
        snapshot: None,
        reason: None,
        user: build_user(),
        workdir: build_home(),
    };
    let build_id = build.id.clone();
    let template_id = templates(app).edit(|set| {
        let template = match set.iter_mut().find(|t| t.name == name) {
            Some(template) => template,
            None => {
                set.push(Template {
                    id: e2b::hex_of(&apz_trust::random::<10>()),
                    name: name.to_string(),
                    created_ms: now,
                    updated_ms: now,
                    builds: Vec::new(),
                });
                set.last_mut().expect("just pushed")
            }
        };
        template.updated_ms = now;
        template.builds.push(build);
        trim_builds(template);
        Ok(template.id.clone())
    })?;
    log(
        &build_id,
        "info",
        format!("Build {build_id} of {name} requested"),
    );
    Ok(Response::json(
        202,
        &json!({
            "templateID": template_id,
            "buildID": build_id,
            "public": false,
            "aliases": [name],
            "names": [name],
            "tags": wanted,
        }),
    ))
}

/// Keeps a template's builds at [`MAX_BUILDS`]: the oldest failed builds go first, then the oldest
/// ready builds that hold no tag (whose snapshots the next finished build deletes anyway).
fn trim_builds(template: &mut Template) {
    while template.builds.len() > MAX_BUILDS {
        let failed = template
            .builds
            .iter()
            .position(|b| b.status == Status::Error);
        match failed {
            Some(index) => {
                let gone = template.builds.remove(index);
                e2b::lock(&LOGS).remove(&gone.id);
            }
            None => break,
        }
    }
}

fn check_hash(hash: &str) -> Result<(), Refused> {
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(e2b::invalid("a files hash is 64 hex digits"))
    }
}

fn files_dir(app: &App) -> PathBuf {
    app.config.state_dir.join(FILES_DIR)
}

fn find_template(app: &App, id: &str) -> Result<Template, Refused> {
    templates(app)
        .list()
        .into_iter()
        .find(|t| t.id == id || t.name == id)
        .ok_or_else(|| template_not_found(id))
}

/// `GET /e2b/templates/{id}/files/{hash}`: whether the archive is here already, and if not a link to
/// put it at, good for one upload within the hour.
fn file_link(app: &App, req: &Request, id: &str, hash: &str) -> Result<Response, Refused> {
    let template = find_template(app, id)?;
    check_hash(hash)?;
    let hash = hash.to_ascii_lowercase();
    if files_dir(app).join(&hash).is_file() {
        return Ok(Response::json(201, &json!({"present": true})));
    }
    let token = e2b::hex_of(&apz_trust::random::<32>());
    let now = apz_plaza::now_ms();
    {
        let mut uploads = e2b::lock(&UPLOADS);
        uploads.retain(|_, u| u.expires_ms > now);
        uploads.insert(
            token.clone(),
            Upload {
                template: template.id.clone(),
                hash: hash.clone(),
                expires_ms: now + UPLOAD_TTL_MS,
            },
        );
    }
    let url = format!(
        "{}/e2b/templates/{}/files/{hash}",
        base_url(app, req),
        template.id
    );
    Ok(Response::json(
        201,
        &json!({"present": false, "url": url, "headers": {UPLOAD_HEADER: token}}),
    ))
}

/// The hub's address as the caller reached it: its `Host`, with the scheme the hub serves.
fn base_url(app: &App, req: &Request) -> String {
    let scheme = if app.config.tls || !app.config.bind.is_loopback() {
        "https"
    } else {
        "http"
    };
    let host = req
        .header("host")
        .filter(|host| {
            !host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        })
        .map_or_else(|| format!("127.0.0.1:{}", app.config.port), str::to_string);
    format!("{scheme}://{host}")
}

/// `PUT /e2b/templates/{id}/files/{hash}`: the archive itself, with the link's token.
fn upload(app: &App, req: &Request, id: &str, hash: &str) -> Result<Response, Refused> {
    let token = req.header(UPLOAD_HEADER).unwrap_or_default();
    let upload = e2b::lock(&UPLOADS).remove(token);
    let Some(upload) = upload.filter(|u| {
        u.expires_ms > apz_plaza::now_ms()
            && u.template == id
            && u.hash == hash.to_ascii_lowercase()
    }) else {
        return Err(Refused::new(
            403,
            "upload_forbidden",
            "this upload link is not valid (ask for a new one)",
        ));
    };
    let dir = files_dir(app);
    let io_error = |e: io::Error| Refused::new(500, "internal_server_error", e.to_string());
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let part = dir.join(format!("{}.part", upload.hash));
    std::fs::write(&part, &req.body).map_err(io_error)?;
    std::fs::rename(&part, dir.join(&upload.hash)).map_err(io_error)?;
    Ok(Response::json(200, &json!({})))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepBody {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    files_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    from_image: Option<String>,
    from_template: Option<String>,
    from_image_registry: Option<Value>,
    #[serde(default)]
    steps: Option<Vec<StepBody>>,
    start_cmd: Option<String>,
    ready_cmd: Option<String>,
}

/// A checked build step.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Run {
        cmd: String,
        user: Option<String>,
    },
    Copy {
        src: String,
        dest: String,
        user: Option<String>,
        mode: Option<String>,
        hash: String,
    },
    Env(Vec<(String, String)>),
    Workdir(String),
    User(String),
}

impl Step {
    fn describe(&self) -> String {
        match self {
            Step::Run { cmd, .. } => format!("RUN {cmd}"),
            Step::Copy { src, dest, .. } => format!("COPY {src} {dest}"),
            Step::Env(pairs) => {
                let pairs: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("ENV {}", pairs.join(" "))
            }
            Step::Workdir(dir) => format!("WORKDIR {dir}"),
            Step::User(user) => format!("USER {user}"),
        }
    }
}

#[derive(Debug, Clone)]
enum Base {
    Image(String),
    Template {
        id: String,
        snapshot: String,
        user: String,
        workdir: String,
    },
}

#[derive(Debug, Clone)]
struct Plan {
    base: Base,
    steps: Vec<Step>,
    start_cmd: Option<String>,
    ready_cmd: Option<String>,
    vcpus: u32,
    memory_mib: u32,
}

fn check_user(user: &str, step: usize) -> Result<(), Refused> {
    if user == "user" || user == "root" {
        Ok(())
    } else {
        Err(e2b::invalid(format!(
            "step {step}: user {user:?}: the sandbox's users are user and root"
        )))
    }
}

fn is_env_name(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Checks the steps of a build: their kinds, arguments, users and uploaded archives.
fn check_steps(files: &Path, steps: &[StepBody]) -> Result<Vec<Step>, Refused> {
    if steps.len() > MAX_STEPS {
        return Err(e2b::invalid(format!("at most {MAX_STEPS} steps")));
    }
    let mut checked = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let n = index + 1;
        let args = step.args.clone().unwrap_or_default();
        if args.iter().any(|a| a.len() > MAX_ARG || a.contains('\0')) {
            return Err(e2b::invalid(format!(
                "step {n}: an argument is longer than {MAX_ARG} bytes or holds a NUL"
            )));
        }
        let nonempty = |i: usize| args.get(i).map(String::as_str).filter(|a| !a.is_empty());
        let bad = |what: &str| e2b::invalid(format!("step {n} ({}): {what}", step.kind));
        let step = match step.kind.as_str() {
            "RUN" => {
                let cmd = nonempty(0).ok_or_else(|| bad("no command"))?.to_string();
                let user = nonempty(1).map(str::to_string);
                if let Some(user) = &user {
                    check_user(user, n)?;
                }
                Step::Run { cmd, user }
            }
            "COPY" => {
                let src = nonempty(0).ok_or_else(|| bad("no source"))?.to_string();
                let dest = nonempty(1)
                    .ok_or_else(|| bad("no destination"))?
                    .to_string();
                let user = nonempty(2).map(str::to_string);
                if let Some(user) = &user {
                    check_user(user, n)?;
                }
                let mode = nonempty(3).map(str::to_string);
                if mode
                    .as_deref()
                    .is_some_and(|m| m.len() > 5 || !m.bytes().all(|b| (b'0'..=b'7').contains(&b)))
                {
                    return Err(bad("the mode is not octal"));
                }
                let hash = step
                    .files_hash
                    .clone()
                    .ok_or_else(|| bad("no filesHash"))?
                    .to_ascii_lowercase();
                check_hash(&hash)?;
                if !files.join(&hash).is_file() {
                    return Err(bad(&format!("the files {hash} were not uploaded")));
                }
                Step::Copy {
                    src,
                    dest,
                    user,
                    mode,
                    hash,
                }
            }
            "ENV" => {
                if args.is_empty() || args.len() % 2 != 0 {
                    return Err(bad("ENV takes name and value pairs"));
                }
                let pairs: Vec<(String, String)> = args
                    .chunks(2)
                    .map(|pair| (pair[0].clone(), pair[1].clone()))
                    .collect();
                if let Some((name, _)) = pairs.iter().find(|(name, _)| !is_env_name(name)) {
                    return Err(bad(&format!("{name:?} is not a variable name")));
                }
                Step::Env(pairs)
            }
            "WORKDIR" => Step::Workdir(nonempty(0).ok_or_else(|| bad("no directory"))?.to_string()),
            "USER" => {
                let user = nonempty(0).ok_or_else(|| bad("no user"))?.to_string();
                check_user(&user, n)?;
                Step::User(user)
            }
            other => {
                return Err(e2b::invalid(format!(
                    "step {n}: {other} is not a build step (RUN, COPY, ENV, WORKDIR, USER)"
                )));
            }
        };
        checked.push(step);
    }
    Ok(checked)
}

/// `POST /e2b/v2/templates/{id}/builds/{build}`: checks the build's steps and starts it.
fn trigger(
    app: &App,
    req: &Request,
    id: &str,
    build_id: &str,
    key: &str,
) -> Result<Response, Refused> {
    if app.control.paused() {
        return Err(e2b::held());
    }
    let body: StartBody = e2b::json_body(req)?;
    let template = find_template(app, id)?;
    let build = template
        .build(build_id)
        .cloned()
        .ok_or_else(|| build_not_found(build_id))?;
    if build.status != Status::Waiting {
        return Err(Refused::new(
            409,
            "template_build_started",
            format!("build {build_id} is {} already", build.status.as_str()),
        ));
    }
    if body
        .from_image_registry
        .as_ref()
        .is_some_and(|v| !v.is_null())
    {
        return Err(e2b::invalid(
            "fromImageRegistry: the plaza pulls no images; build from e2bdev/base or a template",
        ));
    }
    let base = match (body.from_template.as_deref(), body.from_image.as_deref()) {
        (Some(from), _) if !from.is_empty() => {
            let (name, tag) = split_reference(from);
            let source = templates(app)
                .list()
                .into_iter()
                .find(|t| t.id == name || t.name == name)
                .ok_or_else(|| e2b::invalid(format!("fromTemplate {from}: no such template")))?;
            let picked = source
                .pick(tag)
                .ok_or_else(|| e2b::invalid(format!("fromTemplate {from}: no ready build")))?;
            Base::Template {
                id: source.id.clone(),
                snapshot: picked.snapshot.clone().unwrap_or_default(),
                user: picked.user.clone(),
                workdir: picked.workdir.clone(),
            }
        }
        (_, Some(image)) if BASE_IMAGES.contains(&image) => Base::Image(image.to_string()),
        (_, Some(image)) => {
            return Err(e2b::invalid(format!(
                "fromImage {image}: the plaza builds on its own base image (e2bdev/base) or on a built template; no other image is pulled"
            )));
        }
        _ => return Err(e2b::invalid("name fromImage or fromTemplate")),
    };
    let steps = check_steps(&files_dir(app), body.steps.as_deref().unwrap_or_default())?;
    for (what, cmd) in [("startCmd", &body.start_cmd), ("readyCmd", &body.ready_cmd)] {
        if cmd
            .as_ref()
            .is_some_and(|c| c.len() > MAX_ARG || c.contains('\0'))
        {
            return Err(e2b::invalid(format!(
                "{what} is longer than {MAX_ARG} bytes or holds a NUL"
            )));
        }
    }
    let plan = Plan {
        base,
        steps,
        start_cmd: body.start_cmd.filter(|c| !c.trim().is_empty()),
        ready_cmd: body.ready_cmd.filter(|c| !c.trim().is_empty()),
        vcpus: build.vcpus,
        memory_mib: build.memory_mib,
    };
    let template_id = template.id.clone();
    templates(app).edit(|set| {
        let running = set
            .iter()
            .flat_map(|t| t.builds.iter())
            .filter(|b| b.status == Status::Building)
            .count();
        if running >= MAX_RUNNING {
            return Err(Refused::new(
                429,
                "template_builds_busy",
                format!("{running} builds are running; try again when one ends"),
            ));
        }
        let build = set
            .iter_mut()
            .find(|t| t.id == template_id)
            .and_then(|t| t.builds.iter_mut().find(|b| b.id == build_id))
            .ok_or_else(|| build_not_found(build_id))?;
        if build.status != Status::Waiting {
            return Err(Refused::new(
                409,
                "template_build_started",
                "the build started already",
            ));
        }
        build.status = Status::Building;
        Ok(())
    })?;
    let weak = app.handle();
    let (build_id, key) = (build_id.to_string(), key.to_string());
    let spawned = std::thread::Builder::new()
        .name("e2b-template-build".into())
        .spawn({
            let (template_id, build_id) = (template_id.clone(), build_id.clone());
            move || {
                if let Some(app) = weak.upgrade() {
                    run_build(&app, &template_id, &build_id, &plan, &key);
                }
            }
        });
    if let Err(err) = spawned {
        let message = format!("the build did not start: {err}");
        let _ = templates(app).edit_build(&template_id, &build_id, |b| {
            b.status = Status::Error;
            b.reason = Some(Reason {
                message: message.clone(),
                step: "base".into(),
            });
        });
        return Err(Refused::new(500, "internal_server_error", message));
    }
    Ok(Response::json(202, &json!({})))
}

/// A failed build: the step and why.
struct Failure {
    step: String,
    message: String,
}

fn fail(step: &str, message: impl Into<String>) -> Failure {
    Failure {
        step: step.to_string(),
        message: message.into(),
    }
}

/// Where a build is: the user, working directory and environment the next step runs with.
struct Cursor {
    user: String,
    workdir: String,
    env: BTreeMap<String, String>,
}

/// Runs a build and records how it ended.
fn run_build(app: &App, template_id: &str, build_id: &str, plan: &Plan, key: &str) {
    let mut sandbox: Option<Sandbox> = None;
    let result = steps(app, build_id, plan, key, &mut sandbox);
    let now = apz_plaza::now_ms();
    match result {
        Ok((snapshot, cursor)) => {
            log(
                build_id,
                "info",
                format!("Build finished: snapshot {snapshot}"),
            );
            let finished = templates(app).edit(|set| {
                let template = set
                    .iter_mut()
                    .find(|t| t.id == template_id)
                    .ok_or_else(|| template_not_found(template_id))?;
                template.updated_ms = now;
                let wanted = template
                    .build(build_id)
                    .map(|b| b.wanted_tags.clone())
                    .unwrap_or_default();
                for other in template.builds.iter_mut().filter(|b| b.id != build_id) {
                    other.tags.retain(|t| !wanted.contains(&t.tag));
                }
                let build = template
                    .builds
                    .iter_mut()
                    .find(|b| b.id == build_id)
                    .ok_or_else(|| build_not_found(build_id))?;
                build.status = Status::Ready;
                build.finished_ms = Some(now);
                build.snapshot = Some(snapshot.clone());
                build.user = cursor.user.clone();
                build.workdir = cursor.workdir.clone();
                build.tags = wanted
                    .iter()
                    .map(|tag| Tag {
                        tag: tag.clone(),
                        assigned_ms: now,
                    })
                    .collect();
                // Older ready builds that hold no tag now: their snapshots go.
                let orphans: Vec<(String, String)> = template
                    .builds
                    .iter()
                    .filter(|b| b.id != build_id && b.status == Status::Ready && b.tags.is_empty())
                    .filter_map(|b| Some((b.id.clone(), b.snapshot.clone()?)))
                    .collect();
                template
                    .builds
                    .retain(|b| !orphans.iter().any(|(id, _)| *id == b.id));
                Ok(orphans)
            });
            match finished {
                Ok(orphans) => {
                    for (orphan, snapshot) in orphans {
                        e2b::lock(&LOGS).remove(&orphan);
                        if let Err(refused) = e2b::delete_snapshot(app, &snapshot, key) {
                            eprintln!(
                                "apid: template build {orphan}: its snapshot {snapshot} was not deleted: {}",
                                refused.message()
                            );
                        }
                    }
                }
                Err(refused) => {
                    eprintln!(
                        "apid: template build {build_id} finished but was not recorded: {}",
                        refused.message()
                    );
                }
            }
        }
        Err(failure) => {
            log(
                build_id,
                "error",
                format!("Build failed in step {}: {}", failure.step, failure.message),
            );
            if let Some(sandbox) = sandbox.and_then(|s| e2b::sandboxes(app).get(&s.id))
                && let Err(refused) = e2b::destroy(app, &sandbox, "its template build failed", key)
            {
                eprintln!(
                    "apid: template build {build_id}: sandbox {} was not ended: {}",
                    sandbox.id,
                    refused.message()
                );
            }
            let _ = templates(app).edit_build(template_id, build_id, |b| {
                b.status = Status::Error;
                b.finished_ms = Some(now);
                b.reason = Some(Reason {
                    message: failure.message,
                    step: failure.step,
                });
            });
        }
    }
}

/// The build's sandbox, its steps, the start and ready commands, and the snapshot: the snapshot's id
/// and where the build ended. The sandbox started is left in `sandbox` for the caller to end on failure.
fn steps(
    app: &App,
    build_id: &str,
    plan: &Plan,
    key: &str,
    sandbox: &mut Option<Sandbox>,
) -> Result<(String, Cursor), Failure> {
    let mut spec = Spec {
        template: "base".into(),
        timeout_s: BUILD_TTL_S,
        auto_pause: false,
        secure: true,
        allow_internet: true,
        metadata: BTreeMap::from([(BUILD_METADATA.to_string(), build_id.to_string())]),
        env_vars: BTreeMap::new(),
        allow_out: Vec::new(),
        deny_out: Vec::new(),
    };
    let mut cursor = Cursor {
        user: BUILD_USER.into(),
        workdir: BUILD_HOME.into(),
        env: BTreeMap::new(),
    };
    let (run, cid) = match &plan.base {
        Base::Image(image) => {
            log(
                build_id,
                "info",
                format!("Base: {image} (the plaza's base image)"),
            );
            e2b::new_run(app, plan.vcpus, plan.memory_mib).map_err(|r| fail("base", r.message()))?
        }
        Base::Template {
            id,
            snapshot,
            user,
            workdir,
        } => {
            log(build_id, "info", format!("Base: template {id}"));
            let (source, snap) = e2b::snapshot_source(app, snapshot)
                .ok_or_else(|| fail("base", format!("template {id}: its snapshot is gone")))?;
            spec.template = id.clone();
            spec.env_vars = source.env_vars.clone();
            cursor.env = source.env_vars.clone();
            cursor.user = user.clone();
            cursor.workdir = workdir.clone();
            e2b::fork_runs(app, &source, Some(&snap.checkpoint), 1)
                .map_err(|r| fail("base", r.message()))?
                .remove(0)
        }
    };
    let started = e2b::admit(app, &spec, run, cid, key).map_err(|r| fail("base", r.message()))?;
    log(
        build_id,
        "info",
        format!("Build sandbox {} started", started.id),
    );
    *sandbox = Some(started.clone());
    let daemon = Daemon::of(&started);

    let total = plan.steps.len();
    for (index, step) in plan.steps.iter().enumerate() {
        let name = (index + 1).to_string();
        if app.control.paused() {
            return Err(fail(&name, "the plaza is held by STOP ALL"));
        }
        log(
            build_id,
            "info",
            format!("[{}/{total}] {}", index + 1, step.describe()),
        );
        match step {
            Step::Run { cmd, user } => {
                let user = user.as_deref().unwrap_or(&cursor.user);
                let code = daemon
                    .exec(
                        cmd,
                        user,
                        &cursor.workdir,
                        &cursor.env,
                        STEP_TIMEOUT,
                        |line| log(build_id, "info", line),
                    )
                    .map_err(|why| fail(&name, why))?;
                if code != 0 {
                    return Err(fail(
                        &name,
                        format!("the command exited with {code}: {cmd}"),
                    ));
                }
            }
            Step::Copy {
                src,
                dest,
                user,
                mode,
                hash,
            } => {
                let owner = user.as_deref().unwrap_or(&cursor.user);
                copy(
                    app,
                    &daemon,
                    build_id,
                    hash,
                    src,
                    dest,
                    owner,
                    mode.as_deref(),
                    &cursor,
                )
                .map_err(|why| fail(&name, why))?;
            }
            Step::Env(pairs) => {
                for (key, value) in pairs {
                    cursor.env.insert(key.clone(), value.clone());
                }
            }
            Step::Workdir(dir) => {
                let dir = absolute(&cursor.workdir, dir);
                let script =
                    "[ -d \"$D\" ] || { mkdir -p \"$D\" && busybox chown \"$IDS\" \"$D\"; }";
                let env = BTreeMap::from([
                    ("D".to_string(), dir.clone()),
                    ("IDS".to_string(), ids_of(&cursor.user).to_string()),
                ]);
                let code = daemon
                    .exec(script, "root", "/", &env, STEP_TIMEOUT, |line| {
                        log(build_id, "info", line)
                    })
                    .map_err(|why| fail(&name, why))?;
                if code != 0 {
                    return Err(fail(&name, format!("cannot make {dir} ({code})")));
                }
                cursor.workdir = dir;
            }
            Step::User(user) => cursor.user = user.clone(),
        }
    }

    if let Some(start) = &plan.start_cmd {
        log(build_id, "info", format!("Start command: {start}"));
        let script = "if [ -x /bin/bash ]; then s=/bin/bash; else s=/bin/sh; fi\n\
                      (exec \"$s\" -l -c \"$E2B_START_CMD\" </dev/null >>\"$E2B_START_LOG\" 2>&1 &)";
        let mut env = cursor.env.clone();
        env.insert("E2B_START_CMD".into(), start.clone());
        env.insert("E2B_START_LOG".into(), START_LOG.into());
        let code = daemon
            .exec(
                script,
                &cursor.user,
                &cursor.workdir,
                &env,
                STEP_TIMEOUT,
                |line| log(build_id, "info", line),
            )
            .map_err(|why| fail("finalize", why))?;
        if code != 0 {
            return Err(fail(
                "finalize",
                format!("the start command did not start ({code})"),
            ));
        }
    }
    if let Some(ready) = &plan.ready_cmd {
        log(
            build_id,
            "info",
            format!("Waiting for the ready command: {ready}"),
        );
        let started = Instant::now();
        loop {
            let code = daemon
                .exec(
                    ready,
                    &cursor.user,
                    &cursor.workdir,
                    &cursor.env,
                    READY_CALL,
                    |_| {},
                )
                .map_err(|why| fail("finalize", why))?;
            if code == 0 {
                break;
            }
            if started.elapsed() >= READY_TIMEOUT || app.control.paused() {
                return Err(fail(
                    "finalize",
                    format!(
                        "the ready command did not succeed in {}s",
                        READY_TIMEOUT.as_secs()
                    ),
                ));
            }
            std::thread::sleep(READY_EVERY);
        }
    }

    // The daemon keeps the build's environment, user and working directory in memory, so the
    // snapshot's sandboxes start with them; the record keeps the environment for a cold resume.
    let init = json!({
        "envVars": cursor.env,
        "defaultUser": cursor.user,
        "defaultWorkdir": cursor.workdir,
    });
    let reply = e2b::gateway(
        &started.id,
        started.access_token.as_deref(),
        "POST",
        "/init",
        Some(&init),
        e2b::GATEWAY_CALL,
    )
    .map_err(|why| fail("finalize", why))?;
    if !(200..300).contains(&reply.status) {
        return Err(fail(
            "finalize",
            format!("the daemon refused the build's defaults: {}", reply.status),
        ));
    }
    let env = cursor.env.clone();
    let recorded = e2b::sandboxes(app)
        .update(&started.id, |s| s.env_vars = env)
        .map_err(|r| fail("finalize", r.message()))?;
    log(build_id, "info", "Taking the snapshot");
    let snap = e2b::take_snapshot(app, &recorded, "template".into(), true, key)
        .map_err(|r| fail("finalize", r.message()))?;
    // Killed with a snapshot, the sandbox is retired: its VM stops and its run stays for the snapshot.
    let current = e2b::sandboxes(app)
        .get(&started.id)
        .ok_or_else(|| fail("finalize", "the build sandbox is gone"))?;
    e2b::destroy(app, &current, "its template is built", key)
        .map_err(|r| fail("finalize", r.message()))?;
    *sandbox = None;
    Ok((snap.id, cursor))
}

/// The guest's ids of a build user: the sandbox user is uid and gid 1000 in every plaza guest.
fn ids_of(user: &str) -> &'static str {
    if user == "root" { "0:0" } else { "1000:1000" }
}

/// `path` against the working directory `cwd` when it is relative.
fn absolute(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{path}", cwd.trim_end_matches('/'))
    }
}

/// The shell script a `COPY` runs as root: unpacks the archive, then copies what `$SRC` matches in
/// it to `$DEST` by Docker's rules (a directory's contents into `$DEST`; a file to `$DEST`, or into it
/// when it ends in `/`, is a directory, or the source is a pattern), owned by `$IDS` (`uid:gid`),
/// files given `$MODE` when one is set. It needs only the busybox applets every plaza guest has
/// (the initramfs links no `chown`, so busybox's is called by name there).
const COPY_SCRIPT: &str = r#"set -e
if ! command -v chown >/dev/null 2>&1; then chown() { busybox chown "$@"; }; fi
work="$ARCHIVE.d"
rm -rf "$work"; mkdir -p "$work"
tar -xf "$ARCHIVE" -C "$work"
rm -f "$ARCHIVE"
modes() {
  for f in "$1"/* "$1"/.[!.]* "$1"/..?*; do
    [ -e "$f" ] || continue
    if [ -L "$f" ]; then :; elif [ -d "$f" ]; then modes "$f"; else chmod "$MODE" "$f"; fi
  done
}
fix() {
  chown -R "$IDS" "$1"
  if [ -n "$MODE" ] && [ ! -L "$1" ]; then
    if [ -d "$1" ]; then modes "$1"; else chmod "$MODE" "$1"; fi
  fi
}
into() {
  mkdir -p "$DEST"
  if [ -d "$1" ] && [ ! -L "$1" ]; then
    for child in "$1"/* "$1"/.[!.]* "$1"/..?*; do
      [ -e "$child" ] || [ -L "$child" ] || continue
      name=${child##*/}
      cp -a "$child" "$DEST/"
      fix "$DEST/$name"
    done
  else
    cp -a "$1" "$DEST/"
    fix "$DEST/${1##*/}"
  fi
}
cd "$work"
src=${SRC%/}
case "$src" in
  *[*?[]*)
    found=0
    for match in $src; do
      [ -e "$match" ] || continue
      found=1
      into "$work/$match"
    done
    [ "$found" = 1 ] || { echo "COPY: nothing matches $SRC" >&2; exit 1; }
    ;;
  *)
    [ -e "$src" ] || [ -L "$src" ] || { echo "COPY: $SRC is not in the upload" >&2; exit 1; }
    case "$DEST" in
      */) into "$work/$src" ;;
      *)
        if [ -d "$src" ] || [ -d "$DEST" ]; then into "$work/$src"
        else
          mkdir -p "$(dirname "$DEST")"
          cp -a "$work/$src" "$DEST"
          fix "$DEST"
        fi
        ;;
    esac
    ;;
esac
cd /
rm -rf "$work"
"#;

/// A `COPY`: the uploaded archive (gunzipped here when it is gzip) goes into the sandbox, and the
/// copy script places what it holds.
#[allow(clippy::too_many_arguments)]
fn copy(
    app: &App,
    daemon: &Daemon,
    build_id: &str,
    hash: &str,
    src: &str,
    dest: &str,
    owner: &str,
    mode: Option<&str>,
    cursor: &Cursor,
) -> Result<(), String> {
    let archive = files_dir(app).join(hash);
    let plain = files_dir(app).join(format!("{build_id}-{hash}.tar"));
    let unpacked = unpack(&archive, &plain).map_err(|e| format!("the upload {hash}: {e}"));
    let staged = format!("{COPY_STAGING}/{hash}.tar");
    let sent = unpacked.and_then(|path| {
        let mut file = File::open(&path).map_err(|e| e.to_string())?;
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        daemon.put_file(&staged, &mut file, length)
    });
    let _ = std::fs::remove_file(&plain);
    sent?;
    let mode = mode
        .map(|m| m.trim_start_matches('0'))
        .map(|m| if m.is_empty() { "0" } else { m });
    let env = BTreeMap::from([
        ("ARCHIVE".to_string(), staged),
        ("SRC".to_string(), src.to_string()),
        ("DEST".to_string(), absolute(&cursor.workdir, dest)),
        ("IDS".to_string(), ids_of(owner).to_string()),
        ("MODE".to_string(), mode.unwrap_or_default().to_string()),
    ]);
    let code = daemon.exec(COPY_SCRIPT, "root", "/", &env, STEP_TIMEOUT, |line| {
        log(build_id, "info", line)
    })?;
    if code == 0 {
        Ok(())
    } else {
        Err(format!("the copy exited with {code}"))
    }
}

/// The archive at `archive` as a plain tar: itself, or gunzipped to `plain` when it is gzip.
fn unpack(archive: &Path, plain: &Path) -> io::Result<PathBuf> {
    let mut file = File::open(archive)?;
    let mut magic = [0u8; 2];
    let read = file.read(&mut magic)?;
    if read < 2 || magic != [0x1f, 0x8b] {
        return Ok(archive.to_path_buf());
    }
    let mut decoder = flate2::read::GzDecoder::new(File::open(archive)?);
    let mut out = File::create(plain)?;
    io::copy(&mut decoder, &mut out)?;
    out.flush()?;
    Ok(plain.to_path_buf())
}

/// A sandbox's daemon, reached through vsockd's E2B gateway.
struct Daemon {
    id: String,
    token: Option<String>,
}

/// An answer's body: chunked, of a known length, or up to the close.
enum Body {
    Chunked {
        reader: BufReader<TcpStream>,
        left: u64,
        done: bool,
    },
    Length(io::Take<BufReader<TcpStream>>),
    Close(BufReader<TcpStream>),
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Body::Length(reader) => reader.read(buf),
            Body::Close(reader) => reader.read(buf),
            Body::Chunked { reader, left, done } => {
                if *done || buf.is_empty() {
                    return Ok(0);
                }
                if *left == 0 {
                    let mut line = String::new();
                    reader.by_ref().take(1024).read_line(&mut line)?;
                    let size = line.trim().split(';').next().unwrap_or_default().trim();
                    let size = u64::from_str_radix(size, 16).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "a bad chunk size")
                    })?;
                    if size == 0 {
                        *done = true;
                        return Ok(0);
                    }
                    *left = size;
                }
                let want = buf.len().min(usize::try_from(*left).unwrap_or(usize::MAX));
                let read = reader.read(&mut buf[..want])?;
                if read == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                *left -= read as u64;
                if *left == 0 {
                    let mut crlf = [0u8; 2];
                    reader.read_exact(&mut crlf)?;
                }
                Ok(read)
            }
        }
    }
}

struct Answer {
    status: u16,
    body: Body,
    stream: TcpStream,
}

impl Daemon {
    fn of(sandbox: &Sandbox) -> Daemon {
        Daemon {
            id: sandbox.id.clone(),
            token: sandbox.access_token.clone(),
        }
    }

    /// Sends one request (its body `length` bytes from `body`) and reads the answer's head.
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &mut dyn Read,
        length: u64,
        timeout: Duration,
    ) -> io::Result<Answer> {
        let addr = SocketAddr::from(([127, 0, 0, 1], e2b::gateway_port()));
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_read_timeout(Some(timeout))?;
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nE2b-Sandbox-Id: {}\r\nContent-Length: {length}\r\nConnection: close\r\n",
            self.id
        );
        if let Some(token) = &self.token {
            head.push_str(&format!("X-Access-Token: {token}\r\n"));
        }
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        let copied = io::copy(&mut body.take(length), &mut stream)?;
        if copied != length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the body ended early",
            ));
        }
        stream.flush()?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut status_line = String::new();
        reader.by_ref().take(1024).read_line(&mut status_line)?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no HTTP status"))?;
        let (mut chunked, mut content_length) = (false, None);
        let mut head_bytes = status_line.len();
        loop {
            let mut line = String::new();
            reader.by_ref().take(8192).read_line(&mut line)?;
            head_bytes += line.len();
            if line.is_empty() || head_bytes > 64 << 10 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a bad answer head",
                ));
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                let value = value.trim();
                if name.eq_ignore_ascii_case("transfer-encoding") {
                    chunked = value.eq_ignore_ascii_case("chunked");
                } else if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse::<u64>().ok();
                }
            }
        }
        let body = if chunked {
            Body::Chunked {
                reader,
                left: 0,
                done: false,
            }
        } else if let Some(length) = content_length {
            Body::Length(reader.take(length))
        } else {
            Body::Close(reader)
        };
        Ok(Answer {
            status,
            body,
            stream,
        })
    }

    /// Writes `length` bytes from `body` to `path` in the sandbox, as root.
    fn put_file(&self, path: &str, body: &mut dyn Read, length: u64) -> Result<(), String> {
        let target = format!("/files?path={}&username=root", query_escape(path));
        let mut answer = self
            .request(
                "POST",
                &target,
                &[("Content-Type", "application/octet-stream")],
                body,
                length,
                STEP_TIMEOUT,
            )
            .map_err(|e| format!("the upload into the sandbox: {e}"))?;
        if (200..300).contains(&answer.status) {
            Ok(())
        } else {
            let status = answer.status;
            Err(format!(
                "the sandbox refused the upload: {status} {}",
                answer.text()
            ))
        }
    }

    /// Runs `cmd` with `/bin/bash -l -c` as `user` in `cwd`, handing each line of its output to
    /// `line`, and returns its exit code.
    fn exec(
        &self,
        cmd: &str,
        user: &str,
        cwd: &str,
        env: &BTreeMap<String, String>,
        timeout: Duration,
        mut line: impl FnMut(&str),
    ) -> Result<i64, String> {
        let message = json!({
            "process": {"cmd": "/bin/bash", "args": ["-l", "-c", cmd], "envs": env, "cwd": cwd},
        });
        let payload = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        let mut envelope = Vec::with_capacity(payload.len() + 5);
        envelope.push(0);
        envelope.extend_from_slice(
            &u32::try_from(payload.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        envelope.extend_from_slice(&payload);
        let auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:"))
        );
        let deadline = Instant::now() + timeout;
        let mut answer = self
            .request(
                "POST",
                "/process.Process/Start",
                &[
                    ("Content-Type", "application/connect+json"),
                    ("Connect-Protocol-Version", "1"),
                    ("Authorization", &auth),
                ],
                &mut envelope.as_slice(),
                envelope.len() as u64,
                timeout,
            )
            .map_err(|e| format!("the sandbox's daemon: {e}"))?;
        if answer.status != 200 {
            let status = answer.status;
            return Err(format!(
                "the sandbox's daemon answered {status}: {}",
                answer.text()
            ));
        }
        let mut output = Output::default();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            let _ = answer.stream.set_read_timeout(Some(left));
            let (flags, payload) = match next_envelope(&mut answer.body) {
                Ok(Some(envelope)) => envelope,
                Ok(None) => return Err("the process stream ended without the process's end".into()),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(format!("timed out after {}s", timeout.as_secs()));
                }
                Err(e) => return Err(format!("the process stream: {e}")),
            };
            let event: Value =
                serde_json::from_slice(&payload).map_err(|e| format!("the process stream: {e}"))?;
            if flags & 0x02 != 0 {
                return Err(match event.get("error") {
                    Some(error) => format!(
                        "{}: {}",
                        error["code"].as_str().unwrap_or("error"),
                        error["message"].as_str().unwrap_or_default()
                    ),
                    None => "the process stream ended without the process's end".into(),
                });
            }
            if let Some(data) = event.pointer("/event/data") {
                for kind in ["stdout", "stderr", "pty"] {
                    if let Some(encoded) = data.get(kind).and_then(Value::as_str) {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(encoded)
                            .unwrap_or_default();
                        output.push(kind, &bytes, &mut line);
                    }
                }
            }
            if let Some(end) = event.pointer("/event/end") {
                output.flush(&mut line);
                return Ok(end["exitCode"].as_i64().unwrap_or(-1));
            }
        }
    }
}

impl Answer {
    /// The answer's body as text, at most [`MAX_ANSWER`] bytes of it.
    fn text(&mut self) -> String {
        let mut bytes = Vec::new();
        let _ = (&mut self.body)
            .take(MAX_ANSWER as u64)
            .read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).trim().to_string()
    }
}

/// One Connect envelope: its flags and payload; `None` at the stream's end.
fn next_envelope(body: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 5];
    let mut got = 0;
    while got < head.len() {
        let read = body.read(&mut head[got..])?;
        if read == 0 {
            if got == 0 {
                return Ok(None);
            }
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        got += read;
    }
    let length = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if length > MAX_ENVELOPE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "an envelope is too large",
        ));
    }
    let mut payload = vec![0u8; length];
    body.read_exact(&mut payload)?;
    Ok(Some((head[0], payload)))
}

/// A process's output, cut into lines per stream.
#[derive(Default)]
struct Output {
    partial: BTreeMap<&'static str, Vec<u8>>,
}

impl Output {
    fn push(&mut self, kind: &'static str, bytes: &[u8], line: &mut impl FnMut(&str)) {
        let buffer = self.partial.entry(kind).or_default();
        buffer.extend_from_slice(bytes);
        while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            let text: Vec<u8> = buffer.drain(..=end).collect();
            line(String::from_utf8_lossy(&text).trim_end_matches(['\n', '\r']));
        }
        if buffer.len() > MAX_LOG_LINE {
            let text: Vec<u8> = std::mem::take(buffer);
            line(&String::from_utf8_lossy(&text));
        }
    }

    fn flush(&mut self, line: &mut impl FnMut(&str)) {
        for (_, buffer) in std::mem::take(&mut self.partial) {
            if !buffer.is_empty() {
                line(String::from_utf8_lossy(&buffer).trim_end_matches(['\n', '\r']));
            }
        }
    }
}

fn query_escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn query_number(req: &Request, name: &str) -> Option<usize> {
    req.query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.parse().ok()).flatten()
    })
}

/// `GET /e2b/templates/{id}/builds/{build}/status`: the build's status and its log from `logsOffset`.
fn status(app: &App, req: &Request, id: &str, build_id: &str) -> Result<Response, Refused> {
    let template = find_template(app, id)?;
    let build = template
        .build(build_id)
        .ok_or_else(|| build_not_found(build_id))?;
    let offset = query_number(req, "logsOffset").unwrap_or(0);
    let limit = query_number(req, "limit")
        .unwrap_or(STATUS_PAGE)
        .clamp(1, STATUS_PAGE);
    let lines: Vec<LogLine> = e2b::lock(&LOGS)
        .get(build_id)
        .map(|lines| lines.iter().skip(offset).take(limit).cloned().collect())
        .unwrap_or_default();
    let entries: Vec<Value> = lines
        .iter()
        .map(|l| json!({"timestamp": e2b::rfc3339(l.ms), "level": l.level, "message": l.message}))
        .collect();
    let mut view = json!({
        "templateID": template.id,
        "buildID": build.id,
        "status": build.status.as_str(),
        "logs": lines.iter().map(|l| l.message.clone()).collect::<Vec<String>>(),
        "logEntries": entries,
    });
    if let Some(reason) = &build.reason {
        view["reason"] = json!({"message": reason.message, "step": reason.step});
    }
    Ok(Response::json(200, &view))
}

/// `GET /e2b/templates/aliases/{name}`.
fn alias(app: &App, name: &str) -> Result<Response, Refused> {
    let template = templates(app)
        .list()
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| template_not_found(name))?;
    Ok(Response::json(
        200,
        &json!({"templateID": template.id, "public": false}),
    ))
}

/// `GET /e2b/templates/{id}/tags`.
fn tags(app: &App, id: &str) -> Result<Response, Refused> {
    let (name, _) = split_reference(id);
    let template = find_template(app, name)?;
    let rows: Vec<Value> = template
        .builds
        .iter()
        .flat_map(|b| {
            b.tags.iter().map(|t| {
                json!({"tag": t.tag, "buildID": b.id, "createdAt": e2b::rfc3339(t.assigned_ms)})
            })
        })
        .collect();
    Ok(Response::json(200, &rows))
}

#[derive(Debug, Deserialize)]
struct AssignBody {
    target: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RemoveBody {
    name: String,
    tags: Vec<String>,
}

/// `POST /e2b/templates/tags`: the tags move to the build `target` (`name:tag`) names.
fn assign_tags(app: &App, req: &Request) -> Result<Response, Refused> {
    let body: AssignBody = e2b::json_body(req)?;
    for tag in &body.tags {
        check_tag(tag)?;
    }
    if body.tags.is_empty() || body.tags.len() > MAX_TAGS {
        return Err(e2b::invalid(format!("name 1 to {MAX_TAGS} tags")));
    }
    let (name, tag) = split_reference(&body.target);
    let now = apz_plaza::now_ms();
    let build_id = templates(app).edit(|set| {
        let template = set
            .iter_mut()
            .find(|t| t.id == name || t.name == name)
            .ok_or_else(|| template_not_found(name))?;
        let build_id = template.pick(tag).map(|b| b.id.clone()).ok_or_else(|| {
            Refused::new(
                404,
                "template_build_not_found",
                format!("{} names no ready build", body.target),
            )
        })?;
        template.updated_ms = now;
        for build in template.builds.iter_mut() {
            build.tags.retain(|t| !body.tags.contains(&t.tag));
            if build.id == build_id {
                build.tags.extend(body.tags.iter().map(|tag| Tag {
                    tag: tag.clone(),
                    assigned_ms: now,
                }));
            }
        }
        Ok(build_id)
    })?;
    Ok(Response::json(
        201,
        &json!({"buildID": build_id, "tags": body.tags}),
    ))
}

/// `DELETE /e2b/templates/tags`: the tags leave the template's builds (the builds stay).
fn remove_tags(app: &App, req: &Request) -> Result<Response, Refused> {
    let body: RemoveBody = e2b::json_body(req)?;
    let (name, _) = split_reference(&body.name);
    let wanted: BTreeSet<&str> = body.tags.iter().map(String::as_str).collect();
    templates(app).edit(|set| {
        let template = set
            .iter_mut()
            .find(|t| t.id == name || t.name == name)
            .ok_or_else(|| template_not_found(name))?;
        for build in template.builds.iter_mut() {
            build.tags.retain(|t| !wanted.contains(t.tag.as_str()));
        }
        Ok(())
    })?;
    Ok(Response::new(204, apz_http::JSON, Vec::<u8>::new()))
}

/// Deletes the template `id` (its id or its name) with its builds' snapshots; `None` when no
/// template has it, so the caller may take it for a snapshot.
pub(crate) fn delete(app: &App, id: &str, key: &str) -> Option<Result<Response, Refused>> {
    let template = templates(app)
        .list()
        .into_iter()
        .find(|t| t.id == id || t.name == id)?;
    if template.builds.iter().any(|b| b.status == Status::Building) {
        return Some(Err(Refused::new(
            409,
            "template_build_running",
            format!("template {id} has a build running"),
        )));
    }
    for snapshot in template.builds.iter().filter_map(|b| b.snapshot.as_deref()) {
        if e2b::snapshot_source(app, snapshot).is_some()
            && let Err(refused) = e2b::delete_snapshot(app, snapshot, key)
        {
            return Some(Err(refused));
        }
    }
    let removed = templates(app).edit(|set| {
        set.retain(|t| t.id != template.id);
        Ok(())
    });
    if let Err(refused) = removed {
        return Some(Err(refused));
    }
    let mut logs = e2b::lock(&LOGS);
    for build in &template.builds {
        logs.remove(&build.id);
    }
    Some(Ok(Response::new(204, apz_http::JSON, Vec::<u8>::new())))
}

/// The template routes (listed in the module docs). They go before `crate::e2b`'s, whose
/// `/e2b/templates/{id}` would otherwise take `tags` for an id.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/e2b/v3/templates", |app, req, _| {
            e2b::guard(app, req, |_| request_build(app, req))
        })
        .get("/e2b/templates/aliases/{name}", |app, req, p| {
            e2b::guard(app, req, |_| alias(app, p.get("name")))
        })
        .post("/e2b/templates/tags", |app, req, _| {
            e2b::guard(app, req, |_| assign_tags(app, req))
        })
        .delete("/e2b/templates/tags", |app, req, _| {
            e2b::guard(app, req, |_| remove_tags(app, req))
        })
        .get("/e2b/templates/{id}/tags", |app, req, p| {
            e2b::guard(app, req, |_| tags(app, p.get("id")))
        })
        .get("/e2b/templates/{id}/files/{hash}", |app, req, p| {
            e2b::guard(app, req, |_| {
                file_link(app, req, p.get("id"), p.get("hash"))
            })
        })
        // The upload link: no API key, the one-time token is its authority.
        .route("PUT", "/e2b/templates/{id}/files/{hash}", |app, req, p| {
            upload(app, req, p.get("id"), p.get("hash")).unwrap_or_else(e2b::refuse)
        })
        .post("/e2b/v2/templates/{id}/builds/{build}", |app, req, p| {
            e2b::guard(app, req, |key| {
                trigger(app, req, p.get("id"), p.get("build"), key)
            })
        })
        .get(
            "/e2b/templates/{id}/builds/{build}/status",
            |app, req, p| e2b::guard(app, req, |_| status(app, req, p.get("id"), p.get("build"))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(id: &str, status: Status, tags: &[&str], created_ms: u64) -> Build {
        Build {
            id: id.into(),
            status,
            created_ms,
            finished_ms: None,
            vcpus: 1,
            memory_mib: 512,
            wanted_tags: Vec::new(),
            tags: tags
                .iter()
                .map(|t| Tag {
                    tag: t.to_string(),
                    assigned_ms: 0,
                })
                .collect(),
            snapshot: Some(format!("sb:{id}")),
            reason: None,
            user: build_user(),
            workdir: build_home(),
        }
    }

    #[test]
    fn a_reference_picks_its_tag_then_default_then_the_newest_ready_build() {
        let template = Template {
            id: "t".into(),
            name: "env".into(),
            created_ms: 0,
            updated_ms: 0,
            builds: vec![
                build("old", Status::Ready, &["v1"], 1),
                build("new", Status::Ready, &[], 3),
                build("broken", Status::Error, &["v2"], 4),
                build("def", Status::Ready, &["default"], 2),
            ],
        };
        assert_eq!(
            template.pick(Some("v1")).map(|b| b.id.as_str()),
            Some("old")
        );
        assert_eq!(template.pick(None).map(|b| b.id.as_str()), Some("def"));
        assert!(
            template.pick(Some("v2")).is_none(),
            "a failed build is never picked"
        );
        let mut untagged = template.clone();
        untagged.builds.retain(|b| b.id != "def");
        assert_eq!(untagged.pick(None).map(|b| b.id.as_str()), Some("new"));
        assert_eq!(split_reference("team/env:v1"), ("team/env", Some("v1")));
        assert_eq!(split_reference("env"), ("env", None));
    }

    #[test]
    fn names_tags_and_hashes_are_checked() {
        assert!(check_name("my-env").is_ok());
        assert!(check_name("team/my_env.2").is_ok());
        for bad in ["", "base", "-x", "a b", "x:y", &"a".repeat(129)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(check_tag("v1.0-rc_1").is_ok());
        assert!(check_tag("a/b").is_err());
        assert!(check_hash(&"ab".repeat(32)).is_ok());
        assert!(check_hash("abc").is_err());
        assert!(check_hash(&"zz".repeat(32)).is_err());
        let id = uuid_v4();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
    }

    #[test]
    fn output_is_cut_into_lines_per_stream() {
        let mut out = Output::default();
        let mut lines = Vec::new();
        let mut take = |line: &str| lines.push(line.to_string());
        out.push("stdout", b"one\ntw", &mut take);
        out.push("stderr", b"err\r\n", &mut take);
        out.push("stdout", b"o\nthree", &mut take);
        out.flush(&mut take);
        assert_eq!(lines, ["one", "err", "two", "three"]);
    }

    #[test]
    fn chunked_bodies_and_envelopes_are_read() {
        let mut first = vec![0u8];
        first.extend_from_slice(&4u32.to_be_bytes());
        first.extend_from_slice(b"{\"a\"");
        let mut chunked = Vec::new();
        chunked.extend_from_slice(format!("{:x}\r\n", first.len()).as_bytes());
        chunked.extend_from_slice(&first);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let writer = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.write_all(&chunked).unwrap();
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut body = Body::Chunked {
            reader: BufReader::new(stream),
            left: 0,
            done: false,
        };
        let (flags, payload) = next_envelope(&mut body).unwrap().unwrap();
        assert_eq!((flags, payload.as_slice()), (0, b"{\"a\"".as_slice()));
        assert!(next_envelope(&mut body).unwrap().is_none());
        writer.join().unwrap();
    }

    #[test]
    fn steps_are_checked_before_a_build_starts() {
        let step = |kind: &str, args: &[&str]| StepBody {
            kind: kind.into(),
            args: Some(args.iter().map(|a| a.to_string()).collect()),
            files_hash: None,
        };
        let dir = std::env::temp_dir().join(format!("apid-e2b-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ok = check_steps(
            &dir,
            &[
                step("RUN", &["echo hi", "root"]),
                step("ENV", &["A", "1", "B", "two"]),
                step("WORKDIR", &["/app"]),
                step("USER", &["root"]),
            ],
        )
        .unwrap();
        assert_eq!(
            ok[1],
            Step::Env(vec![("A".into(), "1".into()), ("B".into(), "two".into())])
        );
        for bad in [
            step("RUN", &[]),
            step("RUN", &["x", "nobody"]),
            step("ENV", &["A"]),
            step("ENV", &["1A", "x"]),
            step("USER", &["admin"]),
            step("COPY", &["a", "/b"]),
            step("ADD", &["a", "/b"]),
        ] {
            assert!(
                check_steps(&dir, std::slice::from_ref(&bad)).is_err(),
                "{bad:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_targets_resolve_against_the_working_directory() {
        assert_eq!(absolute("/home/user", "app"), "/home/user/app");
        assert_eq!(absolute("/home/user/", "app/"), "/home/user/app/");
        assert_eq!(absolute("/w", "/abs"), "/abs");
        assert_eq!(query_escape("/tmp/a b.tar"), "/tmp/a%20b.tar");
    }
}
