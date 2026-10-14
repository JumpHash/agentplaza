//! `agentplaza mcp servers`: the MCP servers the hub runs for agents (docs/design/mcp.md). The owner lists them,
//! reads the presets, adds, edits and deletes them, starts and stops them, tests them, and reads their tools and logs.
//! A secret is named, never given: `NAME=@vault:SECRET` binds a vault secret the hub already holds, so no secret value
//! goes on the command line. `NAME=@plain:TEXT` sets a plain, non-secret value.

use std::collections::HashSet;
use std::thread;
use std::time::Duration;

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use crate::ctx::{self, confirm, Ctx};

/// The fields a server's config keeps. An edit sends exactly these (the id cannot change).
const CONFIG_KEYS: [&str; 10] =
    ["name", "enabled", "transport", "env", "headers", "tools", "access", "limits", "allow_private", "storage"];

#[derive(Subcommand, Debug)]
pub enum ServersCmd {
    /// The MCP servers: their switch, transport, status and last error.
    List,
    /// The presets `add --preset` takes: each one's image or URL, and the settings it needs.
    Catalog,
    /// One server: its config, status, last error, server info and tool count.
    Show {
        /// The server's id.
        id: String,
    },
    /// Add a server: a preset (--preset) or your own container image or remote URL (--custom --transport).
    Add {
        /// The preset to add, from `mcp servers catalog`. Its id is the default id.
        #[arg(long, conflicts_with = "custom")]
        preset: Option<String>,
        /// A custom server: --transport container with --image, or --transport remote with --url.
        #[arg(long, conflicts_with = "preset")]
        custom: bool,
        /// The server's id: 2 to 32 characters, lower case, starting with a letter.
        #[arg(long)]
        id: Option<String>,
        /// The kind of custom server: container or remote.
        #[arg(long, value_parser = ["container", "remote"])]
        transport: Option<String>,
        #[command(flatten)]
        fields: Fields,
    },
    /// Change a server. Only the flags given change; a change that alters what runs restarts it on next use.
    Edit {
        /// The server's id.
        id: String,
        #[command(flatten)]
        fields: Fields,
    },
    /// Delete a server and its vault bindings. Asks to confirm unless --yes.
    Delete {
        /// The server's id.
        id: String,
        /// Also purge its storage volume (kept by default).
        #[arg(long)]
        purge: bool,
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Start a server now (it starts on first use otherwise).
    Start {
        /// The server's id.
        id: String,
    },
    /// Stop a server's running instances. Its config stays.
    Stop {
        /// The server's id.
        id: String,
    },
    /// Test a server: start it, list its tools and print its server info. A failure prints its stderr tail.
    Test {
        /// The server's id.
        id: String,
        /// The node to run it on (default: this hub).
        #[arg(long)]
        node: Option<String>,
    },
    /// The tools the server last listed, with their risk and whether the allow list offers them.
    Tools {
        /// The server's id.
        id: String,
    },
    /// The server's stderr and stdout lines, oldest first.
    Logs {
        /// The server's id.
        id: String,
        /// How many lines (1 to 500).
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=500))]
        limit: u64,
        /// Keep printing new lines as they come.
        #[arg(long)]
        follow: bool,
    },
}

/// The flags `add` and `edit` share. A flag not given leaves the field alone on edit, and takes the default on add.
#[derive(Args, Debug, Default)]
pub struct Fields {
    /// The display name, 1 to 80 characters (default: the preset's name, or the id).
    #[arg(long)]
    pub name: Option<String>,
    /// An env var: NAME=@vault:SECRET binds a vault secret, NAME=@plain:TEXT sets a plain value, NAME= removes it.
    #[arg(long = "env", value_name = "NAME=REF")]
    pub env: Vec<String>,
    /// An HTTP header, bound like --env. A secret header can add a format: NAME=@vault:SECRET|Bearer {}
    #[arg(long = "header", value_name = "NAME=REF")]
    pub header: Vec<String>,
    /// The roles that may use the server: role names, or all, or none (default: none, so nobody). Comma-separated.
    #[arg(long, value_name = "ROLES", value_delimiter = ',')]
    pub roles: Vec<String>,
    /// The nodes that may run it: node ids, or all, or none (default: none). Comma-separated.
    #[arg(long, value_name = "NODES", value_delimiter = ',')]
    pub nodes: Vec<String>,
    /// A tool the agents may call, repeatable; all or none. Default: all tools.
    #[arg(long = "allow-tool", value_name = "TOOL")]
    pub allow_tool: Vec<String>,
    /// A tool's risk, repeatable: TOOL=read, TOOL=write or TOOL=irreversible.
    #[arg(long, value_name = "TOOL=RISK")]
    pub risk: Vec<String>,
    /// Turn the server on.
    #[arg(long, conflicts_with = "disabled")]
    pub enabled: bool,
    /// Turn the server off. It stays configured.
    #[arg(long, conflicts_with = "enabled")]
    pub disabled: bool,
    /// The container image of a custom container server.
    #[arg(long)]
    pub image: Option<String>,
    /// An argument after the image, repeatable: the container's command.
    #[arg(long = "arg", value_name = "ARG", allow_hyphen_values = true)]
    pub arg: Vec<String>,
    /// The HTTPS endpoint of a custom remote server.
    #[arg(long)]
    pub url: Option<String>,
    /// Keep the server's storage in a docker volume at /data (container servers).
    #[arg(long, conflicts_with = "no_volume")]
    pub volume: bool,
    /// Drop the storage volume setting.
    #[arg(long)]
    pub no_volume: bool,
    /// Let the server reach private and loopback addresses.
    #[arg(long, conflicts_with = "no_allow_private")]
    pub allow_private: bool,
    /// Stop letting it reach private and loopback addresses.
    #[arg(long)]
    pub no_allow_private: bool,
    /// Seconds one call may take (1 to 600).
    #[arg(long, value_name = "SECONDS")]
    pub timeout_s: Option<u64>,
    /// Kilobytes of output a call may return (1 to 4096).
    #[arg(long, value_name = "KB")]
    pub max_output_kb: Option<u64>,
    /// Minutes idle before the server stops (1 to 1440).
    #[arg(long, value_name = "MINUTES")]
    pub idle_stop_min: Option<u64>,
}

