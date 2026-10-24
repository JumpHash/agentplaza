//! MCP server configuration: its shape, its validation and its store.
//!
//! The store is the component `mcp` (`store::components`): every server's config and the tool listing it last
//! showed, as typed tables. A config never holds a secret value: a `{secret}` names a vault entry, a `{value}` is
//! a plain setting the owner typed. A stdio server runs a pinned package (npm, PyPI or a binary by URL and SHA-256),
//! never an image.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_store::rusqlite::Connection;
use apz_store::{Column, RowSet, StoreError as RowError, Table, TableDef};
use serde::{Deserialize, Serialize};

use super::client::ToolInfo;

/// Most servers one hub holds.
pub const MAX_SERVERS: usize = 64;

/// The table definition of the component (one row per server, the tools it last offered as children).
pub fn def() -> &'static TableDef {
    apz_store::root_def!(McpServer)
}

/// One MCP server the owner configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Table)]
#[table(name = "mcp_server")]
pub struct McpServer {
    #[table(key)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Where and how the server runs: a pinned package (stdio) or an https endpoint (http).
    #[table(child)]
    pub transport: Transport,
    /// Stdio only: env var name to its setting.
    #[serde(default)]
    #[table(map)]
    pub env: BTreeMap<String, Setting>,
    /// Http only: header name to its setting.
    #[serde(default)]
    #[table(map)]
    pub headers: BTreeMap<String, Setting>,
    #[serde(default)]
    #[table(child)]
    pub tools: ToolPolicy,
    #[serde(default)]
    #[table(child)]
    pub access: Access,
    #[serde(default)]
    #[table(flatten)]
    pub limits: Limits,
    /// Lets the server reach private and loopback addresses (the egress rules refuse them otherwise).
    #[serde(default)]
    pub allow_private: bool,
    /// What the server keeps between runs (its data directory is wiped at each start unless this is set).
    #[serde(default)]
    #[table(flatten)]
    pub storage: Storage,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
    /// The tools the server last offered. The store keeps them here as children of the server's row (so agents can
    /// be offered the tools while it is stopped); the console's JSON leaves them out.
    #[serde(skip)]
    #[table(child)]
    pub offered: Vec<ToolInfo>,
}

fn yes() -> bool {
    true
}

/// What a server keeps between runs. `volume` keeps the server's `data/` directory from one start to the next.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Table)]
pub struct Storage {
    #[serde(default)]
    pub volume: bool,
}

/// Env vars the hub sets itself on every stdio server (the egress proxy and the runtimes' homes); a config may not
/// name them.
pub const RESERVED_ENV: [&str; 10] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "NO_PROXY",
    "NODE_USE_ENV_PROXY",
    "NODE_OPTIONS",
    "PATH",
    "HOME",
    "TMPDIR",
];

/// Which way a server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Column)]
#[column(rename_all = "lowercase")]
pub enum TransportKind {
    Stdio,
    Http,
}

/// Where a server runs. A stdio server runs `package` under the OS sandbox with `args`; an http server is the
/// endpoint `url`. The JSON is tagged by `kind` (`stdio` or `http`); the stored form is the same fields as columns.
#[derive(Debug, Clone, PartialEq, Table)]
pub struct Transport {
    pub kind: TransportKind,
    /// Stdio only: the pinned package the server runs.
    #[table(child)]
    pub package: Option<Package>,
    /// Stdio only: the arguments after the package's program.
    #[table(list)]
    pub args: Vec<String>,
    /// Stdio only: a path relative to the server's data directory the process starts in (empty for the data directory).
    pub workdir: Option<String>,
    /// Http only: the endpoint (https).
    pub url: Option<String>,
}

impl Default for Transport {
    fn default() -> Self {
        Transport {
            kind: TransportKind::Stdio,
            package: None,
            args: Vec::new(),
            workdir: None,
            url: None,
        }
    }
}

/// Why a stdio server given a container image is refused: an image is no longer a way to run a server.
pub const IMAGE_REFUSED: &str =
    "a container image no longer runs a server: pick the server's package (npm, pypi or binary)";

