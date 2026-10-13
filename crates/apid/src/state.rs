//! The application: configuration and the shared state every module reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::upstream::Upstream;
use crate::{agents, api, events, plaza, providers, scheduler, settings, spawned, transcript};

/// Command-line configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub state_dir: PathBuf,
    pub noded_port: u16,
    pub hub_port: u16,
    pub roles: PathBuf,
    pub prompts: PathBuf,
    pub console: PathBuf,
    pub env_file: Option<PathBuf>,
    pub dev_models: bool,
    /// The address the hub serves on (loopback unless `--bind` says otherwise).
    pub bind: std::net::IpAddr,
    /// How nodes reach this hub; join tokens carry it.
    pub public_url: String,
    /// Serve HTTPS with the hub's certificate (implied off loopback).
    pub tls: bool,
}

impl Config {
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        let home = apz_os::dirs::home().unwrap_or_else(|| PathBuf::from("."));
        let state_dir = apz_os::dirs::state();
        let mut config = Config {
            port: 7700,
            state_dir,
            noded_port: 8899,
            hub_port: 19080,
            roles: PathBuf::from("docs/agents/initial-roles.json"),
            prompts: PathBuf::from("docs/codex-draft/roles"),
            console: PathBuf::from("apps/web/dist"),
            env_file: Some(Path::new(&home).join(".config/agentplaza/keys.env")),
            dev_models: false,
            bind: std::net::Ipv4Addr::LOCALHOST.into(),
            public_url: String::new(),
            tls: false,
        };
        // Installed beside the binary: the flags below still override these.
        if let Some(share) = apz_os::dirs::resources() {
            let console = share.join("console");
            let roles = share.join("roles").join("initial-roles.json");
            let prompts = share.join("prompts");
            if console.is_dir() {
                config.console = console;
            }
            if roles.is_file() {
                config.roles = roles;
            }
            if prompts.is_dir() {
                config.prompts = prompts;
            }
        }
        let mut public_url = None;
        let mut tls = false;
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            if flag == "--dev-models" {
                config.dev_models = true;
                continue;
            }
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            let port = |v: String| v.parse::<u16>().map_err(|e| format!("{flag}: {e}"));
            match flag.as_str() {
                "--port" => config.port = port(value()?)?,
                "--noded-port" => config.noded_port = port(value()?)?,
                "--hub-port" => config.hub_port = port(value()?)?,
                "--state-dir" => config.state_dir = PathBuf::from(value()?),
                "--roles" => config.roles = PathBuf::from(value()?),
                "--prompts" => config.prompts = PathBuf::from(value()?),
                "--console" => config.console = PathBuf::from(value()?),
                "--bind" => config.bind = value()?.parse().map_err(|e| format!("{flag}: {e}"))?,
                "--public-url" => public_url = Some(value()?),
                "--tls" => tls = true,
                "--env-file" => config.env_file = Some(PathBuf::from(value()?)),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        config.tls = tls || !config.bind.is_loopback();
        let scheme = if config.tls { "https" } else { "http" };
        config.public_url = public_url.unwrap_or_else(|| format!("{scheme}://127.0.0.1:{}", config.port));
        Ok(config)
    }
}

