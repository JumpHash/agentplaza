//! `agentplaza mcp`: the hub as an MCP server (docs/design/hub-mcp.md). The owner turns the server
//! on, makes tokens for clients, and saves a token for `serve`, the stdio bridge MCP clients run.
//! The bridge itself lives in `mcp/bridge.rs`.

pub mod bridge;

use std::io::{self, IsTerminal, Read};
use std::time::Duration;

use apz_http::client::Target;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Config, Ctx, SavedHub};

use crate::cmd::hub;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The hub's MCP server: its status, and on and off.
    #[command(subcommand)]
    Server(ServerCmd),
    /// MCP tokens: the keys clients use to reach the server.
    #[command(subcommand)]
    Token(TokenCmd),
    /// Save an MCP token for a hub (--hub). The token is read from stdin, never from the command line.
    Login,
    /// The stdio bridge: JSON-RPC on stdin and stdout, forwarded to the hub's /mcp with the saved token.
    Serve,
}

#[derive(Subcommand, Debug)]
pub enum ServerCmd {
    /// Whether the server is on, its bind, its origins and its URL.
    Status,
    /// Turn the server on. By default it answers on loopback only; --remote answers on the public listener, over TLS.
    Enable {
        /// Answer on the hub's public listener, over TLS.
        #[arg(long)]
        remote: bool,
    },
    /// Turn the server off. Its bind and origins are kept.
    Disable,
}

#[derive(Subcommand, Debug)]
pub enum TokenCmd {
    /// Make a token. It is printed once: it is not shown again.
    Create {
        /// A name for the token, 1 to 80 characters.
        #[arg(long)]
        name: String,
        /// What the token may do: observe (read), operate, or govern (file approvals).
        #[arg(long, value_parser = ["observe", "operate", "govern"])]
        scope: String,
        /// Only these tools (comma-separated); by default, every tool of the scope.
        #[arg(long, value_delimiter = ',')]
        tools: Vec<String>,
        /// When it expires, in days, like 30d (default: never).
        #[arg(long)]
        expires: Option<String>,
    },
    /// The tokens, with their scope, last use and calls.
    List,
    /// Revoke a token by its id, so it no longer opens the server.
    Revoke {
        /// The token's id, from `agentplaza mcp token list`.
        id: String,
    },
}

/// The flags every command takes, as clap resolved them from the command line and the environment.
pub struct Globals {
    pub hub: Option<String>,
    pub token: Option<String>,
    pub pin: Option<String>,
    pub json: bool,
}

pub fn run(cmd: Cmd, globals: Globals) -> Result<(), String> {
    match cmd {
        Cmd::Server(sub) => {
            let api = ctx::resolve(globals.hub, globals.token, globals.pin, globals.json)?;
            server(&api, sub)
        }
        Cmd::Token(sub) => {
            let api = ctx::resolve(globals.hub, globals.token, globals.pin, globals.json)?;
            token(&api, sub)
        }
        Cmd::Login => login(globals.hub.as_deref(), io::stdin().lock()),
        Cmd::Serve => serve(globals.hub.as_deref(), globals.pin.as_deref()),
    }
}

fn server(api: &Ctx, sub: ServerCmd) -> Result<(), String> {
    match sub {
        ServerCmd::Status => {
            let status = api.client.get("/v1/hub-mcp")?;
            api.out(&status, print_status);
            Ok(())
        }
        ServerCmd::Enable { remote } => set_enabled(api, true, remote),
        ServerCmd::Disable => set_enabled(api, false, false),
    }
}

/// Saves the switch. The PUT replaces every setting, so it carries the rest of what the hub has now: the auth mode,
/// the open scope and the origins. The bind is local or remote when turning on, and kept when turning off.
fn set_enabled(api: &Ctx, enabled: bool, remote: bool) -> Result<(), String> {
    let now = api.client.get("/v1/hub-mcp")?;
    let body = switch_body(&now, enabled, remote);
    let saved = api.client.put("/v1/hub-mcp", &body)?;
    api.out(&saved, print_status);
    Ok(())
}

