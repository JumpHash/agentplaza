//! The application: configuration and the shared state every module reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::upstream::Upstream;
use crate::{agents, api, events, plaza, providers, scheduler, settings, transcript};

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
}

impl Config {
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let state_dir = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Path::new(&home).join(".local/state"))
            .join("agentplaza");
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
        };
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
                "--env-file" => config.env_file = Some(PathBuf::from(value()?)),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(config)
    }
}

/// Everything the handlers and background threads share.
pub struct App {
    pub config: Config,
    /// The SSE bus every module publishes on.
    pub bus: events::Bus,
    pub settings: settings::Store,
    pub providers: providers::Store,
    pub roles: agents::Roles,
    pub agents: agents::Roster,
    pub transcripts: transcript::Store,
    pub work: scheduler::Queues,
    pub plaza: plaza::Plaza,
    pub noded: Upstream,
    pub hub: Upstream,
    /// The secret the hub presents on `/internal/*` (`x-apz-internal`).
    pub token: String,
}

impl App {
    /// Loads the state, starts the background threads and serves.
    pub fn start(config: Config) -> std::io::Result<apz_http::Handle> {
        let env = config.env_file.as_deref().map(read_env_file).unwrap_or_default();
        std::fs::create_dir_all(&config.state_dir)?;
        let token = write_token(&config.state_dir)?;
        let app = Arc::new(App {
            bus: events::Bus::default(),
            settings: settings::Store::load(&config.state_dir, config.dev_models)?,
            providers: providers::Store::load(&config.state_dir, env)?,
            roles: agents::Roles::load(&config.roles, &config.prompts)?,
            agents: agents::Roster::default(),
            transcripts: transcript::Store::default(),
            work: scheduler::Queues::default(),
            plaza: plaza::Plaza::new(),
            noded: Upstream::new(config.noded_port),
            hub: Upstream::new(config.hub_port),
            token,
            config,
        });
        events::spawn_relays(Arc::clone(&app));
        agents::spawn_reconcile(Arc::clone(&app));
        scheduler::spawn(Arc::clone(&app));
        let server = apz_http::Server::bind(app.config.port)?;
        let router = api::router();
        server.spawn(move |request| router.handle(&app, request))
    }
}

/// Reads `KEY=VALUE` lines (blank lines and `#` comments skipped; an
/// `export ` prefix and surrounding quotes allowed). The values are provider
/// credentials: they are handed to the gateway, never logged.
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

/// A fresh random token in `<state-dir>/internal.token`, mode 0600.
fn write_token(dir: &Path) -> std::io::Result<String> {
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join("internal.token");
    let _ = std::fs::remove_file(&path);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    file.write_all(token.as_bytes())?;
    Ok(token)
}