/// Everything the handlers and background threads share.
pub struct App {
    pub config: Config,
    /// The event bus every module publishes on (the sockets at `/v1/ws` subscribe to it).
    pub bus: events::Bus,
    /// Linked-note memory libraries, under <state-dir>/memory.
    pub memory: crate::memory::Memory,
    /// Contacts and opinions between agents (AP-071/AP-072), at `<state-dir>/bonds.json`.
    pub bonds: apz_plaza::bonds::Bonds,
    /// The plaza's laws (JHRL), enforced on every tool call, and the violations on record.
    pub laws: &'static crate::laws::Laws,
    /// External-tool connectors (AP-063): settings and vaulted keys.
    pub connectors: crate::connectors::Store,
    pub approvals: crate::approvals::Store,
    /// Agent-made tools (the plaza's library).
    pub library: crate::library::Store,
    /// The Library of deliverables: files agents handed in, in Garage or a local folder.
    pub deliverables: crate::deliverables::Store,
    /// Hire bonuses, posting counts and outreach times.
    pub society: crate::society::Society,
    /// What agents found on the internet and judged, for every agent to use.
    pub toolbox: crate::toolbox::Store,
    /// Projects: every goal's flow from brief to delivery.
    pub projects: crate::projects::Store,
    /// Loops and per-agent harness limits.
    pub harness: crate::harness_control::Store,
    /// The plaza's git host (Forgejo) and the agents' identities on it.
    pub forge: crate::git::Forge,
    /// Postgres, the plaza's state of record (memory is the working copy).
    pub db: Arc<crate::db::Db>,
    /// STOP ALL / resume.
    pub control: crate::control::Control,
    /// Roles the owner defined or approved.
    pub custom_roles: crate::roles_custom::Store,
    /// The orchestrator's proposals for new agents.
    pub agent_proposals: crate::agent_proposals::Store,
    /// The hub's nodes, join codes and command queues.
    pub nodes: crate::nodes::Store,
    /// Model prices (OpenRouter, LiteLLM and the owner's own), for usage costs.
    pub prices: crate::prices::Catalog,
    /// Every model call's tokens, cache hits and cost.
    pub usage: crate::usage::Ledger,
    /// Each agent's robot species (dealt on its first hello, or set by the owner).
    pub looks: crate::looks::Looks,
    /// The Foundry: recipes, datasets, training jobs and their series (`<state>/foundry`).
    pub foundry: crate::foundry::store::Foundry,
    /// The hub's identity and the owner's access (see hub_auth).
    pub hub_auth: crate::hub_auth::Hub,
    pub settings: settings::Store,
    pub providers: providers::Store,
    pub roles: agents::Roles,
    pub agents: agents::Roster,
    /// The model registry: the records on disk and the compiled models in memory.
    pub models: crate::foundry::registry::Models,
    /// The Hugging Face import queue and the owner's token (foundry/hf_import.rs).
    pub hf: crate::foundry::hf_import::Imports,
    /// Agent names by CID, persisted at `<state-dir>/names.json`.
    pub names: agents::Names,
    /// Operator-spawned agents and model overrides, at `<state-dir>/spawned.json`.
    pub spawned: spawned::Store,
    pub transcripts: transcript::Store,
    pub work: scheduler::Queues,
    pub plaza: plaza::Plaza,
    pub noded: Upstream,
    pub hub: Upstream,
    /// The secret the hub presents on `/internal/*` (`x-apz-internal`).
    pub token: String,
    /// This app, weakly: for work that outlives a request (a socket's thread) to read live state.
    me: std::sync::OnceLock<std::sync::Weak<App>>,
}

