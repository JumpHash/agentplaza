//! The hub's E2B-compatible sandbox API: the E2B SDKs (`E2B_API_URL=http://<hub>/e2b`,
//! `E2B_API_KEY=e2b_…`) create and drive plaza sandboxes. A sandbox is one noded run with a sandbox VM
//! (`sandbox: true`, which runs the E2B-envd-compatible daemon). apid keeps its record, its end time,
//! its envd access token and its snapshots; the laws, the egress rules, STOP ALL and the transcript
//! apply to it as they do to an agent.
//!
//! Auth: every `/e2b` route takes an E2B key, `X-API-Key: e2b_…` or `Authorization: Bearer e2b_…`.
//! The owner token alone is not a key. The owner makes keys (`/v1/e2b/keys`, owner-gated by
//! `hub_auth::gate`); the hub keeps only each key's SHA-256, in `<state>/e2b-keys.json` (mode 0600).
//!
//! POST   /v1/e2b/keys {label}                      → {id, label, key, created_ms}  (the key, once)
//! GET    /v1/e2b/keys                              → {keys: [{id, label, created_ms}]}
//! GET    /v1/e2b/sandboxes                         → {sandboxes: [ListedSandbox + cid, run]}  (owner)
//! DELETE /v1/e2b/sandboxes/{id} · POST …/{id}/pause · POST …/{id}/resume       (owner)
//! DELETE /v1/e2b/keys/{id}                         → {ok}
//!
//! POST   /e2b/sandboxes        NewSandbox          → 201 Sandbox
//! GET    /e2b/sandboxes                            → [ListedSandbox] (the running ones)
//! GET    /e2b/v2/sandboxes?state=running,paused    → [ListedSandbox]
//! GET    /e2b/sandboxes/{id}                       → SandboxDetail
//! DELETE /e2b/sandboxes/{id}                       → 204
//! POST   /e2b/sandboxes/{id}/timeout {timeout}     → 204
//! POST   /e2b/sandboxes/{id}/refreshes {duration}  → 204
//! POST   /e2b/sandboxes/{id}/pause                 → 204
//! POST   /e2b/sandboxes/{id}/resume {timeout?}     → 201 Sandbox
//! POST   /e2b/sandboxes/{id}/connect {timeout}     → 201 Sandbox (woken) or 200 (already running)
//! POST   /e2b/sandboxes/{id}/snapshots {name?}     → 201 SnapshotInfo
//! GET    /e2b/snapshots                            → [SnapshotInfo]
//!
//! Errors are E2B's `{code, message}`, with apid's machine-readable `error_code` beside them.
//!
//! The reaper (every 5 s) ends a running sandbox past its end time, or pauses it when it auto-pauses.
//! While the plaza is held (STOP ALL) it ends every sandbox, and creates, wakes and connects refuse
//! with 409. vsockd's gateway reads a sandbox's CID and envd token from
//! `GET /internal/e2b/sandboxes/{id}` (see `internal`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use apz_http::{Request, Response, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::internal::{EgressVerdict, egress_verdict};
use crate::state::App;

const KEYS_FILE: &str = "e2b-keys.json";
const SANDBOXES_FILE: &str = "e2b-sandboxes.json";
/// Every E2B key starts with this, then forty hex digits.
const KEY_PREFIX: &str = "e2b_";
const KEY_LABEL_MAX: usize = 80;
/// The port of vsockd's E2B gateway (`vsockd --e2b-port`) unless `--e2b-gateway-port` says otherwise.
pub const DEFAULT_GATEWAY_PORT: u16 = 49_982;
const REAP_EVERY: Duration = Duration::from_secs(5);
/// A new sandbox's time to live when the request names none.
const DEFAULT_TIMEOUT_S: u32 = 300;
/// The longest time to live, and the longest a sandbox may be kept alive by one request.
const MAX_TIMEOUT_S: u32 = 86_400;
const MAX_REFRESH_S: u32 = 3_600;
const DEFAULT_REFRESH_S: u32 = 300;
/// The envd version sandboxes report. The SDKs turn features on by it: 0.3.0 command stdin, 0.4.0 the
/// daemon's default user (no `username` sent), 0.5.2 `CloseStdin`, 0.5.7 raw octet-stream uploads.
/// The daemon serves all of them; 0.6.2 (file metadata in entries) and later it does not.
const ENVD_VERSION: &str = "0.5.7";
const CLIENT_ID: &str = "agentplaza";
const SANDBOX_VCPUS: u32 = 1;
const SANDBOX_MEMORY_MIB: u32 = 512;
/// Starting a run boots a VM, so it gets more time than a plain noded call.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);
const NODED_TIMEOUT: Duration = Duration::from_secs(60);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a new sandbox's daemon has to answer `/health` through the gateway.
const HEALTH_WAIT: Duration = Duration::from_secs(15);
const GATEWAY_CALL: Duration = Duration::from_secs(10);

static GATEWAY_PORT: OnceLock<u16> = OnceLock::new();
static KEYS: OnceLock<Keys> = OnceLock::new();
static SANDBOXES: OnceLock<Sandboxes> = OnceLock::new();

/// Takes `--e2b-gateway-port PORT` out of the command line, which apid reads before its own flags.
pub fn take_gateway_port(args: Vec<String>) -> Result<Vec<String>, String> {
    let (port, rest) = split_gateway_port(args)?;
    if let Some(port) = port {
        let _ = GATEWAY_PORT.set(port);
    }
    Ok(rest)
}

/// The flag's port (if given) and the other arguments, in order.
fn split_gateway_port(args: Vec<String>) -> Result<(Option<u16>, Vec<String>), String> {
    let mut port = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--e2b-gateway-port" {
            let value = args
                .next()
                .ok_or_else(|| "--e2b-gateway-port needs a value".to_string())?;
            port = Some(
                value
                    .parse::<u16>()
                    .map_err(|e| format!("--e2b-gateway-port: {e}"))?,
            );
        } else {
            rest.push(arg);
        }
    }
    Ok((port, rest))
}

fn gateway_port() -> u16 {
    GATEWAY_PORT.get().copied().unwrap_or(DEFAULT_GATEWAY_PORT)
}

/// A refusal: the HTTP status, apid's machine-readable `error_code`, and the message.
#[derive(Debug)]
pub(crate) struct Refused {
    status: u16,
    code: &'static str,
    message: String,
}

impl Refused {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

fn invalid(message: impl Into<String>) -> Refused {
    Refused::new(400, "invalid_request", message)
}

fn held() -> Refused {
    Refused::new(
        409,
        "sandbox_held",
        "the plaza is held by STOP ALL; sandboxes start again after resume",
    )
}

/// E2B's error body: `{code, message, error_code}`.
fn refuse(refused: Refused) -> Response {
    Response::json(
        refused.status,
        &json!({
            "code": refused.status,
            "message": refused.message,
            "error_code": refused.code,
        }),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes a store's list to its file (0600). A store whose file could not be read writes nothing, so a
/// damaged file is never replaced by a shorter list.
fn persist<T: Serialize>(path: &Path, unreadable: Option<&str>, list: &[T]) -> Result<(), Refused> {
    if let Some(why) = unreadable {
        return Err(Refused::new(
            409,
            "store_unreadable",
            format!(
                "{} is unreadable; fix or remove it first ({why})",
                path.display()
            ),
        ));
    }
    let bytes = serde_json::to_vec_pretty(list)
        .map_err(|e| Refused::new(500, "internal_server_error", e.to_string()))?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    apz_os::private::write(path, &bytes).map_err(|e| {
        Refused::new(
            500,
            "internal_server_error",
            format!("{}: {e}", path.display()),
        )
    })
}

/// Reads a store's JSON list. A missing file is an empty list; a damaged one is reported and kept
/// unwritten (see [`persist`]).
fn read_list<T: DeserializeOwned>(path: &Path) -> (Vec<T>, Option<String>) {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(list) => (list, None),
            Err(e) => (Vec::new(), Some(format!("{}: {e}", path.display()))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(e) => (Vec::new(), Some(format!("{}: {e}", path.display()))),
    }
}

/// The hash the store keeps for a key: SHA-256, hex.
pub fn key_digest(key: &str) -> String {
    hex_of(&apz_trust::sha256(key.as_bytes()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredKey {
    id: String,
    label: String,
    sha256: String,
    created_ms: u64,
}

/// A key as the owner sees it: never the key itself.
#[derive(Debug, Clone, Serialize)]
pub struct KeyView {
    pub id: String,
    pub label: String,
    pub created_ms: u64,
}

fn key_view(stored: &StoredKey) -> KeyView {
    KeyView {
        id: stored.id.clone(),
        label: stored.label.clone(),
        created_ms: stored.created_ms,
    }
}

/// The owner's E2B keys, saved to `<state>/e2b-keys.json` on every change.
pub struct Keys {
    path: PathBuf,
    unreadable: Option<String>,
    set: Mutex<Vec<StoredKey>>,
}

impl Keys {
    pub fn load(state_dir: &Path) -> Keys {
        let path = state_dir.join(KEYS_FILE);
        let (keys, unreadable) = read_list::<StoredKey>(&path);
        if let Some(why) = &unreadable {
            eprintln!("apid: the E2B keys are unreadable, so none works ({why})");
        }
        Keys {
            path,
            unreadable,
            set: Mutex::new(keys),
        }
    }

    pub fn list(&self) -> Vec<KeyView> {
        lock(&self.set).iter().map(key_view).collect()
    }

    /// A new key: its view and the key itself, which is shown once and never stored.
    pub fn create(&self, label: &str, now: u64) -> Result<(KeyView, String), Refused> {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > KEY_LABEL_MAX {
            return Err(invalid(format!(
                "label must be 1 to {KEY_LABEL_MAX} characters"
            )));
        }
        let key = format!("{KEY_PREFIX}{}", hex_of(&apz_trust::random::<20>()));
        let stored = StoredKey {
            id: format!("e2k-{}", hex_of(&apz_trust::random::<6>())),
            label: label.to_string(),
            sha256: key_digest(&key),
            created_ms: now,
        };
        let mut set = lock(&self.set);
        let mut next = set.clone();
        next.push(stored.clone());
        persist(&self.path, self.unreadable.as_deref(), &next)?;
        *set = next;
        Ok((key_view(&stored), key))
    }

    pub fn remove(&self, id: &str) -> Result<(), Refused> {
        let mut set = lock(&self.set);
        let mut next = set.clone();
        let before = next.len();
        next.retain(|stored| stored.id != id);
        if next.len() == before {
            return Err(Refused::new(
                404,
                "key_not_found",
                format!("no E2B key {id}"),
            ));
        }
        persist(&self.path, self.unreadable.as_deref(), &next)?;
        *set = next;
        Ok(())
    }

    /// The id of the key `presented` is, if it is one of the owner's. The owner token is not a key.
    pub fn verify(&self, presented: &str) -> Option<String> {
        if self.unreadable.is_some() || !presented.starts_with(KEY_PREFIX) {
            return None;
        }
        let digest = key_digest(presented);
        lock(&self.set)
            .iter()
            .find(|stored| apz_trust::ct_eq(stored.sha256.as_bytes(), digest.as_bytes()))
            .map(|stored| stored.id.clone())
    }
}

/// Where a sandbox is: its VM running, or asleep (a snapshot of it kept by noded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Running,
    Paused,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Running => "running",
            Phase::Paused => "paused",
        }
    }
}

/// A snapshot the sandbox was asked for: its E2B id (`<sandbox>:<checkpoint>`), the name the caller
/// gave, and noded's checkpoint name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub checkpoint: String,
    pub taken_ms: u64,
}

/// One sandbox. Its envd access token is kept here because vsockd needs it after a hub restart; the
/// file is mode 0600. Environment variables are kept too, so a cold resume can hand them to a fresh
/// daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sandbox {
    /// `sb` and sixteen hex digits.
    pub id: String,
    /// The base image named in the request (`base` or any other template name).
    pub template: String,
    /// noded's run id.
    pub run: String,
    /// The sandbox VM's guest CID.
    pub cid: u64,
    pub started_ms: u64,
    pub end_ms: u64,
    pub phase: Phase,
    pub auto_pause: bool,
    pub allow_internet: bool,
    pub metadata: BTreeMap<String, String>,
    /// The environment variables the sandbox was created with; a cold resume gives them to the new daemon.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_vars: BTreeMap<String, String>,
    /// Paused without memory: it resumes by booting from disk, not from a memory image.
    #[serde(default)]
    pub cold: bool,
    /// The envd access token (`secure` sandboxes only).
    pub access_token: Option<String>,
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
    /// Killed while it had snapshots: its VM is stopped and the record stays only for them, hidden
    /// from every sandbox route. Deleting its last snapshot ends its run and forgets it.
    #[serde(default)]
    pub retired: bool,
    /// The network rules (`allowOut`, `denyOut`; see `crate::e2b_net`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_out: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_out: Vec<String>,
}

/// The sandboxes, saved to `<state>/e2b-sandboxes.json` on every change.
pub struct Sandboxes {
    path: PathBuf,
    unreadable: Option<String>,
    set: Mutex<Vec<Sandbox>>,
}

impl Sandboxes {
    pub fn load(state_dir: &Path) -> Sandboxes {
        let path = state_dir.join(SANDBOXES_FILE);
        let (sandboxes, unreadable) = read_list::<Sandbox>(&path);
        if let Some(why) = &unreadable {
            eprintln!("apid: the E2B sandboxes are unreadable; none is reaped ({why})");
        }
        Sandboxes {
            path,
            unreadable,
            set: Mutex::new(sandboxes),
        }
    }