/// The JSON shape of a transport: a tagged enum, as the console sends it.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum TransportRepr {
    Stdio {
        /// Refused: an image is no longer a way to run a server (kept only to say so).
        #[serde(default, skip_serializing)]
        image: Option<String>,
        #[serde(default)]
        package: Option<Package>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
    Http {
        url: String,
    },
}

impl TryFrom<TransportRepr> for Transport {
    type Error = String;
    fn try_from(repr: TransportRepr) -> Result<Self, String> {
        match repr {
            TransportRepr::Stdio {
                image,
                package,
                args,
                workdir,
            } => {
                if image.is_some() {
                    return Err(IMAGE_REFUSED.into());
                }
                Ok(Transport {
                    kind: TransportKind::Stdio,
                    package,
                    args,
                    workdir,
                    url: None,
                })
            }
            TransportRepr::Http { url } => Ok(Transport {
                kind: TransportKind::Http,
                package: None,
                args: Vec::new(),
                workdir: None,
                url: Some(url),
            }),
        }
    }
}

impl From<Transport> for TransportRepr {
    fn from(t: Transport) -> Self {
        match t.kind {
            TransportKind::Stdio => TransportRepr::Stdio {
                image: None,
                package: t.package,
                args: t.args,
                workdir: t.workdir,
            },
            TransportKind::Http => TransportRepr::Http {
                url: t.url.unwrap_or_default(),
            },
        }
    }
}

impl Serialize for Transport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TransportRepr::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Transport {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = TransportRepr::deserialize(deserializer)?;
        Transport::try_from(repr).map_err(serde::de::Error::custom)
    }
}

/// A pinned package a stdio server runs. Every package is named and fixed: a version, or a SHA-256 for a binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Table)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Package {
    /// An npm package at an exact version, installed under the server's `pkg/` and run with the hub's Node.
    Npm { name: String, version: String },
    /// A PyPI package at an exact version, run as `uv tool run --from name==version <entry>`; `entry` defaults to
    /// the package name.
    Pypi {
        name: String,
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<String>,
    },
    /// A file downloaded from an https URL and checked against its SHA-256. An archive (.tar.gz or .zip) is unpacked;
    /// `entry` is the file to run inside it (or the file itself).
    Binary {
        url: String,
        sha256: String,
        entry: String,
    },
}

/// An env var or header value: a vault secret, or a plain literal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Table)]
#[serde(untagged)]
pub enum Setting {
    /// `{secret: "GITHUB_TOKEN", format: "Bearer {}"}`; `format` is for headers only.
    Secret {
        secret: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// `{value: "non-secret literal"}`. A `{data}` in it is the server's data directory.
    Value { value: String },
}

/// Which tools are offered, and the risk class of each.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Table)]
pub struct ToolPolicy {
    #[serde(default)]
    #[table(child)]
    pub allow: Pick,
    /// Tool name to its risk; a tool without an entry takes its annotations' risk.
    #[serde(default)]
    #[table(map_value)]
    pub risk: BTreeMap<String, Risk>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        ToolPolicy {
            allow: Pick::all(),
            risk: BTreeMap::new(),
        }
    }
}

/// Who may use the server. The role and node lists default to nobody; the project list to every project (and the
/// agents on none, such as the orchestrator), so a server from before projects keeps its users.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Table)]
pub struct Access {
    #[serde(default)]
    #[table(child)]
    pub roles: Pick,
    #[serde(default)]
    #[table(child)]
    pub nodes: Pick,
    #[serde(default = "Pick::all")]
    #[table(child)]
    pub projects: Pick,
}

impl Default for Access {
    fn default() -> Self {
        Self {
            roles: Pick::default(),
            nodes: Pick::default(),
            projects: Pick::all(),
        }
    }
}

/// Every name, or the listed names. On the wire it is `"all"` or a list of names.
#[derive(Debug, Clone, PartialEq, Table)]
pub struct Pick {
    /// Everything is picked (then `names` is empty).
    pub all: bool,
    #[table(list)]
    pub names: Vec<String>,
}

impl Default for Pick {
    fn default() -> Self {
        Pick::only(Vec::new())
    }
}

