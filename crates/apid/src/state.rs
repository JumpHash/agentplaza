//! The application: configuration and the shared state every module reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// The MCP egress proxy's port on 127.0.0.1 (and the bridge gateway where the engine has one).
    pub mcp_proxy_port: u16,
    /// The vault's key file (`--vault-key`; default: the config directory's `vault.key`).
    pub vault_key: PathBuf,
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
            mcp_proxy_port: crate::mcp::egress::DEFAULT_PORT,
            vault_key: apz_os::dirs::config().join("vault.key"),
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
                "--mcp-proxy-port" => config.mcp_proxy_port = port(value()?)?,
                "--state-dir" => config.state_dir = PathBuf::from(value()?),
                "--roles" => config.roles = PathBuf::from(value()?),
                "--prompts" => config.prompts = PathBuf::from(value()?),
                "--console" => config.console = PathBuf::from(value()?),
                "--bind" => config.bind = value()?.parse().map_err(|e| format!("{flag}: {e}"))?,
                "--public-url" => public_url = Some(value()?),
                "--tls" => tls = true,
                "--env-file" => config.env_file = Some(PathBuf::from(value()?)),
                "--vault-key" => config.vault_key = PathBuf::from(value()?),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        config.tls = tls || !config.bind.is_loopback();
        let scheme = if config.tls { "https" } else { "http" };
        config.public_url =
            public_url.unwrap_or_else(|| format!("{scheme}://127.0.0.1:{}", config.port));
        Ok(config)
    }
}