pub fn run(api: &Ctx, sub: ServersCmd) -> Result<(), String> {
    match sub {
        ServersCmd::List => {
            let servers = api.client.get("/v1/mcp/servers")?;
            api.out(&servers, print_list);
            Ok(())
        }
        ServersCmd::Catalog => {
            let presets = api.client.get("/v1/mcp/catalog")?;
            api.out(&presets, print_catalog);
            Ok(())
        }
        ServersCmd::Show { id } => {
            let view = api.client.get(&path(&id))?;
            api.out(&view, print_view);
            Ok(())
        }
        ServersCmd::Add { preset, custom, id, transport, fields } => {
            let catalog = if preset.is_some() { api.client.get("/v1/mcp/catalog")? } else { Value::Null };
            let body = new_config(&catalog, preset.as_deref(), custom, id.as_deref(), transport.as_deref(), &fields)?;
            let view = api.client.post("/v1/mcp/servers", &body)?;
            api.out(&view, print_view);
            Ok(())
        }
        ServersCmd::Edit { id, fields } => {
            let current = api.client.get(&path(&id))?;
            let body = edit_body(&current, &fields)?;
            let view = api.client.patch(&path(&id), &body)?;
            api.out(&view, print_view);
            Ok(())
        }
        ServersCmd::Delete { id, purge, yes } => {
            let volume = if purge { " and purge its storage volume" } else { "" };
            if !confirm(yes, &format!("Delete MCP server {id}{volume}. Its vault bindings are removed."))? {
                println!("cancelled");
                return Ok(());
            }
            let query = if purge { "?purge=1" } else { "" };
            api.client.delete(&format!("{}{query}", path(&id)))?;
            api.out(&json!({ "deleted": id }), |_| println!("deleted MCP server {id}"));
            Ok(())
        }
        ServersCmd::Start { id } => {
            let view = api.client.post(&format!("{}/start", path(&id)), &json!({}))?;
            api.out(&view, print_view);
            Ok(())
        }
        ServersCmd::Stop { id } => {
            let view = api.client.post(&format!("{}/stop", path(&id)), &json!({}))?;
            api.out(&view, print_view);
            Ok(())
        }
        ServersCmd::Test { id, node } => {
            let body = match node {
                Some(node) => json!({ "node": node }),
                None => json!({}),
            };
            let result = api.client.post(&format!("{}/test", path(&id)), &body)?;
            let passed = result["ok"].as_bool() == Some(true);
            api.out(&result, print_test);
            if passed {
                Ok(())
            } else {
                Err(format!("MCP server {id} did not pass its test"))
            }
        }
        ServersCmd::Tools { id } => {
            let rows = api.client.get(&format!("{}/tools", path(&id)))?;
            api.out(&rows, print_tools);
            Ok(())
        }
        ServersCmd::Logs { id, limit, follow } => logs(api, &id, limit, follow),
    }
}

/// `/v1/mcp/servers/{id}`, the id as one path segment.
fn path(id: &str) -> String {
    format!("/v1/mcp/servers/{}", ctx::seg(id))
}

// ---- Bodies -------------------------------------------------------------------------------------------------------

/// A value on the command line: a vault secret by its name (with an optional header format), or a plain value.
#[derive(Debug, Clone, PartialEq)]
enum Reference {
    Vault { secret: String, format: Option<String> },
    Plain(String),
}

/// One `NAME=REF` flag: a setting to set, or (with an empty REF) to remove.
#[derive(Debug, Clone, PartialEq)]
enum Assign {
    Set(String, Reference),
    Unset(String),
}

/// Reads `NAME=@vault:SECRET`, `NAME=@vault:SECRET|FORMAT`, `NAME=@plain:TEXT` or `NAME=`. A raw value is refused:
/// secrets are named in the vault, never typed on the command line.
fn parse_assign(text: &str) -> Result<Assign, String> {
    let (name, reference) =
        text.split_once('=').filter(|(name, _)| !name.is_empty()).ok_or_else(|| format!("{text}: write NAME=@vault:SECRET or NAME=@plain:TEXT"))?;
    if reference.is_empty() {
        return Ok(Assign::Unset(name.to_string()));
    }
    if let Some(rest) = reference.strip_prefix("@vault:") {
        let (secret, format) = match rest.split_once('|') {
            Some((secret, format)) => (secret, Some(format.to_string())),
            None => (rest, None),
        };
        if secret.is_empty() {
            return Err(format!("{name}: @vault: names a vault secret"));
        }
        return Ok(Assign::Set(name.to_string(), Reference::Vault { secret: secret.to_string(), format }));
    }
    if let Some(plain) = reference.strip_prefix("@plain:") {
        return Ok(Assign::Set(name.to_string(), Reference::Plain(plain.to_string())));
    }
    Err(format!("{name}: a value is @vault:SECRET (a vault secret) or @plain:TEXT (a plain value); a raw secret is not taken on the command line"))
}

fn assigns(texts: &[String]) -> Result<Vec<Assign>, String> {
    texts.iter().map(|text| parse_assign(text)).collect()
}

/// The setting a reference is in the config. A header's format is its own, else the one given (a preset's, or the
/// one the header already has).
fn setting(reference: &Reference, header: bool, default_format: Option<&str>) -> Result<Value, String> {
    match reference {
        Reference::Plain(text) => Ok(json!({ "value": text })),
        Reference::Vault { secret, format } => {
            if format.is_some() && !header {
                return Err(format!("{secret}: a format goes with --header, not --env"));
            }
            match format.as_deref().or(default_format) {
                Some(format) => Ok(json!({ "secret": secret, "format": format })),
                None => Ok(json!({ "secret": secret })),
            }
        }
    }
}

/// The reference the last `NAME=REF` flag gives for `name`, if any.
fn reference_for<'a>(assigns: &'a [Assign], name: &str) -> Option<&'a Reference> {
    assigns.iter().rev().find_map(|assign| match assign {
        Assign::Set(set, reference) if set == name => Some(reference),
        _ => None,
    })
}

fn fixed_env(spec: &Value) -> Vec<(String, String)> {
    spec["fixed_env"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| Some((pair.get(0)?.as_str()?.to_string(), pair.get(1)?.as_str()?.to_string())))
        .collect()
}

