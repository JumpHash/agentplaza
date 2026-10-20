//! This hub as a server (parity item 6, the web's `mcp-hub.ts`): the owner turns the hub's own MCP server on, sets its
//! bind, origins and auth mode, mints tokens for outside clients, and reads the tokens and the call log. A new token is
//! shown once, in the reveal box, with the setup text for each client. Leaving the tab clears the reveal, so no token
//! outlives the view. The scope tables and the setup text follow docs/design/hub-mcp.md and the web's
//! `hub-mcp-snippets.ts`; the tests pin the same strings as the web's tests.

use std::collections::BTreeSet;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, button, checkbox, column, container, pick_list, row, text_input};
use iced::{Alignment, Background, Border, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::models::{Settings, save_settings};
use crate::console::{Ctx, client, ui};

const DAY_MS: u64 = 86_400_000;
const CALLS_LIMIT: usize = 100;

// ---- Scopes, binds, auth and the tool lists (docs/design/hub-mcp.md) -------------------------------------

/// A token's scope, in order of power: a token may hold tools of its own scope and of the scopes below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Observe,
    Operate,
    Govern,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Observe, Scope::Operate, Scope::Govern];

    /// The word the hub's API takes.
    pub fn wire(self) -> &'static str {
        match self {
            Scope::Observe => "observe",
            Scope::Operate => "operate",
            Scope::Govern => "govern",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Observe => "Observe",
            Scope::Operate => "Operate",
            Scope::Govern => "Govern",
        }
    }

    /// The plain-words line the web shows beside each scope.
    pub fn plain(self) -> &'static str {
        match self {
            Scope::Observe => "read only",
            Scope::Operate => {
                "acts within the plaza's rules: tasks, messages, stopping or restarting an agent, proposing prompts and skills"
            }
            Scope::Govern => {
                "records recommendations on approvals, and publishes prompts and skills"
            }
        }
    }

    fn rank(self) -> u8 {
        match self {
            Scope::Observe => 0,
            Scope::Operate => 1,
            Scope::Govern => 2,
        }
    }

    /// Whether a token of this scope may hold a tool of scope `tool` (apid's `Scope::permits`).
    pub fn permits(self, tool: Scope) -> bool {
        tool.rank() <= self.rank()
    }

    fn from_wire(word: &str) -> Scope {
        Scope::ALL
            .into_iter()
            .find(|scope| scope.wire() == word)
            .unwrap_or_default()
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

const SCOPE_OPTIONS: [(Scope, &str); 3] = [
    (Scope::Observe, "Observe"),
    (Scope::Operate, "Operate"),
    (Scope::Govern, "Govern"),
];

/// Local only, or remote over TLS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Bind {
    #[default]
    Local,
    Remote,
}

impl Bind {
    fn wire(self) -> &'static str {
        match self {
            Bind::Local => "local",
            Bind::Remote => "remote",
        }
    }

    fn from_wire(word: &str) -> Bind {
        if word == "remote" {
            Bind::Remote
        } else {
            Bind::Local
        }
    }
}

const BIND_OPTIONS: [(Bind, &str); 2] = [
    (Bind::Local, "Local only"),
    (Bind::Remote, "Remote over TLS"),
];

/// Secure needs a token; open lets anonymous callers in with the open scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    Secure,
    Open,
}

impl Auth {
    fn wire(self) -> &'static str {
        match self {
            Auth::Secure => "secure",
            Auth::Open => "open",
        }
    }

    fn from_wire(word: &str) -> Auth {
        if word == "open" {
            Auth::Open
        } else {
            Auth::Secure
        }
    }
}

const AUTH_OPTIONS: [(Auth, &str); 2] = [(Auth::Secure, "Secure"), (Auth::Open, "Open")];

/// The hub's registry (crates/apid/src/hub_mcp/tools/mod.rs), by each tool's own scope. This crate does not depend on
/// apid, so the test below pins the same list; a tool added to the hub must be added here and to the web's list.
const OBSERVE_TOOLS: &[&str] = &[
    "plaza_status",
    "list_agents",
    "get_agent",
    "get_transcript",
    "list_tasks",
    "get_task",
    "list_board",
    "list_projects",
    "get_project",
    "list_approvals",
    "list_machines",
    "treasury_summary",
    "usage_summary",
    "list_laws",
    "search_library",
    "prompts_list",
    "prompt_get",
    "skills_list",
    "skill_get",
];
const OPERATE_TOOLS: &[&str] = &[
    "message_orchestrator",
    "tell_agent",
    "stop_agent",
    "restart_agent",
    "create_task",
    "prompt_propose",
    "skill_propose",
];
const GOVERN_TOOLS: &[&str] = &["request_decision", "prompt_publish", "skill_publish"];

/// The tools of one scope, by their own scope.
pub fn tools_of(scope: Scope) -> &'static [&'static str] {
    match scope {
        Scope::Observe => OBSERVE_TOOLS,
        Scope::Operate => OPERATE_TOOLS,
        Scope::Govern => GOVERN_TOOLS,
    }
}

/// The scope a tool belongs to, or None for a tool the hub does not have.
pub fn tool_scope(name: &str) -> Option<Scope> {
    Scope::ALL
        .into_iter()
        .find(|scope| tools_of(*scope).contains(&name))
}

/// Drops the ticked tools that a token of `scope` may no longer hold (the server would refuse the token).
fn keep_tools_within(picked: &mut BTreeSet<String>, scope: Scope) {
    picked.retain(|name| tool_scope(name).is_some_and(|tool| scope.permits(tool)));
}

// ---- The setup text (the web's setupSnippets) -------------------------------------------------------------

/// The setup text for each client, built from the MCP URL and the token. `token` None means open mode: no token,
/// so no Authorization header and no login line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippets {
    /// The Claude Code command. The bearer header is dropped in open mode.
    pub claude_code: String,
    /// The Claude Desktop JSON block: the stdio bridge against the hub's origin. It never carries a token.
    pub desktop: String,
    /// The line that saves the token (it reads it from stdin). None in open mode.
    pub desktop_login: Option<String>,
    /// The URL and the header, for any other client. In open mode, the URL alone.
    pub generic: String,
}

/// The hub's origin: the scheme and host of its MCP URL, with no path.
pub fn hub_origin(url: &str) -> String {
    let base = url
        .strip_suffix("/mcp/")
        .or_else(|| url.strip_suffix("/mcp"))
        .unwrap_or(url);
    match base.find("://") {
        Some(at) => {
            let rest = &base[at + 3..];
            let host_end = rest.find('/').unwrap_or(rest.len());
            base[..at + 3 + host_end].to_string()
        }
        None => base.to_string(),
    }
}

pub fn setup_snippets(url: &str, token: Option<&str>) -> Snippets {
    let origin = hub_origin(url);
    let header = token
        .map(|t| format!(" --header \"Authorization: Bearer {t}\""))
        .unwrap_or_default();
    let desktop_config = json!({
        "mcpServers": { "agentplaza": { "command": "agentplaza", "args": ["mcp", "serve", "--hub", origin.as_str()] } }
    });
    Snippets {
        claude_code: format!("claude mcp add --transport http agentplaza {url}{header}"),
        desktop: serde_json::to_string_pretty(&desktop_config).unwrap_or_default(),
        desktop_login: token.map(|_| format!("agentplaza mcp login --hub {origin}")),
        generic: match token {
            None => format!("URL: {url}"),
            Some(t) => format!("URL: {url}\nAuthorization: Bearer {t}"),
        },
    }
}

