//! The coding engines as instances the owner manages beside the API providers (`/v1/engines`): the
//! built-in engines the harness ships (Claude Code, Codex, OpenCode …), and extra ones the owner adds from
//! the ACP registry (`cdn.agentclientprotocol.com`) or as a local ACP command.
//!
//! An engine runs only inside an agent's VM, and it reaches models only through the plaza's gateway: no
//! key ever goes into an engine's environment (the agent could read it). An instance holds what the VM
//! needs to start it: the package and version to install with its integrity pin, the launch arguments,
//! plain environment variables, the engine's home folder and its default model route.
//!
//! The built-in engines' on/off switch stays `engines_enabled` in the settings, so every check that reads it
//! keeps working; their other fields, and the extra instances, live in plaza.db (table `engine_instance`, with
//! its install, arguments, environment and registry details as child tables). The ACP registry's offline copy
//! stays a file (`acp-registry.json`): a fetched cache, not state of record. Versions the owner picks, and the
//! extra instances' packages, are pinned when they are saved: the hub resolves the npm `dist.integrity`, the PyPI
//! sha256, or hashes the release archive, and the VM refuses anything else.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::providers::{Failure, failure, lock, write_atomic};
use crate::state::App;

/// A built-in engine as the hub knows it: id, name, the npm (or other) package and the version the harness
/// pins (`agent-harness/src/acp/catalog.rs`; a test keeps the two in step).
pub struct Builtin {
    pub id: &'static str,
    pub label: &'static str,
    /// `npm`, `release` or `pip`: only npm engines can move to another version from the consoles.
    pub source: &'static str,
    pub package: &'static str,
    pub pinned: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    b(
        "claude",
        "Claude Code",
        "npm",
        "@agentclientprotocol/claude-agent-acp",
        "0.84.0",
    ),
    b(
        "codex",
        "Codex",
        "npm",
        "@agentclientprotocol/codex-acp",
        "2.0.1",
    ),
    b("opencode", "OpenCode", "npm", "opencode-ai", "1.18.34"),
    b(
        "gemini",
        "Gemini CLI",
        "npm",
        "@google/gemini-cli",
        "0.62.0",
    ),
    b("qwen", "Qwen Code", "npm", "@qwen-code/qwen-code", "0.24.7"),
    b("kilo", "Kilo Code", "npm", "@kilocode/cli", "7.8.1"),
    b("cline", "Cline", "npm", "cline", "3.0.67"),
    b(
        "copilot",
        "GitHub Copilot",
        "npm",
        "@github/copilot",
        "1.0.90",
    ),
    b("goose", "Goose", "release", "goose", "1.52.0"),
    b(
        "kimi",
        "Kimi Code",
        "npm",
        "@moonshot-ai/kimi-code",
        "2.1.1",
    ),
    b("auggie", "Auggie", "npm", "@augmentcode/auggie", "0.36.0"),
    b("vibe", "Mistral Vibe", "pip", "mistral-vibe", "2.25.0"),
];

const fn b(
    id: &'static str,
    label: &'static str,
    source: &'static str,
    package: &'static str,
    pinned: &'static str,
) -> Builtin {
    Builtin {
        id,
        label,
        source,
        package,
        pinned,
    }
}

/// Where the ACP registry is published.
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
/// How long a fetched registry is served before it is fetched again.
const REGISTRY_TTL_MS: u64 = 6 * 3_600_000;
/// The architectures agent VMs run on, as the registry names them.
const VM_PLATFORMS: [&str; 2] = ["linux-x86_64", "linux-aarch64"];
/// The most an archive the hub pins may weigh.
const ARCHIVE_LIMIT: u64 = 1 << 30;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(600);

/// The most environment variables, arguments and characters an instance may carry.
const MAX_ENV: usize = 40;
const MAX_ARGS: usize = 32;
const MAX_VALUE: usize = 4096;

/// Variables the harness sets to route an engine through the plaza's gateway: an instance never overrides them.
const RESERVED_ENV: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_BASE_URL",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_GEMINI_BASE_URL",
    "HOME",
    "PATH",
    "XDG_CONFIG_HOME",
    "NODE_OPTIONS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// What the VM installs for an instance, and one archive of it: the wire's types, stored as they are sent.
pub use apz_proto::wire::{EngineAsset as Asset, EngineInstall as Install};

/// One engine instance's stored fields. A built-in engine's entry exists only once the owner changed one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stored {
    pub id: String,
    /// A built-in engine id, `acp:<registry id>` or `acp-local`.
    pub engine: String,
    pub label: Option<String>,
    pub accent: Option<String>,
    /// Extra instances only: a built-in engine's switch is `engines_enabled`.
    pub enabled: bool,
    /// The pinned install. None: the catalogue's pin (built-in engines).
    pub install: Option<Install>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// The engine's home folder name under the VM's engines home; None: the engine's id.
    pub home: Option<String>,
    /// The model the engine runs when a call names none, `<provider>/<model>`.
    pub route: Option<String>,
    /// For registry instances: the registry entry's name, description and icon, as added.
    pub about: Option<About>,
}

/// A registry instance's entry as it was added: the registry's name, text, icon and links.
#[derive(Debug, Clone, Default, PartialEq, Serialize, apz_store::Table)]
pub struct About {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub website: Option<String>,
    pub repository: Option<String>,
    pub version: Option<String>,
}

/// An instance as the store holds it (table `engine_instance`, keyed by its id; the list keeps its order). The
/// install, the about and the arguments and environment are child tables; [`From`] and [`TryFrom`] convert at the
/// boundary.
#[derive(Debug, Clone, apz_store::Table)]
#[table(name = "engine_instance")]
struct InstanceRow {
    #[table(key)]
    id: String,
    engine: String,
    label: Option<String>,
    accent: Option<String>,
    enabled: bool,
    #[table(child = "engine_install")]
    install: Option<InstallRow>,
    #[table(list = "engine_arg")]
    args: Vec<String>,
    #[table(map_value = "engine_env")]
    env: BTreeMap<String, String>,
    home: Option<String>,
    route: Option<String>,
    #[table(child = "engine_about")]
    about: Option<About>,
}

/// Which install an [`InstallRow`] holds: each kind uses its own fields of the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, apz_store::Column)]
enum InstallKind {
    Npm,
    Pip,
    Archive,
    Command,
}

/// A pinned install as the store holds it: its kind and the fields that kind uses (the others are unset).
#[derive(Debug, Clone, apz_store::Table)]
struct InstallRow {
    kind: InstallKind,
    package: Option<String>,
    version: Option<String>,
    integrity: Option<String>,
    file: Option<String>,
    sha256: Option<String>,
    command: Option<String>,
    #[table(map = "engine_asset")]
    assets: BTreeMap<String, AssetRow>,
}

/// One architecture's archive as the store holds it.
#[derive(Debug, Clone, apz_store::Table)]
struct AssetRow {
    url: String,
    sha256: String,
    cmd: String,
    #[table(list = "engine_asset_arg")]
    args: Vec<String>,
}

/// The engines' table: `engine_instance` and its child tables.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(InstanceRow)
}