/// The env or headers a preset's specs and the flags make. Each spec takes the flag that names it; a required spec
/// with no flag is refused unless the preset fixes its value. The preset's fixed settings are filled in unless a flag
/// names them, and flags that name no spec are kept too.
fn bound(specs: &Value, fixed: &[(String, String)], flags: &[Assign], header: bool) -> Result<Map<String, Value>, String> {
    let flag = if header { "--header" } else { "--env" };
    let mut out = Map::new();
    let spec_list: Vec<&Value> = specs.as_array().into_iter().flatten().collect();
    for spec in &spec_list {
        let name = ctx::s(spec, "name");
        let default_format = if header { spec["format"].as_str() } else { None };
        if let Some(reference) = reference_for(flags, name) {
            out.insert(name.to_string(), setting(reference, header, default_format)?);
        } else if spec["required"].as_bool() == Some(true) && !fixed.iter().any(|(key, _)| key == name) {
            let example = if spec["secret"].as_bool() == Some(true) { "@vault:SECRET" } else { "@plain:VALUE" };
            return Err(format!("{name} is required: pass {flag} {name}={example}"));
        }
    }
    for (key, value) in fixed {
        if reference_for(flags, key).is_none() {
            out.insert(key.clone(), json!({ "value": value }));
        }
    }
    for assign in flags {
        if let Assign::Set(name, reference) = assign {
            let named = spec_list.iter().any(|spec| ctx::s(spec, "name") == name);
            if !named {
                out.insert(name.clone(), setting(reference, header, None)?);
            }
        }
    }
    Ok(out)
}

fn find_preset<'a>(catalog: &'a Value, id: &str) -> Result<&'a Value, String> {
    catalog
        .as_array()
        .into_iter()
        .flatten()
        .find(|preset| ctx::s(preset, "id") == id)
        .ok_or_else(|| format!("no preset `{id}`: see `agentplaza mcp servers catalog`"))
}

fn preset_transport(preset: &Value) -> Result<Value, String> {
    let id = ctx::s(preset, "id");
    if ctx::s(preset, "transport") == "stdio" {
        let image = preset["image"]
            .as_str()
            .ok_or_else(|| format!("{id} has no official image: add it with --custom --transport container --image IMG"))?;
        let command = preset.get("command").filter(|c| c.is_array()).cloned().unwrap_or_else(|| json!([]));
        return Ok(json!({ "kind": "stdio", "image": image, "command": command }));
    }
    let url = preset["url"].as_str().ok_or_else(|| format!("{id} has no URL in the catalog"))?;
    Ok(json!({ "kind": "http", "url": url }))
}

fn custom_transport(kind: Option<&str>, fields: &Fields) -> Result<Value, String> {
    match kind {
        Some("container") => {
            if fields.url.is_some() {
                return Err("--url goes with --transport remote".to_string());
            }
            let image = fields.image.as_deref().ok_or("--image IMG is required for a container server")?;
            Ok(json!({ "kind": "stdio", "image": image, "command": fields.arg }))
        }
        Some("remote") => {
            if fields.image.is_some() || !fields.arg.is_empty() {
                return Err("--image and --arg go with --transport container".to_string());
            }
            let url = fields.url.as_deref().ok_or("--url URL is required for a remote server")?;
            Ok(json!({ "kind": "http", "url": url }))
        }
        _ => Err("--transport container or remote is required with --custom".to_string()),
    }
}

/// The POST body for `add`: a preset's config (its image or URL, with the settings the flags bind) or a custom one,
/// then the other flags applied. `catalog` is the preset list (only read for a preset).
pub fn new_config(
    catalog: &Value,
    preset: Option<&str>,
    custom: bool,
    id: Option<&str>,
    transport: Option<&str>,
    fields: &Fields,
) -> Result<Value, String> {
    let env_flags = assigns(&fields.env)?;
    let header_flags = assigns(&fields.header)?;
    let (id, name, transport, env, headers, volume) = match (preset, custom) {
        (Some(preset), false) => {
            if transport.is_some() {
                return Err("--transport goes with --custom: a preset fixes its transport".to_string());
            }
            if fields.image.is_some() || fields.url.is_some() || !fields.arg.is_empty() {
                return Err("--image, --url and --arg go with --custom: a preset fixes its image or URL".to_string());
            }
            let spec = find_preset(catalog, preset)?;
            let env = bound(&spec["env"], &fixed_env(spec), &env_flags, false)?;
            let headers = bound(&spec["headers"], &[], &header_flags, true)?;
            let name = ctx::s(spec, "name").to_string();
            let volume = spec["volume"].as_bool() == Some(true);
            (id.unwrap_or(preset).to_string(), name, preset_transport(spec)?, env, headers, volume)
        }
        (None, true) => {
            let id = id.ok_or("--id is required with --custom")?;
            let env = bound(&json!([]), &[], &env_flags, false)?;
            let headers = bound(&json!([]), &[], &header_flags, true)?;
            (id.to_string(), id.to_string(), custom_transport(transport, fields)?, env, headers, false)
        }
        (Some(_), true) => return Err("--preset and --custom are two ways to add a server: give one".to_string()),
        (None, false) => return Err("give --preset NAME for a preset, or --custom with --transport".to_string()),
    };
    let mut config = json!({
        "id": id,
        "name": name,
        "enabled": true,
        "transport": transport,
        "env": Value::Object(env),
        "headers": Value::Object(headers),
        "tools": { "allow": "all", "risk": {} },
        "access": { "roles": [], "nodes": [] },
        "limits": { "timeout_s": 60, "max_output_kb": 256, "idle_stop_min": 10 },
        "allow_private": false,
        "storage": { "volume": volume },
    });
    apply(&mut config, fields)?;
    Ok(config)
}

/// The PATCH body for `edit`: the server's current config with the flags applied. The id is not sent.
pub fn edit_body(current: &Value, fields: &Fields) -> Result<Value, String> {
    let mut config = Map::new();
    for key in CONFIG_KEYS {
        config.insert(key.to_string(), current.get(key).cloned().unwrap_or(Value::Null));
    }
    let mut config = Value::Object(config);
    apply(&mut config, fields)?;
    merge_settings(&mut config["env"], &fields.env, false)?;
    merge_settings(&mut config["headers"], &fields.header, true)?;
    Ok(config)
}

/// Sets the env or headers `flags` name on an existing map: a reference replaces its setting (a header keeps the
/// format it has when the flag names none), an empty one removes it.
fn merge_settings(map: &mut Value, flags: &[String], header: bool) -> Result<(), String> {
    for text in flags {
        match parse_assign(text)? {
            Assign::Unset(name) => {
                if let Some(settings) = map.as_object_mut() {
                    settings.remove(&name);
                }
            }
            Assign::Set(name, reference) => {
                let kept = map[name.as_str()]["format"].as_str().map(str::to_string);
                let value = setting(&reference, header, kept.as_deref())?;
                map.as_object_mut().ok_or("the settings are not an object")?.insert(name, value);
            }
        }
    }
    Ok(())
}