// ---- Plain-words times and query values -------------------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How long ago `at_ms` was, in plain words.
pub fn ago(now: u64, at_ms: u64) -> String {
    let secs = now.saturating_sub(at_ms) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// A UTC date, YYYY-MM-DD, for a moment in milliseconds since the epoch.
pub fn date_text(ms: u64) -> String {
    // Howard Hinnant's civil-from-days, on days since 1970-01-01.
    let z = (ms / DAY_MS) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// When a token expires, in plain words.
pub fn expiry_text(now: u64, expires_ms: Option<u64>) -> String {
    match expires_ms {
        None => "never expires".to_string(),
        Some(at) if at <= now => format!("expired {}", date_text(at)),
        Some(at) => format!("expires {}", date_text(at)),
    }
}

/// Percent-encodes a query value: anything but the unreserved characters.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---- The hub's shapes (owner.rs) --------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HubStatus {
    pub enabled: bool,
    pub bind: String,
    /// Whether the endpoint serves TLS. A remote bind needs it.
    pub tls: bool,
    pub auth_mode: String,
    pub open_scope: String,
    pub warning: Option<String>,
    pub allowed_origins: Vec<String>,
    pub effective_origins: Vec<String>,
    pub url: String,
    /// The live (not revoked) tokens.
    pub tokens: u64,
    /// False when the database is off, so the settings last only until the hub restarts.
    pub stored: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HubToken {
    pub id: String,
    pub name: String,
    pub scope: String,
    /// The tools the token is narrowed to; empty means all the scope's tools.
    pub tools: Vec<String>,
    pub expires_ms: Option<u64>,
    pub last_used_ms: Option<u64>,
    pub calls: u64,
    pub revoked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HubCall {
    pub token_id: String,
    pub tool: String,
    pub ok: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatedToken {
    pub token: String,
    pub record: HubToken,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TokenList {
    tokens: Vec<HubToken>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CallList {
    calls: Vec<HubCall>,
}

/// One editor session the hub holds (`GET /v1/acp/sessions`). Only the fields the list shows are read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AcpSession {
    pub cid: u64,
    pub cwd: String,
    pub client: String,
    pub title: String,
    pub updated_ms: u64,
    pub closed: bool,
}

/// The setup text for each editor. Each block is what the owner pastes into that editor's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorSnippets {
    /// Zed's `agent_servers` entry, which opens the hub's default agent.
    pub zed: String,
    /// The same entry with `--agent <name>`, for one agent the owner names.
    pub zed_agent: String,
    /// The JetBrains ACP settings: a command, then its arguments.
    pub jetbrains: String,
    /// The CodeCompanion adapter for Neovim.
    pub neovim: String,
}

/// The setup text for the editors. Each editor starts `agentplaza acp`, which speaks ACP over stdio.
pub fn editor_snippets() -> EditorSnippets {
    EditorSnippets {
        zed: r#"{"agent_servers": {"AgentPlaza": {"command": "agentplaza", "args": ["acp"]}}}"#
            .to_string(),
        zed_agent: r#"{"agent_servers": {"AgentPlaza": {"command": "agentplaza", "args": ["acp", "--agent", "<name>"]}}}"#
            .to_string(),
        jetbrains: "Command: agentplaza\nArguments: acp".to_string(),
        neovim: r#"adapters = { acp = { agentplaza = { command = "agentplaza", args = { "acp" } } } }"#
            .to_string(),
    }
}

/// A session's title for the list: the first prompt's line, or "untitled" when there is none.
pub fn session_title(session: &AcpSession) -> &str {
    let title = session.title.trim();
    if title.is_empty() { "untitled" } else { title }
}

/// The line under a session's title: its cid, editor, folder, when it last changed, and whether it is closed.
pub fn session_line(session: &AcpSession, now: u64) -> String {
    let mut parts = vec![format!("cid {}", session.cid)];
    if !session.client.is_empty() {
        parts.push(session.client.clone());
    }
    if !session.cwd.is_empty() {
        parts.push(session.cwd.clone());
    }
    parts.push(format!("updated {}", ago(now, session.updated_ms)));
    if session.closed {
        parts.push("closed".to_string());
    }
    parts.join(" · ")
}

fn parse_sessions(value: Value) -> Result<Vec<AcpSession>, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// A choice in a filter list: the value the API takes, and the label the list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

fn token_choices(tokens: &[HubToken]) -> Vec<Choice> {
    let all = Choice {
        value: String::new(),
        label: "All tokens".to_string(),
    };
    let live = tokens
        .iter()
        .filter(|t| t.revoked_at_ms.is_none())
        .map(|t| Choice {
            value: t.id.clone(),
            label: t.name.clone(),
        });
    std::iter::once(all).chain(live).collect()
}

fn tool_choices() -> Vec<Choice> {
    let all = Choice {
        value: String::new(),
        label: "All tools".to_string(),
    };
    let tools = Scope::ALL
        .into_iter()
        .flat_map(|scope| tools_of(scope).iter().copied())
        .map(|name| Choice {
            value: name.to_string(),
            label: name.to_string(),
        });
    std::iter::once(all).chain(tools).collect()
}

// ---- State and messages -----------------------------------------------------------------------------------

/// A new token and its setup text, shown once. Leaving the tab drops it.
#[derive(Debug, Clone)]
pub struct Reveal {
    pub token: String,
    pub name: String,
    pub snippets: Snippets,
}

#[derive(Debug, Default)]
pub struct State {
    /// The hub's settings as last loaded; None until the first load.
    pub status: Option<HubStatus>,
    pub status_error: Option<String>,
    /// The settings the owner edits. They are copied from the hub on the first load and after each save, so a
    /// refresh never overwrites an edit in progress.
    drafted: bool,
    pub enabled: bool,
    pub bind: Bind,
    pub origins: Vec<String>,
    pub origin_input: String,
    pub auth: Auth,
    pub open_scope: Scope,
    pub confirm_open_remote: bool,
    /// The last outcome (text, is an error).
    pub notice: Option<(String, bool)>,
    /// A write is on its way to the hub.
    pub busy: bool,
    // The new-token form.
    pub name: String,
    pub scope: Scope,
    pub picked: BTreeSet<String>,
    pub expires: String,
    pub reveal: Option<Reveal>,
    // The tokens and the call log.
    pub tokens: Option<Vec<HubToken>>,
    pub tokens_error: Option<String>,
    /// The token waiting for its revoke confirmation.
    pub confirm_revoke: Option<String>,
    pub calls: Option<Vec<HubCall>>,
    pub calls_error: Option<String>,
    /// The call filters; an empty string means all.
    pub filter_token: String,
    pub filter_tool: String,
    // The editors section: the ACP switch as the hub holds it (None until the first read), and the sessions.
    pub acp_enabled: Option<bool>,
    /// The write-back switch (`acp_write_back`) as the hub holds it (None until the first read).
    pub acp_write_back: Option<bool>,
    /// The ACP v2 draft switch (`acp_v2`) as the hub holds it (None until the first read).
    pub acp_v2: Option<bool>,
    /// A switch save is on its way to the hub.
    pub acp_busy: bool,
    pub acp_error: Option<String>,
    pub acp_sessions: Option<Vec<AcpSession>>,
    pub acp_sessions_error: Option<String>,
}

/// Drops the new token from the page. Called when the owner leaves the tab or the screen.
pub fn leave(state: &mut State) {
    state.reveal = None;
}

/// The two switches of the Editors (ACP) section. Both are saved on the same settings document.
#[derive(Debug, Clone, Copy)]
pub enum AcpSwitch {
    Enabled,
    WriteBack,
    V2,
}

impl AcpSwitch {
    fn read(self, s: &Settings) -> bool {
        match self {
            Self::Enabled => s.acp_enabled,
            Self::WriteBack => s.acp_write_back,
            Self::V2 => s.acp_v2,
        }
    }

    fn write(self, s: &mut Settings, on: bool) {
        match self {
            Self::Enabled => s.acp_enabled = on,
            Self::WriteBack => s.acp_write_back = on,
            Self::V2 => s.acp_v2 = on,
        }
    }

    fn slot(self, state: &mut State) -> &mut Option<bool> {
        match self {
            Self::Enabled => &mut state.acp_enabled,
            Self::WriteBack => &mut state.acp_write_back,
            Self::V2 => &mut state.acp_v2,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    StatusLoaded(Result<HubStatus, String>),
    /// "Try again" after a failed status load: the status alone is fetched once more.
    RetryStatus,
    Saved(Result<HubStatus, String>),
    TokensLoaded(Result<Vec<HubToken>, String>),
    CallsLoaded(Result<Vec<HubCall>, String>),
    SetEnabled(bool),
    SetBind(Bind),
    OriginInput(String),
    AddOrigin,
    RemoveOrigin(String),
    SetAuth(Auth),
    SetOpenScope(Scope),
    SetConfirm(bool),
    Save,
    SetName(String),
    SetScope(Scope),
    TogglePick(String, bool),
    SetExpires(String),
    CreateToken,
    Created(Result<CreatedToken, String>),
    /// Copies the text to the clipboard.
    Copy(String),
    HideReveal,
    AskRevoke(String),
    CancelRevoke,
    Revoke(String),
    Revoked(Result<String, String>),
    FilterToken(Choice),
    FilterTool(Choice),
    AcpSettingsLoaded(Result<(bool, bool, bool), String>),
    SetAcp(bool),
    SetAcpWriteBack(bool),
    SetAcpV2(bool),
    AcpSaved(AcpSwitch, Result<bool, String>),
    RefreshAcpSessions,
    AcpSessionsLoaded(Result<Vec<AcpSession>, String>),
}

// ---- Loading and writing ----------------------------------------------------------------------------------

fn parse_status(value: Value) -> Result<HubStatus, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn status_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(move || c.get::<HubStatus>("/v1/hub-mcp"), Msg::StatusLoaded)
}

fn tokens_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || {
            c.get::<TokenList>("/v1/hub-mcp/tokens")
                .map(|list| list.tokens)
        },
        Msg::TokensLoaded,
    )
}

fn calls_task(state: &State, ctx: &Ctx) -> Task<Msg> {
    let mut path = format!("/v1/hub-mcp/calls?limit={CALLS_LIMIT}");
    if !state.filter_token.is_empty() {
        path.push_str(&format!("&token={}", encode(&state.filter_token)));
    }
    if !state.filter_tool.is_empty() {
        path.push_str(&format!("&tool={}", encode(&state.filter_tool)));
    }
    let c = ctx.client.clone();
    client::task(
        move || c.get::<CallList>(&path).map(|list| list.calls),
        Msg::CallsLoaded,
    )
}

/// The ACP switch as the hub holds it. Only the switch is read: the other settings belong to the Models tab.
fn acp_settings_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || {
            c.get::<Settings>("/v1/settings")
                .map(|s| (s.acp_enabled, s.acp_write_back, s.acp_v2))
        },
        Msg::AcpSettingsLoaded,
    )
}

fn acp_sessions_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Value>("/v1/acp/sessions").and_then(parse_sessions),
        Msg::AcpSessionsLoaded,
    )
}