    pub fn list(&self) -> Vec<Sandbox> {
        lock(&self.set).clone()
    }

    pub fn get(&self, id: &str) -> Option<Sandbox> {
        lock(&self.set).iter().find(|s| s.id == id).cloned()
    }

    pub fn by_cid(&self, cid: u64) -> Option<Sandbox> {
        lock(&self.set).iter().find(|s| s.cid == cid).cloned()
    }

    pub fn insert(&self, sandbox: Sandbox) -> Result<(), Refused> {
        let mut set = lock(&self.set);
        let mut next = set.clone();
        next.push(sandbox);
        persist(&self.path, self.unreadable.as_deref(), &next)?;
        *set = next;
        Ok(())
    }

    /// Changes sandbox `id` and saves; returns it as changed.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Sandbox)) -> Result<Sandbox, Refused> {
        let mut set = lock(&self.set);
        let mut next = set.clone();
        let Some(sandbox) = next.iter_mut().find(|s| s.id == id) else {
            return Err(not_found(id));
        };
        change(&mut *sandbox);
        let updated = sandbox.clone();
        persist(&self.path, self.unreadable.as_deref(), &next)?;
        *set = next;
        Ok(updated)
    }

    /// Forgets sandbox `id` (a no-op when it is gone).
    pub fn remove(&self, id: &str) -> Result<(), Refused> {
        let mut set = lock(&self.set);
        let mut next = set.clone();
        next.retain(|s| s.id != id);
        if next.len() == set.len() {
            return Ok(());
        }
        persist(&self.path, self.unreadable.as_deref(), &next)?;
        *set = next;
        Ok(())
    }
}

fn not_found(id: &str) -> Refused {
    Refused::new(404, "sandbox_not_found", format!("no sandbox {id}"))
}

/// The hub's keys, loaded from the state dir on first use and kept for the process.
fn keys(app: &App) -> &'static Keys {
    KEYS.get_or_init(|| Keys::load(&app.config.state_dir))
}

/// The hub's sandboxes, loaded on first use. The first use also starts the reaper, which holds the app
/// weakly and ends when the app is gone.
fn sandboxes(app: &App) -> &'static Sandboxes {
    SANDBOXES.get_or_init(|| {
        let store = Sandboxes::load(&app.config.state_dir);
        spawn_reaper(app.handle());
        store
    })
}

/// Loads the hub's sandboxes at start, which starts the reaper: sandboxes persisted by an earlier run
/// end on time even before the first `/e2b` request.
pub(crate) fn start(app: &App) {
    let _ = sandboxes(app);
}

fn spawn_reaper(app: Weak<App>) {
    let spawned = std::thread::Builder::new()
        .name("e2b-reaper".into())
        .spawn(move || {
            loop {
                std::thread::sleep(REAP_EVERY);
                let Some(strong) = app.upgrade() else {
                    return;
                };
                reap(&strong, apz_plaza::now_ms());
            }
        });
    if let Err(err) = spawned {
        eprintln!("apid: the E2B reaper did not start: {err}");
    }
}

/// What the reaper does with one sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reap {
    Keep,
    Kill,
    Pause,
}

/// The reaper's decision. While the plaza is held every sandbox ends (STOP ALL). Otherwise a running
/// sandbox past its end time is paused when it auto-pauses and ended when it does not; a paused sandbox
/// waits for a connect.
pub(crate) fn reap_decision(
    held: bool,
    running: bool,
    end_ms: u64,
    now_ms: u64,
    auto_pause: bool,
) -> Reap {
    if held {
        return Reap::Kill;
    }
    if !running || now_ms < end_ms {
        return Reap::Keep;
    }
    if auto_pause { Reap::Pause } else { Reap::Kill }
}

/// One reaper pass over the sandboxes.
fn reap(app: &App, now: u64) {
    let held = app.control.paused();
    for sandbox in live(app) {
        let decision = reap_decision(
            held,
            sandbox.phase == Phase::Running,
            sandbox.end_ms,
            now,
            sandbox.auto_pause,
        );
        let outcome = match decision {
            Reap::Keep => continue,
            Reap::Kill => {
                let reason = if held {
                    "STOP ALL by the owner"
                } else {
                    "timeout"
                };
                destroy(app, &sandbox, reason, "reaper")
            }
            Reap::Pause => pause_sandbox(app, &sandbox, "reaper", "timeout", true),
        };
        // A failed kill stays on the list and the next pass tries again.
        if let Err(refused) = outcome {
            eprintln!("apid: sandbox {}: {}", sandbox.id, refused.message);
        }
    }
}

/// STOP ALL: every sandbox ends. Called by `control::stop_all` after the hold is set, so nothing new
/// starts meanwhile; a kill that fails is retried by the reaper while the hold lasts.
pub fn stop_all(app: &App) {
    for sandbox in live(app) {
        if let Err(refused) = destroy(app, &sandbox, "STOP ALL by the owner", "owner") {
            eprintln!(
                "apid: sandbox {} not stopped: {}",
                sandbox.id, refused.message
            );
        }
    }
}

/// The sandbox whose VM has CID `cid`, if any. Sandbox CIDs have no agent session.
pub(crate) fn sandbox_by_cid(app: &App, cid: u64) -> Option<Sandbox> {
    sandboxes(app).by_cid(cid)
}

/// vsockd's lookup: the CID and the envd access token of sandbox `id`.
pub(crate) fn lookup(app: &App, id: &str) -> Option<(u64, Option<String>)> {
    sandboxes(app)
        .get(id)
        .map(|sandbox| (sandbox.cid, sandbox.access_token))
}

/// The egress decision for a sandbox's connection to `host`. A sandbox without internet access is
/// refused everywhere (after the laws, which still record a forbidden attempt); with internet access
/// the fetch blocklist and the laws decide, as for an agent.
pub(crate) fn sandbox_verdict(
    sandbox: &Sandbox,
    blocked: bool,
    law_refusal: Option<String>,
    host: &str,
) -> EgressVerdict {
    // The sandbox's own rules: an allowed host goes out even without internet access, a denied one
    // never; the laws and the blocklist decide either way (`egress_verdict`).
    if law_refusal.is_none() {
        match crate::e2b_net::verdict(&sandbox.allow_out, &sandbox.deny_out, host) {
            Some(true) => return egress_verdict(blocked, None, host),
            Some(false) => {
                return EgressVerdict {
                    allow: false,
                    law: None,
                    reason: Some(format!(
                        "refused: sandbox {} denies {host} (denyOut)",
                        sandbox.id
                    )),
                };
            }
            None => {}
        }
    }
    if law_refusal.is_none() && !sandbox.allow_internet {
        return EgressVerdict {
            allow: false,
            law: None,
            reason: Some(format!(
                "refused: sandbox {} has no internet access ({host})",
                sandbox.id
            )),
        };
    }
    egress_verdict(blocked, law_refusal, host)
}

/// The E2B key a request presents: `X-API-Key`, or `Authorization: Bearer`. Returns the key's id.
fn authorize(app: &App, req: &Request) -> Result<String, Refused> {
    authorize_with(keys(app), req)
}

fn authorize_with(store: &Keys, req: &Request) -> Result<String, Refused> {
    let presented = req
        .header("x-api-key")
        .map(str::to_string)
        .or_else(|| bearer(req));
    presented
        .and_then(|key| store.verify(&key))
        .ok_or_else(|| Refused::new(401, "unauthorized", "send an E2B API key: X-API-Key: e2b_…"))
}

fn bearer(req: &Request) -> Option<String> {
    let (scheme, token) = req.header("authorization")?.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim().to_string())
}

/// Runs `f` with the key's id when the request carries a valid E2B key; a refusal otherwise.
fn guard(app: &App, req: &Request, f: impl FnOnce(&str) -> Result<Response, Refused>) -> Response {
    match authorize(app, req).and_then(|key| f(&key)) {
        Ok(response) => response,
        Err(refused) => refuse(refused),
    }
}

/// The body as JSON; an empty body is an error here.
fn json_body<T: DeserializeOwned>(req: &Request) -> Result<T, Refused> {
    serde_json::from_slice(&req.body).map_err(|e| invalid(format!("bad body: {e}")))
}