/// A pick from its words: `all` for every one, `none` for nobody, else the names. No words means the flag was not
/// given, so the field stays as it is.
fn pick_of(words: &[String]) -> Result<Option<Value>, String> {
    match words {
        [] => Ok(None),
        [word] if word == "all" => Ok(Some(json!("all"))),
        [word] if word == "none" => Ok(Some(json!([]))),
        _ if words.iter().any(|word| word == "all" || word == "none") => {
            Err("all and none stand alone: they do not go with names".to_string())
        }
        _ => Ok(Some(json!(words))),
    }
}

/// `TOOL=read`, `TOOL=write` or `TOOL=irreversible`.
fn risk_of(text: &str) -> Result<(String, &'static str), String> {
    let (tool, risk) = text
        .split_once('=')
        .filter(|(tool, _)| !tool.is_empty())
        .ok_or_else(|| format!("{text}: write TOOL=read, TOOL=write or TOOL=irreversible"))?;
    let risk = match risk {
        "read" => "read",
        "write" => "write",
        "irreversible" => "irreversible",
        _ => return Err(format!("{text}: the risk is read, write or irreversible")),
    };
    Ok((tool.to_string(), risk))
}

/// The transport the image, URL and arg flags give. An image makes a container server, a URL a remote one; an arg
/// alone sets the command of a container server.
fn apply_transport(config: &mut Value, fields: &Fields) -> Result<(), String> {
    if fields.image.is_some() && fields.url.is_some() {
        return Err("--image and --url are two kinds of server: give one".to_string());
    }
    if let Some(image) = &fields.image {
        let command = if fields.arg.is_empty() {
            config["transport"].get("command").cloned().unwrap_or_else(|| json!([]))
        } else {
            json!(fields.arg)
        };
        let mut transport = json!({ "kind": "stdio", "image": image, "command": command });
        if let Some(workdir) = config["transport"].get("workdir").filter(|w| !w.is_null()) {
            transport["workdir"] = workdir.clone();
        }
        config["transport"] = transport;
    } else if let Some(url) = &fields.url {
        config["transport"] = json!({ "kind": "http", "url": url });
    } else if !fields.arg.is_empty() {
        if config["transport"]["kind"] != "stdio" {
            return Err("--arg goes with a container server".to_string());
        }
        config["transport"]["command"] = json!(fields.arg);
    }
    Ok(())
}

/// Applies the flags that are given to a config. Each one replaces its field; env and headers are not in here.
fn apply(config: &mut Value, fields: &Fields) -> Result<(), String> {
    if let Some(name) = &fields.name {
        config["name"] = json!(name);
    }
    if fields.enabled {
        config["enabled"] = json!(true);
    } else if fields.disabled {
        config["enabled"] = json!(false);
    }
    apply_transport(config, fields)?;
    if let Some(roles) = pick_of(&fields.roles)? {
        config["access"]["roles"] = roles;
    }
    if let Some(nodes) = pick_of(&fields.nodes)? {
        config["access"]["nodes"] = nodes;
    }
    if let Some(allow) = pick_of(&fields.allow_tool)? {
        config["tools"]["allow"] = allow;
    }
    for text in &fields.risk {
        let (tool, risk) = risk_of(text)?;
        config["tools"]["risk"][tool.as_str()] = json!(risk);
    }
    let limits = [
        ("timeout_s", fields.timeout_s),
        ("max_output_kb", fields.max_output_kb),
        ("idle_stop_min", fields.idle_stop_min),
    ];
    for (key, value) in limits {
        if let Some(value) = value {
            config["limits"][key] = json!(value);
        }
    }
    if fields.volume || fields.no_volume {
        config["storage"]["volume"] = json!(fields.volume);
    }
    if fields.allow_private || fields.no_allow_private {
        config["allow_private"] = json!(fields.allow_private);
    }
    Ok(())
}

// ---- Output -------------------------------------------------------------------------------------------------------

fn yes_no(flag: Option<bool>) -> String {
    (if flag == Some(true) { "yes" } else { "no" }).to_string()
}

/// The transport as one line: `container · image` or `remote · url`.
fn transport_text(transport: &Value) -> String {
    match ctx::s(transport, "kind") {
        "stdio" => format!("container · {}", ctx::s(transport, "image")),
        _ => format!("remote · {}", ctx::s(transport, "url")),
    }
}

/// A pick as the owner reads it: `all`, the names, or the `empty` word.
fn pick_text(pick: &Value, empty: &str) -> String {
    match pick {
        Value::String(word) => word.clone(),
        Value::Array(names) if names.is_empty() => empty.to_string(),
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        _ => empty.to_string(),
    }
}

/// A setting as the owner typed it: `@vault:NAME`, `@vault:NAME|FORMAT` or `@plain:TEXT`. The value of a secret is
/// never shown: the hub does not hold it in the config.
fn setting_text(setting: &Value) -> String {
    match setting["secret"].as_str() {
        Some(secret) => match setting["format"].as_str() {
            Some(format) => format!("@vault:{secret}|{format}"),
            None => format!("@vault:{secret}"),
        },
        None => format!("@plain:{}", ctx::s(setting, "value")),
    }
}

fn bindings_text(settings: &Value) -> String {
    let parts: Vec<String> =
        settings.as_object().into_iter().flatten().map(|(name, setting)| format!("{name}={}", setting_text(setting))).collect();
    if parts.is_empty() { "(none)".to_string() } else { parts.join(", ") }
}

/// The first line of the last error, while the server is not running.
fn error_line(server: &Value) -> String {
    if ctx::s(server, "status") == "running" {
        return String::new();
    }
    server["last_error"].as_str().and_then(|error| error.lines().next()).unwrap_or_default().to_string()
}

fn print_list(servers: &Value) {
    let rows: Vec<Vec<String>> = servers
        .as_array()
        .into_iter()
        .flatten()
        .map(|server| {
            vec![
                ctx::s(server, "id").to_string(),
                ctx::s(server, "name").to_string(),
                transport_text(&server["transport"]),
                if server["enabled"].as_bool() == Some(true) { "on" } else { "off" }.to_string(),
                ctx::s(server, "status").to_string(),
                ctx::text(server, "tools_count"),
                error_line(server),
            ]
        })
        .collect();
    ctx::table(&["id", "name", "transport", "switch", "status", "tools", "last error"], &rows);
}