/// Turns one of the ACP switches on or off. The settings are read again first, so the save carries the hub's current
/// values and not the ones this tab loaded earlier.
fn acp_save_task(switch: AcpSwitch, on: bool, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || {
            let mut settings = c.get::<Settings>("/v1/settings")?;
            switch.write(&mut settings, on);
            save_settings(&c, &settings).map(|saved| switch.read(&saved))
        },
        move |result| Msg::AcpSaved(switch, result),
    )
}

/// Starts a save of one ACP switch, unless a save is already on its way or the hub's values are not read yet.
fn save_acp_switch(state: &mut State, ctx: &Ctx, switch: AcpSwitch, on: bool) -> Task<Msg> {
    if state.acp_busy || state.acp_enabled.is_none() {
        return Task::none();
    }
    state.acp_busy = true;
    state.acp_error = None;
    acp_save_task(switch, on, ctx)
}

/// Loads the hub's settings, tokens, calls and editor sessions (called when the tab opens and on every poll tick).
pub fn load(state: &State, ctx: &Ctx) -> Task<Msg> {
    Task::batch([
        status_task(ctx),
        tokens_task(ctx),
        calls_task(state, ctx),
        acp_settings_task(ctx),
        acp_sessions_task(ctx),
    ])
}

fn apply_draft(state: &mut State, status: &HubStatus) {
    state.enabled = status.enabled;
    state.bind = Bind::from_wire(&status.bind);
    state.origins = status.allowed_origins.clone();
    state.auth = Auth::from_wire(&status.auth_mode);
    state.open_scope = Scope::from_wire(&status.open_scope);
}