/// Everything the handlers and background threads share.
pub struct App {
    pub config: Config,
    /// The event bus every module publishes on (the sockets at `/v1/ws` subscribe to it).
    pub bus: events::Bus,
    /// The plaza's laws (JHRL), enforced on every tool call.
    pub laws: &'static crate::laws::Laws,
    /// External-tool connectors (AP-063): settings and vaulted keys.
    pub connectors: crate::connectors::Store,
    pub approvals: crate::approvals::Store,
    /// The editors' ACP sessions (`/v1/acp`), saved with the plaza state.
    pub acp_sessions: crate::acp::store::Sessions,
    /// The engines' session updates: what each engine call adds to the transcript, per turn.
    pub engine_feed: crate::engine_feed::Feed,
    /// Agent-made tools (the plaza's library).
    pub library: crate::library::Store,
    /// The Library of deliverables: files agents handed in, in Garage or a local folder.
    pub deliverables: Arc<crate::deliverables::Store>,
    /// What agents found on the internet and judged, for every agent to use.
    pub toolbox: crate::toolbox::Store,
    /// Projects: every goal's flow from brief to delivery.
    pub projects: crate::projects::Store,
    /// The project each agent works on; one with none is on the bench and does nothing (`membership`).
    pub members: crate::membership::Members,
    /// The owner's conversations with the orchestrator, each its own thread (`conversations`).
    pub conversations: crate::conversations::Store,
    /// Each chat's context and tool calls, and one-message model choices (`context_track.rs`).
    pub context: crate::context_track::Tracker,
    /// Each project's overrides of the plaza's settings (`project_settings`).
    pub project_settings: crate::project_settings::Store,
    /// Loops and per-agent harness limits.
    pub harness: crate::harness_control::Store,
    /// Weft, the plaza's version control: `weft serve` beside the hub, and apid as its door (weft_hub.rs).
    pub weft: crate::weft_hub::Weft,
    /// The git platforms the owner authorized (GitHub, GitLab, Forgejo) and what the plaza may use there.
    pub platforms: crate::platforms::Store,
    /// Projects in maintenance: their signals, map, proposals and sweeps.
    pub maintenance: crate::maintenance::Store,
    /// Where each project runs.
    pub environments: crate::environments::Store,
    /// The review graph's runs and recipes (review_graph), at `<state-dir>/review-graph.json`.
    pub reviews: crate::review_graph::Reviews,
    /// Postgres, the plaza's state of record (memory is the working copy).
    pub db: Arc<crate::db::Db>,
    /// The hub's store: SQLite in the state folder (store/mod.rs). Memory is the working copy; the saver keeps the
    /// components' tables equal to it.
    pub store: Arc<apz_store::Store>,
    /// What each component last saved (store/saver.rs).
    pub saved: crate::store::saver::Ledger,
    /// The work engine's sessions (sessions.rs), the seats of tasks (seats.rs), the budgets (budgets.rs), the cards
    /// waiting for the owner (cards.rs), the chats (chats.rs) and the seats' threads (threads.rs).
    pub sessions: crate::sessions::Sessions,
    pub seats: crate::seats::Seats,
    pub budgets: crate::budgets::Budgets,
    pub cards: crate::cards::Cards,
    pub chats: crate::chats::Chats,
    pub threads: crate::threads::Threads,
    /// The unfurl answers for the references clients show as chips (unfurl/mod.rs); the memory fallback when there is no Postgres.
    pub unfurl: crate::unfurl::Cache,
    /// STOP ALL / resume.
    pub control: crate::control::Control,
    /// Roles the owner defined or approved.
    pub custom_roles: crate::roles_custom::Store,
    /// The orchestrator's proposals for new agents.
    pub agent_proposals: crate::agent_proposals::Store,
    /// The hub's nodes, join codes and command queues.
    pub nodes: crate::nodes::Store,
    /// The workstations: their join codes, registry, live links and leases (docs/design/workstations.md).
    pub stations: crate::stations::Store,
    /// Model prices (OpenRouter, LiteLLM and the owner's own), for usage costs.
    pub prices: crate::prices::Catalog,
    /// Every model call's tokens, cache hits and cost.
    pub usage: crate::usage::Ledger,
    /// Metrics: what the hub records, what nodes send, the store, alerts and the `/v1/metrics` routes.
    pub metrics: crate::metrics::Metrics,
    /// The Foundry: recipes, datasets, training jobs and their series (`<state>/foundry`).
    pub foundry: crate::foundry::store::Foundry,
    /// MCP servers the owner added: their config, their tool listings and their running instances.
    pub mcp: crate::mcp::Mcp,
    /// The hub's identity and the owner's access (see hub_auth).
    pub hub_auth: crate::hub_auth::Hub,
    pub hub_mcp: crate::hub_mcp::HubMcp,
    /// The prompt library: system prompts and AGENTS.md, versioned and checked (docs/design/prompt-library.md).
    pub prompts: crate::prompts::Prompts,
    /// The skill library: SKILL.md folders, versioned and checked, imported from files or GitHub (docs/design/skill-library.md).
    pub skills: crate::skills::Skills,
    /// The secret vault (docs/design/vault.md): sealed or unsealed, encrypted at rest.
    pub vault: Arc<crate::vault::Store>,
    pub settings: settings::Store,
    /// The owner's appearance: theme, skin, background and mode, and saved themes (appearance.rs).
    pub appearance: crate::appearance::Store,
    /// Agents' artifacts (artifacts/), their versions and data blobs.
    pub artifacts: Arc<crate::artifacts::Store>,
    /// Share links (shares/).
    pub shares: Arc<crate::shares::Store>,
    pub providers: providers::Store,
    /// The coding engines as instances (`engine_instances.rs`).
    pub engines: crate::engine_instances::Store,
    /// The usage providers (CLIProxyAPI hubs), `usage_providers.rs`.
    pub usage_providers: crate::usage_providers::Store,
    pub roles: agents::Roles,
    pub agents: agents::Roster,
    /// The model registry: the records on disk and the compiled models in memory.
    pub models: crate::foundry::registry::Models,
    /// The Hugging Face import queue and the owner's token (foundry/hf_import.rs).
    pub hf: crate::foundry::hf_import::Imports,
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
        let env = config
            .env_file
            .as_deref()
            .map(read_env_file)
            .unwrap_or_default();
        // The vault's startup reads the env file's variables after the providers take their copy.
        let vault_env = env.clone();
        std::fs::create_dir_all(&config.state_dir)?;
        let token = write_token(&config.state_dir)?;
        // The same file the token was just written to: noded and vsockd are told it, and every call
        // this app makes to them presents it.
        let internal = apz_http::auth::InternalToken::file(config.state_dir.join("internal.token"));
        let snapshot = std::fs::read_to_string(config.state_dir.join("treasury.json")).ok();
        let hub_auth =
            crate::hub_auth::Hub::load(&config.state_dir, &config.public_url, config.tls)
                .map_err(std::io::Error::other)?;
        let foundry_dir = config.state_dir.join("foundry");
        // The store first: it takes the folder's lock, so a second hub on this folder stops here.
        let store = Arc::new(crate::store::open(&config.state_dir).map_err(std::io::Error::other)?);
        let db = Arc::new(crate::db::Db::connect(&env));
        let vault_db = db.enabled().then(|| Arc::clone(&db));
        let roster_db = db.enabled().then(|| Arc::clone(&db));
        let hub_mcp_db = db.enabled().then(|| Arc::clone(&db));
        let prompt_store: Arc<dyn crate::prompts::PromptStore> = if db.enabled() {
            Arc::new(crate::prompts::store_db::DbPrompts::new(Arc::clone(&db)))
        } else {
            Arc::new(crate::prompts::store_mem::MemoryPrompts::open(
                &config.state_dir,
            ))
        };
        let vault = Arc::new(crate::vault::Store::open(
            &config.state_dir,
            &config.vault_key,
            vault_db,
        ));
        // The vendor bindings (Auggie's session) show in the console's vault screen from boot.
        crate::vendor_relay::register(&vault);
        // The Library of deliverables, shared with the skill library's file store.
        let library = Arc::new(crate::deliverables::Store::load(
            &config.state_dir,
            &env,
            Arc::clone(&vault),
        )?);
        let app = Arc::new(App {
            me: std::sync::OnceLock::new(),
            bus: events::Bus::default(),
            laws: attached_laws(&config.state_dir),
            connectors: crate::connectors::Store::load(&config.state_dir, Arc::clone(&vault))?,
            approvals: crate::approvals::Store::load(&config.state_dir),
            acp_sessions: crate::acp::store::Sessions::default(),
            engine_feed: crate::engine_feed::Feed::default(),
            library: crate::library::Store::load(&config.state_dir),
            deliverables: Arc::clone(&library),
            toolbox: crate::toolbox::Store::load(&config.state_dir),
            projects: crate::projects::Store::load(&config.state_dir),
            members: crate::membership::Members::load(config.state_dir.join("members.json")),
            context: Default::default(),
            conversations: crate::conversations::Store::load(
                config.state_dir.join("conversations.json"),
            ),
            project_settings: crate::project_settings::Store::load(
                config.state_dir.join("project-settings.json"),
            ),
            harness: crate::harness_control::Store::load(&config.state_dir),
            weft: crate::weft_hub::Weft::default(),
            platforms: crate::platforms::Store::load(&config.state_dir),
            maintenance: crate::maintenance::Store::load(&config.state_dir),
            environments: crate::environments::Store::load(&config.state_dir),
            reviews: crate::review_graph::Reviews::load(&config.state_dir),
            db: Arc::clone(&db),
            store: Arc::clone(&store),
            saved: crate::store::saver::Ledger::default(),
            sessions: Default::default(),
            seats: Default::default(),
            budgets: Default::default(),
            cards: Default::default(),
            chats: Default::default(),
            threads: Default::default(),
            unfurl: crate::unfurl::Cache::default(),
            vault: Arc::clone(&vault),
            control: crate::control::Control::default(),
            custom_roles: crate::roles_custom::Store::default(),
            agent_proposals: crate::agent_proposals::Store::default(),
            nodes: crate::nodes::Store::default(),
            stations: crate::stations::Store::default(),
            prices: crate::prices::Catalog::default(),
            usage: crate::usage::Ledger::default(),
            metrics: crate::metrics::Metrics::open(
                &config.state_dir,
                env.get("APZ_DATABASE_URL")
                    .map(String::as_str)
                    .filter(|u| !u.trim().is_empty()),
                apz_plaza::now_ms(),
            ),
            foundry: crate::foundry::store::Foundry::open(foundry_dir, apz_plaza::now_ms()),
            mcp: crate::mcp::Mcp::load(&config.state_dir, config.mcp_proxy_port),
            hub_auth,
            hub_mcp: crate::hub_mcp::HubMcp::open(
                crate::hub_mcp::default_origins(
                    config.port,
                    &config.public_url,
                    config.tls || !config.bind.is_loopback(),
                ),
                config.tls || !config.bind.is_loopback(),
                hub_mcp_db,
            ),
            prompts: crate::prompts::Prompts::new(prompt_store),
            skills: {
                // Skill files go to the Library's object store (the same Store the App holds) when Garage is configured.
                let blobs: Arc<dyn crate::skills::Blobs> = if library.has_garage() {
                    Arc::new(crate::skills::blobs::LibraryBlobs::new(Arc::clone(
                        &library,
                    )))
                } else {
                    Arc::new(crate::skills::blobs::FsBlobs::open(&config.state_dir))
                };
                crate::skills::Skills::new(
                    if db.enabled() {
                        Arc::new(crate::skills::store_db::DbSkills::new(Arc::clone(&db)))
                            as Arc<dyn crate::skills::SkillStore>
                    } else {
                        Arc::new(crate::skills::store_mem::MemorySkills::open(
                            &config.state_dir,
                        ))
                    },
                    blobs,
                )
            },
            settings: settings::Store::load(&config.state_dir, config.dev_models)?,
            appearance: crate::appearance::Store::load(&config.state_dir),
            artifacts: Arc::new(crate::artifacts::Store::load(
                &config.state_dir,
                Arc::clone(&library),
            )),
            shares: Arc::new(crate::shares::Store::load(&config.state_dir)),
            providers: providers::Store::load(&config.state_dir, Arc::clone(&vault))?,
            engines: crate::engine_instances::Store::load(&config.state_dir)?,
            usage_providers: crate::usage_providers::Store::load(&config.state_dir)?,
            roles: agents::Roles::load(&config.roles, &config.prompts)?,
            spawned: spawned::Store::load(config.state_dir.join("spawned.json")),
            agents: agents::Roster::with_db(roster_db),
            models: crate::foundry::registry::Models::open(&config.state_dir),
            hf: crate::foundry::hf_import::Imports::open(&config.state_dir),
            transcripts: transcript::Store::default(),
            work: scheduler::Queues::default(),
            plaza: plaza::Plaza::new(snapshot.as_deref()),
            noded: Upstream::with_token(config.noded_port, internal.clone()),
            hub: Upstream::with_token(config.hub_port, internal),
            token,
            config,
        });
        // The roster starts from the agents table before reconcile looks at the agents.
        agents::load_roster(&app);
        events::spawn_relays(Arc::clone(&app));
        agents::spawn_reconcile(Arc::clone(&app));
        crate::sleep::spawn_sweeper(Arc::clone(&app));
        crate::e2b::start(&app);
        spawned::spawn_respawn(Arc::clone(&app));
        scheduler::spawn(Arc::clone(&app));
        crate::mover::spawn(Arc::clone(&app));
        crate::steps::spawn(Arc::clone(&app));
        crate::replan::spawn(Arc::clone(&app));
        crate::autoscale::spawn(Arc::clone(&app));
        crate::metrics::spawn(Arc::clone(&app));
        crate::foundry::routes::spawn_tracker(Arc::clone(&app));
        // Model prices: refreshed when stale (at start, then hourly; the catalog decides).
        let prices_app = Arc::clone(&app);
        std::thread::spawn(move || {
            loop {
                prices_app.prices.refresh_if_stale(apz_plaza::now_ms());
                std::thread::sleep(std::time::Duration::from_secs(60 * 60));
            }
        });
        let _ = app.me.set(Arc::downgrade(&app));
        // A treasury.json that was refused is kept aside, not overwritten by the next save.
        if snapshot.is_some() && !app.plaza.restored {
            let path = app.config.state_dir.join("treasury.json");
            let _ = std::fs::rename(&path, path.with_extension("json.rejected"));
        }
        // The store is the state of record: every converted component loads from it.
        for problem in crate::store::saver::load(&app) {
            eprintln!("apid: store: {problem}");
        }
        // A budget hold is written through to plaza.db, acknowledged before its call is sent (budgets.rs).
        app.budgets.attach(Arc::clone(app.store.db(crate::store::PLAZA)));
        // Postgres, for the components not yet in the store.
        crate::db::restore_all(&app);
        // The built-in skills join the skill library (one the library already holds is left alone).
        if let Err(why) = crate::skills::seed_builtins(&app.skills, "owner", apz_plaza::now_ms()) {
            eprintln!("apid: cannot seed the built-in skills: {why}");
        }
        // The built-in role prompts join the prompt library (new ones published, the owner's edits kept).
        crate::prompts::seed::import(
            &app.prompts,
            &app.roles.prompts,
            |role| crate::agents::role_label(role).to_string(),
            apz_plaza::now_ms(),
        );
        // The vault starts after the database is read: it initialises, unseals with its key file, or stays sealed.
        app.vault
            .boot(apz_plaza::now_ms())
            .map_err(|err| std::io::Error::other(err.message()))?;
        app.vault.startup(
            &app,
            app.config.env_file.as_deref(),
            &vault_env,
            apz_plaza::now_ms(),
        );
        crate::vault::spawn_saver(Arc::clone(&app));
        // Weft starts once the vault can give it its keys.
        crate::weft_hub::start(Arc::clone(&app));
        crate::shares::relay::start(Arc::clone(&app));
        // Promoted models come back on the roster with their CIDs.
        app.models.revive(&app.agents);
        crate::db::spawn_writer(Arc::clone(&app));
        crate::store::saver::spawn(Arc::clone(&app));
        spawn_treasury_saver(Arc::clone(&app));
        spawn_component_savers(Arc::clone(&app));
        // A stop signal (SIGTERM, Ctrl-C) writes the state once more, in order, then exits (shutdown.rs).
        let stopping = app.handle();
        crate::shutdown::on_stop(move || {
            let Some(app) = stopping.upgrade() else {
                return;
            };
            eprintln!("apid: stopping: writing state");
            // The archives first: a snapshot that no longer holds what moved out is saved after its lines are.
            save_archives(&app);
            save_treasury(&app);
            save_components(&app);
            crate::store::saver::pass(&app);
            crate::db::write_pass(&app);
            app.store.close();
            crate::weft_hub::stop(&app);
        });
        crate::provider_health::spawn(Arc::clone(&app));
        crate::providers::spawn_refresher(Arc::clone(&app));
        crate::stations::spawn(Arc::clone(&app));
        crate::harness_control::spawn_ticker(Arc::clone(&app));
        crate::maintenance::spawn_ticker(Arc::clone(&app));
        // Deliverables (videos, images) arrive as request bodies: allow up to 512 MiB.
        // On loopback the hub serves plain HTTP as before. Bound anywhere else it is a hub other
        // machines reach: always HTTPS, with the certificate made on first start (its SHA-256 is
        // the pin join tokens carry), and the owner token required off loopback (hub_auth::gate).
        let mut server = if app.config.bind.is_loopback() {
            apz_http::Server::bind(app.config.port)?
        } else {
            apz_http::Server::bind_addr(std::net::SocketAddr::new(
                app.config.bind,
                app.config.port,
            ))?
        };
        if app.config.tls || !app.config.bind.is_loopback() {
            let dir = app.config.state_dir.join("tls");
            let pair = apz_trust::tls::CertPair {
                cert_pem: std::fs::read_to_string(dir.join("cert.pem"))?,
                key_pem: std::fs::read_to_string(dir.join("key.pem"))?,
            };
            let tls = apz_trust::tls::server_config(&pair).map_err(std::io::Error::other)?;
            server = server.tls(tls);
            eprintln!(
                "apid: serving HTTPS on {}:{} (certificate pin {})",
                app.config.bind,
                app.config.port,
                app.hub_auth.fingerprint_b64()
            );
        }
        // Deliverables (videos, images) arrive as request bodies: allow up to 512 MiB.
        let server = server.limits(apz_http::Limits {
            max_head: 16 << 10,
            max_body: 512 << 20,
        });
        // The content origin for HTML artifacts (content.rs): a second listener, kept for the hub's life. A hub
        // that cannot open it still serves everything else; HTML artifacts then show their stills.
        match crate::content::spawn(&app) {
            Ok(handle) => std::mem::forget(handle),
            Err(e) => eprintln!("apid: the content origin did not start: {e}"),
        }
        let router = api::router();
        // Who may send a body, and how much, is decided from the request's headers before the body is
        // read: a caller the gate refuses gets its answer without one (`hub_auth::body_allowance`).
        let admitting = Arc::clone(&app);
        let server =
            server.admit(
                move |request| match crate::hub_auth::gate(&admitting, request) {
                    Some(refused) => Err(refused),
                    None => Ok(crate::hub_auth::body_allowance(&admitting, request)),
                },
            );
        // The hub's front door first (owner token off loopback), then the routes.
        server.spawn(move |request| {
            let started = std::time::Instant::now();
            let response = match crate::hub_auth::gate(&app, request) {
                Some(refused) => refused,
                None => router.handle(&app, request),
            };
            crate::metrics::collect::request(
                &app,
                &request.method,
                &request.path,
                response.status,
                started.elapsed().as_secs_f64() * 1000.0,
            );
            response
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
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("export ").unwrap_or(line).split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            )
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// Saves the Foundry (`<state>/foundry/state.json`) every three seconds whenever it changed. Its file is
/// the copy at start when Postgres does not hold it yet; a component whose load failed is not written
/// (db::Db::is_held).
fn spawn_component_savers(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            crate::mcp::tick(&app);
            save_archives(&app);
            save_components(&app);
        }
    });
}