/// What a preset needs: its required settings, a secret marked `(vault)`.
fn needs_text(preset: &Value) -> String {
    let mut needs = Vec::new();
    for key in ["env", "headers"] {
        for spec in preset[key].as_array().into_iter().flatten() {
            if spec["required"].as_bool() == Some(true) {
                let vault = if spec["secret"].as_bool() == Some(true) { " (vault)" } else { "" };
                needs.push(format!("{}{vault}", ctx::s(spec, "name")));
            }
        }
    }
    if needs.is_empty() { "nothing".to_string() } else { needs.join(", ") }
}

fn print_catalog(presets: &Value) {
    let rows: Vec<Vec<String>> = presets
        .as_array()
        .into_iter()
        .flatten()
        .map(|preset| {
            let target = if ctx::s(preset, "transport") == "stdio" {
                preset["image"].as_str().unwrap_or("(no image: use --custom)").to_string()
            } else {
                ctx::s(preset, "url").to_string()
            };
            vec![
                ctx::s(preset, "id").to_string(),
                ctx::s(preset, "name").to_string(),
                if ctx::s(preset, "transport") == "stdio" { "container" } else { "remote" }.to_string(),
                target,
                needs_text(preset),
                ctx::s(preset, "status").to_string(),
            ]
        })
        .collect();
    ctx::table(&["id", "name", "transport", "target", "needs", "status"], &rows);
}

fn print_view(view: &Value) {
    let last_used = view["last_used_ms"].as_u64().unwrap_or(0);
    let server = match view["server_info"].as_object() {
        Some(info) => format!(
            "{} {} ({})",
            info.get("name").and_then(Value::as_str).unwrap_or(""),
            info.get("version").and_then(Value::as_str).unwrap_or(""),
            info.get("protocol").and_then(Value::as_str).unwrap_or("")
        ),
        None => "(not started yet)".to_string(),
    };
    let limits = &view["limits"];
    ctx::fields(&[
        ("id", ctx::s(view, "id").to_string()),
        ("name", ctx::s(view, "name").to_string()),
        ("switch", if view["enabled"].as_bool() == Some(true) { "on" } else { "off" }.to_string()),
        ("status", ctx::s(view, "status").to_string()),
        ("transport", transport_text(&view["transport"])),
        ("tools", ctx::text(view, "tools_count")),
        ("server", server),
        ("last error", error_line_or_dash(view)),
        ("last used", if last_used == 0 { "never".to_string() } else { ctx::ago(&json!(last_used)) }),
        ("roles", pick_text(&view["access"]["roles"], "nobody")),
        ("nodes", pick_text(&view["access"]["nodes"], "nobody")),
        ("tool allow", pick_text(&view["tools"]["allow"], "none")),
        ("env", bindings_text(&view["env"])),
        ("headers", bindings_text(&view["headers"])),
        ("storage", yes_no(view["storage"]["volume"].as_bool())),
        ("private", yes_no(view["allow_private"].as_bool())),
        (
            "limits",
            format!(
                "{}s a call, {} KB of output, stops after {} min idle",
                ctx::text(limits, "timeout_s"),
                ctx::text(limits, "max_output_kb"),
                ctx::text(limits, "idle_stop_min")
            ),
        ),
    ]);
}

fn error_line_or_dash(view: &Value) -> String {
    let line = error_line(view);
    if line.is_empty() { "-".to_string() } else { line }
}

/// The tool rows: the name the server gives, the name agents see, the risk, and whether the allow list offers it.
fn tool_cells(tool: &Value) -> Vec<String> {
    vec![
        ctx::s(tool, "name").to_string(),
        ctx::s(tool, "agent_name").to_string(),
        ctx::s(tool, "risk").to_string(),
        yes_no(tool["allowed"].as_bool()),
    ]
}

fn print_tools(tools: &Value) {
    let rows: Vec<Vec<String>> = tools.as_array().into_iter().flatten().map(tool_cells).collect();
    ctx::table(&["tool", "agent name", "risk", "allowed"], &rows);
}

fn print_test(result: &Value) {
    let ms = ctx::text(result, "ms");
    if result["ok"].as_bool() != Some(true) {
        println!("failed in {ms} ms: {}", ctx::s(result, "error"));
        for line in result["stderr"].as_array().into_iter().flatten() {
            println!("stderr: {}", ctx::s(line, "line"));
        }
        return;
    }
    let info = &result["server_info"];
    println!("ok in {ms} ms: {} {} ({})", ctx::s(info, "name"), ctx::s(info, "version"), ctx::s(info, "protocol"));
    let tools: Vec<Vec<String>> = result["tools"].as_array().into_iter().flatten().map(tool_cells).collect();
    println!("{} tools:", tools.len());
    ctx::table(&["tool", "agent name", "risk", "allowed"], &tools);
}

/// One log line: when it was written, its stream, its text.
fn log_text(line: &Value) -> String {
    format!("{} {:<6} {}", ctx::when(&line["ts_ms"]), ctx::s(line, "stream"), ctx::s(line, "line"))
}