fn name_of(state: &State, id: &str) -> String {
    state
        .tokens
        .as_ref()
        .and_then(|list| list.iter().find(|t| t.id == id))
        .map(|t| t.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn save_task(state: &State, ctx: &Ctx) -> Task<Msg> {
    let mut body = json!({
        "enabled": state.enabled,
        "bind": state.bind.wire(),
        "allowed_origins": state.origins.clone(),
        "auth_mode": state.auth.wire(),
        "open_scope": state.open_scope.wire(),
    });
    if state.auth == Auth::Open && state.bind == Bind::Remote {
        body["confirm_open_remote"] = json!(state.confirm_open_remote);
    }
    let c = ctx.client.clone();
    client::task(
        move || {
            c.call("PUT", "/v1/hub-mcp", Some(&body))
                .and_then(parse_status)
        },
        Msg::Saved,
    )
}

fn create_task(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let mut body = json!({ "name": state.name.trim(), "scope": state.scope.wire() });
    if !state.picked.is_empty() {
        body["tools"] = json!(state.picked.iter().cloned().collect::<Vec<String>>());
    }
    let days = state.expires.trim();
    if !days.is_empty() {
        match days.parse::<u64>() {
            Ok(n) => body["expires_days"] = json!(n),
            Err(_) => {
                state.notice = Some(("Expiry is a whole number of days.".to_string(), true));
                return Task::none();
            }
        }
    }
    state.busy = true;
    state.notice = None;
    let c = ctx.client.clone();
    client::task(
        move || c.post::<CreatedToken>("/v1/hub-mcp/tokens", &body),
        Msg::Created,
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::StatusLoaded(Ok(status)) => {
            if !state.drafted {
                state.drafted = true;
                apply_draft(state, &status);
            }
            state.status = Some(status);
            state.status_error = None;
            Task::none()
        }
        Msg::StatusLoaded(Err(e)) => {
            state.status_error = Some(e);
            Task::none()
        }
        Msg::RetryStatus => status_task(ctx),
        Msg::Saved(Ok(status)) => {
            state.busy = false;
            apply_draft(state, &status);
            state.status = Some(status);
            state.status_error = None;
            state.confirm_open_remote = false;
            state.notice = Some(("Saved.".to_string(), false));
            Task::none()
        }
        Msg::Saved(Err(e)) => {
            state.busy = false;
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::AcpSettingsLoaded(Ok((on, write_back, v2))) => {
            // A save in flight wins over a read that was taken before it.
            if !state.acp_busy {
                state.acp_enabled = Some(on);
                state.acp_write_back = Some(write_back);
                state.acp_v2 = Some(v2);
            }
            state.acp_error = None;
            Task::none()
        }
        Msg::AcpSettingsLoaded(Err(e)) => {
            state.acp_error = Some(e);
            Task::none()
        }
        Msg::SetAcp(on) => save_acp_switch(state, ctx, AcpSwitch::Enabled, on),
        Msg::SetAcpWriteBack(on) => save_acp_switch(state, ctx, AcpSwitch::WriteBack, on),
        Msg::SetAcpV2(on) => save_acp_switch(state, ctx, AcpSwitch::V2, on),
        Msg::AcpSaved(switch, Ok(on)) => {
            state.acp_busy = false;
            *switch.slot(state) = Some(on);
            let text = match (switch, on) {
                (AcpSwitch::Enabled, true) => "Editors can connect to this hub.",
                (AcpSwitch::Enabled, false) => "Editors can no longer connect to this hub.",
                (AcpSwitch::WriteBack, true) => "Editors will offer to apply an agent's change.",
                (AcpSwitch::WriteBack, false) => {
                    "Editors will no longer offer to apply agents' changes."
                }
                (AcpSwitch::V2, true) => {
                    "Editors and engines that ask for protocol 2 get the ACP v2 draft."
                }
                (AcpSwitch::V2, false) => "Editors and engines are back on the stable v1.",
            };
            state.notice = Some((text.to_string(), false));
            Task::none()
        }
        Msg::AcpSaved(_, Err(e)) => {
            state.acp_busy = false;
            state.acp_error = Some(e);
            Task::none()
        }
        Msg::RefreshAcpSessions => acp_sessions_task(ctx),
        Msg::AcpSessionsLoaded(Ok(list)) => {
            state.acp_sessions = Some(list);
            state.acp_sessions_error = None;
            Task::none()
        }
        Msg::AcpSessionsLoaded(Err(e)) => {
            state.acp_sessions_error = Some(e);
            Task::none()
        }
        Msg::TokensLoaded(Ok(list)) => {
            state.tokens = Some(list);
            state.tokens_error = None;
            Task::none()
        }
        Msg::TokensLoaded(Err(e)) => {
            state.tokens_error = Some(e);
            Task::none()
        }
        Msg::CallsLoaded(Ok(list)) => {
            state.calls = Some(list);
            state.calls_error = None;
            Task::none()
        }
        Msg::CallsLoaded(Err(e)) => {
            state.calls_error = Some(e);
            Task::none()
        }
        Msg::SetEnabled(on) => {
            state.enabled = on;
            Task::none()
        }
        Msg::SetBind(bind) => {
            state.bind = bind;
            Task::none()
        }
        Msg::OriginInput(text) => {
            state.origin_input = text;
            Task::none()
        }
        Msg::AddOrigin => {
            let origin = state.origin_input.trim().to_string();
            if !origin.is_empty() && !state.origins.contains(&origin) {
                state.origins.push(origin);
            }
            state.origin_input.clear();
            Task::none()
        }
        Msg::RemoveOrigin(origin) => {
            state.origins.retain(|o| *o != origin);
            Task::none()
        }
        Msg::SetAuth(auth) => {
            state.auth = auth;
            Task::none()
        }
        Msg::SetOpenScope(scope) => {
            state.open_scope = scope;
            Task::none()
        }
        Msg::SetConfirm(on) => {
            state.confirm_open_remote = on;
            Task::none()
        }
        Msg::Save => {
            state.busy = true;
            state.notice = None;
            save_task(state, ctx)
        }
        Msg::SetName(name) => {
            state.name = name;
            Task::none()
        }
        Msg::SetScope(scope) => {
            state.scope = scope;
            keep_tools_within(&mut state.picked, scope);
            Task::none()
        }
        Msg::TogglePick(name, on) => {
            if on {
                state.picked.insert(name);
            } else {
                state.picked.remove(&name);
            }
            Task::none()
        }
        Msg::SetExpires(text) => {
            state.expires = text;
            Task::none()
        }
        Msg::CreateToken => {
            if state.busy {
                return Task::none();
            }
            create_task(state, ctx)
        }
        Msg::Created(Ok(made)) => {
            state.busy = false;
            let url = state
                .status
                .as_ref()
                .map(|s| s.url.clone())
                .unwrap_or_default();
            let for_clients = if state.auth == Auth::Open {
                None
            } else {
                Some(made.token.as_str())
            };
            let snippets = setup_snippets(&url, for_clients);
            state.reveal = Some(Reveal {
                token: made.token.clone(),
                name: made.record.name.clone(),
                snippets,
            });
            state.name.clear();
            state.expires.clear();
            state.picked.clear();
            state.notice = Some(("Token created.".to_string(), false));
            tokens_task(ctx)
        }
        Msg::Created(Err(e)) => {
            state.busy = false;
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Copy(text) => {
            state.notice = Some(("Copied.".to_string(), false));
            iced::clipboard::write(text)
        }
        Msg::HideReveal => {
            state.reveal = None;
            Task::none()
        }
        Msg::AskRevoke(id) => {
            state.confirm_revoke = Some(id);
            Task::none()
        }
        Msg::CancelRevoke => {
            state.confirm_revoke = None;
            Task::none()
        }
        Msg::Revoke(id) => {
            state.confirm_revoke = None;
            state.busy = true;
            let label = format!("{} revoked.", name_of(state, &id));
            let c = ctx.client.clone();
            let path = format!("/v1/hub-mcp/tokens/{id}");
            client::task(move || c.delete(&path).map(|_| label), Msg::Revoked)
        }
        Msg::Revoked(Ok(label)) => {
            state.busy = false;
            state.notice = Some((label, false));
            Task::batch([tokens_task(ctx), calls_task(state, ctx)])
        }
        Msg::Revoked(Err(e)) => {
            state.busy = false;
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::FilterToken(choice) => {
            state.filter_token = choice.value;
            calls_task(state, ctx)
        }
        Msg::FilterTool(choice) => {
            state.filter_tool = choice.value;
            calls_task(state, ctx)
        }
    }
}

// ---- The view ---------------------------------------------------------------------------------------------

fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

/// The tab's cards in the web's order: Enable, New token, Tokens, Calls, Editors (ACP).
pub fn view(state: &State) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some((text, is_error)) = &state.notice {
        parts.push(if *is_error {
            ui::error_line(text)
        } else {
            ui::faint(text.clone()).into()
        });
    }
    parts.push(enable_card(state));
    parts.push(token_form(state));
    parts.push(tokens_card(state));
    parts.push(calls_card(state));
    parts.push(editors_card(state));
    column(parts).spacing(16).width(Length::Fill).into()
}

/// A labelled control, as the web's field: the label over the control, and a hint under it in ink-3.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: Option<&str>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(label.to_string(), 12.5, crate::theme::palette().ink_2).into(),
        control,
    ];
    if let Some(hint) = hint {
        parts.push(ui::faint(hint.to_string()).into());
    }
    column(parts).spacing(6).width(Length::Fill).into()
}

/// A warning line, in the palette's warn tone (the web's `p.mcp-warn`).
fn warn_text<'a>(line: &str) -> Element<'a, Msg> {
    ui::sans(line.to_string(), 12.5, crate::theme::palette().warn).into()
}

/// A square icon button with no label (the web's `.iconbtn`): the glyph in ink-2, a hover ground.
fn icon_button<'a>(name: &str, msg: Option<Msg>) -> Element<'a, Msg> {
    let glyph =
        container(ui::icon::<Msg>(name, 14.0, crate::theme::palette().ink_2)).center(Length::Fill);
    button(glyph)
        .on_press_maybe(msg)
        .padding(0)
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .style(|_: &Theme, status: button::Status| button::Style {
            background: matches!(status, button::Status::Hovered | button::Status::Pressed)
                .then_some(Background::Color(crate::theme::palette().hover)),
            text_color: crate::theme::palette().ink,
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        })
        .into()
}

/// A block of monospace text on the sheet ground, as the web's `pre.mono`.
fn pre_box<'a>(content: String) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    container(ui::mono(content, 12.0, p.ink))
        .padding([10, 12])
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(p.sheet_2)),
            border: Border {
                radius: crate::theme::skin().radius.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// A table cell at a portion's width, padded as the web's `.tbl td`.
fn cell<'a>(portion: u16, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .width(Length::FillPortion(portion))
        .padding([9, 12])
        .into()
}

/// A rule the width of its line, in the given colour.
fn rule<'a>(color: iced::Color) -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// A table as the web's `.tbl` inside a card: a head in the label voice over a strong rule, a rule under each row.
fn table<'a>(heads: &[(u16, &str)], rows: Vec<Vec<Element<'a, Msg>>>) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let head_cells: Vec<Element<'a, Msg>> = heads
        .iter()
        .map(|(portion, label)| cell(*portion, ui::label(label)))
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![
        row(head_cells).align_y(Alignment::Center).into(),
        rule(p.rule_strong),
    ];
    let n = rows.len();
    for (slot, cells) in rows.into_iter().enumerate() {
        let spans: Vec<Element<'a, Msg>> = cells
            .into_iter()
            .zip(heads.iter())
            .map(|(content, (portion, _))| cell(*portion, content))
            .collect();
        lines.push(row(spans).align_y(Alignment::Center).into());
        if slot + 1 < n {
            lines.push(rule(p.rule));
        }
    }
    ui::scroll_x(column(lines).width(Length::Fill))
        .width(Length::Fill)
        .into()
}