/// Appends what the full task ledger and the full journal moved out ([`apz_plaza::tasks::TaskLedger::take_archived`],
/// [`apz_plaza::treasury::Treasury::take_archived`]) to `<state-dir>/tasks-archive.jsonl` and
/// `<state-dir>/treasury-archive.jsonl`, one JSON line each. Their sums stay in the treasury, so a line that cannot be
/// written is logged and lost from the archive only.
fn save_archives(app: &App) {
    for (name, lines) in [
        ("tasks-archive.jsonl", app.plaza.tasks.take_archived()),
        ("treasury-archive.jsonl", app.plaza.treasury.take_archived()),
    ] {
        if lines.is_empty() {
            continue;
        }
        let path = app.config.state_dir.join(name);
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write;
                let mut text = lines.join("\n");
                text.push('\n');
                file.write_all(text.as_bytes())?;
                file.sync_data()
            });
        if let Err(err) = appended {
            eprintln!(
                "apid: cannot append {} archived lines to {}: {err}",
                lines.len(),
                path.display()
            );
        }
    }
}

/// One pass of the component saves, run by the loop and by the stop: a component Postgres holds is skipped (its
/// load failed); a failed save is logged each time.
fn save_components(app: &App) {
    if !app.db.is_held("foundry")
        && let Err(err) = app.foundry.save()
    {
        eprintln!("apid: cannot save the foundry: {err}");
    }
}