impl From<&Stored> for InstanceRow {
    fn from(s: &Stored) -> Self {
        InstanceRow {
            id: s.id.clone(),
            engine: s.engine.clone(),
            label: s.label.clone(),
            accent: s.accent.clone(),
            enabled: s.enabled,
            install: s.install.as_ref().map(InstallRow::from),
            args: s.args.clone(),
            env: s.env.clone(),
            home: s.home.clone(),
            route: s.route.clone(),
            about: s.about.clone(),
        }
    }
}

impl TryFrom<InstanceRow> for Stored {
    type Error = StoreError;

    fn try_from(row: InstanceRow) -> Result<Self, StoreError> {
        Ok(Stored {
            id: row.id,
            engine: row.engine,
            label: row.label,
            accent: row.accent,
            enabled: row.enabled,
            install: row.install.map(Install::try_from).transpose()?,
            args: row.args,
            env: row.env,
            home: row.home,
            route: row.route,
            about: row.about,
        })
    }
}

impl From<&Install> for InstallRow {
    fn from(install: &Install) -> Self {
        let blank = |kind: InstallKind| InstallRow {
            kind,
            package: None,
            version: None,
            integrity: None,
            file: None,
            sha256: None,
            command: None,
            assets: BTreeMap::new(),
        };
        match install {
            Install::Npm {
                package,
                version,
                integrity,
            } => InstallRow {
                package: Some(package.clone()),
                version: Some(version.clone()),
                integrity: Some(integrity.clone()),
                ..blank(InstallKind::Npm)
            },
            Install::Pip {
                package,
                version,
                file,
                sha256,
            } => InstallRow {
                package: Some(package.clone()),
                version: Some(version.clone()),
                file: Some(file.clone()),
                sha256: Some(sha256.clone()),
                ..blank(InstallKind::Pip)
            },
            Install::Archive { version, assets } => InstallRow {
                version: Some(version.clone()),
                assets: assets
                    .iter()
                    .map(|(platform, asset)| (platform.clone(), AssetRow::from(asset)))
                    .collect(),
                ..blank(InstallKind::Archive)
            },
            Install::Command { command } => InstallRow {
                command: Some(command.clone()),
                ..blank(InstallKind::Command)
            },
        }
    }
}

impl TryFrom<InstallRow> for Install {
    type Error = StoreError;

    fn try_from(row: InstallRow) -> Result<Self, StoreError> {
        Ok(match row.kind {
            InstallKind::Npm => Install::Npm {
                package: need(row.package, "package")?,
                version: need(row.version, "version")?,
                integrity: need(row.integrity, "integrity")?,
            },
            InstallKind::Pip => Install::Pip {
                package: need(row.package, "package")?,
                version: need(row.version, "version")?,
                file: need(row.file, "file")?,
                sha256: need(row.sha256, "sha256")?,
            },
            InstallKind::Archive => Install::Archive {
                version: need(row.version, "version")?,
                assets: row
                    .assets
                    .into_iter()
                    .map(|(platform, asset)| (platform, Asset::from(asset)))
                    .collect(),
            },
            InstallKind::Command => Install::Command {
                command: need(row.command, "command")?,
            },
        })
    }
}

/// A field an install's kind needs: a stored install without it is refused at load.
fn need<T>(value: Option<T>, field: &str) -> Result<T, StoreError> {
    value.ok_or_else(|| StoreError::invalid(format!("an engine's install has no {field}")))
}

impl From<&Asset> for AssetRow {
    fn from(a: &Asset) -> Self {
        AssetRow {
            url: a.url.clone(),
            sha256: a.sha256.clone(),
            cmd: a.cmd.clone(),
            args: a.args.clone(),
        }
    }
}

impl From<AssetRow> for Asset {
    fn from(row: AssetRow) -> Self {
        Asset {
            url: row.url,
            sha256: row.sha256,
            cmd: row.cmd,
            args: row.args,
        }
    }
}

/// A registry field as text: the registry's string, or none.
fn text_of(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// The owner's edit of an instance (`POST` and `PUT /v1/engines…`). Absent fields keep their value.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Edit {
    pub id: Option<String>,
    pub engine: Option<String>,
    pub label: Option<Option<String>>,
    pub accent: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// An npm engine's version; `pinned` goes back to the catalogue's.
    pub version: Option<String>,
    /// `acp-local` only.
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
    pub home: Option<Option<String>>,
    pub route: Option<Option<String>>,
}

/// Fetches a URL for the store: the text of a JSON document, or an archive's sha256. A seam for the tests.
pub trait Fetch: Send + Sync {
    fn text(&self, url: &str) -> Result<String, String>;
    fn sha256(&self, url: &str) -> Result<String, String>;
}

/// The real fetcher: HTTPS from the hub, with the apid user agent.
pub struct Web;

impl Fetch for Web {
    fn text(&self, url: &str) -> Result<String, String> {
        let mut response = get(url, FETCH_TIMEOUT)?;
        response
            .body_mut()
            .with_config()
            .limit(16 << 20)
            .read_to_string()
            .map_err(|e| format!("reading {url}: {e}"))
    }