/// The body as JSON; an empty body is the default.
fn body_or_default<T: DeserializeOwned + Default>(req: &Request) -> Result<T, Refused> {
    if req.body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    json_body(req)
}

/// A number of seconds, checked against `min`..=`max`.
fn seconds(value: i64, min: i64, max: u32, what: &str) -> Result<u32, Refused> {
    match u32::try_from(value) {
        Ok(v) if i64::from(v) >= min && v <= max => Ok(v),
        _ => Err(invalid(format!("{what} must be {min} to {max} seconds"))),
    }
}

fn check_text(text: &str, what: &str, max: usize) -> Result<(), Refused> {
    if text.is_empty() || text.chars().count() > max || text.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{what} must be 1 to {max} characters with no control characters"
        )));
    }
    Ok(())
}

/// Checks a string map: names up to `key_max` characters (no `=`), values up to `value_max`.
fn pairs(
    map: BTreeMap<String, String>,
    what: &str,
    key_max: usize,
    value_max: usize,
) -> Result<BTreeMap<String, String>, Refused> {
    for (key, value) in &map {
        check_text(key, &format!("a {what} name"), key_max)?;
        if key.contains('=') {
            return Err(invalid(format!("a {what} name cannot contain '='")));
        }
        if value.chars().count() > value_max || value.contains('\0') {
            return Err(invalid(format!(
                "a {what} value must be at most {value_max} characters"
            )));
        }
    }
    Ok(map)
}

/// The body of `POST /e2b/sandboxes` (E2B's NewSandbox; the fields apid does not act on are ignored).
#[derive(Debug, Default, Deserialize)]
pub struct NewSandbox {
    #[serde(rename = "templateID")]
    template_id: Option<String>,
    timeout: Option<i64>,
    #[serde(rename = "autoPause")]
    auto_pause: Option<bool>,
    secure: Option<bool>,
    allow_internet_access: Option<bool>,
    metadata: Option<BTreeMap<String, String>>,
    #[serde(rename = "envVars")]
    env_vars: Option<BTreeMap<String, String>>,
    network: Option<NetworkBody>,
    #[serde(rename = "volumeMounts")]
    volume_mounts: Option<Vec<Value>>,
}

/// E2B's network configuration (`SandboxNetworkConfig` at create, `SandboxNetworkUpdateConfig` in
/// `PUT .../network`). The fields the plaza does not act on are refused when set, not ignored.
#[derive(Debug, Default, Deserialize)]
pub struct NetworkBody {
    #[serde(rename = "allowOut")]
    allow_out: Option<Vec<String>>,
    #[serde(rename = "denyOut")]
    deny_out: Option<Vec<String>>,
    /// Only in an update: the internet access, as the create's `allow_internet_access`.
    allow_internet_access: Option<bool>,
    rules: Option<Value>,
    #[serde(rename = "egressProxy")]
    egress_proxy: Option<Value>,
    #[serde(rename = "maskRequestHost")]
    mask_request_host: Option<String>,
}

impl NetworkBody {
    /// The checked `(allowOut, denyOut)`.
    fn lists(&self) -> Result<(Vec<String>, Vec<String>), Refused> {
        let set = |value: &Option<Value>| {
            value.as_ref().is_some_and(|v| match v {
                Value::Null => false,
                Value::Object(map) => !map.is_empty(),
                _ => true,
            })
        };
        if set(&self.rules) {
            return Err(invalid(
                "network.rules are not served: the hub adds credentials only by the owner's egress rules",
            ));
        }
        if set(&self.egress_proxy) {
            return Err(invalid(
                "network.egressProxy is not served: every sandbox goes through the hub's egress proxy",
            ));
        }
        if self
            .mask_request_host
            .as_deref()
            .is_some_and(|m| !m.is_empty())
        {
            return Err(invalid("network.maskRequestHost is not served"));
        }
        let allow =
            crate::e2b_net::check(self.allow_out.as_deref().unwrap_or(&[]), "allowOut", true)
                .map_err(invalid)?;
        let deny = crate::e2b_net::check(self.deny_out.as_deref().unwrap_or(&[]), "denyOut", false)
            .map_err(invalid)?;
        Ok((allow, deny))
    }
}

/// A checked create request.
#[derive(Debug, PartialEq)]
pub struct Spec {
    pub template: String,
    pub timeout_s: u32,
    pub auto_pause: bool,
    pub secure: bool,
    pub allow_internet: bool,
    pub metadata: BTreeMap<String, String>,
    pub env_vars: BTreeMap<String, String>,
    pub allow_out: Vec<String>,
    pub deny_out: Vec<String>,
}

impl Spec {
    /// Checks a create request. The base image is any template name; `base` is the default image.
    pub fn from_body(body: NewSandbox) -> Result<Spec, Refused> {
        let template = body.template_id.unwrap_or_default().trim().to_string();
        check_text(&template, "templateID", 128)?;
        if body
            .volume_mounts
            .as_ref()
            .is_some_and(|mounts| !mounts.is_empty())
        {
            return Err(invalid(
                "volumeMounts are not served: the plaza's volumes are read-only and given by role",
            ));
        }
        let (allow_out, deny_out) = body.network.unwrap_or_default().lists()?;
        Ok(Spec {
            template,
            timeout_s: match body.timeout {
                None => DEFAULT_TIMEOUT_S,
                Some(t) => seconds(t, 1, MAX_TIMEOUT_S, "timeout")?,
            },
            auto_pause: body.auto_pause.unwrap_or(false),
            secure: body.secure.unwrap_or(true),
            allow_internet: body.allow_internet_access.unwrap_or(true),
            metadata: pairs(body.metadata.unwrap_or_default(), "metadata", 128, 1024)?,
            env_vars: pairs(body.env_vars.unwrap_or_default(), "envVars", 128, 4096)?,
            allow_out,
            deny_out,
        })
    }
}

#[derive(Deserialize)]
struct KeyBody {
    #[serde(default)]
    label: String,
}

#[derive(Deserialize, Default)]
struct WakeBody {
    timeout: Option<i64>,
    memory: Option<bool>,
}

#[derive(Deserialize)]
struct TimeoutBody {
    timeout: Option<i64>,
}

#[derive(Deserialize, Default)]
struct RefreshBody {
    duration: Option<i64>,
}

#[derive(Deserialize, Default)]
struct SnapshotBody {
    name: Option<String>,
    memory: Option<bool>,
}

/// When a sandbox ends if it lives `seconds` more from `now`.
fn ends_at(now: u64, seconds: u32) -> u64 {
    now + u64::from(seconds) * 1000
}

/// A refresh keeps the sandbox alive for at least `duration` from now, and never past a day from now.
fn refreshed_end(end_ms: u64, now: u64, duration_s: u32) -> u64 {
    end_ms
        .max(ends_at(now, duration_s))
        .min(ends_at(now, MAX_TIMEOUT_S))
}

/// The sandbox's VM path on noded (sandboxes have one VM, index 0).
fn vm_path(run: &str, action: &str) -> String {
    format!("/v1/runs/{run}/vms/0/{action}")
}

/// A noded call: its status and JSON answer, or why it did not answer.
fn noded(
    app: &App,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let bytes = body.map(Value::to_string);
    let reply = apz_http::client::send(
        app.noded.port,
        method,
        path,
        bytes.as_deref().map(str::as_bytes),
        timeout,
    )
    .map_err(|e| format!("noded unreachable: {e}"))?;
    let value = if reply.body.is_empty() {
        Value::Null
    } else {
        reply.json().unwrap_or(Value::Null)
    };
    Ok((reply.status, value))
}

/// A noded call that must succeed.
fn noded_ok(
    app: &App,
    method: &str,
    path: &str,
    body: &Value,
    timeout: Duration,
) -> Result<Value, Refused> {
    match noded(app, method, path, Some(body), timeout) {
        Ok((status, value)) if (200..300).contains(&status) => Ok(value),
        Ok((status, value)) => Err(Refused::new(
            502,
            "sandbox_node_refused",
            format!("noded answered {status}: {value}"),
        )),
        Err(why) => Err(Refused::new(502, "sandbox_node_unreachable", why)),
    }
}

