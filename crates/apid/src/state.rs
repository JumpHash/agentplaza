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
    /// The domain whose subdomains are previews (`--preview-domain`): `<lane>--<project>.<domain>` reaches the lane's
    /// preview. Empty when the hub serves no preview subdomains.
    pub preview_domain: String,
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
            preview_domain: String::new(),
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
                "--preview-domain" => {
                    config.preview_domain = value()?.trim_matches('.').to_ascii_lowercase()
                }
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
    pub bus: Arc<events::Bus>,
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
    /// The Library of deliverables: files agents handed in, their bytes in the object store (`blobs`).
    pub deliverables: Arc<crate::deliverables::Store>,
    /// What agents found on the internet and judged, for every agent to use.
    pub toolbox: crate::toolbox::Store,
    /// The hub's generations of its cached answers, by source (`sources.rs`, `skills`): a source's key moves with each
    /// write of the source, and a cache answers only while the generation it was built from still holds (PLAN 30.4).
    pub generations: Arc<apz_cache::Generations<String>>,
    /// The sessions' tools sections, built from their sources and cached by them (`tools::Sections`).
    pub tool_sections: crate::tools::Sections,
    /// Projects: every goal's flow from brief to delivery.
    pub projects: crate::projects::Store,
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
    /// The review graph's runs and recipes (review_graph), kept in plaza.db; the change trees are in the object store.
    pub reviews: crate::review_graph::Reviews,
    /// The hub's store: SQLite in the state folder (store/mod.rs). Memory is the working copy; the saver keeps the
    /// components' tables equal to it.
    pub store: Arc<apz_store::Store>,
    /// The object store (apz-blobs, `<state-dir>/blobs`): the bytes of the Library, the artifacts, the skill files and
    /// the stations' frames and sync content, each by its BLAKE3 hash (storage.rs).
    pub blobs: Arc<apz_blobs::Blobs>,
    /// What each component last saved (store/saver.rs).
    pub saved: crate::store::saver::Ledger,
    /// The work engine's sessions (sessions.rs), the seats of tasks (seats.rs), the budgets (budgets.rs), the cards
    /// waiting for the owner (cards.rs), the chats (chats.rs) and the seats' threads (threads.rs).
    pub sessions: crate::sessions::Sessions,
    pub seats: crate::seats::Seats,
    /// The recipe engine's runs and the owner's recipe versions (recipes/runs.rs).
    pub recipe_runs: crate::recipes::runs::Runs,
    /// The releases of every project, each with its life as Weft's statements made it (releases/).
    pub releases: crate::releases::entity::Store,
    /// The triggers of the recipes (14.6): the schedules and the event records, and the deliveries in their windows.
    pub triggers: crate::triggers::Triggers,
    pub budgets: crate::budgets::Budgets,
    pub cards: crate::cards::Cards,
    /// The VMs' workspace reports and the disk quotas' check (disk.rs).
    pub disk: crate::disk::Disk,
    pub chats: crate::chats::Chats,
    pub threads: crate::threads::Threads,
    /// The tasks' journals: notes and handovers (journal.rs, 9.2).
    pub journal: crate::journal::Journals,
    /// The unfurl answers for the references clients show as chips, kept in `plaza.db` (unfurl/mod.rs).
    pub unfurl: crate::unfurl::Cache,
    /// STOP ALL / resume.
    pub control: crate::control::Control,
    /// Roles the owner defined or approved.
    pub custom_roles: crate::roles_custom::Store,
    /// The orchestrator's proposals for new agents.
    pub agent_proposals: crate::agent_proposals::Store,
    /// The project memory: the entries read from each project's main, and the proposals agents made.
    pub project_memory: crate::project_memory::Store,
    /// The projects' curations: the finished tasks whose journals wait for the curator, and its runs (curator.rs).
    pub curator: crate::curator::Store,
    /// The projects' verifiers: when each last started (verifier.rs).
    pub verifier: crate::verifier::Store,
    /// The hub's nodes, join codes and command queues.
    pub nodes: Arc<crate::nodes::Store>,
    /// The routes the server dispatches to (set at start). A node's requests over its tunnel go through them too.
    pub routes: std::sync::OnceLock<Arc<apz_http::Router<App>>>,
    /// The test lab: its runs and their reservations (test_lab.rs).
    pub test_lab: crate::test_lab::Store,
    /// The workstations: their join codes, registry, live links and leases (docs/design/workstations.md).
    pub stations: crate::stations::Store,
    /// Model prices (OpenRouter, LiteLLM and the owner's own), for usage costs.
    pub prices: crate::prices::Catalog,
    /// Every model call's tokens, cache hits and cost.
    pub usage: crate::usage::Ledger,
    /// The eval records of step sessions (`evals.rs`): written as sessions end, read by `/v1/evals`.
    pub evals: crate::evals::Evals,
    /// Test selection (PLAN 15.4): the coverage maps per project and commit, and the misses (`test_select.rs`).
    pub test_select: crate::test_select::TestSelect,
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
    /// The owner's standing rules, with their versions: part 2 of every session's prompt (standing_rules.rs).
    pub standing_rules: crate::standing_rules::StandingRules,
    /// The intake records: GitHub, Linear and Notion items, their connections and write-backs (intake/mod.rs).
    pub intake: crate::intake::Store,
    /// Agents' artifacts (artifacts/), their versions and data blobs.
    pub artifacts: Arc<crate::artifacts::Store>,
    /// Share links (shares/).
    pub shares: Arc<crate::shares::Store>,
    /// Previews of lanes and their share links (previews/).
    pub previews: Arc<crate::previews::Store>,
    pub providers: providers::Store,
    /// The coding engines as instances (`engine_instances.rs`).
    pub engines: crate::engine_instances::Store,
    /// The usage providers (CLIProxyAPI hubs), `usage_providers.rs`.
    pub usage_providers: crate::usage_providers::Store,
    /// The login bridge's switches, per vendor (`login_bridge.rs`).
    pub login_bridges: crate::login_bridge::Store,
    /// The sessions an allowance paused, and the vendor each waits on (`allowances.rs`).
    pub allowance_waits: crate::allowances::Waits,
    pub roles: agents::Roles,
    pub agents: agents::Roster,
    /// The model registry: the records on disk and the compiled models in memory.
    pub models: crate::foundry::registry::Models,
    /// The Hugging Face import queue and the owner's token (foundry/hf_import.rs).
    pub hf: crate::foundry::hf_import::Imports,
    /// The owner's egress credential rules (egress_inject.rs).
    pub egress_rules: crate::egress_inject::Store,
    /// The E2B templates and their builds (e2b_tpl.rs).
    pub e2b_templates: crate::e2b_tpl::Templates,
    /// Operator-spawned agents and model overrides (plaza.db, the `spawned` table).
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
        let generations: Arc<apz_cache::Generations<String>> = Arc::default();
        // The same file the token was just written to: noded and vsockd are told it, and every call
        // this app makes to them presents it.
        let internal = apz_http::auth::InternalToken::file(config.state_dir.join("internal.token"));
        let hub_auth = crate::hub_auth::Hub::new(&config.state_dir, &config.public_url, config.tls)
            .map_err(std::io::Error::other)?;
        let foundry_dir = config.state_dir.join("foundry");
        // The store first: it takes the folder's lock, so a second hub on this folder stops here.
        let store = Arc::new(crate::store::open(&config.state_dir).map_err(std::io::Error::other)?);
        // The object store (apz-blobs): the bytes the Library, the artifacts, the skills and the stations keep, by hash.
        let blobs = Arc::new(
            apz_blobs::Blobs::open(&config.state_dir.join("blobs"))
                .map_err(std::io::Error::other)?,
        );
        let prompt_store: Arc<dyn crate::prompts::PromptStore> = Arc::new(
            crate::prompts::store::SqlitePrompts::new(Arc::clone(store.db(crate::store::PLAZA))),
        );
        let vault = Arc::new(
            crate::vault::Store::open(&config.vault_key, Arc::clone(store.db(crate::store::PLAZA)))
                .with_generations(Arc::clone(&generations)),
        );
        // The vendor bindings (Auggie's session) show in the console's vault screen from boot.
        crate::vendor_relay::register(&vault);
        // The Library of deliverables, over the object store.
        let library = Arc::new(crate::deliverables::Store::open(Arc::clone(&blobs)));
        let bus = Arc::new(events::Bus::default());
        let app = Arc::new(App {
            me: std::sync::OnceLock::new(),
            bus: Arc::clone(&bus),
            laws: crate::laws::global(),
            connectors: crate::connectors::Store::new(Arc::clone(&vault))
                .with_generations(Arc::clone(&generations)),
            approvals: crate::approvals::Store::default(),
            acp_sessions: crate::acp::store::Sessions::default(),
            engine_feed: crate::engine_feed::Feed::default(),
            library: crate::library::Store::open(&config.state_dir),
            deliverables: Arc::clone(&library),
            toolbox: crate::toolbox::Store::default(),
            generations: Arc::clone(&generations),
            tool_sections: crate::tools::Sections::new(),
            projects: crate::projects::Store::default().with_generations(Arc::clone(&generations)),
            context: Default::default(),
            project_settings: Default::default(),
            harness: crate::harness_control::Store::default(),
            weft: crate::weft_hub::Weft::default(),
            platforms: crate::platforms::Store::new(),
            maintenance: Default::default(),
            environments: crate::environments::Store::default()
                .with_generations(Arc::clone(&generations)),
            reviews: crate::review_graph::Reviews::new(Arc::clone(&blobs)),
            store: Arc::clone(&store),
            blobs: Arc::clone(&blobs),
            saved: crate::store::saver::Ledger::default(),
            sessions: Default::default(),
            seats: Default::default(),
            recipe_runs: Default::default(),
            releases: Default::default(),
            triggers: Default::default(),
            budgets: crate::budgets::Budgets::announcing(Arc::clone(&bus)),
            cards: Default::default(),
            disk: Default::default(),
            chats: Default::default(),
            threads: Default::default(),
            journal: Default::default(),
            unfurl: crate::unfurl::Cache::new(Arc::clone(store.db(crate::store::PLAZA))),
            vault: Arc::clone(&vault),
            control: crate::control::Control::default(),
            custom_roles: crate::roles_custom::Store::default()
                .with_generations(Arc::clone(&generations)),
            agent_proposals: crate::agent_proposals::Store::default(),
            project_memory: crate::project_memory::Store::default(),
            curator: crate::curator::Store::default(),
            verifier: crate::verifier::Store::default(),
            nodes: Arc::default(),
            routes: std::sync::OnceLock::new(),
            test_lab: crate::test_lab::Store::default(),
            stations: crate::stations::Store::default(),
            prices: crate::prices::Catalog::default(),
            usage: crate::usage::Ledger::new(Arc::clone(store.db(crate::store::PLAZA))),
            evals: crate::evals::Evals::new(Arc::clone(store.db(crate::store::PLAZA))),
            test_select: crate::test_select::TestSelect::default(),
            metrics: crate::metrics::Metrics::open(
                Arc::clone(store.db(crate::store::METRICS)),
                apz_plaza::now_ms(),
            ),
            foundry: crate::foundry::store::Foundry::new(foundry_dir),
            mcp: crate::mcp::Mcp::load(
                &config.state_dir,
                config.mcp_proxy_port,
                Upstream::with_token(config.noded_port, internal.clone()),
            )
            .with_generations(Arc::clone(&generations)),
            hub_auth,
            hub_mcp: crate::hub_mcp::HubMcp::open(
                crate::hub_mcp::default_origins(
                    config.port,
                    &config.public_url,
                    config.tls || !config.bind.is_loopback(),
                ),
                config.tls || !config.bind.is_loopback(),
                Arc::clone(store.db(crate::store::PLAZA)),
            ),
            prompts: crate::prompts::Prompts::new(prompt_store),
            skills: {
                // Skill files are kept in the object store, by BLAKE3 hash.
                let skill_blobs: Arc<dyn crate::skills::Blobs> =
                    Arc::new(crate::skills::blobs::StoreBlobs::new(Arc::clone(&blobs)));
                crate::skills::Skills::with_generations(
                    Arc::new(crate::skills::store_db::DbSkills::new(Arc::clone(
                        store.db(crate::store::PLAZA),
                    ))),
                    skill_blobs,
                    Arc::clone(&generations),
                )
            },
            settings: settings::Store::new(config.dev_models)
                .with_generations(Arc::clone(&generations)),
            appearance: crate::appearance::Store::new(),
            standing_rules: Default::default(),
            intake: Default::default(),
            artifacts: Arc::new(crate::artifacts::Store::new(
                Arc::clone(store.db(crate::store::PLAZA)),
                Arc::clone(&blobs),
            )),
            shares: Arc::new(crate::shares::Store::open(
                Arc::clone(store.db(crate::store::PLAZA)),
                &config.state_dir,
            )),
            previews: Arc::new(crate::previews::Store::open(
                Arc::clone(store.db(crate::store::PLAZA)),
                &config.state_dir,
            )),
            providers: providers::Store::new(Arc::clone(&vault)),
            engines: crate::engine_instances::Store::new(&config.state_dir)
                .with_generations(Arc::clone(&generations)),
            usage_providers: crate::usage_providers::Store::default(),
            login_bridges: crate::login_bridge::Store::default(),
            allowance_waits: crate::allowances::Waits::default(),
            roles: agents::Roles::load(&config.roles, &config.prompts)?,
            spawned: spawned::Store::default(),
            agents: agents::Roster::default(),
            models: crate::foundry::registry::Models::open(&config.state_dir),
            hf: crate::foundry::hf_import::Imports::new(&config.state_dir),
            egress_rules: Default::default(),
            e2b_templates: Default::default(),
            transcripts: transcript::Store::default(),
            work: scheduler::Queues::default(),
            plaza: plaza::Plaza::new(),
            noded: Upstream::with_token(config.noded_port, internal.clone()),
            hub: Upstream::with_token(config.hub_port, internal),
            token,
            config,
        });
        events::spawn_relays(Arc::clone(&app));
        agents::spawn_reconcile(Arc::clone(&app));
        crate::sleep::spawn_sweeper(Arc::clone(&app));
        crate::previews::life::spawn_sweeper(Arc::clone(&app));
        crate::e2b::start(&app);
        scheduler::spawn(Arc::clone(&app));
        crate::mover::spawn(Arc::clone(&app));
        crate::tunnel::spawn_mesh(Arc::clone(&app));
        crate::transfer::start_broker(Arc::clone(&app));
        crate::steps::spawn(Arc::clone(&app));
        crate::replan::spawn(Arc::clone(&app));
        crate::autoscale::spawn(Arc::clone(&app));
        crate::metrics::spawn(Arc::clone(&app));
        crate::storage::spawn_sweeper(Arc::clone(&app));
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
        // The store is the state of record: every converted component loads from it.
        for problem in crate::store::saver::load(&app) {
            eprintln!("apid: store: {problem}");
        }
        // The egress rules' consumers and the E2B builds the hub stopped during are set up from the loaded store.
        crate::egress_inject::register_consumers(&app);
        crate::e2b_tpl::recover(&app);
        // A VM's transcript entries are keyed by the session it works in.
        let sessions_of = Arc::downgrade(&app);
        app.transcripts.key_by_session(Box::new(move |cid| {
            sessions_of.upgrade()?.sessions.by_cid(cid).map(|s| s.id)
        }));
        // Once the spawned agents are loaded: the respawn reads them as it starts.
        spawned::spawn_respawn(Arc::clone(&app));
        // A budget hold is written through to plaza.db, acknowledged before its call is sent (budgets.rs).
        app.budgets
            .attach(Arc::clone(app.store.db(crate::store::PLAZA)));
        restore_transcripts(&app);
        // The house rules (the prompt library's AGENTS.md for the plaza) become the first standing rules, once: the
        // standing rules replace them in every prompt (standing_rules.rs).
        let house = app
            .prompts
            .bound_body("plaza")
            .is_some_and(|house| app.standing_rules.absorb(&house, apz_plaza::now_ms()));
        if house {
            crate::store::saver::save_now(&app, "standing_rules");
        }
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
        // The providers' command credentials and header values come from the vault (plaza.db keeps them blank).
        app.providers.sync_vault();
        // Weft starts once the vault can give it its keys.
        crate::weft_hub::start(Arc::clone(&app));
        crate::shares::relay::start(Arc::clone(&app));
        // Promoted models come back on the roster with their CIDs.
        app.models.revive(&app.agents);
        crate::store::saver::spawn(Arc::clone(&app));
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
            crate::store::saver::pass(&app);
            app.store.close();
            crate::weft_hub::stop(&app);
        });
        crate::provider_health::spawn(Arc::clone(&app));
        crate::triggers::ensure_builtins(&app, apz_plaza::now_ms());
        crate::providers::spawn_refresher(Arc::clone(&app));
        crate::stations::spawn(Arc::clone(&app));
        crate::harness_control::spawn_ticker(Arc::clone(&app));
        crate::maintenance::spawn_ticker(Arc::clone(&app));
        crate::allowances::spawn_ticker(Arc::clone(&app));
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
        let router = Arc::new(api::router());
        let _ = app.routes.set(Arc::clone(&router));
        // Who may send a body, and how much, is decided from the request's headers before the body is
        // read: a caller the gate refuses gets its answer without one (`hub_auth::body_allowance`).
        // A preview's subdomain is its lane's preview path, so the gate and the routes see that path (a link's token
        // admits it there, and nothing else of the hub is reachable from a subdomain).
        let admitting = Arc::clone(&app);
        let server = server.admit(move |request| {
            let routed =
                crate::previews::route::as_routed(request, &admitting.config.preview_domain);
            match crate::hub_auth::gate(&admitting, &routed) {
                Some(refused) => Err(refused),
                None => Ok(crate::hub_auth::body_allowance(&admitting, &routed)),
            }
        });
        // The hub's front door first (owner token off loopback), then the routes.
        server.spawn(move |request| {
            let started = std::time::Instant::now();
            let routed = crate::previews::route::as_routed(request, &app.config.preview_domain);
            let response = match crate::hub_auth::gate(&app, &routed) {
                Some(refused) => refused,
                None => router.handle(&app, &routed),
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

/// Loads the stored transcripts into the transcript store from `transcripts.db`, which then writes every record from
/// now on there.
fn restore_transcripts(app: &App) {
    if let Err(e) =
        crate::transcript::restore(&app.transcripts, app.store.db(crate::store::TRANSCRIPTS))
    {
        eprintln!("apid: transcripts not restored: {e}");
    }
}

/// Every three seconds: the MCP runtimes' tick and the archive lines the task ledger moved out.
fn spawn_component_savers(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            crate::mcp::tick(&app);
            save_archives(&app);
        }
    });
}

/// Appends what the full task ledger moved out ([`apz_plaza::tasks::TaskLedger::take_archived`]) to
/// `<state-dir>/tasks-archive.jsonl`, one JSON line each. A line that cannot be written is logged and lost from the
/// archive only.
fn save_archives(app: &App) {
    for (name, lines) in [("tasks-archive.jsonl", app.plaza.tasks.take_archived())] {
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
