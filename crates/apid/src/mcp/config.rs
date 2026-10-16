//! MCP server configuration: its shape, its validation and its store.
//!
//! The store is the Postgres component `mcp` (`db::COMPONENTS`), with `<state-dir>/mcp.json` as the
//! file copy. A config never holds a secret value: a `{secret}` names a vault entry, a `{value}` is a
//! plain setting the owner typed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use super::client::ToolInfo;

/// The file the store keeps beside the other state files.
pub const FILE: &str = "mcp.json";
/// Most servers one hub holds.
pub const MAX_SERVERS: usize = 64;

/// One MCP server the owner configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub transport: Transport,
    /// Stdio only: env var name to its setting.
    #[serde(default)]
    pub env: BTreeMap<String, Setting>,
    /// Http only: header name to its setting.
    #[serde(default)]
    pub headers: BTreeMap<String, Setting>,
    #[serde(default)]
    pub tools: ToolPolicy,
    #[serde(default)]
    pub access: Access,
    #[serde(default)]
    pub limits: Limits,
    /// Lets the server reach private and loopback addresses (the egress rules refuse them otherwise).
    #[serde(default)]
    pub allow_private: bool,
    /// Persistent storage for a stdio server (a named docker volume; never a host path).
    #[serde(default)]
    pub storage: Storage,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
}

fn yes() -> bool {
    true
}

/// What a stdio server keeps between runs. `volume` mounts the docker volume `apz-mcp-<id>` at `/data`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    #[serde(default)]
    pub volume: bool,
}

/// The docker volume a server's storage is kept in.
pub fn volume_name(id: &str) -> String {
    format!("apz-mcp-{id}")
}

/// Env vars the hub sets itself on every stdio container (the egress proxy); a config may not name them.
pub const RESERVED_ENV: [&str; 5] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "NODE_USE_ENV_PROXY",
    "NODE_OPTIONS",
];

/// Where a server runs: a container (stdio) or a remote endpoint (http).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Transport {
    Stdio {
        image: String,
        #[serde(default)]
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
    Http {
        url: String,
    },
}

/// An env var or header value: a vault secret, or a plain literal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Setting {
    /// `{secret: "GITHUB_TOKEN", format: "Bearer {}"}`; `format` is for headers only.
    Secret {
        secret: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// `{value: "non-secret literal"}`.
    Value { value: String },
}

/// Which tools are offered, and the risk class of each.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolPolicy {
    #[serde(default)]
    pub allow: Pick,
    /// Tool name to its risk; a tool without an entry takes its annotations' risk.
    #[serde(default)]
    pub risk: BTreeMap<String, Risk>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        ToolPolicy {
            allow: Pick::All,
            risk: BTreeMap::new(),
        }
    }
}

/// Who may use the server. Both lists default to nobody.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Access {
    #[serde(default)]
    pub roles: Pick,
    #[serde(default)]
    pub nodes: Pick,
}

/// `"all"` or a list of names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PickRepr", into = "PickRepr")]
pub enum Pick {
    All,
    Only(Vec<String>),
}

impl Default for Pick {
    fn default() -> Self {
        Pick::Only(Vec::new())
    }
}

impl Pick {
    /// Whether `name` is picked.
    pub fn admits(&self, name: &str) -> bool {
        match self {
            Pick::All => true,
            Pick::Only(names) => names.iter().any(|n| n == name),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum PickRepr {
    Word(String),
    List(Vec<String>),
}

impl TryFrom<PickRepr> for Pick {
    type Error = String;
    fn try_from(repr: PickRepr) -> Result<Self, String> {
        match repr {
            PickRepr::Word(word) if word == "all" => Ok(Pick::All),
            PickRepr::Word(word) => Err(format!("`{word}` is not \"all\" or a list")),
            PickRepr::List(names) => Ok(Pick::Only(names)),
        }
    }
}

impl From<Pick> for PickRepr {
    fn from(pick: Pick) -> Self {
        match pick {
            Pick::All => PickRepr::Word("all".into()),
            Pick::Only(names) => PickRepr::List(names),
        }
    }
}

/// The risk classes the laws read (`mcp.call`, C-004a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Read,
    Write,
    Irreversible,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Write => "write",
            Risk::Irreversible => "irreversible",
        }
    }
}