/// The PUT body that flips the switch and keeps the rest of the hub's settings as `now` (the GET) reports them.
/// An open hub that is already on remote keeps its confirmed setting, so the hub does not refuse the save; a change
/// that newly opens a hub to the network still gets the hub's refusal.
fn switch_body(now: &Value, enabled: bool, remote: bool) -> Value {
    let bind = match (enabled, remote) {
        (true, true) => "remote",
        (true, false) => "local",
        (false, _) => now["bind"].as_str().unwrap_or("local"),
    };
    let auth_mode = now.get("auth_mode").cloned().unwrap_or_else(|| json!("secure"));
    let open_scope = now.get("open_scope").cloned().unwrap_or_else(|| json!("observe"));
    let origins = now.get("allowed_origins").cloned().unwrap_or_else(|| json!([]));
    let kept_open_remote = now["auth_mode"].as_str() == Some("open") && now["bind"].as_str() == Some("remote");
    json!({
        "enabled": enabled,
        "bind": bind,
        "auth_mode": auth_mode,
        "open_scope": open_scope,
        "allowed_origins": origins,
        "confirm_open_remote": kept_open_remote && bind == "remote",
    })
}

fn print_status(status: &Value) {
    let origins: Vec<&str> = status["allowed_origins"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    ctx::fields(&[
        ("server", if status["enabled"].as_bool() == Some(true) { "on".to_string() } else { "off".to_string() }),
        ("bind", ctx::text(status, "bind")),
        ("url", ctx::text(status, "url")),
        ("origins", if origins.is_empty() { "(the hub's own)".to_string() } else { origins.join(", ") }),
        ("tokens", ctx::text(status, "tokens")),
    ]);
}

fn token(api: &Ctx, sub: TokenCmd) -> Result<(), String> {
    match sub {
        TokenCmd::Create { name, scope, tools, expires } => {
            let expires_days = expires.as_deref().map(expiry_days).transpose()?;
            let body = json!({ "name": name, "scope": scope, "tools": tools, "expires_days": expires_days });
            let made = api.client.post("/v1/hub-mcp/tokens", &body)?;
            let secret = made["token"].as_str().ok_or("the hub did not return a token")?.to_string();
            api.out(&made, |_| println!("{secret}"));
            eprintln!("warning: this token is shown only now. It is not stored in readable form and cannot be shown again.");
            Ok(())
        }
        TokenCmd::List => {
            let list = api.client.get("/v1/hub-mcp/tokens")?;
            api.out(&list, |v| {
                let rows: Vec<Vec<String>> = v["tokens"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| {
                        vec![
                            ctx::text(t, "id"),
                            ctx::text(t, "name"),
                            ctx::text(t, "scope"),
                            tools_text(t),
                            ctx::when(t.get("expires_ms").unwrap_or(&Value::Null)),
                            ctx::ago(t.get("last_used_ms").unwrap_or(&Value::Null)),
                            ctx::text(t, "calls"),
                            if t["revoked_at_ms"].is_null() { "live".to_string() } else { "revoked".to_string() },
                        ]
                    })
                    .collect();
                ctx::table(&["id", "name", "scope", "tools", "expires", "last used", "calls", "state"], &rows);
            });
            Ok(())
        }
        TokenCmd::Revoke { id } => {
            api.client.delete(&format!("/v1/hub-mcp/tokens/{}", ctx::seg(&id)))?;
            api.out(&json!({ "revoked": id }), |_| println!("revoked {id}"));
            Ok(())
        }
    }
}

fn tools_text(token: &Value) -> String {
    token["tools"]
        .as_array()
        .filter(|tools| !tools.is_empty())
        .map(|tools| tools.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))
        .unwrap_or_else(|| "all in scope".to_string())
}

/// `--expires`: days, as `30d` or `30`.
fn expiry_days(text: &str) -> Result<u64, String> {
    text.strip_suffix('d')
        .unwrap_or(text)
        .parse::<u64>()
        .ok()
        .filter(|days| *days >= 1)
        .ok_or_else(|| format!("--expires is a number of days, like 30d: {text}"))
}