fn enable_card(state: &State) -> Element<'_, Msg> {
    let remote = state.bind == Bind::Remote;
    let open = state.auth == Auth::Open;
    let mut parts: Vec<Element<'_, Msg>> = vec![
        status_line(state),
        checkbox(state.enabled)
            .label("Serve this hub over MCP")
            .on_toggle(Msg::SetEnabled)
            .into(),
        field(
            "Bind",
            ui::seg(&BIND_OPTIONS, state.bind, Msg::SetBind),
            None,
        ),
        field(
            "Auth",
            ui::seg(&AUTH_OPTIONS, state.auth, Msg::SetAuth),
            Some(if open {
                "Open: no token"
            } else {
                "Secure: token required"
            }),
        ),
    ];
    if open {
        parts.push(field(
            "Open scope (anonymous callers)",
            pick_list(
                Scope::ALL.to_vec(),
                Some(state.open_scope),
                Msg::SetOpenScope,
            )
            .style(crate::theme::pick_list)
            .into(),
            None,
        ));
        parts.push(warn_text(
            "Open mode lets callers without a token use this hub at the open scope.",
        ));
        if let Some(warning) = state.status.as_ref().and_then(|s| s.warning.as_deref()) {
            parts.push(warn_text(warning));
        }
        if remote {
            parts.push(
                checkbox(state.confirm_open_remote)
                    .label("I understand anyone on the network can use it")
                    .on_toggle(Msg::SetConfirm)
                    .into(),
            );
        }
    }
    // The origins: the input and its Add, the list with each one's remove, then the effective origins read-only.
    let origin_input = row![
        text_input("https://app.example.com", &state.origin_input)
            .on_input(Msg::OriginInput)
            .on_submit(Msg::AddOrigin)
            .padding(8)
            .style(crate::theme::input),
        ui::btn_sm("Add", Some("plus"), ui::Kind::Ghost, Some(Msg::AddOrigin)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut origin_rows: Vec<Element<'_, Msg>> = Vec::new();
    if state.origins.is_empty() {
        origin_rows.push(ui::faint("No origins yet.").into());
    }
    for origin in &state.origins {
        origin_rows.push(
            row![
                ui::mono(origin.clone(), 12.0, crate::theme::palette().ink_2),
                icon_button("x", Some(Msg::RemoveOrigin(origin.clone()))),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    parts.push(field(
        "Allowed origins",
        column![
            origin_input,
            column(origin_rows).spacing(4).width(Length::Fill)
        ]
        .spacing(8)
        .into(),
        None,
    ));
    let effective = match state.status.as_ref() {
        Some(status) if !status.effective_origins.is_empty() => status.effective_origins.join(", "),
        _ => "None.".to_string(),
    };
    parts.push(ui::faint(format!("Effective origins: {effective}")).into());
    parts.push(ui::btn_sm(
        "Copy URL",
        None,
        ui::Kind::Ghost,
        state.status.as_ref().map(|s| Msg::Copy(s.url.clone())),
    ));
    ui::card_with(
        "Enable",
        vec![ui::btn_sm(
            "Save",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::Save),
        )],
        column(parts).spacing(14).width(Length::Fill).into(),
    )
}

/// The status block: the on or off badge with the TLS badge and its note, the live tokens and the URL, and the
/// database note when it is off. A failed status load shows its error with a retry.
fn status_line(state: &State) -> Element<'_, Msg> {
    if let Some(error) = &state.status_error {
        return column![
            ui::empty("triangle-alert", "Could not load the hub server", error),
            ui::btn_sm("Try again", None, ui::Kind::Ghost, Some(Msg::RetryStatus)),
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into();
    }
    let Some(status) = &state.status else {
        return ui::faint("Loading...").into();
    };
    let remote = Bind::from_wire(&status.bind) == Bind::Remote;
    let label = if !status.enabled {
        "Off"
    } else if remote {
        "On, remote over TLS"
    } else {
        "On, local only"
    };
    let tone = if status.enabled {
        ui::Tone::Ok
    } else {
        ui::Tone::Muted
    };
    let (tls_label, tls_tone, tls_note) = if status.tls {
        (
            "TLS on",
            ui::Tone::Ok,
            "This endpoint serves TLS, so remote bind is available.",
        )
    } else {
        (
            "TLS off",
            ui::Tone::Warn,
            "Remote bind needs TLS. This endpoint has none, so the hub refuses remote bind until it serves TLS.",
        )
    };
    let plural = if status.tokens == 1 { "" } else { "s" };
    let mut lines: Vec<Element<'_, Msg>> = vec![
        row![
            ui::badge(label, tone),
            ui::badge(tls_label, tls_tone),
            ui::faint(tls_note),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
        row![
            ui::faint(format!("{} live token{plural} · ", status.tokens)),
            ui::mono(status.url.clone(), 12.0, crate::theme::palette().ink_2),
        ]
        .align_y(Alignment::Center)
        .into(),
    ];
    if !status.stored {
        lines.push(warn_text(
            "The database is off, so these settings last only until the hub restarts.",
        ));
    }
    column(lines).spacing(8).width(Length::Fill).into()
}

fn token_form(state: &State) -> Element<'_, Msg> {
    let mut scope_block: Vec<Element<'_, Msg>> =
        vec![ui::seg(&SCOPE_OPTIONS, state.scope, Msg::SetScope)];
    for scope in Scope::ALL {
        scope_block.push(ui::faint(format!("{}: {}", scope.label(), scope.plain())).into());
    }
    let parts: Vec<Element<'_, Msg>> = vec![
        field(
            "Name",
            text_input("laptop", &state.name)
                .on_input(Msg::SetName)
                .padding(8)
                .style(crate::theme::input)
                .into(),
            Some("What this client is, such as laptop or ci."),
        ),
        field(
            "Scope",
            column(scope_block).spacing(6).width(Length::Fill).into(),
            None,
        ),
        tools_block(state),
        field(
            "Expires after (days)",
            text_input("Never", &state.expires)
                .on_input(Msg::SetExpires)
                .padding(8)
                .style(crate::theme::input)
                .into(),
            Some("Blank means the token never expires."),
        ),
        ui::btn(
            "Create token",
            Some("plus"),
            ui::Kind::Primary,
            gate(state.busy, Msg::CreateToken),
        ),
        match &state.reveal {
            Some(reveal) => reveal_box(reveal),
            None => Space::new().into(),
        },
    ];
    ui::card(
        "New token",
        column(parts).spacing(14).width(Length::Fill).into(),
    )
}

/// The tool checklist, grouped by scope: a group above the chosen scope is greyed out.
fn tools_block(state: &State) -> Element<'_, Msg> {
    let p = crate::theme::palette();
    let mut groups: Vec<Element<'_, Msg>> = vec![
        ui::label("Tools (optional)").into(),
        ui::faint("Leave them all unticked for every tool the scope allows.").into(),
    ];
    for group in Scope::ALL {
        let allowed = state.scope.permits(group);
        let heading = if allowed {
            group.label().to_string()
        } else {
            format!("{} (above this scope)", group.label())
        };
        let mut lines: Vec<Element<'_, Msg>> = vec![ui::label(&heading).into()];
        for name in tools_of(group) {
            let mut tool = checkbox(state.picked.contains(*name)).label(*name);
            if allowed {
                let owned = name.to_string();
                tool = tool.on_toggle(move |on| Msg::TogglePick(owned.clone(), on));
            }
            lines.push(tool.into());
        }
        groups.push(column(lines).spacing(6).width(Length::Fill).into());
    }
    container(column(groups).spacing(12).width(Length::Fill))
        .padding([10, 14])
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            border: Border {
                color: p.rule,
                width: 1.0,
                radius: crate::theme::skin().radius.into(),
            },
            ..Default::default()
        })
        .into()
}