impl Pick {
    /// Everything picked.
    pub fn all() -> Self {
        Pick {
            all: true,
            names: Vec::new(),
        }
    }

    /// Exactly these names.
    pub fn only(names: Vec<String>) -> Self {
        Pick { all: false, names }
    }

    /// Whether `name` is picked.
    pub fn admits(&self, name: &str) -> bool {
        self.all || self.names.iter().any(|n| n == name)
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
            PickRepr::Word(word) if word == "all" => Ok(Pick::all()),
            PickRepr::Word(word) => Err(format!("`{word}` is not \"all\" or a list")),
            PickRepr::List(names) => Ok(Pick::only(names)),
        }
    }
}

impl From<Pick> for PickRepr {
    fn from(pick: Pick) -> Self {
        if pick.all {
            PickRepr::Word("all".into())
        } else {
            PickRepr::List(pick.names)
        }
    }
}

impl Serialize for Pick {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        PickRepr::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Pick {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = PickRepr::deserialize(deserializer)?;
        Pick::try_from(repr).map_err(serde::de::Error::custom)
    }
}

/// The risk classes the laws read (`mcp.call`, C-004a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Column)]
#[serde(rename_all = "lowercase")]
#[column(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Table)]
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

/// A path relative to a server's directory: no root, no `..`, no NUL, at most 256 bytes.
fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 256
        && !path.contains('\0')
        && !path.starts_with(['/', '\\'])
        && !path.split(['/', '\\']).any(|part| part == "..")
        && !path.contains(':')
}

/// An npm package name, scoped or not: lowercase segments of letters, digits and `-._~`.
fn npm_name(name: &str) -> bool {
    let segment = |s: &str| {
        let bytes = s.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 214
            && bytes[0].is_ascii_lowercase() | bytes[0].is_ascii_digit()
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~".contains(b))
    };
    match name.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, pkg)) => segment(scope) && segment(pkg),
            None => false,
        },
        None => segment(name),
    }
}

/// An exact npm version: `major.minor.patch` with an optional pre-release and build, no ranges.
fn npm_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    version.len() <= 64
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-+.".contains(&b))
}

/// A PyPI project name: letters, digits, and `.`, `_`, `-` between them.
fn pypi_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    name.len() <= 100
        && !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b))
}

/// An exact PyPI version (a `==` pin): starts with a digit and holds no operators or spaces.
fn pypi_version(version: &str) -> bool {
    version.len() <= 64
        && version.as_bytes()[0..1]
            .first()
            .is_some_and(u8::is_ascii_digit)
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+!-".contains(&b))
}