impl App {
    /// Loads the state, starts the background threads and serves.
    pub fn start(config: Config) -> std::io::Result<apz_http::Handle> {
        // Parse the species index now: a broken index stops the hub here, not at the first hello.
        crate::looks::catalog();
        let env = config.env_file.as_deref().map(read_env_file).unwrap_or_default();
        std::fs::create_dir_all(&config.state_dir)?;
        let token = write_token(&config.state_dir)?;
        let snapshot = std::fs::read_to_string(config.state_dir.join("treasury.json")).ok();
        let hub_auth = crate::hub_auth::Hub::load(&config.state_dir, &config.public_url, config.tls)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        let foundry_dir = config.state_dir.join("foundry");
        let app = Arc::new(App {
            me: std::sync::OnceLock::new(),
            bus: events::Bus::default(),
            memory: crate::memory::Memory::new(config.state_dir.join("memory")),
            bonds: apz_plaza::bonds::Bonds::load(Some(config.state_dir.join("bonds.json"))),
            laws: attached_laws(&config.state_dir),
            connectors: crate::connectors::Store::load(&config.state_dir, env.clone())?,
            approvals: crate::approvals::Store::load(&config.state_dir),
            library: crate::library::Store::load(&config.state_dir),
            deliverables: crate::deliverables::Store::load(&config.state_dir, &env)?,
            society: crate::society::Society::default(),
            toolbox: crate::toolbox::Store::load(&config.state_dir),
            projects: crate::projects::Store::load(&config.state_dir),
            harness: crate::harness_control::Store::load(&config.state_dir),
            forge: crate::git::Forge::load(&config.state_dir, &env),
            db: Arc::new(crate::db::Db::connect(&env)),
            control: crate::control::Control::default(),
            custom_roles: crate::roles_custom::Store::default(),
            agent_proposals: crate::agent_proposals::Store::default(),
            nodes: crate::nodes::Store::default(),
            prices: crate::prices::Catalog::default(),
            usage: crate::usage::Ledger::default(),
            looks: crate::looks::Looks::open(config.state_dir.join("looks.json")),
            foundry: crate::foundry::store::Foundry::open(foundry_dir, apz_plaza::now_ms()),
            hub_auth,
            settings: settings::Store::load(&config.state_dir, config.dev_models)?,
            providers: providers::Store::load(&config.state_dir, env)?,
            roles: agents::Roles::load(&config.roles, &config.prompts)?,
            names: agents::Names::load(config.state_dir.join("names.json")),
            spawned: spawned::Store::load(config.state_dir.join("spawned.json")),
            agents: agents::Roster::default(),
            models: crate::foundry::registry::Models::open(&config.state_dir),
            hf: crate::foundry::hf_import::Imports::open(&config.state_dir),
            transcripts: transcript::Store::default(),
            work: scheduler::Queues::default(),
            plaza: plaza::Plaza::new(snapshot.as_deref()),
            noded: Upstream::new(config.noded_port),
            hub: Upstream::new(config.hub_port),
            token,
            config,
        });
        events::spawn_relays(Arc::clone(&app));
        agents::spawn_reconcile(Arc::clone(&app));
        spawned::spawn_respawn(Arc::clone(&app));
        scheduler::spawn(Arc::clone(&app));
        crate::autoscale::spawn(Arc::clone(&app));
        crate::foundry::routes::spawn_tracker(Arc::clone(&app));
        // Model prices: refreshed when stale (at start, then hourly; the catalog decides).
        let prices_app = Arc::clone(&app);
        std::thread::spawn(move || loop {
            prices_app.prices.refresh_if_stale(apz_plaza::now_ms());
            std::thread::sleep(std::time::Duration::from_secs(60 * 60));
        });
        // Every note an agent writes is a live `memory` event for the console.
        let weak = Arc::downgrade(&app);
        let _ = app.me.set(Arc::downgrade(&app));
        app.memory.set_listener(move |cid, meta| {
            if let Some(app) = weak.upgrade() {
                app.bus.publish("memory", &serde_json::json!({ "cid": cid, "slug": meta.slug, "title": meta.title, "kind": meta.kind }));
            }
        });
        // A treasury.json that was refused is kept aside, not overwritten by the next save.
        if snapshot.is_some() && !app.plaza.restored {
            let path = app.config.state_dir.join("treasury.json");
            let _ = std::fs::rename(&path, path.with_extension("json.rejected"));
        }
        // Postgres is the state of record: restore from it, then keep it current.
        crate::db::restore_all(&app);
        // Promoted models come back on the roster with their CIDs.
        app.models.revive(&app.agents, &app.looks);
        crate::db::spawn_writer(Arc::clone(&app));
        spawn_treasury_saver(Arc::clone(&app));
        spawn_component_savers(Arc::clone(&app));
        crate::harness_control::spawn_ticker(Arc::clone(&app));
        // Deliverables (videos, images) arrive as request bodies: allow up to 512 MiB.
        // On loopback the hub serves plain HTTP as before. Bound anywhere else it is a hub other
        // machines reach: always HTTPS, with the certificate made on first start (its SHA-256 is
        // the pin join tokens carry), and the owner token required off loopback (hub_auth::gate).
        let mut server = if app.config.bind.is_loopback() {
            apz_http::Server::bind(app.config.port)?
        } else {
            apz_http::Server::bind_addr(std::net::SocketAddr::new(app.config.bind, app.config.port))?
        };
        if app.config.tls || !app.config.bind.is_loopback() {
            let dir = app.config.state_dir.join("tls");
            let pair = apz_trust::tls::CertPair {
                cert_pem: std::fs::read_to_string(dir.join("cert.pem"))?,
                key_pem: std::fs::read_to_string(dir.join("key.pem"))?,
            };
            let tls = apz_trust::tls::server_config(&pair).map_err(std::io::Error::other)?;
            server = server.tls(tls);
            eprintln!("apid: serving HTTPS on {}:{} (certificate pin {})", app.config.bind, app.config.port, app.hub_auth.fingerprint_b64());
        }
        // Deliverables (videos, images) arrive as request bodies: allow up to 512 MiB.
        let server = server.limits(apz_http::Limits { max_head: 16 << 10, max_body: 512 << 20 });
        let router = api::router();
        // The hub's front door first (owner token off loopback), then the routes.
        server.spawn(move |request| match crate::hub_auth::gate(&app, &request) {
            Some(refused) => refused,
            None => router.handle(&app, request),
        })
    }
}