fn reveal_box(reveal: &Reveal) -> Element<'_, Msg> {
    let p = crate::theme::palette();
    let mut parts: Vec<Element<'_, Msg>> = vec![
        ui::sans(format!("Token for {}", reveal.name), 13.0, p.ink).into(),
        warn_text("Shown once. Copy it now, because it cannot be shown again."),
        row![
            container(pre_box(reveal.token.clone())).width(Length::Fill),
            ui::btn_sm(
                "Copy token",
                None,
                ui::Kind::Ghost,
                Some(Msg::Copy(reveal.token.clone())),
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
        snippet(
            "Claude Code",
            &reveal.snippets.claude_code,
            "Run this in a terminal.",
        ),
        snippet(
            "Claude Desktop",
            &reveal.snippets.desktop,
            "Add this to the MCP servers of Claude Desktop or another stdio client. It runs the CLI bridge, which uses the token saved by the login line below.",
        ),
    ];
    if let Some(login) = &reveal.snippets.desktop_login {
        parts.push(snippet(
            "Save the token",
            login,
            "Reads the token from stdin and saves it in cli.json. Paste the token, then end the input.",
        ));
    }
    parts.push(snippet(
        "Generic",
        &reveal.snippets.generic,
        "The URL and the header, for any other client.",
    ));
    parts.push(ui::btn_sm(
        "Hide",
        None,
        ui::Kind::Ghost,
        Some(Msg::HideReveal),
    ));
    container(column(parts).spacing(12).width(Length::Fill))
        .padding(14)
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(p.raised)),
            border: Border {
                color: p.rule_strong,
                width: 1.0,
                radius: crate::theme::skin().radius_lg.into(),
            },
            ..Default::default()
        })
        .into()
}

/// One block of setup text: its title and copy button, the note on what it does, then the text in a block.
fn snippet<'a>(title: &str, text: &str, note: &str) -> Element<'a, Msg> {
    column![
        row![
            ui::sans(title.to_string(), 13.0, crate::theme::palette().ink),
            ui::btn_sm(
                "Copy",
                None,
                ui::Kind::Ghost,
                Some(Msg::Copy(text.to_string())),
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        ui::faint(note.to_string()),
        pre_box(text.to_string()),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

fn tokens_card(state: &State) -> Element<'_, Msg> {
    let body: Element<'_, Msg> = if let Some(error) = &state.tokens_error {
        ui::empty("triangle-alert", "Could not load the tokens", error)
    } else {
        match &state.tokens {
            None => ui::loading(),
            Some(list) => {
                let live: Vec<&HubToken> =
                    list.iter().filter(|t| t.revoked_at_ms.is_none()).collect();
                if live.is_empty() {
                    ui::empty(
                        "key",
                        "No tokens",
                        "Make one above to give an outside client access to this hub.",
                    )
                } else {
                    tokens_table(state, &live)
                }
            }
        }
    };
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some(confirm) = revoke_card(state) {
        parts.push(confirm);
    }
    parts.push(body);
    ui::card(
        "Tokens",
        column(parts).spacing(14).width(Length::Fill).into(),
    )
}

/// The revoke confirmation as the web's dialog: the warning, Cancel, and Revoke in the danger tone.
fn revoke_card(state: &State) -> Option<Element<'_, Msg>> {
    let id = state.confirm_revoke.as_deref()?;
    let name = state
        .tokens
        .as_deref()
        .and_then(|list| list.iter().find(|t| t.id == id))
        .map(|t| t.name.clone())
        .unwrap_or_else(|| id.to_string());
    Some(ui::card(
        &format!("Revoke {name}"),
        column![
            ui::faint("Clients that use this token lose access at once. This cannot be undone."),
            row![
                ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelRevoke)),
                ui::btn_sm(
                    "Revoke",
                    None,
                    ui::Kind::Danger,
                    gate(state.busy, Msg::Revoke(id.to_string())),
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into(),
    ))
}

fn tokens_table<'a>(state: &'a State, tokens: &[&'a HubToken]) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let now = now_ms();
    let rows: Vec<Vec<Element<'a, Msg>>> = tokens
        .iter()
        .map(|t| -> Vec<Element<'a, Msg>> {
            let tools: Element<'a, Msg> = if t.tools.is_empty() {
                ui::faint("All the scope's tools").into()
            } else {
                ui::mono(ui::ellipsize(&t.tools.join(", "), 80), 12.0, p.ink_2).into()
            };
            vec![
                column![
                    ui::sans(t.name.clone(), 13.0, p.ink),
                    ui::mono(t.id.clone(), 11.0, p.ink_3),
                ]
                .spacing(2)
                .into(),
                ui::badge(&t.scope, ui::Tone::Info),
                tools,
                ui::faint(match t.last_used_ms.filter(|&ms| ms > 0) {
                    Some(ms) => ago(now, ms),
                    None => "never".to_string(),
                })
                .into(),
                ui::faint(t.calls.to_string()).into(),
                ui::faint(match t.expires_ms {
                    Some(ms) => date_text(ms),
                    None => "never".to_string(),
                })
                .into(),
                ui::btn_sm(
                    "Revoke",
                    Some("trash-2"),
                    ui::Kind::Danger,
                    gate(state.busy, Msg::AskRevoke(t.id.clone())),
                ),
            ]
        })
        .collect();
    table(
        &[
            (3, "Name"),
            (2, "Scope"),
            (3, "Tools"),
            (2, "Last used"),
            (1, "Calls"),
            (2, "Expires"),
            (2, "Actions"),
        ],
        rows,
    )
}