/// A SHA-256 as lowercase hex: 64 digits.
fn sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_package(package: &Package) -> Result<(), Invalid> {
    let bad = |m: &str| Err(Invalid::new("transport.package", m));
    match package {
        Package::Npm { name, version } => {
            if !npm_name(name) {
                return bad("an npm package name, such as @modelcontextprotocol/server-memory");
            }
            if !npm_version(version) {
                return bad("an exact npm version such as 1.2.3 (no ranges)");
            }
        }
        Package::Pypi {
            name,
            version,
            entry,
        } => {
            if !pypi_name(name) {
                return bad("a PyPI project name, such as mcp-server-time");
            }
            if !pypi_version(version) {
                return bad("an exact PyPI version such as 2026.10.19 (no ranges)");
            }
            if let Some(entry) = entry
                && !(pypi_name(entry) && entry.len() <= 100)
            {
                return bad("the command name the package installs, such as mcp-server-time");
            }
        }
        Package::Binary { url, sha256, entry } => {
            if check_url(url, false).is_err() {
                return bad("an https URL of the binary (or of its archive)");
            }
            if !sha256_hex(sha256) {
                return bad("the file's SHA-256 as 64 lowercase hex digits");
            }
            if !relative_path(entry) {
                return bad("the file to run, relative to the download, such as github-mcp-server");
            }
        }
    }
    Ok(())
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
    let transport = &server.transport;
    match transport.kind {
        TransportKind::Stdio => {
            let Some(package) = &transport.package else {
                return Err(Invalid::new(
                    "transport.package",
                    "pick the server's pinned package: npm, pypi or binary",
                ));
            };
            check_package(package)?;
            if transport.url.is_some() {
                return Err(Invalid::new(
                    "transport.url",
                    "a URL applies to http servers only",
                ));
            }
            if transport.args.len() > 32
                || transport
                    .args
                    .iter()
                    .any(|arg| arg.len() > 512 || arg.contains('\0'))
            {
                return Err(Invalid::new(
                    "transport.args",
                    "at most 32 arguments of 512 bytes each",
                ));
            }
            if let Some(dir) = &transport.workdir
                && !dir.is_empty()
                && !relative_path(dir)
            {
                return Err(Invalid::new(
                    "transport.workdir",
                    "a path inside the server's data directory, such as sub/dir",
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
        TransportKind::Http => {
            if transport.package.is_some() {
                return Err(Invalid::new(
                    "transport.package",
                    "a package applies to stdio servers only",
                ));
            }
            if !transport.args.is_empty() || transport.workdir.is_some() {
                return Err(Invalid::new(
                    "transport.args",
                    "arguments and a workdir apply to stdio servers only",
                ));
            }
            if server.storage.volume {
                return Err(Invalid::new(
                    "storage",
                    "a volume applies to stdio servers only",
                ));
            }
            check_url(transport.url.as_deref().unwrap_or(""), server.allow_private)?;
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
    if !server.tools.allow.all
        && server
            .tools
            .allow
            .names
            .iter()
            .any(|n| n.is_empty() || n.len() > 128)
    {
        return Err(Invalid::new(
            "tools.allow",
            "tool names of 1 to 128 characters",
        ));
    }
    for (field, pick) in [
        ("access.roles", &server.access.roles),
        ("access.nodes", &server.access.nodes),
        ("access.projects", &server.access.projects),
    ] {
        if !pick.all && pick.names.iter().any(|n| !role_or_node(n)) {
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

/// The store's contents: the servers by id, and the tool listing each last showed by id.
#[derive(Default)]
struct Inner {
    servers: BTreeMap<String, McpServer>,
    tools: BTreeMap<String, Vec<ToolInfo>>,
}

/// The store: every server by id. It is the component's state; the hub saves its rows to SQLite.
#[derive(Default)]
pub struct Store {
    inner: Mutex<Inner>,
    /// The hub's generations: every change to a server or its tool listing bumps `mcp`.
    gens: std::sync::Arc<apz_cache::Generations<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Store {
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

    /// Replaces a server's config. The id cannot change. The tool listing it showed is kept.
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

    /// Counts this store's changes in `gens` (the hub's generations).
    pub fn with_generations(
        mut self,
        gens: std::sync::Arc<apz_cache::Generations<String>>,
    ) -> Self {
        self.gens = gens;
        self
    }

    /// Republishes the tool risks the laws read, and bumps the MCP source.
    fn changed(&self, inner: &Inner) {
        crate::mcp::publish_risks(&inner.servers, &inner.tools);
        crate::sources::bump(&self.gens, crate::sources::MCP);
    }

    /// The component's rows as its state is now: each server, with the tools it last offered as its children.
    pub fn rows(&self) -> Result<RowSet, RowError> {
        let inner = lock(&self.inner);
        let servers: Vec<McpServer> = inner
            .servers
            .values()
            .map(|server| {
                let mut server = server.clone();
                server.offered = inner.tools.get(&server.id).cloned().unwrap_or_default();
                server
            })
            .collect();
        RowSet::list(def(), &servers)
    }

    /// Replaces the store with what the database holds (at start), and republishes the risks.
    pub fn load(&self, conn: &Connection) -> Result<(), RowError> {
        let loaded: Vec<McpServer> = apz_store::table::load_list(conn, def())?;
        let mut next = Inner::default();
        for mut server in loaded {
            let offered = std::mem::take(&mut server.offered);
            if !offered.is_empty() {
                next.tools.insert(server.id.clone(), offered);
            }
            next.servers.insert(server.id.clone(), server);
        }
        let mut inner = lock(&self.inner);
        *inner = next;
        self.changed(&inner);
        Ok(())
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

    fn npm(id: &str) -> McpServer {
        McpServer {
            id: id.into(),
            name: "Test".into(),
            enabled: true,
            transport: Transport {
                package: Some(Package::Npm {
                    name: "@modelcontextprotocol/server-memory".into(),
                    version: "2.0.0".into(),
                }),
                ..Transport::default()
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
                roles: Pick::all(),
                nodes: Pick::all(),
                projects: Pick::all(),
            },
            limits: Limits::default(),
            allow_private: false,
            storage: Storage::default(),
            created_ms: 0,
            updated_ms: 0,
            offered: Vec::new(),
        }
    }

    fn http(id: &str, url: &str) -> McpServer {
        let mut server = npm(id);
        server.transport = Transport {
            kind: TransportKind::Http,
            url: Some(url.into()),
            ..Transport::default()
        };
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
            assert!(validate(&npm("github")).is_ok());
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
            let mut server = npm("x");
            server.id = "Bad_Id".into();
            assert_eq!(field(&server), "id");
        }
        // a_stdio_server_names_a_pinned_package_and_no_image
        {
            let mut server = npm("github");
            server.transport.package = None;
            assert_eq!(field(&server), "transport.package");
            let mut server = npm("github");
            server.transport.package = Some(Package::Npm {
                name: "@modelcontextprotocol/server-memory".into(),
                version: "^2.0.0".into(),
            });
            assert_eq!(
                validate(&server).unwrap_err().to_string(),
                "transport.package: an exact npm version such as 1.2.3 (no ranges)"
            );
            let mut server = npm("github");
            server.transport.package = Some(Package::Pypi {
                name: "mcp-server-time".into(),
                version: ">=1".into(),
                entry: None,
            });
            assert_eq!(field(&server), "transport.package");
            let mut server = npm("github");
            server.transport.package = Some(Package::Binary {
                url: "http://example.com/x.tar.gz".into(),
                sha256: "0".repeat(64),
                entry: "x".into(),
            });
            assert_eq!(field(&server), "transport.package");
            let mut server = npm("github");
            server.transport.package = Some(Package::Binary {
                url: "https://example.com/x.tar.gz".into(),
                sha256: "ABC".into(),
                entry: "x".into(),
            });
            assert_eq!(field(&server), "transport.package");
        }
        // the_message_names_the_bad_field
        {
            let mut server = npm("github");
            server
                .env
                .insert("lower".into(), Setting::Value { value: "x".into() });
            assert_eq!(field(&server), "env");
            let mut server = npm("github");
            server.env.insert(
                "OK".into(),
                Setting::Value {
                    value: "two\nlines".into(),
                },
            );
            assert_eq!(field(&server), "env.OK");
            let mut server = npm("github");
            server
                .env
                .insert("PATH".into(), Setting::Value { value: "/x".into() });
            assert_eq!(field(&server), "env");
        }
        // secrets_must_name_vault_entries_and_formats_fit_headers
        {
            let mut server = npm("github");
            server.env.insert(
                "TOKEN".into(),
                Setting::Secret {
                    secret: "lower".into(),
                    format: None,
                },
            );
            assert_eq!(field(&server), "env.TOKEN");
            let mut server = npm("github");
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
            let mut server = npm("github");
            server
                .headers
                .insert("X-Key".into(), Setting::Value { value: "1".into() });
            assert_eq!(field(&server), "headers");
            let mut server = http("linear", "https://mcp.linear.app/mcp");
            server.transport.package = Some(Package::Npm {
                name: "x".into(),
                version: "1.0.0".into(),
            });
            assert_eq!(field(&server), "transport.package");
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
            let mut server = npm("github");
            server.limits.timeout_s = 0;
            assert_eq!(field(&server), "limits.timeout_s");
            let mut server = npm("github");
            server.access.roles = Pick::only(vec!["bad role!".into()]);
            assert_eq!(field(&server), "access.roles");
        }
        // a_volume_is_for_stdio_servers_and_the_proxy_names_are_reserved
        {
            let mut server = http("linear", "https://mcp.linear.app/mcp");
            server.storage.volume = true;
            assert_eq!(field(&server), "storage");
            let mut server = npm("github");
            server
                .env
                .insert("HTTP_PROXY".into(), Setting::Value { value: "x".into() });
            assert_eq!(field(&server), "env");
            let mut server = npm("memory");
            server.storage.volume = true;
            assert!(validate(&server).is_ok());
        }
    }

    #[test]
    fn a_workdir_stays_inside_the_data_directory() {
        let mut server = npm("github");
        server.transport.workdir = Some("sub/dir".into());
        assert!(validate(&server).is_ok());
        for bad in ["/etc", "../up", "a/../../b", "C:\\x", ""] {
            server.transport.workdir = Some(bad.into());
            if bad.is_empty() {
                // An empty workdir means the data directory itself.
                assert!(validate(&server).is_ok());
            } else {
                assert_eq!(field(&server), "transport.workdir", "{bad}");
            }
        }
    }

    #[test]
    fn the_wire_shape_is_tagged_and_an_image_is_refused() {
        let server = npm("memory");
        let json = serde_json::to_value(&server).unwrap();
        assert_eq!(json["transport"]["kind"], "stdio");
        assert_eq!(json["transport"]["package"]["kind"], "npm");
        assert_eq!(json["transport"]["package"]["version"], "2.0.0");
        assert!(json.get("offered").is_none());
        let back: McpServer = serde_json::from_value(json).unwrap();
        assert_eq!(back, server);
        let image = r#"{"kind":"stdio","image":"ghcr.io/x/y:1"}"#;
        let err = serde_json::from_str::<Transport>(image)
            .unwrap_err()
            .to_string();
        assert!(err.contains("pick the server's package"), "{err}");
        assert_eq!(
            serde_json::to_value(http("linear", "https://mcp.linear.app/mcp").transport).unwrap(),
            serde_json::json!({"kind": "http", "url": "https://mcp.linear.app/mcp"})
        );
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
    fn the_store_refuses_duplicates_and_round_trips_its_rows() {
        let store = Store::default();
        store.insert(npm("github")).unwrap();
        assert_eq!(
            store.insert(npm("github")).unwrap_err(),
            StoreError::Duplicate("github".into())
        );
        store.set_tools(
            "github",
            vec![ToolInfo {
                name: "search".into(),
                description: "Searches.".into(),
                input_schema: serde_json::json!({"type": "object"}),
                annotations: super::super::client::ToolAnnotations {
                    read_only_hint: Some(true),
                    ..Default::default()
                },
            }],
        );
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let reloaded = Store::default();
        reloaded.load(&conn).unwrap();
        assert_eq!(reloaded.get("github"), Some(npm("github")));
        assert_eq!(reloaded.tools_of("github").len(), 1);
        assert_eq!(
            reloaded.tools_of("github")[0].annotations.read_only_hint,
            Some(true)
        );
    }

    #[test]
    fn replace_keeps_the_id_and_refuses_a_missing_server() {
        let store = Store::default();
        store.insert(npm("github")).unwrap();
        assert_eq!(
            store.replace("nope", npm("nope")).unwrap_err(),
            StoreError::Missing
        );
        assert!(matches!(
            store.replace("github", npm("other")),
            Err(StoreError::Invalid(_))
        ));
        let mut renamed = npm("github");
        renamed.name = "Renamed".into();
        assert_eq!(store.replace("github", renamed).unwrap().name, "Renamed");
    }

    #[test]
    fn a_change_to_a_server_or_its_tools_moves_the_mcp_source() {
        let gens: std::sync::Arc<apz_cache::Generations<String>> = Default::default();
        let store = Store::default().with_generations(std::sync::Arc::clone(&gens));
        let key = crate::sources::MCP.to_string();
        store.insert(npm("docs")).expect("insert");
        let inserted = gens.get(&key);
        assert!(inserted > 0, "adding a server moves the MCP source");
        store.set_tools("docs", Vec::new());
        let listed = gens.get(&key);
        assert!(listed > inserted, "a tool listing moves it");
        assert!(store.remove("docs"));
        assert!(gens.get(&key) > listed, "a removal moves it");
    }
}