/// One call through vsockd's E2B gateway to the sandbox's daemon, by the sandbox's id.
fn gateway(
    id: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<apz_http::client::Reply, String> {
    let mut headers = vec![("E2b-Sandbox-Id", id)];
    if let Some(token) = token {
        headers.push(("X-Access-Token", token));
    }
    let bytes = body.map(Value::to_string);
    apz_http::client::send_with(
        gateway_port(),
        method,
        path,
        &headers,
        bytes.as_deref().map(str::as_bytes),
        timeout,
    )
    .map_err(|e| format!("the sandbox gateway: {e}"))
}

/// Polls the daemon's `/health` through the gateway until it answers, or `wait` runs out.
fn wait_healthy(id: &str, token: Option<&str>, wait: Duration) -> Result<(), String> {
    let started = Instant::now();
    loop {
        // The gateway asks for the sandbox's token on every request, the hub's own included.
        if let Ok(reply) = gateway(id, token, "GET", "/health", None, Duration::from_secs(2)) {
            if (200..300).contains(&reply.status) {
                return Ok(());
            }
        }
        if started.elapsed() >= wait {
            return Err(format!(
                "the sandbox's daemon did not answer /health in {}s",
                wait.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The body of `/init`. A sandbox starts as a restore of a warm template, so every `/init` also carries
/// the host's clock (RFC 3339, UTC) and 32 bytes of the host's CSPRNG (hex): the daemon sets its clock
/// from the first and mixes the second into its kernel pool.
fn init_body(env: &BTreeMap<String, String>, token: Option<&str>) -> Value {
    let facts = apz_proto::wire::HostFacts::now();
    json!({
        "envVars": env,
        "accessToken": token,
        "timestamp": rfc3339(facts.time_ms),
        "entropy": facts.entropy,
    })
}

/// Waits for the daemon, then hands it the environment, the envd token, the clock and entropy.
fn boot(id: &str, env: &BTreeMap<String, String>, token: Option<&str>) -> Result<(), String> {
    wait_healthy(id, token, HEALTH_WAIT)?;
    init(id, &init_body(env, token), token)
}

/// `/init` through the gateway; any answer but 2xx is an error.
fn init(id: &str, body: &Value, token: Option<&str>) -> Result<(), String> {
    let reply = gateway(id, token, "POST", "/init", Some(body), GATEWAY_CALL)?;
    if (200..300).contains(&reply.status) {
        Ok(())
    } else {
        Err(format!("the daemon refused /init: {}", reply.status))
    }
}

/// Ends a noded run whose sandbox could not be set up. Nothing is recorded for it.
fn destroy_run(app: &App, run: &str) {
    if let Err(why) = noded(
        app,
        "DELETE",
        &format!("/v1/runs/{run}"),
        None,
        NODED_TIMEOUT,
    ) {
        eprintln!("apid: a sandbox run {run} was not ended: {why}");
    }
}

/// Records one sandbox event: a transcript entry under the sandbox's CID, and an `e2b` bus event.
fn audit(app: &App, sandbox: &Sandbox, kind: &'static str, event: &str, actor: &str, extra: Value) {
    let mut record = json!({
        "event": event,
        "sandbox": sandbox.id,
        "cid": sandbox.cid,
        "template": sandbox.template,
        "by": actor,
    });
    if let (Value::Object(base), Value::Object(extra)) = (&mut record, extra) {
        base.extend(extra);
    }
    app.transcripts
        .record(&app.bus, sandbox.cid, 0, kind, record.clone());
    app.bus.publish("e2b", &record);
}

fn find(app: &App, id: &str) -> Result<Sandbox, Refused> {
    sandboxes(app)
        .get(id)
        .filter(|s| !s.retired)
        .ok_or_else(|| not_found(id))
}

/// The sandboxes the API shows: every one but the retired.
fn live(app: &App) -> Vec<Sandbox> {
    sandboxes(app)
        .list()
        .into_iter()
        .filter(|s| !s.retired)
        .collect()
}

/// The snapshot `template` names, with the sandbox it was taken of (retired ones too): a snapshot id
/// (`<sandbox>:<checkpoint>`), or a snapshot name when exactly one snapshot has it.
fn snapshot_source(app: &App, template: &str) -> Option<(Sandbox, Snapshot)> {
    pick_snapshot(&sandboxes(app).list(), template)
}

/// [`snapshot_source`] over the sandboxes `all`.
fn pick_snapshot(all: &[Sandbox], template: &str) -> Option<(Sandbox, Snapshot)> {
    let by_id = all.iter().find_map(|s| {
        s.snapshots
            .iter()
            .find(|snap| snap.id == template)
            .map(|snap| (s.clone(), snap.clone()))
    });
    if by_id.is_some() {
        return by_id;
    }
    let mut named = all.iter().flat_map(|s| {
        s.snapshots
            .iter()
            .filter(|snap| snap.name == template)
            .map(move |snap| (s.clone(), snap.clone()))
    });
    match (named.next(), named.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// Starts a sandbox: a run on this node, the daemon's health, its environment and token. The record is
/// kept only when the daemon answers; a failed start ends its run.
fn create(app: &App, req: &Request, actor: &str) -> Result<Response, Refused> {
    if app.control.paused() {
        return Err(held());
    }
    let mut spec = Spec::from_body(json_body::<NewSandbox>(req)?)?;
    // A snapshot's id (or its unique name) as the template: the new sandbox is a fork of that
    // checkpoint, with the source's environment under the request's.
    let (run, cid) = match snapshot_source(app, &spec.template) {
        Some((source, snap)) => {
            let mut env = source.env_vars.clone();
            env.extend(std::mem::take(&mut spec.env_vars));
            spec.env_vars = env;
            spec.template = snap.id.clone();
            fork_runs(app, &source, Some(&snap.checkpoint), 1)?.remove(0)
        }
        None => new_run(app)?,
    };
    let sandbox = admit(app, &spec, run, cid, actor)?;
    Ok(Response::json(201, &created_view(&sandbox)))
}

/// A new sandbox run on this node: its run id and its VM's CID.
fn new_run(app: &App) -> Result<(String, u64), Refused> {
    let (status, view) = noded(
        app,
        "POST",
        "/v1/runs",
        Some(&json!({"n": 1, "vcpus": SANDBOX_VCPUS, "memory_mib": SANDBOX_MEMORY_MIB, "sandbox": true})),
        RUN_TIMEOUT,
    )
    .map_err(|why| Refused::new(503, "sandbox_node_unreachable", why))?;
    if !(200..300).contains(&status) {
        return Err(if status == 503 {
            Refused::new(
                503,
                "sandbox_capacity_unavailable",
                format!("noded has no room for a sandbox: {view}"),
            )
        } else {
            Refused::new(
                500,
                "sandbox_create_failed",
                format!("noded could not start the sandbox: {status} {view}"),
            )
        });
    }
    let (Some(run), Some(cid)) = (
        view["id"].as_str().map(str::to_string),
        view["vms"][0]["guest_cid"].as_u64(),
    ) else {
        return Err(Refused::new(
            500,
            "sandbox_create_failed",
            "noded's run has no sandbox VM",
        ));
    };
    Ok((run, cid))
}

/// `n` forks of `source`'s VM, each a new single-VM run: of checkpoint `checkpoint`, or of a
/// checkpoint taken now when `None`. Returns each fork's run and CID.
fn fork_runs(
    app: &App,
    source: &Sandbox,
    checkpoint: Option<&str>,
    n: u32,
) -> Result<Vec<(String, u64)>, Refused> {
    let mut body = json!({ "n": n });
    if let Some(checkpoint) = checkpoint {
        body["checkpoint"] = json!(checkpoint);
    }
    let view = noded_ok(
        app,
        "POST",
        &vm_path(&source.run, "fork"),
        &body,
        SNAPSHOT_TIMEOUT,
    )?;
    let forks: Vec<(String, u64)> = view["forks"]
        .as_array()
        .map(|forks| {
            forks
                .iter()
                .filter_map(|fork| Some((fork["run"].as_str()?.to_string(), fork["cid"].as_u64()?)))
                .collect()
        })
        .unwrap_or_default();
    if forks.len() != n as usize {
        for (run, _) in &forks {
            destroy_run(app, run);
        }
        return Err(Refused::new(
            500,
            "sandbox_create_failed",
            format!(
                "noded's fork answer has {} of {n} forks: {view}",
                forks.len()
            ),
        ));
    }
    Ok(forks)
}

/// Makes run `run` (VM `cid`) a sandbox: the record, the daemon's health, its environment and
/// token. The record is kept only when the daemon answers; a failed start ends the run.
fn admit(app: &App, spec: &Spec, run: String, cid: u64, actor: &str) -> Result<Sandbox, Refused> {
    let now = apz_plaza::now_ms();
    let access_token = spec.secure.then(|| hex_of(&apz_trust::random::<32>()));
    let sandbox = Sandbox {
        id: format!("sb{}", hex_of(&apz_trust::random::<8>())),
        template: spec.template.clone(),
        run: run.clone(),
        cid,
        started_ms: now,
        end_ms: ends_at(now, spec.timeout_s),
        phase: Phase::Running,
        auto_pause: spec.auto_pause,
        allow_internet: spec.allow_internet,
        metadata: spec.metadata.clone(),
        env_vars: spec.env_vars.clone(),
        access_token: access_token.clone(),
        snapshots: Vec::new(),
        cold: false,
        retired: false,
        allow_out: spec.allow_out.clone(),
        deny_out: spec.deny_out.clone(),
    };
    // The record comes first: vsockd finds the CID and the token by the sandbox's id on each gateway call.
    if let Err(refused) = sandboxes(app).insert(sandbox.clone()) {
        destroy_run(app, &run);
        return Err(refused);
    }
    if let Err(why) = boot(&sandbox.id, &spec.env_vars, access_token.as_deref()) {
        let _ = sandboxes(app).remove(&sandbox.id);
        destroy_run(app, &run);
        return Err(Refused::new(503, "sandbox_placement_timeout", why));
    }
    // STOP ALL may have come while the daemon booted; then the sandbox does not stand.
    if app.control.paused() || sandboxes(app).get(&sandbox.id).is_none() {
        let _ = sandboxes(app).remove(&sandbox.id);
        destroy_run(app, &run);
        return Err(held());
    }
    audit(
        app,
        &sandbox,
        "sandbox_create",
        "create",
        actor,
        json!({"timeout": spec.timeout_s, "allow_internet_access": spec.allow_internet}),
    );
    Ok(sandbox)
}

/// Ends a sandbox: its run goes, then its record. A kill that noded refuses keeps the record. A
/// sandbox with snapshots is retired instead: its VM stops (no memory image is written) and the run
/// stays for the snapshots, which new sandboxes can still start from.
fn destroy(app: &App, sandbox: &Sandbox, reason: &str, actor: &str) -> Result<(), Refused> {
    if !sandbox.snapshots.is_empty() {
        if sandbox.phase == Phase::Running {
            noded_ok(
                app,
                "POST",
                &vm_path(&sandbox.run, "sleep"),
                &memory_body(false),
                NODED_TIMEOUT,
            )?;
        }
        sandboxes(app).update(&sandbox.id, |s| {
            s.retired = true;
            s.phase = Phase::Paused;
            s.cold = true;
        })?;
        audit(
            app,
            sandbox,
            "sandbox_kill",
            "kill",
            actor,
            json!({"reason": reason, "kept_for_snapshots": sandbox.snapshots.len()}),
        );
        return Ok(());
    }
    end_run(app, sandbox)?;
    sandboxes(app).remove(&sandbox.id)?;
    audit(
        app,
        sandbox,
        "sandbox_kill",
        "kill",
        actor,
        json!({"reason": reason}),
    );
    Ok(())
}

/// Deletes sandbox `sandbox`'s run on noded (a run noded no longer has is already gone).
fn end_run(app: &App, sandbox: &Sandbox) -> Result<(), Refused> {
    match noded(
        app,
        "DELETE",
        &format!("/v1/runs/{}", sandbox.run),
        None,
        NODED_TIMEOUT,
    ) {
        // A run noded no longer has is already gone.
        Ok((status, _)) if (200..300).contains(&status) || status == 404 => {}
        Ok((status, value)) => {
            return Err(Refused::new(
                502,
                "sandbox_node_refused",
                format!("noded answered {status} to the kill: {value}"),
            ));
        }
        Err(why) => return Err(Refused::new(502, "sandbox_node_unreachable", why)),
    }
    Ok(())
}

/// noded's body for sleep and wake: `keep_memory` keeps the memory image, otherwise the sandbox is cold.
fn memory_body(keep_memory: bool) -> Value {
    if keep_memory {
        json!({})
    } else {
        json!({"memory": false})
    }
}

/// Asks the sandbox's daemon to flush the guest's filesystems (`POST /sync`). A failure is logged and
/// the pause goes on: the disk then holds what the guest had written back.
fn flush(sandbox: &Sandbox) {
    let token = sandbox.access_token.as_deref();
    match gateway(&sandbox.id, token, "POST", "/sync", None, GATEWAY_CALL) {
        Ok(reply) if (200..300).contains(&reply.status) => {}
        Ok(reply) => eprintln!(
            "apid: sandbox {}: /sync answered {}",
            sandbox.id, reply.status
        ),
        Err(why) => eprintln!("apid: sandbox {}: /sync failed: {why}", sandbox.id),
    }
}

fn pause_sandbox(
    app: &App,
    sandbox: &Sandbox,
    actor: &str,
    reason: &str,
    keep_memory: bool,
) -> Result<(), Refused> {
    if !keep_memory && sandbox.phase == Phase::Running {
        flush(sandbox);
    }
    noded_ok(
        app,
        "POST",
        &vm_path(&sandbox.run, "sleep"),
        &memory_body(keep_memory),
        NODED_TIMEOUT,
    )?;
    sandboxes(app).update(&sandbox.id, |s| {
        s.phase = Phase::Paused;
        s.cold = !keep_memory;
    })?;
    audit(
        app,
        sandbox,
        "sandbox_pause",
        "pause",
        actor,
        json!({"reason": reason}),
    );
    Ok(())
}

/// Wakes a paused sandbox for `timeout_s` from now. A cold wake (asked for, or a sandbox paused without
/// memory) boots from disk and gives the daemon its environment again; a memory wake keeps the one set at boot.
fn wake(app: &App, id: &str, timeout_s: u32, actor: &str, cold: bool) -> Result<Sandbox, Refused> {
    if app.control.paused() {
        return Err(held());
    }
    let sandbox = find(app, id)?;
    let cold = cold || sandbox.cold;
    noded_ok(
        app,
        "POST",
        &vm_path(&sandbox.run, "wake"),
        &memory_body(!cold),
        NODED_TIMEOUT,
    )?;
    // A wake is a restore too: the daemon gets a fresh clock and entropy before the sandbox is handed
    // out. An empty environment map leaves a memory-restored daemon's environment as it is.
    let env = if cold {
        sandbox.env_vars.clone()
    } else {
        BTreeMap::new()
    };
    let reseeded = wait_healthy(id, sandbox.access_token.as_deref(), HEALTH_WAIT).and_then(|()| {
        init(
            id,
            &init_body(&env, sandbox.access_token.as_deref()),
            sandbox.access_token.as_deref(),
        )
    });
    if let Err(why) = reseeded {
        // The sandbox stays paused: noded puts it back to sleep, and the caller learns why.
        let _ = noded(
            app,
            "POST",
            &vm_path(&sandbox.run, "sleep"),
            Some(&memory_body(!cold)),
            NODED_TIMEOUT,
        );
        return Err(Refused::new(503, "sandbox_create_failed", why));
    }
    let now = apz_plaza::now_ms();
    let woken = sandboxes(app).update(id, |s| {
        s.phase = Phase::Running;
        s.end_ms = ends_at(now, timeout_s);
        s.cold = false;
    })?;
    audit(
        app,
        &woken,
        "sandbox_resume",
        "resume",
        actor,
        json!({"timeout": timeout_s}),
    );
    Ok(woken)
}

/// The `metadata` filter of a list: the `key=value` pairs, joined by `&`, sent percent-encoded in the query
/// (`metadata=k%3Dv%26k2%3Dv2`); each key and value is then decoded again (a space is `%2520`).
fn metadata_filter(req: &Request) -> Result<Vec<(String, String)>, Refused> {
    let Some(raw) = req.query_param("metadata") else {
        return Ok(Vec::new());
    };
    parse_metadata_filter(&crate::plaza::percent_decode(raw))
}

fn parse_metadata_filter(text: &str) -> Result<Vec<(String, String)>, Refused> {
    let mut pairs = Vec::new();
    for part in text.split('&').filter(|part| !part.is_empty()) {
        let Some((key, value)) = part.split_once('=') else {
            return Err(invalid(format!(
                "metadata filter {part:?} is not key=value"
            )));
        };
        pairs.push((
            crate::plaza::percent_decode(key),
            crate::plaza::percent_decode(value),
        ));
    }
    Ok(pairs)
}

/// Whether the sandbox carries every `key=value` of the filter in its metadata.
fn matches(sandbox: &Sandbox, filter: &[(String, String)]) -> bool {
    filter
        .iter()
        .all(|(key, value)| sandbox.metadata.get(key) == Some(value))
}

fn list(app: &App, req: &Request) -> Result<Response, Refused> {
    let filter = metadata_filter(req)?;
    let mut running: Vec<Sandbox> = live(app)
        .into_iter()
        .filter(|s| s.phase == Phase::Running && matches(s, &filter))
        .collect();
    running.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
    Ok(Response::json(
        200,
        &running.iter().map(listed_view).collect::<Vec<Value>>(),
    ))
}

fn list_v2(app: &App, req: &Request) -> Result<Response, Refused> {
    let filter = metadata_filter(req)?;
    let phases = match req.query_param("state") {
        None => vec![Phase::Running],
        Some(text) => text
            .split(',')
            .map(|state| match state.trim() {
                "running" => Ok(Phase::Running),
                "paused" => Ok(Phase::Paused),
                other => Err(invalid(format!("unknown state {other:?}"))),
            })
            .collect::<Result<Vec<Phase>, Refused>>()?,
    };
    let mut listed: Vec<Sandbox> = live(app)
        .into_iter()
        .filter(|s| phases.contains(&s.phase) && matches(s, &filter))
        .collect();
    listed.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
    Ok(Response::json(
        200,
        &listed.iter().map(listed_view).collect::<Vec<Value>>(),
    ))
}

fn detail(app: &App, id: &str) -> Result<Response, Refused> {
    Ok(Response::json(200, &detail_view(&find(app, id)?)))
}

fn kill(app: &App, id: &str, key: &str) -> Result<Response, Refused> {
    destroy(app, &find(app, id)?, "requested", key)?;
    Ok(no_content())
}

fn set_timeout(app: &App, req: &Request, id: &str) -> Result<Response, Refused> {
    let body: TimeoutBody = json_body(req)?;
    let Some(timeout) = body.timeout else {
        return Err(invalid("timeout is required"));
    };
    let timeout = seconds(timeout, 0, MAX_TIMEOUT_S, "timeout")?;
    let now = apz_plaza::now_ms();
    find(app, id)?;
    sandboxes(app).update(id, |s| s.end_ms = ends_at(now, timeout))?;
    Ok(no_content())
}

fn refresh(app: &App, req: &Request, id: &str) -> Result<Response, Refused> {
    let body: RefreshBody = body_or_default(req)?;
    let duration = match body.duration {
        None => DEFAULT_REFRESH_S,
        Some(d) => seconds(d, 0, MAX_REFRESH_S, "duration")?,
    };
    let now = apz_plaza::now_ms();
    find(app, id)?;
    sandboxes(app).update(id, |s| s.end_ms = refreshed_end(s.end_ms, now, duration))?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
struct PauseBody {
    memory: Option<bool>,
}

fn pause(app: &App, req: &Request, id: &str, key: &str) -> Result<Response, Refused> {
    let body: PauseBody = body_or_default(req)?;
    let sandbox = find(app, id)?;
    if sandbox.phase == Phase::Paused {
        return Ok(no_content());
    }
    pause_sandbox(app, &sandbox, key, "requested", body.memory.unwrap_or(true))?;
    Ok(no_content())
}

fn resume(app: &App, req: &Request, id: &str, key: &str) -> Result<Response, Refused> {
    let body: WakeBody = body_or_default(req)?;
    let timeout = match body.timeout {
        None => DEFAULT_TIMEOUT_S,
        Some(t) => seconds(t, 0, MAX_TIMEOUT_S, "timeout")?,
    };
    if find(app, id)?.phase == Phase::Running {
        return Err(Refused::new(
            409,
            "sandbox_not_paused",
            "the sandbox is running",
        ));
    }
    let woken = wake(app, id, timeout, key, body.memory == Some(false))?;
    Ok(Response::json(201, &created_view(&woken)))
}

/// Connect: a paused sandbox wakes (201); a running one gets its time to live set (200). The v1 body
/// must name a timeout; the v2 one (`ConnectSandboxV2`) defaults it.
fn connect(app: &App, req: &Request, id: &str, key: &str, v2: bool) -> Result<Response, Refused> {
    let body: WakeBody = body_or_default(req)?;
    let timeout = match (body.timeout, v2) {
        (Some(timeout), _) => timeout,
        (None, true) => i64::from(DEFAULT_TIMEOUT_S),
        (None, false) => return Err(invalid("timeout is required")),
    };
    let timeout = seconds(timeout, 0, MAX_TIMEOUT_S, "timeout")?;
    if find(app, id)?.phase == Phase::Running {
        let now = apz_plaza::now_ms();
        let running = sandboxes(app).update(id, |s| s.end_ms = ends_at(now, timeout))?;
        return Ok(Response::json(200, &created_view(&running)));
    }
    let woken = wake(app, id, timeout, key, body.memory == Some(false))?;
    Ok(Response::json(201, &created_view(&woken)))
}

fn snapshot(app: &App, req: &Request, id: &str, key: &str) -> Result<Response, Refused> {
    let body: SnapshotBody = body_or_default(req)?;
    if body.memory == Some(false) {
        return Err(invalid(
            "a filesystem-only snapshot is not supported yet; snapshots keep memory",
        ));
    }
    let label = match body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        Some(name) => crate::vm_snapshots::check_label(name).map_err(|why| invalid(why))?,
        None => "snapshot".to_string(),
    };
    let sandbox = find(app, id)?;
    let view = noded_ok(
        app,
        "POST",
        &vm_path(&sandbox.run, "checkpoints"),
        &json!({"label": label}),
        SNAPSHOT_TIMEOUT,
    )?;
    let checkpoint = view["name"].as_str().unwrap_or(label.as_str()).to_string();
    let snap = Snapshot {
        id: format!("{}:{checkpoint}", sandbox.id),
        name: label,
        checkpoint,
        taken_ms: apz_plaza::now_ms(),
    };
    let stored = snap.clone();
    sandboxes(app).update(&sandbox.id, |s| s.snapshots.push(stored))?;
    audit(
        app,
        &sandbox,
        "sandbox_snapshot",
        "snapshot",
        key,
        json!({"snapshot": snap.id}),
    );
    Ok(Response::json(201, &snapshot_view(&snap)))
}

/// A running sandbox's daemon answers `path` (a GET) with JSON; `None` for a paused one.
fn daemon_json(sandbox: &Sandbox, path: &str) -> Result<Option<Value>, Refused> {
    if sandbox.phase != Phase::Running {
        return Ok(None);
    }
    let reply = gateway(
        &sandbox.id,
        sandbox.access_token.as_deref(),
        "GET",
        path,
        None,
        GATEWAY_CALL,
    )
    .map_err(|why| Refused::new(502, "sandbox_unreachable", why))?;
    if !(200..300).contains(&reply.status) {
        return Err(Refused::new(
            502,
            "sandbox_unreachable",
            format!("the daemon answered {} to {path}", reply.status),
        ));
    }
    serde_json::from_slice(&reply.body)
        .map(Some)
        .map_err(|err| Refused::new(502, "sandbox_unreachable", format!("{path}: {err}")))
}

/// `GET /e2b/sandboxes/{id}/metrics`: the daemon's sample now, one entry (none while paused).
fn metrics(app: &App, id: &str) -> Result<Response, Refused> {
    let sandbox = find(app, id)?;
    let list: Vec<Value> = daemon_json(&sandbox, "/metrics")?
        .map(|envd| crate::e2b_obs::metric(&envd, rfc3339))
        .into_iter()
        .collect();
    Ok(Response::json(200, &list))
}

/// `GET /e2b/sandboxes/metrics?sandbox_ids=a,b`: each running sandbox's sample, by id.
fn metrics_many(app: &App, req: &Request) -> Result<Response, Refused> {
    let ids = req
        .query_param("sandbox_ids")
        .map(|raw| crate::plaza::percent_decode(raw))
        .ok_or_else(|| invalid("sandbox_ids is required"))?;
    let ids: Vec<&str> = ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect();
    if ids.len() > 100 {
        return Err(invalid("at most 100 sandbox_ids"));
    }
    let mut out = serde_json::Map::new();
    for id in ids {
        let Ok(sandbox) = find(app, id) else { continue };
        if let Ok(Some(envd)) = daemon_json(&sandbox, "/metrics") {
            out.insert(id.to_string(), crate::e2b_obs::metric(&envd, rfc3339));
        }
    }
    Ok(Response::json(200, &json!({ "sandboxes": out })))
}

/// `GET /e2b/v2/sandboxes/{id}/logs` (and the v1 form): the daemon's log, filtered and paged.
fn logs(app: &App, req: &Request, id: &str, v2: bool) -> Result<Response, Refused> {
    let sandbox = find(app, id)?;
    let number = |name: &str| -> Result<Option<u64>, Refused> {
        req.query_param(name)
            .map(|raw| {
                raw.parse::<u64>()
                    .map_err(|_| invalid(format!("{name} is not a number")))
            })
            .transpose()
    };
    let level = req.query_param("level").map(str::to_string);
    if let Some(level) = &level
        && !["debug", "info", "warn", "error"].contains(&level.as_str())
    {
        return Err(invalid(format!("unknown level {level:?}")));
    }
    let query = crate::e2b_obs::LogQuery {
        // v1 names its start `start`; v2 its `cursor`.
        cursor: if v2 {
            number("cursor")?
        } else {
            number("start")?
        },
        limit: number("limit")?.map_or(1000, |n| n.min(1000) as usize),
        backward: req.query_param("direction") == Some("backward"),
        level,
        search: req
            .query_param("search")
            .map(|raw| crate::plaza::percent_decode(raw)),
    };
    let daemon = daemon_json(&sandbox, "/logs")?.unwrap_or_else(|| json!([]));
    let entries = crate::e2b_obs::entries(&daemon, &query, rfc3339);
    if v2 {
        return Ok(Response::json(200, &json!({ "logs": entries })));
    }
    let lines: Vec<Value> = entries
        .iter()
        .map(|entry| json!({"timestamp": entry["timestamp"], "line": entry["message"]}))
        .collect();
    Ok(Response::json(
        200,
        &json!({ "logs": lines, "logEntries": entries }),
    ))
}

/// `PUT /e2b/sandboxes/{id}/network`: replaces the sandbox's network rules (an omitted list is
/// cleared, an omitted `allow_internet_access` is on). Takes effect on the next connection.
fn network(app: &App, req: &Request, id: &str, key: &str) -> Result<Response, Refused> {
    let body: NetworkBody = json_body(req)?;
    let (allow_out, deny_out) = body.lists()?;
    let allow_internet = body.allow_internet_access.unwrap_or(true);
    find(app, id)?;
    let updated = sandboxes(app).update(id, |s| {
        s.allow_out = allow_out;
        s.deny_out = deny_out;
        s.allow_internet = allow_internet;
    })?;
    audit(
        app,
        &updated,
        "sandbox_network",
        "network",
        key,
        json!({"allowOut": updated.allow_out, "denyOut": updated.deny_out, "allow_internet_access": allow_internet}),
    );
    Ok(no_content())
}

/// The body of `POST /e2b/sandboxes/{id}/fork` (E2B's SandboxForkRequest).
#[derive(Deserialize, Default)]
struct ForkBody {
    timeout: Option<i64>,
    count: Option<i64>,
}

/// The most forks one request makes (noded's limit).
const MAX_FORKS: u32 = 8;

/// Forks a running or paused sandbox into `count` new ones, each from one checkpoint taken now, with
/// the source's template, metadata, environment and settings, and tokens of their own. Each fork
/// stands or fails on its own: the answer has one `{"sandbox"}` or `{"error"}` entry per fork.
fn fork(app: &App, req: &Request, id: &str, key: &str) -> Result<Response, Refused> {
    if app.control.paused() {
        return Err(held());
    }
    let body: ForkBody = body_or_default(req)?;
    let count = match body.count {
        None => 1,
        Some(n) => seconds(n, 1, MAX_FORKS, "count")?,
    };
    let timeout_s = match body.timeout {
        None => DEFAULT_TIMEOUT_S,
        Some(t) => seconds(t, 1, MAX_TIMEOUT_S, "timeout")?,
    };
    let source = find(app, id)?;
    let spec = Spec {
        template: source.template.clone(),
        timeout_s,
        auto_pause: source.auto_pause,
        secure: source.access_token.is_some(),
        allow_internet: source.allow_internet,
        metadata: source.metadata.clone(),
        // The forks' daemons are copies of the source's: they hold its environment already, and a
        // fresh `/init` only adds the clock and entropy.
        env_vars: BTreeMap::new(),
        allow_out: source.allow_out.clone(),
        deny_out: source.deny_out.clone(),
    };
    let forks = fork_runs(app, &source, None, count)?;
    let results: Vec<Value> = forks
        .into_iter()
        .map(|(run, cid)| match admit(app, &spec, run, cid, key) {
            Ok(mut sandbox) => {
                // A cold resume of a fork boots its daemon fresh, so it keeps the source's environment.
                let env = source.env_vars.clone();
                if let Ok(updated) = sandboxes(app).update(&sandbox.id, |s| s.env_vars = env) {
                    sandbox = updated;
                }
                json!({ "sandbox": created_view(&sandbox) })
            }
            Err(refused) => json!({
                "error": { "code": refused.status, "message": refused.message }
            }),
        })
        .collect();
    Ok(Response::json(201, &results))
}

/// Deletes a snapshot named by its id or its unique name: noded's checkpoint goes, then the record's
/// entry; a retired sandbox whose last snapshot this was ends with it.
fn delete_snapshot(app: &App, template: &str, key: &str) -> Result<Response, Refused> {
    let template = crate::plaza::percent_decode(template);
    let Some((owner, snap)) = snapshot_source(app, &template) else {
        return Err(not_found(&template));
    };
    match noded(
        app,
        "DELETE",
        &vm_path(&owner.run, &format!("checkpoints/{}", snap.checkpoint)),
        None,
        NODED_TIMEOUT,
    ) {
        Ok((status, _)) if (200..300).contains(&status) || status == 404 => {}
        Ok((status, value)) => {
            return Err(Refused::new(
                502,
                "sandbox_node_refused",
                format!("noded answered {status} to the snapshot's deletion: {value}"),
            ));
        }
        Err(why) => return Err(Refused::new(502, "sandbox_node_unreachable", why)),
    }
    let updated = sandboxes(app).update(&owner.id, |s| s.snapshots.retain(|x| x.id != snap.id))?;
    audit(
        app,
        &updated,
        "sandbox_snapshot",
        "snapshot_delete",
        key,
        json!({"snapshot": snap.id}),
    );
    if updated.retired && updated.snapshots.is_empty() {
        end_run(app, &updated)?;
        sandboxes(app).remove(&updated.id)?;
    }
    Ok(no_content())
}

fn snapshots_list(app: &App) -> Response {
    let mut snaps: Vec<Snapshot> = sandboxes(app)
        .list()
        .into_iter()
        .flat_map(|s| s.snapshots)
        .collect();
    snaps.sort_by(|a, b| b.taken_ms.cmp(&a.taken_ms));
    Response::json(
        200,
        &snaps.iter().map(snapshot_view).collect::<Vec<Value>>(),
    )
}

fn no_content() -> Response {
    Response::new(204, apz_http::JSON, Vec::<u8>::new())
}

/// The E2B `Sandbox` object (create, resume, connect).
fn created_view(s: &Sandbox) -> Value {
    let mut view = json!({
        "templateID": s.template,
        "sandboxID": s.id,
        "clientID": CLIENT_ID,
        "envdVersion": ENVD_VERSION,
        "domain": null,
    });
    if let Some(token) = &s.access_token {
        view["envdAccessToken"] = json!(token);
    }
    view
}

/// The E2B `ListedSandbox` object. `diskSizeMB` is 0: noded does not measure a sandbox's disk.
fn listed_view(s: &Sandbox) -> Value {
    json!({
        "templateID": s.template,
        "sandboxID": s.id,
        "clientID": CLIENT_ID,
        "startedAt": rfc3339(s.started_ms),
        "endAt": rfc3339(s.end_ms),
        "cpuCount": SANDBOX_VCPUS,
        "memoryMB": SANDBOX_MEMORY_MIB,
        "diskSizeMB": 0,
        "metadata": s.metadata,
        "state": s.phase.as_str(),
        "envdVersion": ENVD_VERSION,
    })
}

/// The E2B `SandboxDetail` object: the listed fields, plus the network and token fields.
fn detail_view(s: &Sandbox) -> Value {
    let mut view = listed_view(s);
    view["allowInternetAccess"] = json!(s.allow_internet);
    view["domain"] = Value::Null;
    if let Some(token) = &s.access_token {
        view["envdAccessToken"] = json!(token);
    }
    view
}

/// The E2B `SnapshotInfo` object.
fn snapshot_view(snap: &Snapshot) -> Value {
    json!({"snapshotID": snap.id, "names": [snap.name]})
}

/// An epoch-millisecond instant as RFC 3339 in UTC, with milliseconds.
fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let rem = secs % 86_400;
    let (year, month, day) = civil((secs / 86_400) as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

/// The civil date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Who acts when the owner ends, pauses or resumes a sandbox from the console (the transcript's actor).
const OWNER: &str = "owner";

/// The owner's key routes and the E2B routes (listed in the module docs).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/e2b/keys", |app, _, _| {
            Response::json(200, &json!({"keys": keys(app).list()}))
        })
        .post("/v1/e2b/keys", |app, req, _| match json_body::<KeyBody>(req) {
            Ok(body) => match keys(app).create(&body.label, apz_plaza::now_ms()) {
                Ok((view, key)) => Response::json(
                    200,
                    &json!({"id": view.id, "label": view.label, "key": key, "created_ms": view.created_ms}),
                ),
                Err(refused) => refuse(refused),
            },
            Err(refused) => refuse(refused),
        })
        .delete("/v1/e2b/keys/{id}", |app, _, p| {
            match keys(app).remove(p.get("id")) {
                Ok(()) => Response::json(200, &json!({"ok": true})),
                Err(refused) => refuse(refused),
            }
        })
        // The owner's view of the sandboxes (the console): every one, running or paused, with its
        // VM's CID and run; never its envd token. Ending, pausing and resuming act as the owner.
        .get("/v1/e2b/sandboxes", |app, _, _| {
            let mut all = live(app);
            all.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
            let rows: Vec<Value> = all
                .iter()
                .map(|s| {
                    let mut row = listed_view(s);
                    row["cid"] = json!(s.cid);
                    row["run"] = json!(s.run);
                    row["allowInternetAccess"] = json!(s.allow_internet);
                    row["autoPause"] = json!(s.auto_pause);
                    row
                })
                .collect();
            Response::json(200, &json!({ "sandboxes": rows }))
        })
        .delete("/v1/e2b/sandboxes/{id}", |app, _, p| {
            kill(app, p.get("id"), OWNER).unwrap_or_else(refuse)
        })
        .post("/v1/e2b/sandboxes/{id}/pause", |app, req, p| {
            pause(app, req, p.get("id"), OWNER).unwrap_or_else(refuse)
        })
        .post("/v1/e2b/sandboxes/{id}/resume", |app, req, p| {
            resume(app, req, p.get("id"), OWNER).unwrap_or_else(refuse)
        })
        .post("/e2b/sandboxes", |app, req, _| {
            guard(app, req, |key| create(app, req, key))
        })
        .get("/e2b/sandboxes", |app, req, _| {
            guard(app, req, |_| list(app, req))
        })
        .get("/e2b/v2/sandboxes", |app, req, _| {
            guard(app, req, |_| list_v2(app, req))
        })
        .get("/e2b/sandboxes/{id}", |app, req, p| {
            guard(app, req, |_| detail(app, p.get("id")))
        })
        .delete("/e2b/sandboxes/{id}", |app, req, p| {
            guard(app, req, |key| kill(app, p.get("id"), key))
        })
        .post("/e2b/sandboxes/{id}/timeout", |app, req, p| {
            guard(app, req, |_| set_timeout(app, req, p.get("id")))
        })
        .post("/e2b/sandboxes/{id}/refreshes", |app, req, p| {
            guard(app, req, |_| refresh(app, req, p.get("id")))
        })
        .post("/e2b/sandboxes/{id}/pause", |app, req, p| {
            guard(app, req, |key| pause(app, req, p.get("id"), key))
        })
        .post("/e2b/sandboxes/{id}/resume", |app, req, p| {
            guard(app, req, |key| resume(app, req, p.get("id"), key))
        })
        .post("/e2b/sandboxes/{id}/connect", |app, req, p| {
            guard(app, req, |key| connect(app, req, p.get("id"), key, false))
        })
        // The v2 routes the current SDKs call: create (`NewSandboxV2`, always secured) and connect.
        .post("/e2b/v2/sandboxes", |app, req, _| {
            guard(app, req, |key| create(app, req, key))
        })
        .post("/e2b/v2/sandboxes/{id}/connect", |app, req, p| {
            guard(app, req, |key| connect(app, req, p.get("id"), key, true))
        })
        // Metrics are not sampled for sandboxes: the list is empty, as for a sandbox that just started.
        .get("/e2b/sandboxes/{id}/metrics", |app, req, p| {
            guard(app, req, |_| metrics(app, p.get("id")))
        })
        .get("/e2b/sandboxes/metrics", |app, req, _| {
            guard(app, req, |_| metrics_many(app, req))
        })
        .get("/e2b/v2/sandboxes/{id}/logs", |app, req, p| {
            guard(app, req, |_| logs(app, req, p.get("id"), true))
        })
        .get("/e2b/sandboxes/{id}/logs", |app, req, p| {
            guard(app, req, |_| logs(app, req, p.get("id"), false))
        })
        .route("PUT", "/e2b/sandboxes/{id}/network", |app, req, p| {
            guard(app, req, |key| network(app, req, p.get("id"), key))
        })
        .post("/e2b/sandboxes/{id}/snapshots", |app, req, p| {
            guard(app, req, |key| snapshot(app, req, p.get("id"), key))
        })
        .get("/e2b/snapshots", |app, req, _| {
            guard(app, req, |_| Ok(snapshots_list(app)))
        })
        .post("/e2b/sandboxes/{id}/fork", |app, req, p| {
            guard(app, req, |key| fork(app, req, p.get("id"), key))
        })
        // E2B deletes a snapshot as the template it is.
        .delete("/e2b/templates/{id}", |app, req, p| {
            guard(app, req, |key| delete_snapshot(app, p.get("id"), key))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-e2b-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_header(name: &str, value: &str) -> Request {
        Request {
            headers: vec![(name.to_string(), value.to_string())],
            ..Default::default()
        }
    }

    fn sample(allow_internet: bool) -> Sandbox {
        Sandbox {
            id: "sb0123456789abcdef".into(),
            template: "base".into(),
            run: "run-1".into(),
            cid: 42,
            started_ms: 1_000,
            end_ms: 301_000,
            phase: Phase::Running,
            auto_pause: false,
            allow_internet,
            metadata: BTreeMap::from([("purpose".to_string(), "test".to_string())]),
            access_token: Some("token".into()),
            env_vars: BTreeMap::new(),
            cold: false,
            snapshots: Vec::new(),
            retired: false,
            allow_out: Vec::new(),
            deny_out: Vec::new(),
        }
    }

    #[test]
    fn a_sandbox_s_network_rules_open_and_close_hosts_but_never_the_laws() {
        let mut offline = sample(false);
        offline.allow_out = vec!["pypi.org".into()];
        assert!(
            sandbox_verdict(&offline, false, None, "pypi.org").allow,
            "allowOut opens a host"
        );
        assert!(!sandbox_verdict(&offline, false, None, "example.com").allow);
        assert!(
            !sandbox_verdict(&offline, true, None, "pypi.org").allow,
            "the blocklist still applies"
        );
        let law = Some("refused by law S-1".to_string());
        assert!(
            !sandbox_verdict(&offline, false, law, "pypi.org").allow,
            "the laws still apply"
        );
        let mut online = sample(true);
        online.deny_out = vec!["0.0.0.0/0".into()];
        online.allow_out = vec!["*.example.com".into()];
        assert!(!sandbox_verdict(&online, false, None, "github.com").allow);
        assert!(sandbox_verdict(&online, false, None, "api.example.com").allow);
    }

    #[test]
    fn network_bodies_refuse_what_is_not_served() {
        let body = |v: Value| serde_json::from_value::<NetworkBody>(v).unwrap().lists();
        assert!(body(json!({"allowOut": ["*.example.com"], "denyOut": ["10.0.0.0/8"]})).is_ok());
        assert!(body(json!({"denyOut": ["example.com"]})).is_err());
        assert!(body(json!({"rules": {"api.example.com": [{}]}})).is_err());
        assert!(
            body(json!({"rules": {}})).is_ok(),
            "an empty rule set is no rule"
        );
        assert!(body(json!({"egressProxy": {"url": "http://x"}})).is_err());
        let mounts: NewSandbox = serde_json::from_value(
            json!({"templateID": "base", "volumeMounts": [{"name": "v", "path": "/v"}]}),
        )
        .unwrap();
        assert!(Spec::from_body(mounts).is_err());
    }

    #[test]
    fn a_snapshot_template_is_found_by_its_id_or_its_one_name() {
        let snap = |sandbox: &str, checkpoint: &str, name: &str| Snapshot {
            id: format!("{sandbox}:{checkpoint}"),
            name: name.into(),
            checkpoint: checkpoint.into(),
            taken_ms: 1,
        };
        let mut a = sample(true);
        a.snapshots = vec![
            snap("sba", "cp-1-x", "first"),
            snap("sba", "cp-2-y", "both"),
        ];
        let mut b = sample(true);
        b.id = "sbb".into();
        b.retired = true;
        b.snapshots = vec![snap("sbb", "cp-3-z", "both"), snap("sbb", "cp-4-w", "kept")];
        let all = vec![a, b];
        let found = |t: &str| pick_snapshot(&all, t).map(|(s, snap)| (s.id, snap.checkpoint));
        assert_eq!(
            found("sba:cp-2-y"),
            Some(("sb0123456789abcdef".into(), "cp-2-y".into()))
        );
        assert_eq!(
            found("first"),
            Some(("sb0123456789abcdef".into(), "cp-1-x".into()))
        );
        // A retired sandbox's snapshots are still templates.
        assert_eq!(found("kept"), Some(("sbb".into(), "cp-4-w".into())));
        assert_eq!(
            found("both"),
            None,
            "a name two snapshots share names neither"
        );
        assert_eq!(found("base"), None, "a template that is no snapshot");
    }

    #[test]
    fn metadata_filters_parse_and_match() {
        let req = Request {
            query: "metadata=purpose%3Dtest%26team%3Da%2520b".into(),
            ..Default::default()
        };
        let Ok(filter) = metadata_filter(&req) else {
            panic!("a two-pair filter parses");
        };
        assert_eq!(
            filter,
            vec![
                ("purpose".to_string(), "test".to_string()),
                ("team".to_string(), "a b".to_string()),
            ]
        );
        let Ok(none) = metadata_filter(&Request::default()) else {
            panic!("no filter parses");
        };
        assert!(none.is_empty());
        assert!(parse_metadata_filter("purpose").is_err());

        let sandbox = sample(false);
        assert!(matches(
            &sandbox,
            &[("purpose".to_string(), "test".to_string())]
        ));
        assert!(matches(&sandbox, &[]));
        assert!(!matches(
            &sandbox,
            &[("purpose".to_string(), "other".to_string())]
        ));
        assert!(!matches(
            &sandbox,
            &[("missing".to_string(), "x".to_string())]
        ));
    }

    #[test]
    fn sandbox_record_keeps_env_and_cold_state() {
        let mut sandbox = sample(false);
        sandbox.env_vars = BTreeMap::from([("K".to_string(), "V".to_string())]);
        sandbox.cold = true;
        let text = serde_json::to_string(&sandbox).unwrap();
        let back: Sandbox = serde_json::from_str(&text).unwrap();
        assert_eq!(back.env_vars["K"], "V");
        assert!(back.cold);

        // A record written before these fields existed still parses, as warm with no environment.
        let mut old = serde_json::to_value(sample(false)).unwrap();
        old.as_object_mut().unwrap().remove("cold");
        let old: Sandbox = serde_json::from_value(old).unwrap();
        assert!(old.env_vars.is_empty());
        assert!(!old.cold);
    }

    #[test]
    fn keys_are_hashed_verified_and_kept_private() {
        let dir = scratch("keys");
        let keys = Keys::load(&dir);
        let (view, key) = keys.create("ci runner", 5).unwrap();
        assert!(key.starts_with("e2b_"));
        assert_eq!(key.len(), 4 + 40);
        assert!(key[4..].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(keys.verify(&key), Some(view.id.clone()));
        assert_eq!(keys.verify("e2b_0000"), None);
        assert_eq!(keys.verify("owner-token-without-prefix"), None);

        let file = std::fs::read_to_string(dir.join(KEYS_FILE)).unwrap();
        assert!(!file.contains(&key), "the key itself is never stored");
        assert!(file.contains(&key_digest(&key)));
        assert_eq!(key_digest(&key).len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(KEYS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let reloaded = Keys::load(&dir);
        assert_eq!(reloaded.verify(&key), Some(view.id.clone()));
        assert_eq!(reloaded.list()[0].label, "ci runner");

        reloaded.remove(&view.id).unwrap();
        assert_eq!(reloaded.verify(&key), None);
        assert!(reloaded.remove(&view.id).is_err());
        assert!(keys.create("   ", 1).is_err());
    }

    #[test]
    fn e2b_calls_need_a_key_and_the_owner_token_is_not_one() {
        let dir = scratch("auth");
        let keys = Keys::load(&dir);
        let (_, key) = keys.create("sdk", 1).unwrap();
        let owner = "owner-token-0123456789abcdef";

        assert_eq!(
            authorize_with(&keys, &Request::default())
                .unwrap_err()
                .status,
            401
        );
        assert_eq!(
            authorize_with(&keys, &with_header("x-api-key", owner))
                .unwrap_err()
                .status,
            401
        );
        assert_eq!(
            authorize_with(
                &keys,
                &with_header("authorization", &format!("Bearer {owner}"))
            )
            .unwrap_err()
            .status,
            401
        );
        assert!(authorize_with(&keys, &with_header("x-api-key", &key)).is_ok());
        assert!(
            authorize_with(
                &keys,
                &with_header("authorization", &format!("Bearer {key}"))
            )
            .is_ok()
        );
        assert!(
            authorize_with(
                &keys,
                &with_header("authorization", &format!("Basic {key}"))
            )
            .is_err()
        );
    }

    #[test]
    fn e2b_paths_pass_the_hub_gate_and_the_owner_key_routes_do_not() {
        assert!(crate::hub_auth::is_public("POST", "/e2b/sandboxes"));
        assert!(crate::hub_auth::is_public("GET", "/e2b/sandboxes/sb1/"));
        assert!(!crate::hub_auth::is_public("POST", "/v1/e2b/keys"));
    }

    #[test]
    fn a_new_sandbox_is_checked_and_defaults_as_the_spec_says() {
        let parse = |text: &str| serde_json::from_str::<NewSandbox>(text).unwrap();
        let spec = Spec::from_body(parse(r#"{"templateID":"base"}"#)).unwrap();
        assert_eq!(spec.timeout_s, 300);
        assert!(spec.secure);
        assert!(spec.allow_internet);
        assert!(!spec.auto_pause);
        assert_eq!(spec.template, "base");

        let custom = Spec::from_body(parse(
            r#"{"templateID":"my-image:v2","secure":false,"allow_internet_access":false,"autoPause":true,"timeout":86400,"metadata":{"a":"b"},"envVars":{"K":"V"}}"#,
        ))
        .unwrap();
        assert!(!custom.secure && !custom.allow_internet && custom.auto_pause);
        assert_eq!(custom.timeout_s, 86_400);
        assert_eq!(custom.env_vars["K"], "V");

        for bad in [
            r#"{}"#,
            r#"{"templateID":"   "}"#,
            r#"{"templateID":"base","timeout":86401}"#,
            r#"{"templateID":"base","timeout":0}"#,
            r#"{"templateID":"base","timeout":-5}"#,
            r#"{"templateID":"base","envVars":{"A=B":"x"}}"#,
        ] {
            let body = serde_json::from_str::<NewSandbox>(bad);
            let refused = body.ok().map(Spec::from_body);
            assert!(!matches!(refused, Some(Ok(_))), "{bad} should be refused");
        }
        // A metadata value that is not a string is refused at parse time.
        assert!(
            serde_json::from_str::<NewSandbox>(r#"{"templateID":"base","metadata":{"a":1}}"#)
                .is_err()
        );
        assert_eq!(seconds(10, 0, 3600, "duration").unwrap(), 10);
        assert!(seconds(3601, 0, 3600, "duration").is_err());
    }

    #[test]
    fn the_responses_use_the_spec_field_names() {
        let sandbox = sample(true);
        let created = created_view(&sandbox);
        assert_eq!(created["sandboxID"], "sb0123456789abcdef");
        assert_eq!(created["templateID"], "base");
        assert_eq!(created["clientID"], "agentplaza");
        assert_eq!(created["envdVersion"], "0.5.7");
        assert_eq!(created["envdAccessToken"], "token");
        assert!(created["domain"].is_null());

        let mut unsecured = sample(true);
        unsecured.access_token = None;
        assert!(created_view(&unsecured).get("envdAccessToken").is_none());

        let listed = listed_view(&sandbox);
        for field in [
            "templateID",
            "sandboxID",
            "clientID",
            "startedAt",
            "endAt",
            "cpuCount",
            "memoryMB",
            "diskSizeMB",
            "metadata",
            "state",
            "envdVersion",
        ] {
            assert!(listed.get(field).is_some(), "ListedSandbox lacks {field}");
        }
        assert_eq!(listed["state"], "running");
        assert_eq!(listed["startedAt"], "1970-01-01T00:00:01.000Z");

        let mut paused = sample(false);
        paused.phase = Phase::Paused;
        let detail = detail_view(&paused);
        assert_eq!(detail["state"], "paused");
        assert_eq!(detail["allowInternetAccess"], false);
        assert_eq!(detail["envdAccessToken"], "token");

        let snap = Snapshot {
            id: "sb0123456789abcdef:cp-1-x".into(),
            name: "before-deploy".into(),
            checkpoint: "cp-1-x".into(),
            taken_ms: 1,
        };
        assert_eq!(snapshot_view(&snap)["names"][0], "before-deploy");
        assert_eq!(
            snapshot_view(&snap)["snapshotID"],
            "sb0123456789abcdef:cp-1-x"
        );
    }

    #[test]
    fn timestamps_are_rfc3339_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn the_reaper_decides_from_the_end_time_and_held_ends_everything() {
        // Held: every sandbox ends, whatever its phase and end time.
        assert_eq!(reap_decision(true, false, u64::MAX, 0, true), Reap::Kill);
        // Running and before its end: kept.
        assert_eq!(reap_decision(false, true, 1_000, 999, false), Reap::Keep);
        // Running and past its end: paused when it auto-pauses, ended when it does not.
        assert_eq!(reap_decision(false, true, 1_000, 1_000, true), Reap::Pause);
        assert_eq!(reap_decision(false, true, 1_000, 1_000, false), Reap::Kill);
        // Paused: kept until a connect wakes it.
        assert_eq!(reap_decision(false, false, 1_000, 9_000, false), Reap::Keep);
    }

    #[test]
    fn a_refresh_keeps_alive_for_its_duration_and_never_past_a_day() {
        let now = 1_000_000;
        assert_eq!(refreshed_end(5_000_000, now, 10), 5_000_000);
        assert_eq!(refreshed_end(now + 1_000, now, 60), now + 60_000);
        assert_eq!(
            refreshed_end(u64::MAX / 2, now, 10),
            ends_at(now, MAX_TIMEOUT_S)
        );
    }

    #[test]
    fn egress_from_a_sandbox_follows_its_internet_flag_and_the_laws() {
        let offline = sample(false);
        let online = sample(true);
        // No internet: refused even for a host nothing blocks.
        let verdict = sandbox_verdict(&offline, false, None, "example.com");
        assert!(!verdict.allow);
        assert!(verdict.reason.unwrap().contains("no internet access"));
        // Internet: the fetch blocklist decides, and a public host passes.
        assert!(!sandbox_verdict(&online, true, None, "evil.example").allow);
        assert!(sandbox_verdict(&online, false, None, "example.com").allow);
        // A law refusal names the law, with or without internet.
        let law = Some("refused by law S-013d: no sandbox egress".to_string());
        let refused = sandbox_verdict(&online, false, law.clone(), "example.com");
        assert!(!refused.allow);
        assert_eq!(refused.law.as_deref(), Some("S-013d"));
        let refused = sandbox_verdict(&offline, false, law, "example.com");
        assert_eq!(refused.law.as_deref(), Some("S-013d"));
    }

    #[test]
    fn the_sandbox_store_survives_a_restart_and_a_damaged_file_is_never_replaced() {
        let dir = scratch("sandboxes");
        let store = Sandboxes::load(&dir);
        store.insert(sample(true)).unwrap();
        store
            .update("sb0123456789abcdef", |s| s.phase = Phase::Paused)
            .unwrap();
        let reloaded = Sandboxes::load(&dir);
        let back = reloaded.get("sb0123456789abcdef").unwrap();
        assert_eq!(back.phase, Phase::Paused);
        assert_eq!(back.cid, 42);
        assert_eq!(reloaded.by_cid(42).unwrap().id, "sb0123456789abcdef");
        assert!(reloaded.by_cid(7).is_none());

        reloaded.remove("sb0123456789abcdef").unwrap();
        assert!(Sandboxes::load(&dir).list().is_empty());

        std::fs::write(dir.join(SANDBOXES_FILE), b"{ not json").unwrap();
        let damaged = Sandboxes::load(&dir);
        assert!(damaged.insert(sample(true)).is_err());
        assert_eq!(
            std::fs::read(dir.join(SANDBOXES_FILE)).unwrap(),
            b"{ not json"
        );
    }

    #[test]
    fn the_gateway_port_flag_is_read_and_taken_out_of_the_arguments() {
        let args = [
            "--port",
            "7700",
            "--e2b-gateway-port",
            "4242",
            "--dev-models",
        ]
        .map(str::to_string)
        .to_vec();
        let (port, rest) = split_gateway_port(args).unwrap();
        assert_eq!(port, Some(4242));
        assert_eq!(rest, ["--port", "7700", "--dev-models"]);
        assert!(split_gateway_port(vec!["--e2b-gateway-port".into()]).is_err());
        assert!(split_gateway_port(vec!["--e2b-gateway-port".into(), "x".into()]).is_err());
        assert_eq!(split_gateway_port(vec![]).unwrap().0, None);
    }

    #[test]
    fn sandbox_ids_are_sb_and_sixteen_hex_digits() {
        let id = format!("sb{}", hex_of(&apz_trust::random::<8>()));
        assert_eq!(id.len(), 18);
        assert!(id.starts_with("sb"));
        assert!(id[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
