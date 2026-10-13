//! `agentplaza hub`: saved hubs. `login` checks the hub answers the owner token, then saves it
//! (in the platform's config directory, private to this user) so later commands need no flags.

use clap::Subcommand;
use serde_json::json;

use crate::ctx::{self, Client, Config, SavedHub};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Sign in to a hub and save it (the first saved hub becomes the default).
    Login {
        /// The hub's URL, e.g. https://plaza.example:7700 (default: the local hub).
        url: Option<String>,
        /// The owner token (`apid` keeps it in its state directory; with no URL, the local hub's is read there).
        #[arg(long, env = "APZ_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// The hub certificate's SHA-256 pin, hex (for a hub with its own certificate).
        #[arg(long)]
        pin: Option<String>,
        /// A short name for it (default: its host).
        #[arg(long)]
        name: Option<String>,
    },
    /// Forget a saved hub.
    Logout { name: String },
    /// The saved hubs; the default is marked.
    List,
    /// Make a saved hub the default.
    Use { name: String },
    /// Check the default (or `--hub`) hub answers and who it is.
    Status {
        #[arg(long, env = "APZ_HUB")]
        hub: Option<String>,
    },
}

pub fn run(cmd: Cmd, json_out: bool) -> Result<(), String> {
    let mut config = Config::load();
    match cmd {
        Cmd::Login { url, token, pin, name } => {
            let url = url.unwrap_or_else(|| ctx::LOCAL_HUB.to_string()).trim_end_matches('/').to_string();
            let token = token.or_else(|| (url == ctx::LOCAL_HUB).then(ctx::local_owner_token).flatten());
            let pin_bytes = pin.as_deref().map(ctx::parse_pin).transpose()?;
            let client = Client::new(&url, pin_bytes, token.clone());
            client.get("/v1/auth/check").map_err(|e| format!("{url} did not accept the sign-in: {e}"))?;
            let name = name.unwrap_or_else(|| host_of(&url));
            config.hubs.insert(name.clone(), SavedHub { url: url.clone(), token, pin });
            if config.default.is_none() {
                config.default = Some(name.clone());
            }
            config.save()?;
            println!("signed in to {url} as `{name}`{}", if config.default.as_deref() == Some(&name) { " (default)" } else { "" });
        }
        Cmd::Logout { name } => {
            if config.hubs.remove(&name).is_none() {
                return Err(format!("no saved hub `{name}`"));
            }
            if config.default.as_deref() == Some(&name) {
                config.default = config.hubs.keys().next().cloned();
            }
            config.save()?;
            println!("forgot `{name}`");
        }
        Cmd::List => {
            if json_out {
                let hubs: serde_json::Map<String, serde_json::Value> = config
                    .hubs
                    .iter()
                    .map(|(n, h)| (n.clone(), json!({ "url": h.url, "pinned": h.pin.is_some(), "token": h.token.is_some() })))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json!({ "default": config.default, "hubs": hubs })).unwrap_or_default());
                return Ok(());
            }
            let rows: Vec<Vec<String>> = config
                .hubs
                .iter()
                .map(|(n, h)| {
                    vec![
                        if config.default.as_deref() == Some(n) { format!("* {n}") } else { format!("  {n}") },
                        h.url.clone(),
                        if h.pin.is_some() { "pinned".into() } else { String::new() },
                    ]
                })
                .collect();
            ctx::table(&["hub", "url", "tls"], &rows);
        }
        Cmd::Use { name } => {
            if !config.hubs.contains_key(&name) {
                return Err(format!("no saved hub `{name}` (see `agentplaza hub list`)"));
            }
            config.default = Some(name.clone());
            config.save()?;
            println!("default hub: {name}");
        }
        Cmd::Status { hub } => {
            let ctx = ctx::resolve(hub, None, None, json_out)?;
            let check = ctx.client.get("/v1/auth/check")?;
            ctx.out(&check, |_| println!("{}: signed in", ctx.client.base()));
        }
    }
    Ok(())
}

/// `https://plaza.example:7700` → `plaza.example`.
fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    host.trim_matches(['[', ']']).to_string()
}