/// Time and size bounds for one server.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default = "timeout_s")]
    pub timeout_s: u64,
    #[serde(default = "max_output_kb")]
    pub max_output_kb: u64,
    #[serde(default = "idle_stop_min")]
    pub idle_stop_min: u64,
}

fn timeout_s() -> u64 {
    60
}
fn max_output_kb() -> u64 {
    256
}
fn idle_stop_min() -> u64 {
    10
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            timeout_s: timeout_s(),
            max_output_kb: max_output_kb(),
            idle_stop_min: idle_stop_min(),
        }
    }
}

/// A refused config: the field that is wrong and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub field: String,
    pub message: String,
}

impl Invalid {
    fn new(field: &str, message: impl Into<String>) -> Self {
        Invalid {
            field: field.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// Whether `id` is a valid server id: `[a-z][a-z0-9-]{1,31}`.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn env_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_uppercase() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
}

fn secret_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=64).contains(&bytes.len())
        && bytes[0].is_ascii_uppercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
}

fn header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn plain(text: &str) -> bool {
    text.len() <= 4096 && !text.contains(['\n', '\r', '\0'])
}

fn role_or_node(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Checks every field of a server config. The error names the field that is wrong.
pub fn validate(server: &McpServer) -> Result<(), Invalid> {
    if !valid_id(&server.id) {
        return Err(Invalid::new(
            "id",
            "use 2 to 32 characters: a lowercase letter, then lowercase letters, digits or '-'",
        ));
    }
    let name_len = server.name.trim().chars().count();
    if name_len == 0 || name_len > 80 || server.name.chars().any(char::is_control) {
        return Err(Invalid::new("name", "use 1 to 80 printable characters"));
    }
    match &server.transport {
        Transport::Stdio {
            image,
            command,
            workdir,
        } => {
            let ok = !image.is_empty()
                && image.len() <= 200
                && !image.starts_with('-')
                && image
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._/:@-".contains(&b));
            if !ok {
                return Err(Invalid::new(
                    "transport.image",
                    "a container image name such as ghcr.io/org/server:1.0",
                ));
            }
            if command.len() > 32
                || command
                    .iter()
                    .any(|arg| arg.len() > 512 || arg.contains('\0'))
            {
                return Err(Invalid::new(
                    "transport.command",
                    "at most 32 arguments of 512 bytes each",
                ));
            }
            if let Some(dir) = workdir
                && (!dir.starts_with('/') || dir.len() > 256 || dir.contains('\0'))
            {
                return Err(Invalid::new(
                    "transport.workdir",
                    "an absolute path inside the container",
                ));
            }
            for (name, setting) in &server.env {
                if !env_name(name) || RESERVED_ENV.contains(&name.as_str()) {
                    return Err(Invalid::new(
                        "env",
                        format!("`{name}` is not an env var a server may set"),
                    ));
                }
                check_setting(&format!("env.{name}"), setting, false)?;
            }
            if !server.headers.is_empty() {
                return Err(Invalid::new(
                    "headers",
                    "headers apply to http servers only",
                ));
            }
        }
        Transport::Http { url } => {
            if server.storage.volume {
                return Err(Invalid::new(
                    "storage",
                    "a volume applies to stdio servers only",
                ));
            }
            check_url(url, server.allow_private)?;
            for (name, setting) in &server.headers {
                if !header_name(name) {
                    return Err(Invalid::new(
                        "headers",
                        format!("`{name}` is not a header name"),
                    ));
                }
                check_setting(&format!("headers.{name}"), setting, true)?;
            }
            if !server.env.is_empty() {
                return Err(Invalid::new("env", "env applies to stdio servers only"));
            }
        }
    }
    for tool in server.tools.risk.keys() {
        if tool.is_empty() || tool.len() > 128 {
            return Err(Invalid::new(
                "tools.risk",
                "a tool name of 1 to 128 characters",
            ));
        }
    }
    if let Pick::Only(names) = &server.tools.allow
        && names.iter().any(|n| n.is_empty() || n.len() > 128)
    {
        return Err(Invalid::new(
            "tools.allow",
            "tool names of 1 to 128 characters",
        ));
    }
    for (field, pick, check) in [
        (
            "access.roles",
            &server.access.roles,
            role_or_node as fn(&str) -> bool,
        ),
        (
            "access.nodes",
            &server.access.nodes,
            role_or_node as fn(&str) -> bool,
        ),
    ] {
        if let Pick::Only(names) = pick
            && names.iter().any(|n| !check(n))
        {
            return Err(Invalid::new(field, "names of letters, digits, '-' or '_'"));
        }
    }
    let limits = &server.limits;
    if !(1..=600).contains(&limits.timeout_s) {
        return Err(Invalid::new("limits.timeout_s", "between 1 and 600"));
    }
    if !(1..=4096).contains(&limits.max_output_kb) {
        return Err(Invalid::new("limits.max_output_kb", "between 1 and 4096"));
    }
    if !(1..=1440).contains(&limits.idle_stop_min) {
        return Err(Invalid::new("limits.idle_stop_min", "between 1 and 1440"));
    }
    Ok(())
}

fn check_setting(field: &str, setting: &Setting, header: bool) -> Result<(), Invalid> {
    match setting {
        Setting::Secret { secret, format } => {
            if !secret_name(secret) {
                return Err(Invalid::new(
                    field,
                    format!("`{secret}` is not a vault secret name"),
                ));
            }
            match format {
                Some(f) if !header => Err(Invalid::new(
                    field,
                    format!("`format` ({f}) applies to headers only"),
                )),
                Some(f) if !f.contains("{}") || f.len() > 256 || !plain(f) => Err(Invalid::new(
                    field,
                    "format must contain {} where the secret goes",
                )),
                _ => Ok(()),
            }
        }
        Setting::Value { value } => {
            if plain(value) {
                Ok(())
            } else {
                Err(Invalid::new(field, "a single line of at most 4096 bytes"))
            }
        }
    }
}

/// An http endpoint: https, or http for a loopback or private host when `allow_private` says so.
fn check_url(url: &str, allow_private: bool) -> Result<(), Invalid> {
    let bad = |m: &str| Err(Invalid::new("transport.url", m));
    if url.len() > 2048 {
        return bad("at most 2048 characters");
    }
    let (scheme, rest) = match url.split_once("://") {
        Some(parts) => parts,
        None => return bad("an absolute https:// URL"),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
        return bad("a host with no user name or password in the URL");
    }
    match scheme {
        "https" => Ok(()),
        "http" if allow_private => Ok(()),
        "http" => bad("https only (http is allowed when allow_private is set)"),
        _ => bad("https only"),
    }
}

/// The store's contents: the servers, and the tool listing each last showed (so agents can be offered
/// the tools while the server is stopped).
#[derive(Default)]
struct Inner {
    servers: BTreeMap<String, McpServer>,
    tools: BTreeMap<String, Vec<ToolInfo>>,
}

/// The store: every server by id, saved to `mcp.json` on each change (unless the database holds it).
pub struct Store {
    path: PathBuf,
    inner: Mutex<Inner>,
    version: AtomicU64,
    saved: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Store {
    /// Loads `path`. A file that does not parse is set aside as `mcp.json.rejected`, and the store starts empty.
    pub fn load(path: PathBuf) -> Store {
        let store = Store {
            path,
            inner: Mutex::new(Inner::default()),
            version: AtomicU64::new(0),
            saved: AtomicU64::new(0),
        };
        if let Ok(text) = std::fs::read_to_string(&store.path)
            && let Err(err) = store.restore(&text)
        {
            eprintln!("apid: mcp.json refused ({err}); kept aside as mcp.json.rejected");
            let _ = std::fs::rename(&store.path, store.path.with_extension("json.rejected"));
        }
        store
            .saved
            .store(store.version.load(Ordering::SeqCst), Ordering::SeqCst);
        store
    }

    /// Every server, sorted by id.
    pub fn list(&self) -> Vec<McpServer> {
        lock(&self.inner).servers.values().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<McpServer> {
        lock(&self.inner).servers.get(id).cloned()
    }

    /// The tools `id` last listed (empty until it has run or been tested).
    pub fn tools_of(&self, id: &str) -> Vec<ToolInfo> {
        lock(&self.inner).tools.get(id).cloned().unwrap_or_default()
    }

    /// Keeps the tool listing a server just showed. Ignored for an unknown server.
    pub fn set_tools(&self, id: &str, tools: Vec<ToolInfo>) {
        let mut inner = lock(&self.inner);
        if inner.servers.contains_key(id) {
            inner.tools.insert(id.to_string(), tools);
            self.changed(&inner);
        }
    }

    /// Adds a server. Errors are the 409 (duplicate) or 400 (invalid, or too many) cases.
    pub fn insert(&self, server: McpServer) -> Result<McpServer, StoreError> {
        validate(&server).map_err(StoreError::Invalid)?;
        let mut inner = lock(&self.inner);
        if inner.servers.contains_key(&server.id) {
            return Err(StoreError::Duplicate(server.id));
        }
        if inner.servers.len() >= MAX_SERVERS {
            return Err(StoreError::Full);
        }
        inner.servers.insert(server.id.clone(), server.clone());
        self.changed(&inner);
        Ok(server)
    }

    /// Replaces a server's config. The id cannot change.
    pub fn replace(&self, id: &str, server: McpServer) -> Result<McpServer, StoreError> {
        validate(&server).map_err(StoreError::Invalid)?;
        if server.id != id {
            return Err(StoreError::Invalid(Invalid::new(
                "id",
                "an id cannot change",
            )));
        }
        let mut inner = lock(&self.inner);
        if !inner.servers.contains_key(id) {
            return Err(StoreError::Missing);
        }
        inner.servers.insert(id.to_string(), server.clone());
        self.changed(&inner);
        Ok(server)
    }

    /// Removes a server and its tool listing; true when it existed.
    pub fn remove(&self, id: &str) -> bool {
        let mut inner = lock(&self.inner);
        let removed = inner.servers.remove(id).is_some();
        inner.tools.remove(id);
        if removed {
            crate::mcp::forget_risks(id);
            self.changed(&inner);
        }
        removed
    }

    /// Bumps the version (the file saver looks at it) and republishes the tool risks the laws read.
    fn changed(&self, inner: &Inner) {
        self.version.fetch_add(1, Ordering::SeqCst);
        crate::mcp::publish_risks(&inner.servers, &inner.tools);
    }

    /// The whole store as JSON (`{format, servers, tools}`); the Postgres snapshot and the file share it.
    pub fn snapshot(&self) -> String {
        let inner = lock(&self.inner);
        let servers: Vec<&McpServer> = inner.servers.values().collect();
        serde_json::to_string_pretty(
            &serde_json::json!({ "format": 1, "servers": servers, "tools": inner.tools }),
        )
        .unwrap_or_default()
    }

    /// Replaces the store with a snapshot. A snapshot with an invalid server changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let list = value
            .get("servers")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        let servers: Vec<McpServer> = serde_json::from_value(list).map_err(|e| e.to_string())?;
        let mut next = Inner::default();
        for server in servers {
            validate(&server).map_err(|e| e.to_string())?;
            if next.servers.insert(server.id.clone(), server).is_some() {
                return Err("a duplicate server id".into());
            }
        }
        if let Some(tools) = value.get("tools") {
            next.tools = serde_json::from_value(tools.clone()).map_err(|e| e.to_string())?;
            next.tools.retain(|id, _| next.servers.contains_key(id));
        }
        let mut inner = lock(&self.inner);
        *inner = next;
        self.changed(&inner);
        Ok(())
    }

    /// Writes the file when the store changed since the last write. Returns whether it wrote.
    pub fn save_if_changed(&self) -> std::io::Result<bool> {
        let version = self.version.load(Ordering::SeqCst);
        if version == self.saved.load(Ordering::SeqCst) {
            return Ok(false);
        }
        crate::state::write_atomic(&self.path, &self.snapshot())?;
        self.saved.store(version, Ordering::SeqCst);
        Ok(true)
    }
}

/// Why a store change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Invalid(Invalid),
    Duplicate(String),
    Missing,
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-mcp-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn stdio(id: &str) -> McpServer {
        McpServer {
            id: id.into(),
            name: "Test".into(),
            enabled: true,
            transport: Transport::Stdio {
                image: "ghcr.io/example/server:1.0".into(),
                command: vec![],
                workdir: None,
            },
            env: BTreeMap::from([(
                "TOKEN".to_string(),
                Setting::Secret {
                    secret: "GITHUB_TOKEN".into(),
                    format: None,
                },
            )]),
            headers: BTreeMap::new(),
            tools: ToolPolicy::default(),
            access: Access {
                roles: Pick::All,
                nodes: Pick::All,
            },
            limits: Limits::default(),
            allow_private: false,
            storage: Storage::default(),
            created_ms: 0,
            updated_ms: 0,
        }
    }

    fn http(id: &str, url: &str) -> McpServer {
        let mut server = stdio(id);
        server.transport = Transport::Http { url: url.into() };
        server.env.clear();
        server.headers = BTreeMap::from([(
            "Authorization".to_string(),
            Setting::Secret {
                secret: "LINEAR_KEY".into(),
                format: Some("Bearer {}".into()),
            },
        )]);
        server
    }

    fn field(server: &McpServer) -> String {
        validate(server).unwrap_err().field
    }

    #[test]
    fn server_config_validation_names_each_bad_field() {
        // a_good_stdio_and_http_server_validate
        {
            assert!(validate(&stdio("github")).is_ok());
            assert!(validate(&http("linear", "https://mcp.linear.app/mcp")).is_ok());
        }
        // ids_follow_the_pattern
        {
            assert!(valid_id("ab"));
            assert!(!valid_id("a"));
            assert!(!valid_id("1abc"));
            assert!(!valid_id("Abc"));
            assert!(!valid_id("a_b"));
            assert!(!valid_id(&"a".repeat(33)));
            let mut server = stdio("x");
            server.id = "Bad_Id".into();
            assert_eq!(field(&server), "id");
        }
        // the_message_names_the_bad_field
        {
            let mut server = stdio("github");
            server.transport = Transport::Stdio {
                image: "-rm".into(),
                command: vec![],
                workdir: None,
            };
            assert_eq!(
                validate(&server).unwrap_err().to_string(),
                "transport.image: a container image name such as ghcr.io/org/server:1.0"
            );
            let mut server = stdio("github");
            server
                .env
                .insert("lower".into(), Setting::Value { value: "x".into() });
            assert_eq!(field(&server), "env");
            let mut server = stdio("github");
            server.env.insert(
                "OK".into(),
                Setting::Value {
                    value: "two\nlines".into(),
                },
            );
            assert_eq!(field(&server), "env.OK");
        }
        // secrets_must_name_vault_entries_and_formats_fit_headers
        {
            let mut server = stdio("github");
            server.env.insert(
                "TOKEN".into(),
                Setting::Secret {
                    secret: "lower".into(),
                    format: None,
                },
            );
            assert_eq!(field(&server), "env.TOKEN");
            let mut server = stdio("github");
            server.env.insert(
                "TOKEN".into(),
                Setting::Secret {
                    secret: "GITHUB_TOKEN".into(),
                    format: Some("Bearer {}".into()),
                },
            );
            assert_eq!(field(&server), "env.TOKEN");
            let mut server = http("linear", "https://mcp.linear.app/mcp");
            server.headers.insert(
                "Authorization".into(),
                Setting::Secret {
                    secret: "LINEAR_KEY".into(),
                    format: Some("Bearer".into()),
                },
            );
            assert_eq!(field(&server), "headers.Authorization");
        }
        // transports_do_not_mix_settings
        {
            let mut server = http("linear", "https://mcp.linear.app/mcp");
            server
                .env
                .insert("X".into(), Setting::Value { value: "1".into() });
            assert_eq!(field(&server), "env");
            let mut server = stdio("github");
            server
                .headers
                .insert("X-Key".into(), Setting::Value { value: "1".into() });
            assert_eq!(field(&server), "headers");
        }
        // urls_must_be_https_without_credentials
        {
            assert_eq!(
                field(&http("linear", "http://mcp.linear.app/mcp")),
                "transport.url"
            );
            assert!(validate(&http("linear", "http://127.0.0.1:9000/mcp")).is_err());
            let mut local = http("local", "http://127.0.0.1:9000/mcp");
            local.allow_private = true;
            assert!(validate(&local).is_ok());
            assert_eq!(
                field(&http("linear", "https://user:pw@mcp.linear.app/mcp")),
                "transport.url"
            );
            assert_eq!(
                field(&http("linear", "mcp.linear.app/mcp")),
                "transport.url"
            );
        }
        // limits_and_access_are_bounded
        {
            let mut server = stdio("github");
            server.limits.timeout_s = 0;
            assert_eq!(field(&server), "limits.timeout_s");
            let mut server = stdio("github");
            server.access.roles = Pick::Only(vec!["bad role!".into()]);
            assert_eq!(field(&server), "access.roles");
        }
        // a_volume_is_for_stdio_servers_and_the_proxy_names_are_reserved
        {
            let mut server = http("linear", "https://mcp.linear.app/mcp");
            server.storage.volume = true;
            assert_eq!(field(&server), "storage");
            let mut server = stdio("github");
            server
                .env
                .insert("HTTP_PROXY".into(), Setting::Value { value: "x".into() });
            assert_eq!(field(&server), "env");
            let mut server = stdio("memory");
            server.storage.volume = true;
            assert!(validate(&server).is_ok());
            assert_eq!(volume_name("memory"), "apz-mcp-memory");
        }
    }

    #[test]
    fn pick_reads_all_or_a_list_and_refuses_other_words() {
        let all: Pick = serde_json::from_str(r#""all""#).unwrap();
        assert!(all.admits("anything"));
        let some: Pick = serde_json::from_str(r#"["builder"]"#).unwrap();
        assert!(some.admits("builder") && !some.admits("reviewer"));
        assert!(serde_json::from_str::<Pick>(r#""some""#).is_err());
        assert!(!Access::default().roles.admits("builder"));
    }

    #[test]
    fn the_store_refuses_duplicates_and_saves_the_file() {
        let dir = scratch("store");
        let store = Store::load(dir.join(FILE));
        store.insert(stdio("github")).unwrap();
        assert_eq!(
            store.insert(stdio("github")).unwrap_err(),
            StoreError::Duplicate("github".into())
        );
        assert!(store.save_if_changed().unwrap());
        assert!(!store.save_if_changed().unwrap());
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(text.contains("GITHUB_TOKEN") && !text.contains("sk-"));
        let reloaded = Store::load(dir.join(FILE));
        assert_eq!(reloaded.get("github"), Some(stdio("github")));
    }

    #[test]
    fn a_snapshot_with_a_bad_server_changes_nothing() {
        let dir = scratch("restore");
        let store = Store::load(dir.join(FILE));
        store.insert(stdio("github")).unwrap();
        let bad = r#"{"format":1,"servers":[{"id":"BAD","name":"x","transport":{"kind":"http","url":"https://a.test/mcp"}}]}"#;
        assert!(store.restore(bad).is_err());
        assert!(store.get("github").is_some());
        let good = store.snapshot();
        store.remove("github");
        store.restore(&good).unwrap();
        assert!(store.get("github").is_some());
    }

    #[test]
    fn replace_keeps_the_id_and_refuses_a_missing_server() {
        let dir = scratch("replace");
        let store = Store::load(dir.join(FILE));
        store.insert(stdio("github")).unwrap();
        assert_eq!(
            store.replace("nope", stdio("nope")).unwrap_err(),
            StoreError::Missing
        );
        assert!(matches!(
            store.replace("github", stdio("other")),
            Err(StoreError::Invalid(_))
        ));
        let mut renamed = stdio("github");
        renamed.name = "Renamed".into();
        assert_eq!(store.replace("github", renamed).unwrap().name, "Renamed");
    }
}