/// Reads the MCP token from `input` (stdin in the command). It must be an `apz_mcp_` token, alone on its text.
fn read_token(mut input: impl Read) -> Result<String, String> {
    let mut text = String::new();
    input.read_to_string(&mut text).map_err(|e| format!("reading the token from stdin: {e}"))?;
    let token = text.trim();
    if !token.starts_with("apz_mcp_") || token.chars().any(char::is_whitespace) {
        return Err("stdin does not hold an apz_mcp_ token".to_string());
    }
    Ok(token.to_string())
}

fn login(hub: Option<&str>, input: impl Read) -> Result<(), String> {
    let hub = hub.ok_or("--hub URL is required: the hub this token is for")?;
    if io::stdin().is_terminal() {
        eprintln!("paste the token, then press Ctrl-D");
    }
    let token = read_token(input)?;
    let mut config = Config::load();
    let url = config.hubs.get(hub).map(|saved| saved.url.clone()).unwrap_or_else(|| hub.trim_end_matches('/').to_string());
    attach_token(&mut config, &url, &token);
    config.save()?;
    println!("saved an MCP token for {url}");
    Ok(())
}

/// Saves `token` as the MCP token of the entry for `url`, making one (named for its host) when there is none.
/// The entry's owner token is left as it is.
fn attach_token(config: &mut Config, url: &str, token: &str) {
    let name = config.hubs.iter().find(|(_, saved)| saved.url == url).map(|(name, _)| name.clone()).unwrap_or_else(|| hub::host_of(url));
    let entry = config.hubs.entry(name.clone()).or_insert_with(|| SavedHub { url: url.to_string(), token: None, pin: None, mcp_token: None });
    entry.mcp_token = Some(token.to_string());
    if config.default.is_none() {
        config.default = Some(name);
    }
}

/// The saved hub `--hub` names (or whose URL it is), else the default one.
fn saved_hub(config: &Config, hub: Option<&str>) -> Result<SavedHub, String> {
    let key = match hub {
        Some(key) => key.to_string(),
        None => config.default.clone().ok_or("no saved hub: sign in with `agentplaza hub login`, or pass --hub")?,
    };
    if let Some(saved) = config.hubs.get(&key) {
        return Ok(saved.clone());
    }
    let url = key.trim_end_matches('/');
    if let Some(saved) = config.hubs.values().find(|saved| saved.url == url) {
        return Ok(saved.clone());
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(SavedHub { url: url.to_string(), token: None, pin: None, mcp_token: None });
    }
    Err(format!("no saved hub `{key}`"))
}