/// Saves the treasury to `<state-dir>/treasury.json` every three seconds whenever
/// its version has moved; the first tick always writes.
fn spawn_treasury_saver(app: Arc<App>) {
    std::thread::spawn(move || {
        let mut saved = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let version = app.plaza.treasury.version();
            if saved == Some(version) {
                continue;
            }
            if save_treasury(&app) {
                saved = Some(version);
            }
        }
    });
}

/// One pass of the treasury save, run by the loop and by the stop. Returns whether the write landed; a failure is
/// logged.
fn save_treasury(app: &App) -> bool {
    let path = app.config.state_dir.join("treasury.json");
    match write_atomic(&path, &app.plaza.treasury.snapshot()) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("apid: cannot save {}: {err}", path.display());
            false
        }
    }
}

/// Counts the temporary files this process has started, so no two writes share a temporary name.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `text` to `path` through a temporary file and a rename, so a crash
/// mid-write leaves the previous snapshot whole. The temporary name carries the process id and
/// a counter, so the stop pass and a saver loop never write the same temporary file.
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let count = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = path
        .file_name()
        .unwrap_or(std::ffi::OsStr::new("snapshot"))
        .to_os_string();
    tmp_name.push(format!(".{}.{count}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_file_is_read_without_touching_the_process_environment() {
        let dir = std::env::temp_dir().join(format!("apid-envfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keys.env");
        std::fs::write(&file, "ENVFILE_KEY_4417=SENTINEL-ENVFILE-4417\n").unwrap();
        let map = read_env_file(&file);
        assert_eq!(
            map.get("ENVFILE_KEY_4417").map(String::as_str),
            Some("SENTINEL-ENVFILE-4417")
        );
        assert!(
            std::env::var("ENVFILE_KEY_4417").is_err(),
            "the loader must not set the process environment"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