/// Reads `KEY=VALUE` lines (blank lines and `#` comments skipped; an
/// `export ` prefix and surrounding quotes allowed). The values are provider
/// credentials: they are handed to the gateway, never logged.
/// The plaza's one law set (`laws::global`), made to read and write the state directory.
fn attached_laws(state_dir: &Path) -> &'static crate::laws::Laws {
    let laws = crate::laws::global();
    laws.attach(state_dir);
    laws
}

fn read_env_file(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeMap::new() };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("export ").unwrap_or(line).split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// Saves the Foundry (`<state>/foundry/state.json`) and the looks (`<state>/looks.json`) every three
/// seconds whenever they changed. Their files are the copy at start when Postgres does not hold them
/// yet; a component whose load failed is not written (db::Db::is_held).
fn spawn_component_savers(app: Arc<App>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(3));
        if !app.db.is_held("foundry") {
            if let Err(err) = app.foundry.save() {
                eprintln!("apid: cannot save the foundry: {err}");
            }
        }
        if !app.db.is_held("looks") {
            if let Err(err) = app.looks.save() {
                eprintln!("apid: cannot save the looks: {err}");
            }
        }
    });
}

/// Saves the treasury to `<state-dir>/treasury.json` every three seconds whenever
/// its version has moved; the first tick always writes.
fn spawn_treasury_saver(app: Arc<App>) {
    std::thread::spawn(move || {
        let path = app.config.state_dir.join("treasury.json");
        let mut saved = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let version = app.plaza.treasury.version();
            if saved == Some(version) {
                continue;
            }
            match write_atomic(&path, &app.plaza.treasury.snapshot()) {
                Ok(()) => saved = Some(version),
                Err(err) => eprintln!("apid: cannot save {}: {err}", path.display()),
            }
        }
    });
}

/// Writes `text` to `path` through a temporary file and a rename, so a crash
/// mid-write leaves the previous snapshot whole.
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// A fresh random token in `<state-dir>/internal.token`, mode 0600.
fn write_token(dir: &Path) -> std::io::Result<String> {
    let token = apz_os::random::hex(24)?;
    apz_os::private::write(&dir.join("internal.token"), token.as_bytes())?;
    Ok(token)
}

impl App {
    /// This app as a weak handle (for a socket's thread; it reads the live state through it).
    pub fn handle(&self) -> std::sync::Weak<App> {
        self.me.get().cloned().unwrap_or_default()
    }
}