fn serve(hub: Option<&str>, pin: Option<&str>) -> Result<(), String> {
    let config = Config::load();
    let saved = saved_hub(&config, hub)?;
    let token = saved
        .mcp_token
        .clone()
        .ok_or_else(|| format!("no MCP token for {}: run `agentplaza mcp login --hub {}` first", saved.url, saved.url))?;
    let pin = pin.map(str::to_string).or_else(|| saved.pin.clone()).map(|hex| ctx::parse_pin(&hex)).transpose()?;
    let target = Target { base: saved.url.trim_end_matches('/').to_string(), pin, bearer: Some(token), headers: Vec::new() };
    let transport = bridge::HubTransport::new(target, Duration::from_secs(120));
    bridge::serve(transport, io::stdin().lock(), io::stdout().lock()).map_err(|e| format!("the bridge stopped: {e}"))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// The subcommands alone, as the command line parses them (the global flags live in main).
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn saved(url: &str, token: Option<&str>) -> SavedHub {
        SavedHub { url: url.into(), token: token.map(str::to_string), pin: None, mcp_token: None }
    }

    #[test]
    fn login_takes_no_token_as_an_argument() {
        assert!(TestCli::try_parse_from(["agentplaza", "login", "apz_mcp_secret"]).is_err(), "a positional token is refused");
        assert!(TestCli::try_parse_from(["agentplaza", "login", "--token", "apz_mcp_secret"]).is_err(), "a --token flag is refused");
        assert!(TestCli::try_parse_from(["agentplaza", "login"]).is_ok());
    }

    #[test]
    fn the_login_token_comes_from_stdin_and_must_be_an_mcp_token() {
        assert_eq!(read_token("  apz_mcp_abc123\n".as_bytes()).unwrap(), "apz_mcp_abc123");
        assert!(read_token("owner-token".as_bytes()).is_err(), "an owner token is not an MCP token");
        assert!(read_token("apz_mcp_a b".as_bytes()).is_err(), "one token only");
        assert!(read_token("".as_bytes()).is_err());
    }

    #[test]
    fn login_keeps_the_owner_token_and_saves_the_mcp_one_beside_it() {
        let mut config = Config::default();
        config.hubs.insert("plaza".into(), saved("https://plaza.test", Some("owner-token")));
        attach_token(&mut config, "https://plaza.test", "apz_mcp_x");
        let entry = &config.hubs["plaza"];
        assert_eq!(entry.token.as_deref(), Some("owner-token"), "the owner token is untouched");
        assert_eq!(entry.mcp_token.as_deref(), Some("apz_mcp_x"));
    }

    #[test]
    fn login_makes_an_entry_named_for_an_unknown_hub() {
        let mut config = Config::default();
        attach_token(&mut config, "https://hub.example:7700", "apz_mcp_y");
        assert_eq!(config.hubs["hub.example"].mcp_token.as_deref(), Some("apz_mcp_y"));
        assert_eq!(config.default.as_deref(), Some("hub.example"));
    }

    #[test]
    fn the_serve_hub_is_the_named_or_default_saved_one() {
        let mut config = Config::default();
        config.hubs.insert("plaza".into(), SavedHub { mcp_token: Some("apz_mcp_z".into()), ..saved("https://plaza.test", None) });
        config.default = Some("plaza".into());
        assert_eq!(saved_hub(&config, None).unwrap().url, "https://plaza.test");
        assert_eq!(saved_hub(&config, Some("https://plaza.test/")).unwrap().mcp_token.as_deref(), Some("apz_mcp_z"));
        assert!(saved_hub(&config, Some("nowhere")).is_err());
    }

    #[test]
    fn switching_the_server_keeps_the_auth_mode_open_scope_and_origins() {
        let now = json!({
            "enabled": false, "bind": "local", "auth_mode": "open", "open_scope": "operate",
            "allowed_origins": ["https://console.test"],
        });
        let body = switch_body(&now, true, false);
        assert_eq!(body["auth_mode"], "open");
        assert_eq!(body["open_scope"], "operate");
        assert_eq!(body["allowed_origins"], json!(["https://console.test"]));
        assert_eq!(body["bind"], "local");
        assert_eq!(body["confirm_open_remote"], false, "a local open hub needs no confirmation");
    }

    #[test]
    fn switching_an_open_remote_hub_off_keeps_it_open_and_remote() {
        let now = json!({ "enabled": true, "bind": "remote", "auth_mode": "open", "open_scope": "observe", "allowed_origins": [] });
        let body = switch_body(&now, false, false);
        assert_eq!(body["enabled"], false);
        assert_eq!(body["bind"], "remote");
        assert_eq!(body["auth_mode"], "open");
        assert_eq!(body["confirm_open_remote"], true, "the open remote setting was confirmed when it was set");
    }

    #[test]
    fn enabling_remote_does_not_open_a_local_secure_hub_without_a_confirmation() {
        let now = json!({ "enabled": false, "bind": "local", "auth_mode": "secure", "open_scope": "observe", "allowed_origins": [] });
        let body = switch_body(&now, true, true);
        assert_eq!(body["bind"], "remote");
        assert_eq!(body["auth_mode"], "secure");
        assert_eq!(body["confirm_open_remote"], false);
    }

    #[test]
    fn a_hub_without_an_auth_mode_in_its_status_is_saved_secure() {
        let body = switch_body(&json!({ "bind": "local" }), true, false);
        assert_eq!(body["auth_mode"], "secure");
        assert_eq!(body["open_scope"], "observe");
        assert_eq!(body["allowed_origins"], json!([]));
    }

    #[test]
    fn expiry_is_days() {
        assert_eq!(expiry_days("30d"), Ok(30));
        assert_eq!(expiry_days("7"), Ok(7));
        assert!(expiry_days("0d").is_err());
        assert!(expiry_days("soon").is_err());
    }
}