fn logs(api: &Ctx, id: &str, limit: u64, follow: bool) -> Result<(), String> {
    let url = format!("{}/logs?limit={limit}", path(id));
    let mut window = api.client.get(&url)?;
    if !follow {
        api.out(&window, |lines| {
            for line in lines.as_array().into_iter().flatten() {
                println!("{}", log_text(line));
            }
        });
        return Ok(());
    }
    // The window is the last `limit` lines; each poll prints the lines the previous window did not hold.
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        let mut next_seen = HashSet::new();
        for line in window.as_array().into_iter().flatten() {
            let key = format!("{}|{}|{}", ctx::text(line, "ts_ms"), ctx::s(line, "stream"), ctx::s(line, "line"));
            if !seen.contains(&key) {
                if api.json {
                    println!("{line}");
                } else {
                    println!("{}", log_text(line));
                }
            }
            next_seen.insert(key);
        }
        seen = next_seen;
        thread::sleep(Duration::from_secs(2));
        window = api.client.get(&url)?;
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// The server subcommands alone, as the command line parses them (the global flags live in main).
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        cmd: ServersCmd,
    }

    fn parse(args: &[&str]) -> Result<ServersCmd, clap::Error> {
        let mut full = vec!["agentplaza"];
        full.extend_from_slice(args);
        TestCli::try_parse_from(full).map(|cli| cli.cmd)
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// A catalogue shaped like the hub's `GET /v1/mcp/catalog`: one stdio preset with a required secret and a fixed
    /// setting, one remote preset whose secret header has a format, and one with no image.
    fn catalog() -> Value {
        json!([
            {
                "id": "github", "name": "GitHub", "transport": "stdio",
                "image": "ghcr.io/github/github-mcp-server", "command": [], "url": null,
                "env": [
                    { "name": "GITHUB_PERSONAL_ACCESS_TOKEN", "secret": true, "required": true, "help": "", "format": null },
                    { "name": "GITHUB_HOST", "secret": false, "required": false, "help": "", "format": null }
                ],
                "headers": [], "fixed_env": [["GITHUB_TOOLSETS", "default"]], "volume": false,
                "status": "verified", "note": ""
            },
            {
                "id": "linear", "name": "Linear", "transport": "http",
                "image": null, "command": [], "url": "https://mcp.linear.app/mcp",
                "env": [], "headers": [
                    { "name": "Authorization", "secret": true, "required": true, "help": "", "format": "Bearer {}" }
                ],
                "fixed_env": [], "volume": false, "status": "verified", "note": ""
            },
            {
                "id": "memory", "name": "Memory", "transport": "stdio",
                "image": "ghcr.io/example/memory", "command": ["serve"], "url": null,
                "env": [], "headers": [], "fixed_env": [], "volume": true, "status": "verified", "note": ""
            },
            {
                "id": "custom-only", "name": "Custom only", "transport": "stdio",
                "image": null, "command": [], "url": null,
                "env": [], "headers": [], "fixed_env": [], "volume": false, "status": "custom image needed", "note": ""
            }
        ])
    }

    fn fields(f: impl FnOnce(&mut Fields)) -> Fields {
        let mut out = Fields::default();
        f(&mut out);
        out
    }

    #[test]
    fn a_binding_names_a_vault_secret_or_a_plain_value_and_never_a_raw_one() {
        assert_eq!(
            parse_assign("GITHUB_PERSONAL_ACCESS_TOKEN=@vault:GH_TOKEN").unwrap(),
            Assign::Set("GITHUB_PERSONAL_ACCESS_TOKEN".into(), Reference::Vault { secret: "GH_TOKEN".into(), format: None })
        );
        assert_eq!(
            parse_assign("Authorization=@vault:LINEAR_KEY|Bearer {}").unwrap(),
            Assign::Set(
                "Authorization".into(),
                Reference::Vault { secret: "LINEAR_KEY".into(), format: Some("Bearer {}".into()) }
            )
        );
        assert_eq!(parse_assign("GITHUB_HOST=@plain:https://ghe.test").unwrap(), Assign::Set("GITHUB_HOST".into(), Reference::Plain("https://ghe.test".into())));
        assert_eq!(parse_assign("GITHUB_HOST=").unwrap(), Assign::Unset("GITHUB_HOST".into()));
        assert!(parse_assign("GITHUB_PERSONAL_ACCESS_TOKEN=ghp_rawsecret").is_err(), "a raw value is refused");
        assert!(parse_assign("NO_EQUALS").is_err());
        assert!(parse_assign("=@vault:X").is_err(), "a binding needs a name");
        assert!(parse_assign("X=@vault:").is_err(), "@vault: needs a secret name");
    }

    #[test]
    fn a_format_goes_with_a_header_only() {
        let env = parse_assign("TOKEN=@vault:GH|Bearer {}").unwrap();
        let Assign::Set(name, reference) = env else { panic!("a set") };
        assert_eq!(name, "TOKEN");
        assert!(setting(&reference, false, None).is_err(), "an env var takes no format");
        assert!(setting(&reference, true, None).is_ok());
    }

    #[test]
    fn a_preset_add_binds_its_secret_and_keeps_its_fixed_settings() {
        let body = new_config(
            &catalog(),
            Some("github"),
            false,
            None,
            None,
            &fields(|f| f.env = strings(&["GITHUB_PERSONAL_ACCESS_TOKEN=@vault:GH_TOKEN"])),
        )
        .unwrap();
        assert_eq!(body["id"], "github", "the preset id is the default id");
        assert_eq!(body["name"], "GitHub");
        assert_eq!(body["enabled"], true);
        assert_eq!(body["transport"], json!({ "kind": "stdio", "image": "ghcr.io/github/github-mcp-server", "command": [] }));
        assert_eq!(body["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"], json!({ "secret": "GH_TOKEN" }));
        assert_eq!(body["env"]["GITHUB_TOOLSETS"], json!({ "value": "default" }), "a fixed setting is filled in");
        assert!(body["env"].get("GITHUB_HOST").is_none(), "an optional setting nobody gave is left out");
        assert_eq!(body["tools"]["allow"], "all");
        assert_eq!(body["access"], json!({ "roles": [], "nodes": [] }), "nobody may use it until the owner says so");
        assert_eq!(body["limits"], json!({ "timeout_s": 60, "max_output_kb": 256, "idle_stop_min": 10 }));
    }

    #[test]
    fn a_preset_refuses_a_missing_required_secret_and_names_the_flag() {
        let err = new_config(&catalog(), Some("github"), false, None, None, &Fields::default()).unwrap_err();
        assert!(err.contains("GITHUB_PERSONAL_ACCESS_TOKEN is required"), "{err}");
        assert!(err.contains("--env GITHUB_PERSONAL_ACCESS_TOKEN=@vault:SECRET"), "{err}");
    }

    #[test]
    fn a_preset_header_keeps_its_format_unless_the_flag_names_one() {
        let body = new_config(
            &catalog(),
            Some("linear"),
            false,
            Some("lin"),
            None,
            &fields(|f| f.header = strings(&["Authorization=@vault:LINEAR_KEY"])),
        )
        .unwrap();
        assert_eq!(body["id"], "lin");
        assert_eq!(body["headers"]["Authorization"], json!({ "secret": "LINEAR_KEY", "format": "Bearer {}" }));
        assert_eq!(body["transport"], json!({ "kind": "http", "url": "https://mcp.linear.app/mcp" }));
    }

    #[test]
    fn a_preset_without_an_image_says_to_go_custom() {
        let err = new_config(&catalog(), Some("custom-only"), false, None, None, &Fields::default()).unwrap_err();
        assert!(err.contains("no official image") && err.contains("--custom"), "{err}");
        assert!(new_config(&catalog(), Some("nope"), false, None, None, &Fields::default()).is_err());
    }

    #[test]
    fn a_preset_takes_the_volume_it_has_and_the_flags_set_the_rest() {
        let body = new_config(
            &catalog(),
            Some("memory"),
            false,
            None,
            None,
            &fields(|f| {
                f.disabled = true;
                f.roles = strings(&["builder", "reviewer"]);
                f.allow_tool = strings(&["remember", "recall"]);
                f.risk = strings(&["remember=write"]);
                f.timeout_s = Some(30);
            }),
        )
        .unwrap();
        assert_eq!(body["storage"]["volume"], true);
        assert_eq!(body["enabled"], false, "--disabled adds it off");
        assert_eq!(body["access"]["roles"], json!(["builder", "reviewer"]));
        assert_eq!(body["tools"]["allow"], json!(["remember", "recall"]));
        assert_eq!(body["tools"]["risk"], json!({ "remember": "write" }));
        assert_eq!(body["limits"]["timeout_s"], 30);
        assert_eq!(body["transport"]["command"], json!(["serve"]));
    }

    #[test]
    fn a_preset_does_not_take_a_transport_or_an_image_flag() {
        let err = new_config(&catalog(), Some("github"), false, None, None, &fields(|f| f.image = Some("x".into()))).unwrap_err();
        assert!(err.contains("--custom"), "{err}");
        assert!(new_config(&catalog(), Some("github"), false, None, Some("container"), &Fields::default()).is_err());
        assert!(new_config(&catalog(), None, false, None, None, &Fields::default()).is_err(), "a server is a preset or custom");
    }

    #[test]
    fn a_custom_container_takes_its_image_and_command() {
        let body = new_config(
            &json!([]),
            None,
            true,
            Some("my-server"),
            Some("container"),
            &fields(|f| {
                f.image = Some("ghcr.io/me/server:1.0".into());
                f.arg = strings(&["serve", "--stdio"]);
                f.env = strings(&["TOKEN=@vault:MY_TOKEN"]);
            }),
        )
        .unwrap();
        assert_eq!(body["id"], "my-server");
        assert_eq!(body["name"], "my-server", "the name defaults to the id");
        assert_eq!(body["transport"], json!({ "kind": "stdio", "image": "ghcr.io/me/server:1.0", "command": ["serve", "--stdio"] }));
        assert_eq!(body["env"], json!({ "TOKEN": { "secret": "MY_TOKEN" } }));
        assert_eq!(body["storage"]["volume"], false);
    }

    #[test]
    fn a_custom_remote_takes_its_url_and_refuses_container_flags() {
        let body = new_config(
            &json!([]),
            None,
            true,
            Some("docs"),
            Some("remote"),
            &fields(|f| {
                f.url = Some("https://docs.test/mcp".into());
                f.header = strings(&["Authorization=@vault:DOCS_KEY|Bearer {}"]);
                f.name = Some("Docs".into());
            }),
        )
        .unwrap();
        assert_eq!(body["transport"], json!({ "kind": "http", "url": "https://docs.test/mcp" }));
        assert_eq!(body["headers"]["Authorization"], json!({ "secret": "DOCS_KEY", "format": "Bearer {}" }));
        assert_eq!(body["name"], "Docs");
        let err = new_config(&json!([]), None, true, Some("docs"), Some("remote"), &fields(|f| f.image = Some("x".into()))).unwrap_err();
        assert!(err.contains("--transport container"), "{err}");
    }

    #[test]
    fn a_custom_server_names_its_id_and_its_kind() {
        assert!(new_config(&json!([]), None, true, None, Some("container"), &fields(|f| f.image = Some("x".into()))).is_err());
        assert!(new_config(&json!([]), None, true, Some("svc"), None, &fields(|f| f.image = Some("x".into()))).is_err());
        let no_image = new_config(&json!([]), None, true, Some("svc"), Some("container"), &Fields::default()).unwrap_err();
        assert!(no_image.contains("--image"), "{no_image}");
        let no_url = new_config(&json!([]), None, true, Some("svc"), Some("remote"), &Fields::default()).unwrap_err();
        assert!(no_url.contains("--url"), "{no_url}");
    }

    fn current() -> Value {
        json!({
            "id": "github", "name": "GitHub", "enabled": true,
            "transport": { "kind": "stdio", "image": "ghcr.io/github/github-mcp-server", "command": [] },
            "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": { "secret": "GH_TOKEN" }, "GITHUB_HOST": { "value": "https://old.test" } },
            "headers": {},
            "tools": { "allow": ["list_issues"], "risk": { "list_issues": "read" } },
            "access": { "roles": "all", "nodes": ["n0"] },
            "limits": { "timeout_s": 60, "max_output_kb": 256, "idle_stop_min": 10 },
            "allow_private": false, "storage": { "volume": true },
            "status": "running", "tools_count": 3, "last_error": null, "last_used_ms": 0, "server_info": null,
            "created_ms": 1, "updated_ms": 2
        })
    }

    #[test]
    fn an_edit_changes_only_what_is_given() {
        let body = edit_body(
            &current(),
            &fields(|f| {
                f.disabled = true;
                f.env = strings(&["GITHUB_PERSONAL_ACCESS_TOKEN=@vault:GH_TOKEN_2", "GITHUB_HOST="]);
                f.risk = strings(&["create_issue=write"]);
                f.idle_stop_min = Some(5);
            }),
        )
        .unwrap();
        assert!(body.get("id").is_none(), "the id is not sent");
        assert!(body.get("status").is_none() && body.get("tools_count").is_none(), "only config is sent");
        assert_eq!(body["enabled"], false);
        assert_eq!(body["env"], json!({ "GITHUB_PERSONAL_ACCESS_TOKEN": { "secret": "GH_TOKEN_2" } }), "one rebound, one removed");
        assert_eq!(body["tools"]["allow"], json!(["list_issues"]), "an unflagged pick stays");
        assert_eq!(body["tools"]["risk"], json!({ "list_issues": "read", "create_issue": "write" }), "risks merge");
        assert_eq!(body["access"], json!({ "roles": "all", "nodes": ["n0"] }));
        assert_eq!(body["storage"]["volume"], true, "the volume setting is kept");
        assert_eq!(body["limits"]["idle_stop_min"], 5);
        assert_eq!(body["limits"]["timeout_s"], 60);
    }

    #[test]
    fn an_edit_keeps_a_header_format_and_can_clear_a_pick() {
        let mut current = current();
        current["headers"] = json!({ "Authorization": { "secret": "OLD", "format": "Bearer {}" } });
        let body = edit_body(
            &current,
            &fields(|f| {
                f.header = strings(&["Authorization=@vault:NEW"]);
                f.roles = strings(&["none"]);
                f.allow_tool = strings(&["all"]);
                f.name = Some("Renamed".into());
            }),
        )
        .unwrap();
        assert_eq!(body["headers"]["Authorization"], json!({ "secret": "NEW", "format": "Bearer {}" }));
        assert_eq!(body["access"]["roles"], json!([]));
        assert_eq!(body["tools"]["allow"], "all");
        assert_eq!(body["name"], "Renamed");
    }

    #[test]
    fn an_edit_switches_a_container_to_a_remote_url_and_keeps_its_settings() {
        let body = edit_body(&current(), &fields(|f| f.url = Some("https://mcp.test/mcp".into()))).unwrap();
        assert_eq!(body["transport"], json!({ "kind": "http", "url": "https://mcp.test/mcp" }));
        let container = edit_body(&current(), &fields(|f| f.arg = strings(&["x"])));
        assert_eq!(container.unwrap()["transport"]["command"], json!(["x"]), "an arg sets the command of a container");
        let remote = edit_body(&json!({ "transport": { "kind": "http", "url": "https://a.test/mcp" } }), &fields(|f| f.arg = strings(&["x"])));
        assert!(remote.unwrap_err().contains("container"));
    }

    #[test]
    fn picks_are_all_none_or_names() {
        assert_eq!(pick_of(&[]).unwrap(), None);
        assert_eq!(pick_of(&strings(&["all"])).unwrap(), Some(json!("all")));
        assert_eq!(pick_of(&strings(&["none"])).unwrap(), Some(json!([])));
        assert_eq!(pick_of(&strings(&["a", "b"])).unwrap(), Some(json!(["a", "b"])));
        assert!(pick_of(&strings(&["all", "a"])).is_err());
        assert_eq!(pick_text(&json!("all"), "nobody"), "all");
        assert_eq!(pick_text(&json!([]), "nobody"), "nobody");
        assert_eq!(pick_text(&json!(["a", "b"]), "nobody"), "a, b");
    }

    #[test]
    fn a_risk_names_a_tool_and_one_of_three_levels() {
        assert_eq!(risk_of("create_issue=write").unwrap(), ("create_issue".to_string(), "write"));
        assert_eq!(risk_of("drop=irreversible").unwrap().1, "irreversible");
        assert!(risk_of("drop=dangerous").is_err());
        assert!(risk_of("=read").is_err());
        assert!(risk_of("drop").is_err());
    }

    #[test]
    fn the_settings_and_the_transport_read_back_for_the_owner() {
        assert_eq!(setting_text(&json!({ "secret": "GH", "format": "Bearer {}" })), "@vault:GH|Bearer {}");
        assert_eq!(setting_text(&json!({ "value": "https://x" })), "@plain:https://x");
        assert_eq!(bindings_text(&json!({})), "(none)");
        assert_eq!(transport_text(&json!({ "kind": "http", "url": "https://x/mcp" })), "remote · https://x/mcp");
        assert_eq!(needs_text(&catalog()[0]), "GITHUB_PERSONAL_ACCESS_TOKEN (vault)");
        assert_eq!(needs_text(&catalog()[2]), "nothing");
    }

    #[test]
    fn the_flags_parse_as_the_command_line_gives_them() {
        assert!(matches!(parse(&["list"]), Ok(ServersCmd::List)));
        assert!(matches!(parse(&["catalog"]), Ok(ServersCmd::Catalog)));
        assert!(matches!(parse(&["show", "github"]), Ok(ServersCmd::Show { id }) if id == "github"));
        let ServersCmd::Add { preset, custom, id, transport, fields } = parse(&[
            "add", "--preset", "github", "--id", "gh", "--name", "Work GitHub", "--env", "GITHUB_PERSONAL_ACCESS_TOKEN=@vault:GH",
            "--allow-tool", "list_issues", "--allow-tool", "get_me", "--roles", "builder,reviewer", "--disabled",
        ])
        .unwrap() else {
            panic!("an add")
        };
        assert_eq!(preset.as_deref(), Some("github"));
        assert!(!custom && transport.is_none());
        assert_eq!(id.as_deref(), Some("gh"));
        assert_eq!(fields.name.as_deref(), Some("Work GitHub"));
        assert_eq!(fields.allow_tool, strings(&["list_issues", "get_me"]));
        assert_eq!(fields.roles, strings(&["builder", "reviewer"]));
        assert!(fields.disabled && !fields.enabled);
        assert!(parse(&["add", "--preset", "github", "--custom"]).is_err(), "a preset and a custom server are two");
        assert!(parse(&["add", "--custom", "--transport", "lambda", "--id", "x"]).is_err());
        assert!(parse(&["add", "--custom", "--transport", "remote", "--id", "docs", "--url", "https://d.test/mcp"]).is_ok());
        assert!(parse(&["add", "--preset", "github", "--enabled", "--disabled"]).is_err());
    }

    #[test]
    fn delete_edit_and_the_other_verbs_parse() {
        assert!(matches!(parse(&["delete", "gh", "--purge", "--yes"]), Ok(ServersCmd::Delete { purge: true, yes: true, .. })));
        assert!(matches!(parse(&["edit", "gh", "--no-volume", "--timeout-s", "90"]), Ok(ServersCmd::Edit { .. })));
        assert!(matches!(parse(&["start", "gh"]), Ok(ServersCmd::Start { .. })));
        assert!(matches!(parse(&["stop", "gh"]), Ok(ServersCmd::Stop { .. })));
        assert!(matches!(parse(&["test", "gh", "--node", "n2"]), Ok(ServersCmd::Test { node: Some(n), .. }) if n == "n2"));
        assert!(matches!(parse(&["tools", "gh"]), Ok(ServersCmd::Tools { .. })));
        assert!(matches!(parse(&["logs", "gh", "--follow", "--limit", "500"]), Ok(ServersCmd::Logs { limit: 500, follow: true, .. })));
        assert!(parse(&["logs", "gh", "--limit", "0"]).is_err(), "the limit is 1 to 500");
        assert!(parse(&["logs", "gh", "--limit", "501"]).is_err());
        assert!(parse(&["edit", "gh", "--volume", "--no-volume"]).is_err());
    }
}