    fn sha256(&self, url: &str) -> Result<String, String> {
        let mut response = get(url, ARCHIVE_TIMEOUT)?;
        let mut reader = response
            .body_mut()
            .with_config()
            .limit(ARCHIVE_LIMIT)
            .reader();
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("reading {url}: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex(&hasher.finalize()))
    }
}

fn get(url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, String> {
    if !url.starts_with("https://") {
        return Err(format!("{url} is not an https address"));
    }
    let request = ureq::http::Request::builder()
        .method("GET")
        .uri(url)
        .header("user-agent", "agentplaza")
        .body(Vec::new())
        .map_err(|e| format!("bad request: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    let response = ureq::Agent::new_with_config(config)
        .run(request)
        .map_err(|e| format!("{url} did not answer: {e}"))?;
    match response.status().as_u16() {
        200 => Ok(response),
        status => Err(format!("{url} answered {status}")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How long a looked-up newest version of an npm engine is kept before it is looked up again.
const LATEST_FOR: Duration = Duration::from_secs(3600);

/// The engines store: the instances (component `engines`), the cached registry, and the last check of each instance.
pub struct Store {
    /// The hub's generations: a save of the instances bumps `engines`.
    gens: std::sync::Arc<apz_cache::Generations<String>>,
    /// The offline copy of the ACP registry (`acp-registry.json`), read when first asked for.
    registry_path: PathBuf,
    stored: Mutex<Vec<Stored>>,
    registry: Mutex<Option<(u64, Value)>>,
    /// The newest version of each npm-installed instance and when it was looked up (PLAN 30.4: kept for
    /// [`LATEST_FOR`]; dropped when the instance is installed, updated or removed).
    latest: Mutex<BTreeMap<String, (String, Instant)>>,
    health: Mutex<BTreeMap<String, Value>>,
    write: Mutex<()>,
    fetch: Box<dyn Fetch>,
}

impl Store {
    /// An empty store that fetches the registry over HTTPS, with the offline copy in `state_dir`. It reads nothing
    /// yet: [`Store::load`] fills the instances at start.
    pub fn new(state_dir: &Path) -> Self {
        Self::with_fetch(state_dir, Box::new(Web))
    }

    /// As [`Store::new`], with the fetcher the caller gives (the tests answer from memory).
    pub fn with_fetch(state_dir: &Path, fetch: Box<dyn Fetch>) -> Self {
        Self {
            gens: Default::default(),
            registry_path: state_dir.join("acp-registry.json"),
            stored: Mutex::new(Vec::new()),
            registry: Mutex::new(None),
            latest: Mutex::new(BTreeMap::new()),
            health: Mutex::new(BTreeMap::new()),
            write: Mutex::new(()),
            fetch,
        }
    }

    /// Replaces the instances with what plaza.db holds (at start). A stored install that lacks a field its kind needs
    /// fails the load, and the saver then keeps the component out of writes.
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let rows: Vec<InstanceRow> = apz_store::table::load_list(conn, def())?;
        let stored = rows
            .into_iter()
            .map(Stored::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        *lock(&self.stored) = stored;
        Ok(())
    }

    /// The instances as rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let rows: Vec<InstanceRow> = lock(&self.stored).iter().map(InstanceRow::from).collect();
        RowSet::list(def(), rows.iter())
    }

    fn stored(&self, id: &str) -> Option<Stored> {
        lock(&self.stored).iter().find(|s| s.id == id).cloned()
    }

    /// Counts this store's changes in `gens` (the hub's generations).
    pub fn with_generations(
        mut self,
        gens: std::sync::Arc<apz_cache::Generations<String>>,
    ) -> Self {
        self.gens = gens;
        self
    }

    /// Replaces the instances in memory and counts the change; the saver writes them to plaza.db.
    fn save(&self, all: Vec<Stored>) -> Result<(), Failure> {
        *lock(&self.stored) = all;
        crate::sources::bump(&self.gens, crate::sources::ENGINES);
        Ok(())
    }

    /// The extra instances that are enabled.
    pub fn enabled_extras(&self) -> Vec<String> {
        lock(&self.stored)
            .iter()
            .filter(|s| builtin(&s.id).is_none() && s.enabled)
            .map(|s| s.id.clone())
            .collect()
    }

    /// What the VM needs to start instance `id` beyond the catalogue's defaults: its pinned install, its
    /// arguments, environment, home and route. None for a built-in engine the owner never changed.
    pub fn launch_spec(&self, id: &str) -> Option<apz_proto::wire::EngineSpec> {
        let s = self.stored(id)?;
        Some(apz_proto::wire::EngineSpec {
            engine: s.engine,
            install: s.install,
            args: s.args,
            env: s.env,
            home: s.home,
            route: s.route,
        })
    }

    /// Every instance's view: the built-in engines in the catalogue's order, then the extra ones.
    fn views(&self, enabled_builtins: &[String]) -> Vec<Value> {
        let stored = lock(&self.stored).clone();
        let mut out: Vec<Value> = BUILTINS
            .iter()
            .map(|b| {
                let s = stored
                    .iter()
                    .find(|s| s.id == b.id)
                    .cloned()
                    .unwrap_or_else(|| Stored {
                        id: b.id.into(),
                        engine: b.id.into(),
                        ..Default::default()
                    });
                self.view(&s, enabled_builtins.iter().any(|e| e == b.id))
            })
            .collect();
        out.extend(
            stored
                .iter()
                .filter(|s| builtin(&s.id).is_none())
                .map(|s| self.view(s, s.enabled)),
        );
        out
    }

    fn view(&self, s: &Stored, enabled: bool) -> Value {
        let b = builtin(&s.engine);
        let version = installed_version(s);
        let latest = lock(&self.latest).get(&s.id).map(|(v, _)| v.clone());
        let health = if enabled {
            lock(&self.health)
                .get(&s.id)
                .cloned()
                .unwrap_or_else(|| json!({ "status": "unknown", "message": "Not checked yet" }))
        } else {
            json!({ "status": "off", "message": "Disabled" })
        };
        let label = s
            .label
            .clone()
            .or_else(|| b.map(|b| b.label.to_string()))
            .or_else(|| s.about.as_ref().and_then(|a| a.name.clone()))
            .unwrap_or_else(|| s.id.clone());
        json!({
            "id": s.id,
            "family": "engine",
            "engine": s.engine,
            "builtin": b.is_some() && s.id == s.engine,
            "label": label,
            "accent": s.accent,
            "enabled": enabled,
            "source": b.map_or_else(|| install_source(s), |b| b.source.to_string()),
            "package": b.map(|b| b.package),
            "pinned": b.map(|b| b.pinned),
            "version": version,
            "latest": latest,
            "install": s.install,
            "args": s.args,
            "env": s.env,
            "home": s.home.clone().unwrap_or_else(|| s.id.clone()),
            "route": s.route,
            "about": s.about,
            "health": health,
        })
    }

    /// Adds an extra instance from the registry (`acp:<id>`) or a local command (`acp-local`).
    fn add(
        &self,
        edit: Edit,
        reg: impl FnOnce() -> Result<Value, Failure>,
    ) -> Result<Stored, Failure> {
        let _guard = lock(&self.write);
        let id = edit.id.clone().ok_or((400, "id is required".to_string()))?;
        if !valid_id(&id) {
            return Err((400, "id must match [a-z0-9_-]{1,40}".into()));
        }
        if builtin(&id).is_some() || lock(&self.stored).iter().any(|s| s.id == id) {
            return Err((409, format!("engine `{id}` exists")));
        }
        let engine = edit
            .engine
            .clone()
            .ok_or((400, "engine is required".to_string()))?;
        let mut s = Stored {
            id: id.clone(),
            engine: engine.clone(),
            enabled: true,
            ..Default::default()
        };
        if let Some(rid) = engine.strip_prefix("acp:") {
            let registry = reg()?;
            let entry = registry["agents"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["id"] == rid))
                .cloned()
                .ok_or_else(|| (400, format!("the ACP registry has no agent `{rid}`")))?;
            s.install = Some(self.pin_entry(&entry)?);
            s.args = entry_args(&entry);
            s.about = Some(About {
                name: text_of(&entry["name"]),
                description: text_of(&entry["description"]),
                icon: text_of(&entry["icon"]),
                website: text_of(&entry["website"]),
                repository: text_of(&entry["repository"]),
                version: text_of(&entry["version"]),
            });
        } else if builtin(&engine).is_some() {
            // A second instance of a built-in engine: the same package, its own settings.
            s.install = None;
        } else if engine != "acp-local" {
            return Err((
                400,
                "engine must be a built-in engine, acp:<registry id> or acp-local".into(),
            ));
        }
        if engine == "acp-local" {
            let command = edit.command.clone().unwrap_or_default();
            check_command(&command)?;
            s.install = Some(Install::Command { command });
        }
        apply(&mut s, &edit, false)?;
        if let Some(version) = &edit.version {
            s.install = self.pin_version(&s, version)?;
        }
        let mut all = lock(&self.stored).clone();
        all.push(s.clone());
        self.save(all)?;
        Ok(s)
    }

    /// Changes an instance. A built-in engine's `enabled` is answered by the caller (the settings own it).
    fn edit(&self, id: &str, edit: &Edit) -> Result<Stored, Failure> {
        let _guard = lock(&self.write);
        let mut s = match (self.stored(id), builtin(id)) {
            (Some(s), _) => s,
            (None, Some(b)) => Stored {
                id: b.id.into(),
                engine: b.id.into(),
                ..Default::default()
            },
            (None, None) => return Err((404, format!("no engine `{id}`"))),
        };
        if edit.id.as_deref().is_some_and(|i| i != id)
            || edit.engine.as_deref().is_some_and(|e| e != s.engine)
        {
            return Err((400, "an engine's id and engine do not change".into()));
        }
        apply(&mut s, edit, builtin(id).is_some())?;
        if let Some(version) = &edit.version {
            s.install = self.pin_version(&s, version)?;
        }
        if let (Some(command), Some(Install::Command { .. })) = (&edit.command, &s.install) {
            check_command(command)?;
            s.install = Some(Install::Command {
                command: command.clone(),
            });
        }
        let mut all = lock(&self.stored).clone();
        all.retain(|o| o.id != id);
        all.push(s.clone());
        self.save(all)?;
        Ok(s)
    }

    fn remove(&self, id: &str) -> Result<(), Failure> {
        let _guard = lock(&self.write);
        if builtin(id).is_some() {
            return Err((400, "a built-in engine is turned off, not removed".into()));
        }
        let mut all = lock(&self.stored).clone();
        let before = all.len();
        all.retain(|s| s.id != id);
        if all.len() == before {
            return Err((404, format!("no engine `{id}`")));
        }
        lock(&self.health).remove(id);
        lock(&self.latest).remove(id);
        self.save(all)
    }

    /// The npm package an instance installs from, when it installs from npm.
    fn npm_package(&self, s: &Stored) -> Option<String> {
        match (&s.install, builtin(&s.engine)) {
            (Some(Install::Npm { package, .. }), _) => Some(package.clone()),
            (None, Some(b)) if b.source == "npm" => Some(b.package.to_string()),
            _ => None,
        }
    }

    /// Pins an npm instance to `version` (`pinned`: back to the catalogue's).
    fn pin_version(&self, s: &Stored, version: &str) -> Result<Option<Install>, Failure> {
        if version == "pinned" {
            if builtin(&s.engine).is_none() {
                return Err((400, "only a built-in engine has a catalogue pin".into()));
            }
            return Ok(None);
        }
        let package = self.npm_package(s).ok_or_else(|| {
            (
                400,
                "only an engine installed from npm changes version here".to_string(),
            )
        })?;
        Ok(Some(self.npm_pin(&package, version)?))
    }

    /// The npm registry's integrity for `package@version` (`latest` resolves the newest).
    fn npm_pin(&self, package: &str, version: &str) -> Result<Install, Failure> {
        if !valid_package(package) || !valid_version(version) {
            return Err((
                400,
                format!("{package}@{version} is not an npm package version"),
            ));
        }
        let url = format!(
            "https://registry.npmjs.org/{}/{version}",
            package.replace('/', "%2F")
        );
        let text = self.fetch.text(&url).map_err(|e| (502, e))?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| (502, format!("npm answered: {e}")))?;
        let (Some(version), Some(integrity)) =
            (doc["version"].as_str(), doc["dist"]["integrity"].as_str())
        else {
            return Err((502, format!("npm has no integrity for {package}@{version}")));
        };
        Ok(Install::Npm {
            package: package.into(),
            version: version.into(),
            integrity: integrity.into(),
        })
    }

    /// A registry entry's distribution, pinned: npx through npm's integrity, uvx through PyPI's sha256,
    /// binary archives by hashing them.
    fn pin_entry(&self, entry: &Value) -> Result<Install, Failure> {
        let dist = &entry["distribution"];
        if let Some(npx) = dist.get("npx") {
            let spec = npx["package"].as_str().unwrap_or("");
            let (package, version) = split_spec(spec);
            return self.npm_pin(package, version.unwrap_or("latest"));
        }
        if let Some(uvx) = dist.get("uvx") {
            let spec = uvx["package"].as_str().unwrap_or("");
            let (package, version) = split_spec(spec);
            let version = version.or(entry["version"].as_str()).unwrap_or("");
            return self.pip_pin(package, version);
        }
        if let Some(binary) = dist.get("binary") {
            let mut assets = BTreeMap::new();
            for platform in VM_PLATFORMS {
                let Some(asset) = binary.get(platform) else {
                    continue;
                };
                let url = asset["archive"].as_str().unwrap_or("").to_string();
                let sha256 = self.fetch.sha256(&url).map_err(|e| (502, e))?;
                assets.insert(
                    platform.to_string(),
                    Asset {
                        url,
                        sha256,
                        cmd: asset["cmd"].as_str().unwrap_or("").to_string(),
                        args: strings(&asset["args"]),
                    },
                );
            }
            if assets.is_empty() {
                return Err((
                    400,
                    "this agent ships no Linux build, so it cannot run in an agent's VM".into(),
                ));
            }
            return Ok(Install::Archive {
                version: entry["version"].as_str().unwrap_or("").to_string(),
                assets,
            });
        }
        Err((
            400,
            "this registry entry has no distribution the VM can install".into(),
        ))
    }

    fn pip_pin(&self, package: &str, version: &str) -> Result<Install, Failure> {
        if !valid_package(package) || !valid_version(version) {
            return Err((
                400,
                format!("{package}=={version} is not a PyPI package version"),
            ));
        }
        let text = self
            .fetch
            .text(&format!("https://pypi.org/pypi/{package}/{version}/json"))
            .map_err(|e| (502, e))?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| (502, format!("PyPI answered: {e}")))?;
        let wheel = doc["urls"]
            .as_array()
            .and_then(|urls| urls.iter().find(|u| u["packagetype"] == "bdist_wheel"))
            .ok_or_else(|| (502, format!("PyPI has no wheel for {package}=={version}")))?;
        Ok(Install::Pip {
            package: package.into(),
            version: doc["info"]["version"].as_str().unwrap_or(version).into(),
            file: wheel["filename"].as_str().unwrap_or("").into(),
            sha256: wheel["digests"]["sha256"].as_str().unwrap_or("").into(),
        })
    }

    /// The registry held in memory, or else the offline copy in `acp-registry.json` (read the first time it is asked for).
    fn cached_registry(&self) -> Option<(u64, Value)> {
        let mut held = lock(&self.registry);
        if held.is_none() {
            *held = std::fs::read(&self.registry_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|v| Some((v["fetched_ms"].as_u64()?, v["registry"].clone())));
        }
        held.clone()
    }

    /// The registry, fetched again when older than [`REGISTRY_TTL_MS`] or when `refresh`; the last copy
    /// when the fetch fails (offline hubs keep working).
    pub fn registry(&self, refresh: bool) -> Result<(u64, Value), Failure> {
        let now = apz_plaza::now_ms();
        let cached = self.cached_registry();
        if let Some((at, reg)) = &cached
            && !refresh
            && now.saturating_sub(*at) < REGISTRY_TTL_MS
        {
            return Ok((*at, reg.clone()));
        }
        match self
            .fetch
            .text(REGISTRY_URL)
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
        {
            Ok(reg) => {
                let doc = json!({ "fetched_ms": now, "registry": reg });
                let _ = write_atomic(&self.registry_path, doc.to_string().as_bytes(), 0o600);
                *lock(&self.registry) = Some((now, reg.clone()));
                Ok((now, reg))
            }
            Err(e) => cached.ok_or((502, format!("the ACP registry could not be read: {e}"))),
        }
    }

    /// Looks up the newest version of every npm instance and rebuilds each enabled instance's health from
    /// its version and its route's provider.
    fn check(&self, app: &App, enabled_builtins: &[String]) -> Value {
        self.refresh_latest(enabled_builtins);
        let mut out = serde_json::Map::new();
        for view in self.views(enabled_builtins) {
            let id = view["id"].as_str().unwrap_or("").to_string();
            if !view["enabled"].as_bool().unwrap_or(false) {
                out.insert(id, view["health"].clone());
                continue;
            }
            let s = self.stored(&id).unwrap_or_else(|| Stored {
                id: id.clone(),
                engine: id.clone(),
                ..Default::default()
            });
            let latest = self
                .npm_package(&s)
                .and_then(|_| lock(&self.latest).get(&id).map(|(v, _)| v.clone()));
            let health = engine_health(app, &view, latest.as_deref());
            lock(&self.health).insert(id.clone(), health.clone());
            out.insert(id, health);
        }
        Value::Object(out)
    }

    /// Looks up the newest version of every enabled npm instance whose last lookup is older than [`LATEST_FOR`],
    /// all at once (one thread each).
    fn refresh_latest(&self, enabled_builtins: &[String]) {
        let due: Vec<(String, String)> = self
            .views(enabled_builtins)
            .into_iter()
            .filter(|view| view["enabled"].as_bool().unwrap_or(false))
            .filter_map(|view| {
                let id = view["id"].as_str()?.to_string();
                let s = self.stored(&id).unwrap_or_else(|| Stored {
                    id: id.clone(),
                    engine: id.clone(),
                    ..Default::default()
                });
                let package = self.npm_package(&s)?;
                let fresh = lock(&self.latest)
                    .get(&id)
                    .is_some_and(|(_, at)| at.elapsed() < LATEST_FOR);
                (!fresh).then_some((id, package))
            })
            .collect();
        let found: Vec<(String, String)> = std::thread::scope(|scope| {
            let running: Vec<_> = due
                .iter()
                .map(|(id, package)| {
                    scope.spawn(move || match self.npm_pin(package, "latest") {
                        Ok(Install::Npm { version, .. }) => Some((id.clone(), version)),
                        _ => None,
                    })
                })
                .collect();
            running
                .into_iter()
                .filter_map(|t| t.join().ok().flatten())
                .collect()
        });
        let now = Instant::now();
        let mut latest = lock(&self.latest);
        for (id, version) in found {
            latest.insert(id, (version, now));
        }
    }

    /// The npm instances with a newer version than the one they install, and — unless `dry_run` — pins
    /// each to it.
    fn update(&self, enabled_builtins: &[String], dry_run: bool) -> Result<Vec<Value>, Failure> {
        let mut changes = Vec::new();
        for view in self.views(enabled_builtins) {
            let id = view["id"].as_str().unwrap_or("").to_string();
            let s = self.stored(&id).unwrap_or_else(|| Stored {
                id: id.clone(),
                engine: id.clone(),
                ..Default::default()
            });
            let Some(package) = self.npm_package(&s) else {
                continue;
            };
            let Ok(pin) = self.npm_pin(&package, "latest") else {
                continue;
            };
            let Install::Npm { version: to, .. } = &pin else {
                continue;
            };
            let from = installed_version(&s).unwrap_or_default();
            if newer(to, &from) {
                changes.push(json!({ "id": id, "from": from, "to": to }));
                if !dry_run {
                    let mut all = lock(&self.stored).clone();
                    let mut next = s.clone();
                    next.install = Some(pin.clone());
                    all.retain(|o| o.id != id);
                    all.push(next);
                    self.save(all)?;
                }
            }
        }
        Ok(changes)
    }
}

/// An instance's health from what the hub knows: off, its version against the newest, and the health of
/// the provider its route goes through.
fn engine_health(app: &App, view: &Value, latest: Option<&str>) -> Value {
    let checked_ms = apz_plaza::now_ms();
    let version = view["version"].as_str().unwrap_or("");
    let route = view["route"]
        .as_str()
        .filter(|r| !r.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| app.settings.get().default_model);
    let provider = route.split('/').next().unwrap_or("").to_string();
    let provider_health = app
        .providers
        .views_for_engines()
        .into_iter()
        .find(|p| p["id"] == provider.as_str())
        .map(|p| p["health"].clone());
    let (status, message) = match provider_health.as_ref().and_then(|h| h["status"].as_str()) {
        None if provider.is_empty() => (
            "warn",
            "No model route · choose a default model".to_string(),
        ),
        None => (
            "crit",
            format!("Routes through {provider}, which is not set up"),
        ),
        Some("crit") => (
            "crit",
            format!(
                "{} · through {provider}",
                provider_health
                    .as_ref()
                    .and_then(|h| h["message"].as_str())
                    .unwrap_or("Not authenticated")
            ),
        ),
        Some("off") => (
            "crit",
            format!("Routes through {provider}, which is disabled"),
        ),
        Some(_) => match latest {
            Some(latest) if newer(latest, version) => {
                ("warn", format!("Update available · {latest}"))
            }
            _ => ("ok", format!("Ready · through {provider}")),
        },
    };
    json!({ "status": status, "message": message, "checked_ms": checked_ms, "route": route })
}

/// The version an instance installs.
fn installed_version(s: &Stored) -> Option<String> {
    match &s.install {
        Some(
            Install::Npm { version, .. }
            | Install::Pip { version, .. }
            | Install::Archive { version, .. },
        ) => Some(version.clone()),
        Some(Install::Command { .. }) => None,
        None => builtin(&s.engine).map(|b| b.pinned.to_string()),
    }
}

fn install_source(s: &Stored) -> String {
    match &s.install {
        Some(Install::Npm { .. }) => "npm",
        Some(Install::Pip { .. }) => "pip",
        Some(Install::Archive { .. }) => "release",
        Some(Install::Command { .. }) => "command",
        None => "catalogue",
    }
    .into()
}

pub fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// Applies the owner's edit to the fields every instance shares, checking each.
fn apply(s: &mut Stored, edit: &Edit, is_builtin: bool) -> Result<(), Failure> {
    if let Some(label) = &edit.label {
        if let Some(l) = label
            && (l.trim().is_empty() || l.chars().count() > 60 || l.chars().any(char::is_control))
        {
            return Err((
                400,
                "label must be 1 to 60 characters, with no control characters".into(),
            ));
        }
        s.label = label.clone();
    }
    if let Some(accent) = &edit.accent {
        if let Some(a) = accent
            && !(a.len() == 7
                && a.starts_with('#')
                && a[1..].bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err((400, "accent must be a #rrggbb colour".into()));
        }
        s.accent = accent.clone();
    }
    if let Some(enabled) = edit.enabled
        && !is_builtin
    {
        s.enabled = enabled;
    }
    if let Some(args) = &edit.args {
        if args.len() > MAX_ARGS || args.iter().any(|a| a.len() > 512 || a.contains('\0')) {
            return Err((
                400,
                format!("at most {MAX_ARGS} arguments of at most 512 characters"),
            ));
        }
        s.args = args.clone();
    }
    if let Some(env) = &edit.env {
        check_env(env)?;
        s.env = env.clone();
    }
    if let Some(home) = &edit.home {
        if let Some(h) = home
            && !valid_id(h)
        {
            return Err((400, "home is a folder name: [a-z0-9_-]{1,40}".into()));
        }
        s.home = home.clone();
    }
    if let Some(route) = &edit.route {
        if let Some(r) = route
            && !r
                .split_once('/')
                .is_some_and(|(p, m)| !p.is_empty() && !m.is_empty())
        {
            return Err((400, "route is a <provider>/<model> name".into()));
        }
        s.route = route.clone();
    }
    Ok(())
}

/// An engine's environment holds plain settings only: never a key (the agent can read it) and never a
/// variable the harness sets to route the engine through the gateway.
fn check_env(env: &BTreeMap<String, String>) -> Result<(), Failure> {
    if env.len() > MAX_ENV {
        return Err((400, format!("at most {MAX_ENV} variables")));
    }
    for (name, value) in env {
        let valid = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !valid {
            return Err((
                400,
                format!("{name:?} is not a variable name (A-Z, 0-9, _)"),
            ));
        }
        if RESERVED_ENV.contains(&name.as_str()) {
            return Err((
                400,
                format!("{name} is set by the plaza to route the engine through its gateway"),
            ));
        }
        if ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"]
            .iter()
            .any(|w| name.contains(w))
        {
            return Err((
                400,
                format!(
                    "{name} looks like a secret. An engine's environment is readable by the agent, so keys do not \
                     go here: add the key to a provider and route the engine through it"
                ),
            ));
        }
        if value.len() > MAX_VALUE || value.contains('\0') {
            return Err((
                400,
                format!("{name}'s value is at most {MAX_VALUE} characters"),
            ));
        }
    }
    Ok(())
}

fn check_command(command: &str) -> Result<(), Failure> {
    let ok = !command.is_empty()
        && command.len() <= 512
        && !command.contains('\0')
        && !command.chars().any(char::is_whitespace);
    if ok {
        Ok(())
    } else {
        Err((
            400,
            "command is one program (a name on the VM's PATH or a path); put its arguments in args"
                .into(),
        ))
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn valid_package(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 214
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@/._-".contains(&b))
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
}

/// `pkg@1.2.3` → (`pkg`, Some(`1.2.3`)); `@scope/pkg@1` → (`@scope/pkg`, Some(`1`)); `pkg==1` for uvx.
fn split_spec(spec: &str) -> (&str, Option<&str>) {
    if let Some((p, v)) = spec.split_once("==") {
        return (p, Some(v));
    }
    match spec.rfind('@') {
        Some(at) if at > 0 => (&spec[..at], Some(&spec[at + 1..])),
        _ => (spec, None),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The launch arguments a registry entry names for its npx or uvx distribution (binary ones are per asset).
fn entry_args(entry: &Value) -> Vec<String> {
    let dist = &entry["distribution"];
    strings(
        dist.get("npx")
            .or(dist.get("uvx"))
            .map_or(&Value::Null, |d| &d["args"]),
    )
}

/// Whether version `a` is newer than `b`, comparing dotted numbers (pre-release tags sort lower).
pub(crate) fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> (Vec<u64>, bool) {
        let (core, pre) = v.split_once('-').map_or((v, false), |(c, _)| (c, true));
        (
            core.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
            pre,
        )
    };
    let ((an, apre), (bn, bpre)) = (parts(a), parts(b));
    match an.cmp(&bn) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => bpre && !apre,
    }
}

/// The registry's agents as the Add dialog lists them: name, description, icon, how it installs, and
/// whether it can run in an agent's VM (a Linux build, or npx/uvx).
fn registry_view(fetched_ms: u64, reg: &Value) -> Value {
    let agents: Vec<Value> = reg["agents"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    let dist = &e["distribution"];
                    let kind = ["npx", "uvx", "binary"].into_iter().find(|k| dist.get(*k).is_some());
                    let runs = match kind {
                        Some("binary") => VM_PLATFORMS.iter().any(|p| dist["binary"].get(*p).is_some()),
                        Some(_) => true,
                        None => false,
                    };
                    json!({
                        "id": e["id"], "name": e["name"], "version": e["version"], "description": e["description"],
                        "website": e["website"], "repository": e["repository"], "icon": e["icon"],
                        "license": e["license"], "distribution": kind, "runs_in_vm": runs,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "fetched_ms": fetched_ms, "agents": agents })
}

// ---- routes

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/engines", list_route)
        .doc(RouteDoc::new("List the coding engines: the built-in ones and the instances the owner added"))
        .post("/v1/engines", create_route)
        .doc(
            RouteDoc::new("Add an engine instance from the ACP registry (acp:<id>) or a local ACP command")
                .status(201)
                .outbound()
                .errors(&[400, 409, 502]),
        )
        .post("/v1/engines/check", check_route)
        .doc(RouteDoc::new("Check every enabled engine: its newest version and its model route").outbound())
        .post("/v1/engines/update", update_route)
        .doc(
            RouteDoc::new("Move the npm engines to their newest versions ({dry_run: true} lists the changes)")
                .outbound()
                .errors(&[502]),
        )
        .get("/v1/engines/{id}", get_route)
        .doc(RouteDoc::new("Show one engine").errors(&[404]))
        .route("PUT", "/v1/engines/{id}", edit_route)
        .doc(RouteDoc::new("Change an engine: label, accent, on/off, version, arguments, environment, home, route").errors(&[400, 404, 502]))
        .delete("/v1/engines/{id}", delete_route)
        .doc(RouteDoc::new("Remove an engine instance the owner added").errors(&[400, 404]))
        .get("/v1/acp/registry", registry_route)
        .doc(RouteDoc::new("The ACP agent registry (?refresh=1 fetches it again)").outbound().errors(&[502]))
        .get("/v1/provider-catalog", catalog_route)
        .doc(RouteDoc::new("What the Add provider dialog offers: the API kinds and the coding engines"))
}

/// Checks every enabled engine now (the background round in `provider_health.rs` and `POST /v1/engines/check`).
pub fn check_now(app: &App) -> Value {
    app.engines.check(app, &enabled_builtins(app))
}

fn enabled_builtins(app: &App) -> Vec<String> {
    app.settings.get().engines_enabled
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &app.engines.views(&enabled_builtins(app)))
}

fn find_view(app: &App, id: &str) -> Option<Value> {
    app.engines
        .views(&enabled_builtins(app))
        .into_iter()
        .find(|v| v["id"] == id)
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match find_view(app, p.get("id")) {
        Some(v) => Response::json(200, &v),
        None => failure((404, format!("no engine `{}`", p.get("id")))),
    }
}

fn create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let edit: Edit = match req.json() {
        Ok(e) => e,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    match app
        .engines
        .add(edit, || app.engines.registry(false).map(|(_, r)| r))
    {
        Ok(s) => {
            app.bus.publish("engines.changed", &json!({ "id": s.id }));
            Response::json(201, &find_view(app, &s.id).unwrap_or(Value::Null))
        }
        Err(f) => failure(f),
    }
}

fn edit_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let edit: Edit = match req.json() {
        Ok(e) => e,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    // A built-in engine's switch is `engines_enabled`, which never empties.
    if let (Some(on), Some(_)) = (edit.enabled, builtin(id)) {
        let mut settings = app.settings.get();
        settings.engines_enabled.retain(|e| e != id);
        if on {
            settings.engines_enabled.push(id.to_string());
            settings
                .engines_enabled
                .sort_by_key(|e| BUILTINS.iter().position(|b| b.id == e.as_str()));
        }
        if settings.engines_enabled.is_empty() {
            return failure((400, "at least one built-in engine stays enabled".into()));
        }
        if let Err(e) = app.settings.put(settings) {
            return failure((400, e));
        }
    }
    match app.engines.edit(id, &edit) {
        Ok(_) => {
            app.bus.publish("engines.changed", &json!({ "id": id }));
            Response::json(200, &find_view(app, id).unwrap_or(Value::Null))
        }
        Err(f) => failure(f),
    }
}

fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.engines.remove(p.get("id")) {
        Ok(()) => {
            app.bus
                .publish("engines.changed", &json!({ "id": p.get("id") }));
            Response::json(200, &json!({ "deleted": p.get("id") }))
        }
        Err(f) => failure(f),
    }
}

fn check_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let health = check_now(app);
    app.bus.publish("engines.health", &health);
    Response::json(200, &health)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UpdateBody {
    dry_run: bool,
}

fn update_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: UpdateBody = if req.body.is_empty() {
        UpdateBody::default()
    } else {
        match req.json() {
            Ok(b) => b,
            Err(e) => return failure((400, format!("bad body: {e}"))),
        }
    };
    match app.engines.update(&enabled_builtins(app), body.dry_run) {
        Ok(changes) => {
            if !body.dry_run && !changes.is_empty() {
                app.bus
                    .publish("engines.changed", &json!({ "updated": changes }));
            }
            Response::json(200, &json!({ "dry_run": body.dry_run, "changes": changes }))
        }
        Err(f) => failure(f),
    }
}

fn registry_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let refresh = req
        .query_param("refresh")
        .is_some_and(|v| v == "1" || v == "true");
    match app.engines.registry(refresh) {
        Ok((at, reg)) => Response::json(200, &registry_view(at, &reg)),
        Err(f) => failure(f),
    }
}

fn catalog_route(_: &App, _: &Request, _: &Params<'_>) -> Response {
    let engines: Vec<Value> = BUILTINS
        .iter()
        .map(|b| json!({ "id": b.id, "label": b.label, "source": b.source, "package": b.package, "pinned": b.pinned }))
        .collect();
    Response::json(
        200,
        &json!({ "api": apz_gateway::presets(), "engines": engines, "registry": REGISTRY_URL }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers fixed documents by URL and hashes nothing.
    struct Fake(BTreeMap<String, String>);

    impl Fetch for Fake {
        fn text(&self, url: &str) -> Result<String, String> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url}: offline"))
        }
        fn sha256(&self, url: &str) -> Result<String, String> {
            Ok(hex(&Sha256::digest(url.as_bytes())))
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-engines-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A store whose registry fetches answer `docs`; its folder holds only the offline registry copy.
    fn store(name: &str, docs: &[(&str, Value)]) -> (PathBuf, Store) {
        let dir = scratch(name);
        let docs = docs
            .iter()
            .map(|(u, v)| (u.to_string(), v.to_string()))
            .collect();
        let s = Store::with_fetch(&dir, Box::new(Fake(docs)));
        (dir, s)
    }

    #[test]
    fn the_builtin_table_matches_the_harness_catalogue() {
        let catalogue = include_str!("../../agent-harness/src/acp/catalog.rs");
        for b in BUILTINS {
            assert!(
                catalogue.contains(&format!("id: \"{}\"", b.id)),
                "{} is not in the catalogue",
                b.id
            );
            assert!(
                catalogue.contains(&format!("version: \"{}\"", b.pinned)),
                "{}'s pin {} moved",
                b.id,
                b.pinned
            );
        }
        assert_eq!(BUILTINS.len(), crate::settings::ENGINE_IDS.len());
    }

    #[test]
    fn builtins_list_first_and_follow_engines_enabled() {
        let (_d, s) = store("views", &[]);
        let views = s.views(&["codex".into()]);
        assert_eq!(views.len(), BUILTINS.len());
        assert_eq!(views[0]["id"], "claude");
        assert_eq!(views[0]["enabled"], false);
        assert_eq!(views[0]["health"]["status"], "off");
        assert_eq!(views[1]["enabled"], true);
        assert_eq!(views[1]["version"], "2.0.1");
        assert_eq!(views[1]["builtin"], true);
    }

    #[test]
    fn a_registry_agent_is_pinned_when_added() {
        let reg = json!({"agents": [
            {"id": "pi-acp", "name": "pi ACP", "version": "0.0.34", "distribution": {"npx": {"package": "pi-acp@0.0.34", "args": ["--x"]}}},
            {"id": "cursor", "name": "Cursor", "version": "1", "distribution": {"binary": {
                "linux-x86_64": {"archive": "https://dl.example/c-x64.tgz", "cmd": "./cursor-agent", "args": ["acp"]},
                "darwin-aarch64": {"archive": "https://dl.example/c-mac.tgz", "cmd": "./cursor-agent"}}}},
            {"id": "mac-only", "name": "Mac", "version": "1", "distribution": {"binary": {"darwin-aarch64": {"archive": "https://x/y", "cmd": "./y"}}}}
        ]});
        let (_d, s) = store(
            "registry",
            &[
                (REGISTRY_URL, reg.clone()),
                (
                    "https://registry.npmjs.org/pi-acp/0.0.34",
                    json!({"version": "0.0.34", "dist": {"integrity": "sha512-abc"}}),
                ),
            ],
        );
        let reg = || s.registry(false).map(|(_, r)| r);
        let edit = |id: &str, engine: &str| Edit {
            id: Some(id.into()),
            engine: Some(engine.into()),
            ..Default::default()
        };
        let pi = s.add(edit("pi", "acp:pi-acp"), reg).unwrap();
        assert_eq!(
            pi.install,
            Some(Install::Npm {
                package: "pi-acp".into(),
                version: "0.0.34".into(),
                integrity: "sha512-abc".into()
            })
        );
        assert_eq!(pi.args, ["--x"]);
        let cursor = s
            .add(edit("cursor", "acp:cursor"), || {
                s.registry(false).map(|(_, r)| r)
            })
            .unwrap();
        let Some(Install::Archive { assets, .. }) = cursor.install else {
            panic!("archive")
        };
        assert_eq!(assets.keys().collect::<Vec<_>>(), ["linux-x86_64"]);
        assert_eq!(assets["linux-x86_64"].sha256.len(), 64);
        let err = s
            .add(edit("mac", "acp:mac-only"), || {
                s.registry(false).map(|(_, r)| r)
            })
            .unwrap_err();
        assert!(err.1.contains("no Linux build"));
        assert_eq!(
            s.add(edit("pi", "acp:pi-acp"), || s
                .registry(false)
                .map(|(_, r)| r))
                .unwrap_err()
                .0,
            409
        );
        assert_eq!(
            s.add(edit("codex", "acp-local"), || s
                .registry(false)
                .map(|(_, r)| r))
                .unwrap_err()
                .0,
            409
        );
        assert_eq!(s.enabled_extras(), ["pi", "cursor"]);
        assert!(s.launch_spec("pi").is_some() && s.launch_spec("claude").is_none());
    }

    #[test]
    fn secrets_never_go_into_an_engines_environment() {
        let env = |k: &str| BTreeMap::from([(k.to_string(), "v".to_string())]);
        assert!(check_env(&env("CODEX_PROFILE")).is_ok());
        assert!(
            check_env(&env("OPENAI_API_KEY"))
                .unwrap_err()
                .1
                .contains("routes")
                || check_env(&env("OPENAI_API_KEY")).is_err()
        );
        assert!(
            check_env(&env("MY_TOKEN"))
                .unwrap_err()
                .1
                .contains("secret")
        );
        assert!(
            check_env(&env("ANTHROPIC_BASE_URL"))
                .unwrap_err()
                .1
                .contains("gateway")
        );
        assert!(check_env(&env("lower")).is_err());
    }

    #[test]
    fn local_commands_and_versions_are_checked() {
        let (_d, s) = store(
            "versions",
            &[(
                "https://registry.npmjs.org/@agentclientprotocol%2Fcodex-acp/latest",
                json!({"version": "2.1.0", "dist": {"integrity": "sha512-new"}}),
            )],
        );
        let none = || Err((502, "offline".to_string()));
        let local = Edit {
            id: Some("mine".into()),
            engine: Some("acp-local".into()),
            command: Some("my-agent --acp".into()),
            ..Default::default()
        };
        assert!(s.add(local, none).unwrap_err().1.contains("arguments"));
        let local = Edit {
            id: Some("mine".into()),
            engine: Some("acp-local".into()),
            command: Some("my-agent".into()),
            args: Some(vec!["--acp".into()]),
            ..Default::default()
        };
        assert_eq!(
            s.add(local, none).unwrap().install,
            Some(Install::Command {
                command: "my-agent".into()
            })
        );
        let dry = s.update(&["codex".into()], true).unwrap();
        assert_eq!(
            dry,
            [json!({"id": "codex", "from": "2.0.1", "to": "2.1.0"})]
        );
        assert!(s.stored("codex").is_none(), "a dry run changes nothing");
        s.update(&["codex".into()], false).unwrap();
        assert_eq!(
            installed_version(&s.stored("codex").unwrap()).as_deref(),
            Some("2.1.0")
        );
        let back = s
            .edit(
                "codex",
                &Edit {
                    version: Some("pinned".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(installed_version(&back).as_deref(), Some("2.0.1"));
        assert_eq!(s.remove("codex").unwrap_err().0, 400);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("2.10.0", "2.9.9"));
        assert!(!newer("2.0.1", "2.0.1"));
        assert!(newer("1.0.0", "1.0.0-beta"));
        assert!(!newer("1.0.0-beta", "1.0.0"));
        assert_eq!(
            split_spec("@xai-official/grok@1.0.63"),
            ("@xai-official/grok", Some("1.0.63"))
        );
        assert_eq!(split_spec("pi-acp"), ("pi-acp", None));
    }

    #[test]
    fn the_registry_is_served_offline_from_the_last_copy() {
        let dir = scratch("offline");
        std::fs::write(
            dir.join("acp-registry.json"),
            json!({"fetched_ms": 1, "registry": {"agents": [{"id": "a", "distribution": {"npx": {"package": "a"}}}]}}).to_string(),
        )
        .unwrap();
        let s = Store::with_fetch(&dir, Box::new(Fake(BTreeMap::new())));
        let (at, reg) = s.registry(true).unwrap();
        assert_eq!(at, 1);
        assert_eq!(registry_view(at, &reg)["agents"][0]["runs_in_vm"], true);
    }

    #[test]
    fn a_save_of_the_instances_moves_the_engines_source() {
        let dir = scratch("gens");
        let gens: std::sync::Arc<apz_cache::Generations<String>> = Default::default();
        let store = Store::new(&dir).with_generations(std::sync::Arc::clone(&gens));
        store.save(Vec::new()).expect("save");
        assert!(gens.get(&crate::sources::ENGINES.to_string()) > 0);
        assert!(lock(&store.stored).is_empty());
    }

    #[test]
    fn the_instances_round_trip_through_their_tables() {
        let (_d, s) = store(
            "rows",
            &[
                (
                    REGISTRY_URL,
                    json!({"agents": [{"id": "pi-acp", "name": "pi ACP", "version": "0.0.34",
                        "description": "A pi agent", "distribution": {"npx": {"package": "pi-acp@0.0.34", "args": ["--x"]}}}]}),
                ),
                (
                    "https://registry.npmjs.org/pi-acp/0.0.34",
                    json!({"version": "0.0.34", "dist": {"integrity": "sha512-abc"}}),
                ),
            ],
        );
        s.add(
            Edit {
                id: Some("pi".into()),
                engine: Some("acp:pi-acp".into()),
                ..Default::default()
            },
            || s.registry(false).map(|(_, r)| r),
        )
        .unwrap();
        s.add(
            Edit {
                id: Some("mine".into()),
                engine: Some("acp-local".into()),
                command: Some("my-agent".into()),
                args: Some(vec!["--acp".into()]),
                env: Some(BTreeMap::from([("CODEX_PROFILE".into(), "work".into())])),
                label: Some(Some("Mine".into())),
                accent: Some(Some("#112233".into())),
                home: Some(Some("mine-home".into())),
                route: Some(Some("anthropic/model".into())),
                ..Default::default()
            },
            || Err((502, "offline".to_string())),
        )
        .unwrap();
        s.edit(
            "codex",
            &Edit {
                label: Some(Some("Codex work".into())),
                ..Default::default()
            },
        )
        .unwrap();
        let before = lock(&s.stored).clone();
        assert_eq!(before.len(), 3);

        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &s.rows().unwrap()).unwrap();
        let reloaded = Store::with_fetch(&scratch("rows-back"), Box::new(Fake(BTreeMap::new())));
        reloaded.load(&conn).unwrap();
        assert_eq!(*lock(&reloaded.stored), before);
        assert_eq!(
            lock(&reloaded.stored)[0]
                .about
                .as_ref()
                .and_then(|a| a.description.as_deref()),
            Some("A pi agent")
        );
    }
}