/// The editors section, as the web's Editors (ACP) card: the switches, the setup text for each editor, and the
/// sessions the hub holds.
fn editors_card(state: &State) -> Element<'_, Msg> {
    let enabled = state.acp_enabled.unwrap_or(false);
    // Without an on_toggle the checkbox is greyed out: the write-back switch waits for editors to be on.
    let mut write_back = checkbox(state.acp_write_back.unwrap_or(false))
        .label("Offer to apply an agent's change to the editor's project");
    if enabled {
        write_back = write_back.on_toggle(Msg::SetAcpWriteBack);
    }
    // The v2 switch is never greyed out: the engines inside the VMs speak v2 too, so Editors being off does not matter.
    let v2 = checkbox(state.acp_v2.unwrap_or(false))
        .label("Speak the ACP v2 draft")
        .on_toggle(Msg::SetAcpV2);
    let mut parts: Vec<Element<'_, Msg>> = vec![
        checkbox(enabled)
            .label("Let editors prompt a plaza agent")
            .on_toggle(Msg::SetAcp)
            .into(),
        ui::faint("Zed, JetBrains IDEs, Neovim and other ACP clients prompt a plaza agent from the editor; its approvals are asked in the editor. Agents never reach your machine: the editor's MCP servers and files stay local.").into(),
        write_back.into(),
        ui::faint("Write-back runs through agentplaza acp --repo, on a lane of its own, mirrored in Weft. The editor writes the files; the agent never does.").into(),
        v2.into(),
        ui::faint("v2 is a draft that still changes, and it has no write-back: the editor's file methods are gone in v2.").into(),
    ];
    if let Some(error) = &state.acp_error {
        parts.push(ui::error_line(error));
    }
    let snippets = editor_snippets();
    parts.push(snippet(
        "Zed",
        &snippets.zed,
        "Add this to the settings.json of Zed. It runs the default plaza agent.",
    ));
    parts.push(snippet(
        "Zed, one agent",
        &snippets.zed_agent,
        "The same, pinned to one plaza agent. Replace <name> with the agent's name.",
    ));
    parts.push(snippet(
        "JetBrains IDEs",
        &snippets.jetbrains,
        "AI Assistant › ACP agents: enter the command and the arguments.",
    ));
    parts.push(snippet(
        "Neovim (CodeCompanion)",
        &snippets.neovim,
        "Under the adapters of the CodeCompanion setup.",
    ));
    parts.push(sessions_table(state));
    ui::card_with(
        "Editors (ACP)",
        vec![ui::btn_sm(
            "Refresh",
            None,
            ui::Kind::Ghost,
            Some(Msg::RefreshAcpSessions),
        )],
        column(parts).spacing(14).width(Length::Fill).into(),
    )
}

fn sessions_table(state: &State) -> Element<'_, Msg> {
    if let Some(error) = &state.acp_sessions_error {
        return ui::empty("triangle-alert", "Could not load the sessions", error);
    }
    match &state.acp_sessions {
        None => ui::loading(),
        Some(list) if list.is_empty() => ui::empty(
            "message-square",
            "No editor sessions",
            "Sessions appear here once an editor prompts a plaza agent.",
        ),
        Some(list) => {
            let p = crate::theme::palette();
            let now = now_ms();
            let rows: Vec<Vec<Element<'_, Msg>>> = list
                .iter()
                .map(|s| -> Vec<Element<'_, Msg>> {
                    vec![
                        ui::sans(session_title(s).to_string(), 12.5, p.ink).into(),
                        ui::mono(s.cid.to_string(), 12.0, p.ink_2).into(),
                        ui::faint(if s.client.is_empty() {
                            "unknown".to_string()
                        } else {
                            s.client.clone()
                        })
                        .into(),
                        ui::mono(ui::ellipsize(&s.cwd, 60), 12.0, p.ink_2).into(),
                        ui::faint(ago(now, s.updated_ms)).into(),
                        if s.closed {
                            ui::badge("closed", ui::Tone::Muted)
                        } else {
                            ui::badge("open", ui::Tone::Ok)
                        },
                    ]
                })
                .collect();
            table(
                &[
                    (3, "Title"),
                    (2, "Agent"),
                    (2, "Client"),
                    (3, "Folder"),
                    (2, "Updated"),
                    (1, "State"),
                ],
                rows,
            )
        }
    }
}

/// The Calls card: the token and tool filters over the call log.
fn calls_card(state: &State) -> Element<'_, Msg> {
    let token_options = token_choices(state.tokens.as_deref().unwrap_or(&[]));
    let selected_token = token_options
        .iter()
        .find(|c| c.value == state.filter_token)
        .cloned();
    let tool_options = tool_choices();
    let selected_tool = tool_options
        .iter()
        .find(|c| c.value == state.filter_tool)
        .cloned();
    let filters: Element<'_, Msg> = row![
        field(
            "Token",
            pick_list(token_options, selected_token, Msg::FilterToken)
                .style(crate::theme::pick_list)
                .into(),
            None,
        ),
        field(
            "Tool",
            pick_list(tool_options, selected_tool, Msg::FilterTool)
                .style(crate::theme::pick_list)
                .into(),
            None,
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into();
    let body: Element<'_, Msg> = if let Some(error) = &state.calls_error {
        ui::empty("triangle-alert", "Could not load the calls", error)
    } else {
        match &state.calls {
            None => ui::loading(),
            Some(list) if list.is_empty() => ui::empty(
                "message-square",
                "No calls",
                "Calls appear here once a client uses a token.",
            ),
            Some(list) => calls_table(state, list),
        }
    };
    ui::card(
        "Calls",
        column![filters, body]
            .spacing(14)
            .width(Length::Fill)
            .into(),
    )
}

fn calls_table<'a>(state: &'a State, calls: &'a [HubCall]) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let rows: Vec<Vec<Element<'a, Msg>>> = calls
        .iter()
        .map(|call| -> Vec<Element<'a, Msg>> {
            let result: Element<'a, Msg> = if call.ok {
                ui::badge("ok", ui::Tone::Ok)
            } else {
                let mut parts: Vec<Element<'a, Msg>> = vec![ui::badge("error", ui::Tone::Danger)];
                if let Some(error) = call.error.as_deref().filter(|e| !e.is_empty()) {
                    parts.push(ui::faint(ui::ellipsize(error, 120)).into());
                }
                row(parts).spacing(8).align_y(Alignment::Center).into()
            };
            vec![
                ui::faint(stamp(call.at_ms)).into(),
                ui::faint(name_of(state, &call.token_id)).into(),
                ui::mono(call.tool.clone(), 12.0, p.ink).into(),
                result,
                ui::faint(format!("{} ms", call.duration_ms)).into(),
            ]
        })
        .collect();
    table(
        &[
            (3, "Time"),
            (3, "Token"),
            (3, "Tool"),
            (4, "Result"),
            (2, "Duration"),
        ],
        rows,
    )
}

/// A moment as the calls log shows it: the UTC date and the time, to the second.
fn stamp(ms: u64) -> String {
    let day = ms % DAY_MS;
    format!(
        "{} {:02}:{:02}:{:02}",
        date_text(ms),
        day / 3_600_000,
        (day / 60_000) % 60,
        (day / 1_000) % 60
    )
}

// ---- Load-free helpers the tests cover --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://hub.example.test/mcp";
    const TOKEN: &str = "apz_mcp_test";

    #[test]
    fn mcp_hub_setup_snippets_match_the_web_strings_and_drop_the_token_in_open_mode() {
        // Original: mcp_hub_setup_snippets_match_the_web_strings
        {
            let s = setup_snippets(URL, Some(TOKEN));
            assert_eq!(
                s.claude_code,
                "claude mcp add --transport http agentplaza https://hub.example.test/mcp --header \"Authorization: Bearer apz_mcp_test\""
            );
            assert_eq!(
                s.desktop_login.as_deref(),
                Some("agentplaza mcp login --hub https://hub.example.test")
            );
            assert_eq!(
                s.generic,
                "URL: https://hub.example.test/mcp\nAuthorization: Bearer apz_mcp_test"
            );
            let desktop: Value =
                serde_json::from_str(&s.desktop).expect("the desktop block is JSON");
            assert_eq!(
                desktop["mcpServers"]["agentplaza"]["args"],
                json!(["mcp", "serve", "--hub", "https://hub.example.test"])
            );
            assert!(
                !s.desktop.contains(TOKEN),
                "the desktop block never carries the token"
            );
        }
        // Original: mcp_hub_open_mode_setup_drops_the_token
        {
            let s = setup_snippets(URL, None);
            assert_eq!(
                s.claude_code,
                "claude mcp add --transport http agentplaza https://hub.example.test/mcp"
            );
            assert_eq!(s.desktop_login, None);
            assert_eq!(s.generic, "URL: https://hub.example.test/mcp");
        }
    }

    #[test]
    fn mcp_hub_scopes_permit_their_own_tools_and_the_ones_below() {
        assert!(Scope::Observe.permits(Scope::Observe));
        assert!(!Scope::Observe.permits(Scope::Operate));
        assert!(Scope::Operate.permits(Scope::Observe));
        assert!(!Scope::Operate.permits(Scope::Govern));
        assert!(Scope::Govern.permits(Scope::Operate));
        assert_eq!(tool_scope("create_task"), Some(Scope::Operate));
        assert_eq!(tool_scope("skill_publish"), Some(Scope::Govern));
        assert_eq!(tool_scope("no_such_tool"), None);
        let total: usize = Scope::ALL.iter().map(|scope| tools_of(*scope).len()).sum();
        assert_eq!(total, 29);
    }

    #[test]
    fn mcp_hub_tool_lists_are_the_hub_registry() {
        // The hub's registry, in its order (crates/apid/src/hub_mcp/tools/mod.rs). This crate does not depend on apid,
        // so this pins the same names and scopes; a change to the registry must change this list too.
        let registry: [(&str, Scope); 29] = [
            ("plaza_status", Scope::Observe),
            ("list_agents", Scope::Observe),
            ("get_agent", Scope::Observe),
            ("get_transcript", Scope::Observe),
            ("list_tasks", Scope::Observe),
            ("get_task", Scope::Observe),
            ("list_board", Scope::Observe),
            ("list_projects", Scope::Observe),
            ("get_project", Scope::Observe),
            ("list_approvals", Scope::Observe),
            ("list_machines", Scope::Observe),
            ("treasury_summary", Scope::Observe),
            ("usage_summary", Scope::Observe),
            ("list_laws", Scope::Observe),
            ("search_library", Scope::Observe),
            ("prompts_list", Scope::Observe),
            ("prompt_get", Scope::Observe),
            ("skills_list", Scope::Observe),
            ("skill_get", Scope::Observe),
            ("message_orchestrator", Scope::Operate),
            ("tell_agent", Scope::Operate),
            ("stop_agent", Scope::Operate),
            ("restart_agent", Scope::Operate),
            ("create_task", Scope::Operate),
            ("prompt_propose", Scope::Operate),
            ("skill_propose", Scope::Operate),
            ("request_decision", Scope::Govern),
            ("prompt_publish", Scope::Govern),
            ("skill_publish", Scope::Govern),
        ];
        let mut listed: Vec<&str> = Scope::ALL
            .into_iter()
            .flat_map(|scope| tools_of(scope).iter().copied())
            .collect();
        let mut pinned: Vec<&str> = registry.iter().map(|(name, _)| *name).collect();
        listed.sort_unstable();
        pinned.sort_unstable();
        assert_eq!(listed, pinned);
        for (name, scope) in registry {
            assert_eq!(tool_scope(name), Some(scope), "{name}");
        }
        for gone in ["post_board", "propose_agent", "propose_law_change"] {
            assert_eq!(tool_scope(gone), None, "{gone} is not a tool of the hub");
        }
    }

    #[test]
    fn mcp_hub_changing_scope_drops_the_tools_above_it() {
        let mut picked: BTreeSet<String> = ["plaza_status", "create_task", "request_decision"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        keep_tools_within(&mut picked, Scope::Operate);
        assert_eq!(
            picked.into_iter().collect::<Vec<_>>(),
            vec!["create_task".to_string(), "plaza_status".to_string()]
        );
    }

    #[test]
    fn mcp_hub_status_and_tokens_parse_from_the_owner_api() {
        let status: HubStatus = serde_json::from_str(
            r#"{"enabled":true,"bind":"remote","auth_mode":"open","open_scope":"observe","warning":"open mode","tls":true,
                "allowed_origins":["https://console.test"],"effective_origins":["https://console.test"],
                "url":"https://hub.test/mcp","tokens":2,"stored":false}"#,
        )
        .expect("status parses");
        assert!(status.enabled);
        assert_eq!(Auth::from_wire(&status.auth_mode), Auth::Open);
        assert_eq!(Bind::from_wire(&status.bind), Bind::Remote);
        assert!(
            status.tls,
            "the status says whether the endpoint serves TLS"
        );
        assert_eq!(
            status.allowed_origins,
            vec!["https://console.test".to_string()]
        );
        assert!(!status.stored);
        let list: TokenList = serde_json::from_str(
            r#"{"tokens":[{"id":"mcp_1","name":"laptop","scope":"operate","tools":[],"created_ms":1,"expires_ms":null,
                "last_used_ms":0,"calls":3,"revoked_at_ms":null}]}"#,
        )
        .expect("tokens parse");
        assert_eq!(list.tokens[0].name, "laptop");
        assert_eq!(list.tokens[0].expires_ms, None);
        assert_eq!(list.tokens[0].calls, 3);
    }

    #[test]
    fn mcp_hub_calls_parse_and_filter_by_token_and_tool() {
        let calls: CallList = serde_json::from_str(
            r#"{"calls":[{"token_id":"mcp_1","client_ip":null,"tool":"list_agents","args_sha256":"00","ok":false,
                "error":"denied","duration_ms":4,"at_ms":9}]}"#,
        )
        .expect("calls parse");
        assert_eq!(calls.calls[0].tool, "list_agents");
        assert!(!calls.calls[0].ok);
        assert_eq!(calls.calls[0].error.as_deref(), Some("denied"));
        assert_eq!(encode("mcp_0a1b"), "mcp_0a1b");
        assert_eq!(encode("a b&c"), "a%20b%26c");
    }

    #[test]
    fn mcp_hub_editor_snippets_are_the_pasted_text() {
        let s = editor_snippets();
        assert_eq!(
            s.zed,
            r#"{"agent_servers": {"AgentPlaza": {"command": "agentplaza", "args": ["acp"]}}}"#
        );
        assert_eq!(s.jetbrains, "Command: agentplaza\nArguments: acp");
        assert_eq!(
            s.neovim,
            r#"adapters = { acp = { agentplaza = { command = "agentplaza", args = { "acp" } } } }"#
        );
        let named: Value = serde_json::from_str(&s.zed_agent).expect("the named block is JSON");
        assert_eq!(
            named["agent_servers"]["AgentPlaza"]["args"],
            json!(["acp", "--agent", "<name>"])
        );
    }

    #[test]
    fn mcp_hub_sessions_parse_from_the_editor_api() {
        let raw = json!([
            {"id": "acp-0123456789abcdef", "cid": 7, "cwd": "/work/app", "client": "zed 0.200",
             "title": "fix the build", "works": [3, 4], "created_ms": 1000, "updated_ms": 2000, "closed": false},
            {"id": "acp-fedcba9876543210", "cid": 8, "cwd": "", "client": "", "title": "  ",
             "works": [], "created_ms": 1000, "updated_ms": 3000, "closed": true}
        ]);
        let list = parse_sessions(raw).expect("sessions parse");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].cid, 7);
        assert_eq!(session_title(&list[0]), "fix the build");
        assert_eq!(session_title(&list[1]), "untitled");
        assert!(list[1].closed);
        assert!(parse_sessions(json!({"error": "nope"})).is_err());
    }

    #[test]
    fn mcp_hub_leaving_the_tab_drops_the_token() {
        let mut state = State {
            reveal: Some(Reveal {
                token: TOKEN.to_string(),
                name: "laptop".to_string(),
                snippets: setup_snippets(URL, Some(TOKEN)),
            }),
            ..Default::default()
        };
        leave(&mut state);
        assert!(state.reveal.is_none());
    }
}
