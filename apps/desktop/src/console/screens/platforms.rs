//! The Git platforms screen (parity with the web console's platforms view): the plaza's authorizations on GitHub,
//! GitLab and Forgejo, what each may use (its namespaces and repositories), and the connections of granted repositories
//! to projects. Adding an authorization starts a flow in the browser, polled until the browser comes back.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use iced::widget::{Space, checkbox, column, pick_list, row, text_input};
use iced::{Alignment, Element, Subscription, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Id, history};
use crate::console::client::{self, enc};
use crate::console::ui::{self, INK, INK_2, Kind, Tone};
use crate::console::{Ctx, events};
use crate::theme;

/// How often a started flow is asked how it went.
const FLOW_POLL: Duration = Duration::from_secs(2);

// ---- The hub's shapes (`GET /v1/platforms` and the answers around it; only the fields this screen reads) ------

/// `GET /v1/platforms`: the kinds, the registered apps, the authorizations, and the redirect URI.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Listing {
    pub kinds: Vec<KindRow>,
    pub registrations: Vec<Registration>,
    pub integrations: Vec<Integration>,
    pub callback: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct KindRow {
    pub kind: String,
    pub default_host: String,
}

/// An app the plaza is registered as on a host: the plaza's GitHub App (`app` set) or an OAuth app the owner made.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Registration {
    pub id: String,
    pub kind: String,
    pub host: String,
    pub app: Option<AppRow>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AppRow {
    pub name: String,
    pub owner: String,
}

/// The account an authorization is for, or the owner as the platform knows them (the author of exported commits).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Account {
    pub login: String,
    pub name: String,
    pub email: String,
}

/// One authorization of the plaza on a platform.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Integration {
    pub id: String,
    pub kind: String,
    pub host: String,
    /// `app`, `oauth` or `token`.
    pub via: String,
    /// `user`, `org` or `repo`.
    pub level: String,
    pub target: Option<String>,
    /// An installation's choice on GitHub: `all` or `selected`.
    pub repository_selection: Option<String>,
    pub account: Account,
    pub identity: Account,
    /// Set when the authorization needs the owner's attention (uninstalled on the platform, an expired sign-in).
    pub error: Option<String>,
}

/// One namespace an authorization reaches, with the owner's grant (None when the plaza may not use it).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NamespaceRow {
    pub name: String,
    pub kind: String,
    pub display: String,
    pub grant: Option<NsGrant>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NsGrant {
    pub all: bool,
    pub repos: Vec<String>,
}

/// A repository of a namespace, with whether it is granted and where it is connected.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepoRow {
    pub full_name: String,
    pub name: String,
    pub private: bool,
    pub can_push: bool,
    pub granted: bool,
    pub connections: Vec<RepoConnection>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepoConnection {
    pub weft_repo: String,
    pub project: String,
    pub connection: String,
    pub state: String,
}

/// A project a repository can join (`GET /v1/projects`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ProjectChoice {
    pub id: String,
    pub title: String,
}

/// `GET /v1/weft/repos`: each Weft repository and its connections. A platform connection carries the authorization it
/// came through in `credential` (`platform:<id>`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WeftList {
    pub repos: Vec<WeftRepo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WeftRepo {
    pub connections: Vec<WeftConn>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WeftConn {
    pub credential: Option<String>,
}

/// `POST /v1/platforms/{id}/connect`: what the connection made.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConnectAnswer {
    pub author: String,
    /// `{commits, releases}`, `{commits: 0, empty: true}` or `{error}`.
    pub imported: Option<Value>,
    /// `"registered"` or `{error}`.
    pub signing_key: Option<Value>,
}

/// `POST` of a flow start: the state to poll, and the page the browser opens.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Started {
    pub state: String,
    pub go: String,
}

/// `GET /v1/platforms/flows/{state}`: `pending`, `done` (with `detail`), `failed` or `expired`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FlowStatus {
    pub status: String,
    pub detail: Value,
    pub error: Option<String>,
}

// ---- Platforms, levels and labels ----------------------------------------------------------------------------

/// A git platform the screen adds an authorization on. Its card is `Platform::ALL[index]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Github,
    Gitlab,
    Forgejo,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::Github, Platform::Gitlab, Platform::Forgejo];

    /// The hub's name for the kind.
    fn key(self) -> &'static str {
        match self {
            Platform::Github => "github",
            Platform::Gitlab => "gitlab",
            Platform::Forgejo => "forgejo",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Platform::Github => "GitHub",
            Platform::Gitlab => "GitLab",
            Platform::Forgejo => "Forgejo",
        }
    }

    /// The host a card starts with, when the hub names none.
    fn default_host(self) -> &'static str {
        match self {
            Platform::Github => "https://github.com",
            Platform::Gitlab => "https://gitlab.com",
            Platform::Forgejo => "https://codeberg.org",
        }
    }

    fn parse(kind: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.key() == kind)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The level a token or an authorization covers. Its label depends on the platform (a GitLab group, a GitHub
/// organisation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    User,
    Org,
    Repo,
}

impl Scope {
    fn key(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Org => "org",
            Scope::Repo => "repo",
        }
    }

    fn label(self, p: Platform) -> &'static str {
        level_label(p.key(), self.key())
    }
}

/// A level in plain words: "Your account", "Organisation" or "Group", "One repository".
pub fn level_label(kind: &str, level: &str) -> &'static str {
    match level {
        "user" => "Your account",
        "org" if kind == "gitlab" => "Group",
        "org" => "Organisation",
        "repo" => "One repository",
        _ => "Other",
    }
}

/// How an authorization was given, in plain words.
pub fn via_label(via: &str) -> &'static str {
    match via {
        "app" => "GitHub App",
        "oauth" => "Signed in",
        "token" => "Token",
        _ => "Other",
    }
}

/// A namespace's kind in plain words.
fn namespace_kind(kind: &str) -> &'static str {
    match kind {
        "user" => "Account",
        "org" => "Organisation",
        "group" => "Group",
        _ => "Namespace",
    }
}

/// A scope choice in the level picker: the scope and its label for this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeChoice {
    pub scope: Scope,
    pub label: &'static str,
}

impl fmt::Display for ScopeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label)
    }
}

/// The levels a token can be given at, as the picker names them.
fn scope_choices(p: Platform) -> Vec<ScopeChoice> {
    [Scope::User, Scope::Org, Scope::Repo]
        .into_iter()
        .map(|scope| ScopeChoice {
            scope,
            label: match scope {
                Scope::Org => "Organisation or group",
                _ => scope.label(p),
            },
        })
        .collect()
}

impl fmt::Display for ProjectChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.title)
    }
}

/// A host without its scheme, as the owner reads it.
pub fn host_name(host: &str) -> &str {
    host.trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://")
}

/// Whether two hosts are the same one, whatever the trailing slash.
fn same_host(a: &str, b: &str) -> bool {
    a.trim_end_matches('/')
        .eq_ignore_ascii_case(b.trim_end_matches('/'))
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// ---- Pure views of the data ----------------------------------------------------------------------------------

/// The line at the top: how many authorizations, how many repositories are connected, and what needs the owner.
pub fn state_line(authorizations: usize, connected: usize, attention: usize) -> String {
    if authorizations == 0 {
        return "Nothing authorized yet: add GitHub, GitLab or Forgejo.".to_string();
    }
    let mut parts = vec![format!(
        "{}.",
        plural(authorizations as u64, "authorization", "authorizations")
    )];
    parts.push(if connected == 0 {
        "No repository connected.".to_string()
    } else {
        format!(
            "{}.",
            plural(
                connected as u64,
                "repository connected",
                "repositories connected"
            )
        )
    });
    if attention > 0 {
        let verb = if attention == 1 { "needs" } else { "need" };
        parts.push(format!("{attention} {verb} attention."));
    }
    parts.join(" ")
}

/// The top line for a listing and Weft's repositories.
fn summary(listing: &Listing, weft: &[WeftRepo]) -> String {
    let attention = listing
        .integrations
        .iter()
        .filter(|i| warning(i).is_some())
        .count();
    state_line(listing.integrations.len(), connected_repos(weft), attention)
}

/// The platform an authorization's Weft connection came through: its id, from `platform:<id>`.
fn platform_of(conn: &WeftConn) -> Option<&str> {
    conn.credential
        .as_deref()
        .and_then(|c| c.strip_prefix("platform:"))
}

/// How many Weft repositories connect to a platform, per authorization id.
pub fn connected_by_authorization(weft: &[WeftRepo]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for repo in weft {
        let through: BTreeSet<&str> = repo.connections.iter().filter_map(platform_of).collect();
        for id in through {
            *counts.entry(id.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// How many Weft repositories connect to any platform.
fn connected_repos(weft: &[WeftRepo]) -> usize {
    weft.iter()
        .filter(|repo| repo.connections.iter().any(|c| platform_of(c).is_some()))
        .count()
}

/// The authorizations grouped by host, in host order.
pub fn by_host(list: &[Integration]) -> BTreeMap<&str, Vec<&Integration>> {
    let mut groups: BTreeMap<&str, Vec<&Integration>> = BTreeMap::new();
    for integration in list {
        groups
            .entry(integration.host.as_str())
            .or_default()
            .push(integration);
    }
    groups
}

/// An authorization's warning, when the hub gave one.
fn warning(i: &Integration) -> Option<&str> {
    i.error.as_deref().filter(|e| !e.is_empty())
}

fn connected_text(n: usize) -> String {
    if n == 0 {
        "No repository connected".to_string()
    } else {
        plural(n as u64, "repository connected", "repositories connected")
    }
}

/// What an installation reaches on the platform, in plain words.
fn selection_text(selection: &str) -> &'static str {
    match selection {
        "all" => "Installed on every repository",
        "selected" => "Installed on chosen repositories",
        _ => "",
    }
}

/// Who exports are authored by, on the platform.
pub fn identity_text(a: &Account) -> String {
    let who = if a.name.is_empty() {
        a.login.as_str()
    } else {
        a.name.as_str()
    };
    if a.email.is_empty() {
        format!("Exports as {who}.")
    } else {
        format!("Exports as {who} <{}>.", a.email)
    }
}

/// The grants a namespace has in the draft: all of its repositories, or the ones named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NsDraft {
    pub all: bool,
    pub repos: BTreeSet<String>,
}

/// The draft grants from the namespaces' saved grants, keeping only the namespaces the plaza may use.
pub fn draft_of(list: &[NamespaceRow]) -> BTreeMap<String, NsDraft> {
    list.iter()
        .filter_map(|ns| {
            let grant = ns.grant.as_ref()?;
            let draft = NsDraft {
                all: grant.all,
                repos: grant.repos.iter().cloned().collect(),
            };
            (draft.all || !draft.repos.is_empty()).then(|| (ns.name.clone(), draft))
        })
        .collect()
}

/// The owner's toggle of a namespace's "All repositories".
pub fn set_all(draft: &mut BTreeMap<String, NsDraft>, ns: &str, on: bool) {
    draft.entry(ns.to_string()).or_default().all = on;
}

/// The owner's tick of one repository of a namespace.
pub fn set_repo(draft: &mut BTreeMap<String, NsDraft>, ns: &str, repo: &str, on: bool) {
    let entry = draft.entry(ns.to_string()).or_default();
    if on {
        entry.repos.insert(repo.to_string());
    } else {
        entry.repos.remove(repo);
    }
}

/// The body of `PUT /v1/platforms/{id}/grants`: every namespace the plaza keeps. A namespace with All has no repository
/// list; one with nothing ticked is left out.
pub fn grants_body(draft: &BTreeMap<String, NsDraft>) -> Value {
    let mut namespaces = serde_json::Map::new();
    for (name, d) in draft {
        if !d.all && d.repos.is_empty() {
            continue;
        }
        let repos: Vec<&String> = if d.all {
            Vec::new()
        } else {
            d.repos.iter().collect()
        };
        namespaces.insert(name.clone(), json!({ "all": d.all, "repos": repos }));
    }
    json!({ "namespaces": namespaces })
}

/// The connect result in plain words: what was connected, what came in, the signing key, and when the commits go out.
pub fn connect_sentence(project: &str, repo: &str, answer: &ConnectAnswer, pushed: bool) -> String {
    let mut parts = vec![format!("{repo} is connected to {project}.")];
    if let Some(imported) = &answer.imported {
        if let Some(error) = imported.get("error").and_then(Value::as_str) {
            parts.push(format!("Its history did not import: {error}."));
        } else if imported.get("empty").and_then(Value::as_bool) == Some(true) {
            parts.push("The repository is empty, so there was no history to import.".to_string());
        } else if let Some(commits) = imported.get("commits").and_then(Value::as_u64) {
            let releases = imported
                .get("releases")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let mut text = format!("Imported {}", plural(commits, "commit", "commits"));
            if releases > 0 {
                text += &format!(" and {}", plural(releases as u64, "release", "releases"));
            }
            parts.push(format!("{text}."));
        }
    }
    match &answer.signing_key {
        Some(key) if key.as_str() == Some("registered") => parts.push(
            "The hub's signing key is registered on the account, so exports show as verified."
                .to_string(),
        ),
        Some(key) => {
            if let Some(error) = key.get("error").and_then(Value::as_str) {
                parts.push(format!("The signing key was not registered: {error}."));
            }
        }
        None => {}
    }
    parts.push(
        if pushed {
            "Pushing now."
        } else {
            "Exports start at the next landing."
        }
        .to_string(),
    );
    if !answer.author.is_empty() {
        let name = answer
            .author
            .split_once(" <")
            .map_or(answer.author.as_str(), |(name, _)| name);
        parts.push(format!("Exports are by {name}."));
    }
    parts.join(" ")
}

/// A finished flow in plain words: how many authorizations it added or updated.
fn flow_done_line(detail: &Value) -> String {
    let n = detail
        .get("integrations")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if n == 0 {
        "Done.".to_string()
    } else {
        format!(
            "Done. {} added or updated.",
            plural(n as u64, "authorization", "authorizations")
        )
    }
}

/// Whether a flow's page may be opened: an https link, or a link to the hub this console talks to (plain HTTP on
/// loopback, which `ui::open_url` refuses), and nothing with a space or a control character.
pub fn browser_link(go: &str, hub: &str) -> bool {
    if go.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let https = go.starts_with("https://") && go.len() > "https://".len();
    let ours = !hub.is_empty() && go.starts_with(&format!("{}/", hub.trim_end_matches('/')));
    https || ours
}

/// Opens a flow's page in the system browser: `open` on macOS, the URL protocol handler on Windows, `xdg-open` elsewhere.
/// The URL is one argument, and only the links `browser_link` accepts are opened.
fn open_browser(go: &str, hub: &str) -> Result<(), String> {
    if !browser_link(go, hub) {
        return Err("the sign-in page is not a link to the hub or to https".to_string());
    }
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    let mut child = command
        .arg(go)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Reap the opener when it exits, so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ---- State and messages --------------------------------------------------------------------------------------

/// One add card's fields. The client secret and the token are typed into secure inputs, and cleared once their
/// request is built.
#[derive(Default)]
pub struct Card {
    /// The host the owner typed; empty means the kind's default.
    host: String,
    /// GitHub: the organisation the App is created for, optional.
    org: String,
    client_id: String,
    client_secret: String,
    token: String,
    scope: Scope,
    target: String,
    token_open: bool,
    error: Option<String>,
}

impl fmt::Debug for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Card")
            .field("host", &self.host)
            .field("scope", &self.scope)
            .field("token_open", &self.token_open)
            .field("secrets", &"<redacted>")
            .finish()
    }
}

/// A started flow, waiting for the browser: its state, and whether a poll of it is on its way.
#[derive(Debug, Clone)]
pub struct Flow {
    id: String,
    in_flight: bool,
}

/// The repositories of one namespace, once asked for.
#[derive(Debug, Clone)]
pub enum RepoList {
    Pending,
    Ready(Vec<RepoRow>),
    Failed(String),
}

/// A repository's connect form: its project, whether to register the hub's signing key, and whether to push now.
#[derive(Debug, Clone)]
pub struct ConnectForm {
    repo: String,
    project: Option<ProjectChoice>,
    sign: bool,
    push: bool,
}

/// The grants view of one authorization: the namespaces, the owner's draft against what is saved, the open
/// namespaces and their repositories.
#[derive(Debug, Default)]
pub struct GrantsView {
    integration: String,
    loaded: bool,
    namespaces: Vec<NamespaceRow>,
    saved: BTreeMap<String, NsDraft>,
    draft: BTreeMap<String, NsDraft>,
    open: BTreeSet<String>,
    repos: BTreeMap<String, RepoList>,
    connect: Option<ConnectForm>,
    error: Option<String>,
}

/// What a load fetched: the listing, and Weft's repositories.
#[derive(Debug, Clone)]
pub struct Snapshot {
    listing: Listing,
    weft: Vec<WeftRepo>,
}

#[derive(Debug, Default)]
pub struct State {
    /// The hub's answer to the last load; None until the first one.
    listing: Option<Listing>,
    weft: Vec<WeftRepo>,
    load_error: Option<String>,
    /// The last action's failure, shown until the next action.
    error: Option<String>,
    /// The last action's line.
    notice: Option<String>,
    /// A write is on its way to the hub.
    busy: bool,
    cards: [Card; 3],
    /// The projects a repository can join; loaded when a grants view opens.
    projects: Vec<ProjectChoice>,
    /// The authorization whose Remove waits for its confirmation.
    confirm_remove: Option<String>,
    /// A sign-in or install in the browser.
    flow: Option<Flow>,
    grants: Option<GrantsView>,
    /// The last connect's line, under its repository: (repository, sentence).
    connected_note: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Listed(Result<Snapshot, String>),
    Host(Platform, String),
    Org(Platform, String),
    ClientId(Platform, String),
    ClientSecret(Platform, String),
    TokenValue(Platform, String),
    TokenTarget(Platform, String),
    TokenScope(Platform, Scope),
    ToggleToken(Platform),
    /// Edits to the read-only redirect URI are dropped.
    Noop,
    CreateApp(Platform),
    Install(String),
    SignIn(String),
    Register(Platform),
    Registered(Platform, Result<Value, String>),
    SubmitToken(Platform),
    TokenSaved(Platform, Result<Value, String>),
    Started(Result<Started, String>),
    FlowTick,
    FlowPolled(String, Result<FlowStatus, String>),
    CancelFlow,
    Sync(String),
    Synced(Result<Value, String>),
    AskRemove(String),
    CancelRemove,
    Remove(String),
    Removed(Result<Value, String>),
    OpenGrants(String),
    CloseGrants,
    NamespacesLoaded(String, Result<Vec<NamespaceRow>, String>),
    ProjectsLoaded(Result<Vec<ProjectChoice>, String>),
    ToggleNamespace(String),
    ReposLoaded(String, String, Result<Vec<RepoRow>, String>),
    SetAll(String, bool),
    SetRepo(String, String, bool),
    SaveGrants,
    GrantsSaved(Result<Value, String>),
    OpenConnect(String),
    CloseConnect,
    ConnectProject(ProjectChoice),
    ConnectSign(bool),
    ConnectPush(bool),
    Connect,
    Connected {
        repo: String,
        project: String,
        pushed: bool,
        result: Result<ConnectAnswer, String>,
    },
    Disconnect(String, String),
    Disconnected(Result<Value, String>),
    /// A hub event: a platform change, or a Weft one that changes a connection, reloads the screen.
    Event(events::Event),
}

/// The navigation this message asks for: none, the screen keeps its own state.
pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

/// Drops the secrets the owner typed (a client secret, a token) when the screen is left.
pub fn leave(state: &mut State) {
    for card in &mut state.cards {
        card.client_secret.clear();
        card.token.clear();
    }
    state.confirm_remove = None;
}

// ---- Loading -----------------------------------------------------------------------------------------------

/// Loads the authorizations and Weft's repositories (called when the screen opens and on the shell's tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    refresh(ctx)
}

fn refresh(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let listing = client.get::<Listing>("/v1/platforms")?;
            // Without Weft there are no connections to count.
            let weft = client
                .get::<WeftList>("/v1/weft/repos")
                .map(|list| list.repos)
                .unwrap_or_default();
            Ok(Snapshot { listing, weft })
        },
        Msg::Listed,
    )
}

fn namespaces_task(ctx: &Ctx, integration: &str) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/platforms/{}/namespaces", enc(integration));
    let id = integration.to_string();
    client::task(
        move || client.get::<Vec<NamespaceRow>>(&path),
        move |res| Msg::NamespacesLoaded(id.clone(), res),
    )
}

fn repos_task(ctx: &Ctx, integration: &str, namespace: &str) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!(
        "/v1/platforms/{}/repos?namespace={}",
        enc(integration),
        enc(namespace)
    );
    let (id, ns) = (integration.to_string(), namespace.to_string());
    client::task(
        move || client.get::<Vec<RepoRow>>(&path),
        move |res| Msg::ReposLoaded(id.clone(), ns.clone(), res),
    )
}

fn projects_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<Vec<ProjectChoice>>("/v1/projects"),
        Msg::ProjectsLoaded,
    )
}

/// Reloads the repositories of every open namespace, and the authorizations (after a connect or a disconnect).
fn reload_repos(state: &State, ctx: &Ctx) -> Task<Msg> {
    let mut tasks = vec![refresh(ctx)];
    if let Some(view) = &state.grants {
        for ns in &view.open {
            tasks.push(repos_task(ctx, &view.integration, ns));
        }
    }
    Task::batch(tasks)
}

/// Starts a flow or a write: the hub's `{state, go}` answer comes back as `Started`.
fn start(ctx: &Ctx, path: &str, body: Value) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = path.to_string();
    client::task(move || client.post::<Started>(&path, &body), Msg::Started)
}

/// A write is starting: the old line goes.
fn begin(state: &mut State) {
    state.busy = true;
    state.error = None;
    state.notice = None;
}

/// A press that does something only when `enabled`, and nothing while a write is on its way.
fn press(enabled: bool, msg: Msg) -> Option<Msg> {
    enabled.then_some(msg)
}

// ---- Update ------------------------------------------------------------------------------------------------

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Listed(Ok(snapshot)) => {
            state.load_error = None;
            state.listing = Some(snapshot.listing);
            state.weft = snapshot.weft;
            Task::none()
        }
        Msg::Listed(Err(e)) => {
            state.load_error = Some(e);
            Task::none()
        }
        Msg::Host(p, v) => {
            state.cards[p.index()].host = v;
            Task::none()
        }
        Msg::Org(p, v) => {
            state.cards[p.index()].org = v;
            Task::none()
        }
        Msg::ClientId(p, v) => {
            state.cards[p.index()].client_id = v;
            Task::none()
        }
        Msg::ClientSecret(p, v) => {
            state.cards[p.index()].client_secret = v;
            Task::none()
        }
        Msg::TokenValue(p, v) => {
            state.cards[p.index()].token = v;
            Task::none()
        }
        Msg::TokenTarget(p, v) => {
            state.cards[p.index()].target = v;
            Task::none()
        }
        Msg::TokenScope(p, scope) => {
            state.cards[p.index()].scope = scope;
            Task::none()
        }
        Msg::ToggleToken(p) => {
            let card = &mut state.cards[p.index()];
            card.token_open = !card.token_open;
            if !card.token_open {
                card.token.clear();
                card.target.clear();
                card.error = None;
            }
            Task::none()
        }
        Msg::Noop => Task::none(),
        Msg::CreateApp(p) => {
            if state.busy {
                return Task::none();
            }
            let host = host_of(state, p);
            let org = state.cards[p.index()].org.trim().to_string();
            let mut body = json!({ "host": host });
            if !org.is_empty() {
                body["org"] = json!(org);
            }
            begin(state);
            start(ctx, "/v1/platforms/github/app", body)
        }
        Msg::Install(registration) => {
            if state.busy {
                return Task::none();
            }
            begin(state);
            start(
                ctx,
                "/v1/platforms/github/install",
                json!({ "registration": registration }),
            )
        }
        Msg::SignIn(registration) => {
            if state.busy {
                return Task::none();
            }
            begin(state);
            start(
                ctx,
                "/v1/platforms/authorize",
                json!({ "registration": registration }),
            )
        }
        Msg::Register(p) => {
            if state.busy {
                return Task::none();
            }
            let host = host_of(state, p);
            let card = &mut state.cards[p.index()];
            let body = json!({
                "kind": p.key(),
                "host": host,
                "client_id": card.client_id.trim(),
                "client_secret": card.client_secret.trim(),
            });
            // The secret leaves the screen with its request.
            card.client_secret.clear();
            card.error = None;
            begin(state);
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Value>("/v1/platforms/registrations", &body),
                move |res| Msg::Registered(p, res),
            )
        }
        Msg::Registered(p, Ok(_)) => {
            state.busy = false;
            let card = &mut state.cards[p.index()];
            card.client_id.clear();
            card.client_secret.clear();
            refresh(ctx)
        }
        Msg::Registered(p, Err(e)) => {
            state.busy = false;
            state.cards[p.index()].error = Some(e);
            Task::none()
        }
        Msg::SubmitToken(p) => {
            if state.busy {
                return Task::none();
            }
            let host = host_of(state, p);
            let card = &mut state.cards[p.index()];
            let mut body = json!({
                "kind": p.key(),
                "host": host,
                "token": card.token.trim(),
                "level": card.scope.key(),
            });
            if card.scope != Scope::User {
                body["target"] = json!(card.target.trim());
            }
            // The token leaves the screen with its request.
            card.token.clear();
            card.error = None;
            begin(state);
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Value>("/v1/platforms/tokens", &body),
                move |res| Msg::TokenSaved(p, res),
            )
        }
        Msg::TokenSaved(p, Ok(_)) => {
            state.busy = false;
            let card = &mut state.cards[p.index()];
            card.token_open = false;
            card.target.clear();
            card.error = None;
            state.notice = Some(format!("{} token saved.", p.label()));
            refresh(ctx)
        }
        Msg::TokenSaved(p, Err(e)) => {
            state.busy = false;
            state.cards[p.index()].error = Some(e);
            Task::none()
        }
        Msg::Started(Ok(answer)) => {
            state.busy = false;
            let hub = ctx.client.base().to_string();
            match open_browser(&answer.go, &hub) {
                Ok(()) => {
                    state.flow = Some(Flow {
                        id: answer.state,
                        in_flight: false,
                    });
                }
                Err(e) => state.error = Some(format!("Could not open the browser: {e}")),
            }
            Task::none()
        }
        Msg::Started(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Msg::FlowTick => {
            let Some(flow) = state.flow.as_mut() else {
                return Task::none();
            };
            if flow.in_flight {
                return Task::none();
            }
            flow.in_flight = true;
            let id = flow.id.clone();
            let client = ctx.client.clone();
            let path = format!("/v1/platforms/flows/{}", enc(&id));
            client::task(
                move || client.get::<FlowStatus>(&path),
                move |res| Msg::FlowPolled(id.clone(), res),
            )
        }
        Msg::FlowPolled(id, res) => {
            if state.flow.as_ref().is_none_or(|f| f.id != id) {
                return Task::none();
            }
            let status = match res {
                Ok(status) => status,
                // A failed poll is tried again on the next tick.
                Err(_) => {
                    if let Some(flow) = state.flow.as_mut() {
                        flow.in_flight = false;
                    }
                    return Task::none();
                }
            };
            match status.status.as_str() {
                "done" => {
                    state.flow = None;
                    state.notice = Some(flow_done_line(&status.detail));
                    refresh(ctx)
                }
                "failed" => {
                    state.flow = None;
                    state.error = Some(match status.error {
                        Some(e) if !e.is_empty() => {
                            format!("Could not finish on the platform: {e}")
                        }
                        _ => "Could not finish on the platform.".to_string(),
                    });
                    Task::none()
                }
                "expired" => {
                    state.flow = None;
                    state.error =
                        Some("The browser step expired after 15 minutes. Start it again.".into());
                    Task::none()
                }
                _ => {
                    if let Some(flow) = state.flow.as_mut() {
                        flow.in_flight = false;
                    }
                    Task::none()
                }
            }
        }
        Msg::CancelFlow => {
            state.flow = None;
            Task::none()
        }
        Msg::Sync(id) => {
            if state.busy {
                return Task::none();
            }
            begin(state);
            let client = ctx.client.clone();
            let path = format!("/v1/platforms/{}/sync", enc(&id));
            client::task(move || client.post::<Value>(&path, &json!({})), Msg::Synced)
        }
        Msg::Synced(Ok(_)) => {
            state.busy = false;
            state.notice = Some("Synced.".to_string());
            refresh(ctx)
        }
        Msg::Synced(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Msg::AskRemove(id) => {
            state.confirm_remove = Some(id);
            Task::none()
        }
        Msg::CancelRemove => {
            state.confirm_remove = None;
            Task::none()
        }
        Msg::Remove(id) => {
            if state.busy {
                return Task::none();
            }
            begin(state);
            let client = ctx.client.clone();
            let path = format!("/v1/platforms/{}", enc(&id));
            client::task(move || client.delete(&path), Msg::Removed)
        }
        Msg::Removed(Ok(_)) => {
            state.busy = false;
            state.confirm_remove = None;
            state.notice = Some("Removed.".to_string());
            refresh(ctx)
        }
        Msg::Removed(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Msg::OpenGrants(id) => {
            state.grants = Some(GrantsView {
                integration: id.clone(),
                ..GrantsView::default()
            });
            Task::batch([namespaces_task(ctx, &id), projects_task(ctx)])
        }
        Msg::CloseGrants => {
            state.grants = None;
            Task::none()
        }
        Msg::NamespacesLoaded(id, res) => {
            let Some(view) = state.grants.as_mut().filter(|v| v.integration == id) else {
                return Task::none();
            };
            view.loaded = true;
            match res {
                Ok(list) => {
                    view.saved = draft_of(&list);
                    view.draft = view.saved.clone();
                    view.namespaces = list;
                    view.error = None;
                }
                Err(e) => view.error = Some(e),
            }
            Task::none()
        }
        Msg::ProjectsLoaded(Ok(list)) => {
            state.projects = list;
            Task::none()
        }
        Msg::ProjectsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::ToggleNamespace(ns) => {
            let Some(view) = state.grants.as_mut() else {
                return Task::none();
            };
            // Shutting a namespace keeps its repositories: opening it again shows them while they are fetched again.
            if view.open.remove(&ns) {
                return Task::none();
            }
            view.open.insert(ns.clone());
            view.repos.entry(ns.clone()).or_insert(RepoList::Pending);
            let integration = view.integration.clone();
            repos_task(ctx, &integration, &ns)
        }
        Msg::ReposLoaded(id, ns, res) => {
            if let Some(view) = state.grants.as_mut().filter(|v| v.integration == id) {
                view.repos.insert(
                    ns,
                    match res {
                        Ok(list) => RepoList::Ready(list),
                        Err(e) => RepoList::Failed(e),
                    },
                );
            }
            Task::none()
        }
        Msg::SetAll(ns, on) => {
            if let Some(view) = state.grants.as_mut() {
                set_all(&mut view.draft, &ns, on);
            }
            Task::none()
        }
        Msg::SetRepo(ns, repo, on) => {
            if let Some(view) = state.grants.as_mut() {
                set_repo(&mut view.draft, &ns, &repo, on);
            }
            Task::none()
        }
        Msg::SaveGrants => {
            let Some(view) = state.grants.as_ref() else {
                return Task::none();
            };
            if state.busy || grants_body(&view.draft) == grants_body(&view.saved) {
                return Task::none();
            }
            let body = grants_body(&view.draft);
            let path = format!("/v1/platforms/{}/grants", enc(&view.integration));
            begin(state);
            let client = ctx.client.clone();
            client::task(
                move || client.call("PUT", &path, Some(&body)),
                Msg::GrantsSaved,
            )
        }
        Msg::GrantsSaved(Ok(_)) => {
            state.busy = false;
            state.notice = Some("Saved.".to_string());
            let Some(view) = state.grants.as_ref() else {
                return refresh(ctx);
            };
            Task::batch([
                namespaces_task(ctx, &view.integration),
                reload_repos(state, ctx),
            ])
        }
        Msg::GrantsSaved(Err(e)) => {
            state.busy = false;
            if let Some(view) = state.grants.as_mut() {
                view.error = Some(e);
            }
            Task::none()
        }
        Msg::OpenConnect(repo) => {
            if let Some(view) = state.grants.as_mut() {
                view.connect = Some(ConnectForm {
                    repo,
                    project: None,
                    sign: true,
                    push: false,
                });
            }
            Task::none()
        }
        Msg::CloseConnect => {
            if let Some(view) = state.grants.as_mut() {
                view.connect = None;
            }
            Task::none()
        }
        Msg::ConnectProject(project) => {
            if let Some(form) = state.grants.as_mut().and_then(|v| v.connect.as_mut()) {
                form.project = Some(project);
            }
            Task::none()
        }
        Msg::ConnectSign(on) => {
            if let Some(form) = state.grants.as_mut().and_then(|v| v.connect.as_mut()) {
                form.sign = on;
            }
            Task::none()
        }
        Msg::ConnectPush(on) => {
            if let Some(form) = state.grants.as_mut().and_then(|v| v.connect.as_mut()) {
                form.push = on;
            }
            Task::none()
        }
        Msg::Connect => {
            if state.busy {
                return Task::none();
            }
            let Some(view) = state.grants.as_ref() else {
                return Task::none();
            };
            let Some(form) = view.connect.as_ref() else {
                return Task::none();
            };
            let Some(project) = form.project.clone() else {
                return Task::none();
            };
            let body = json!({
                "repo": form.repo,
                "project": project.id,
                "sign": form.sign,
                "export": form.push,
            });
            let path = format!("/v1/platforms/{}/connect", enc(&view.integration));
            let (repo, pushed, title) = (form.repo.clone(), form.push, project.title.clone());
            begin(state);
            let client = ctx.client.clone();
            client::task(
                move || client.post::<ConnectAnswer>(&path, &body),
                move |result| Msg::Connected {
                    repo: repo.clone(),
                    project: title.clone(),
                    pushed,
                    result,
                },
            )
        }
        Msg::Connected {
            repo,
            project,
            pushed,
            result,
        } => {
            state.busy = false;
            match result {
                Ok(answer) => {
                    state.connected_note = Some((
                        repo.clone(),
                        connect_sentence(&project, &repo, &answer, pushed),
                    ));
                    if let Some(view) = state.grants.as_mut() {
                        view.connect = None;
                    }
                    reload_repos(state, ctx)
                }
                Err(e) => {
                    state.connected_note = Some((repo.clone(), format!("Could not connect: {e}")));
                    Task::none()
                }
            }
        }
        Msg::Disconnect(weft_repo, connection) => {
            if state.busy {
                return Task::none();
            }
            let Some(view) = state.grants.as_ref() else {
                return Task::none();
            };
            let path = format!("/v1/platforms/{}/disconnect", enc(&view.integration));
            let body = json!({ "weft_repo": weft_repo, "connection": connection });
            begin(state);
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Value>(&path, &body),
                Msg::Disconnected,
            )
        }
        Msg::Disconnected(Ok(_)) => {
            state.busy = false;
            state.notice = Some("Disconnected.".to_string());
            reload_repos(state, ctx)
        }
        Msg::Disconnected(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Msg::Event(event) => match event.kind.as_str() {
            "platform" | "weft_change" | "weft_queue" | "weft_land" | "weft_release"
            | "weft_export" | "weft_op" => refresh(ctx),
            _ => Task::none(),
        },
    }
}

/// The host a card means: what the owner typed, or the kind's default.
fn host_of(state: &State, p: Platform) -> String {
    let typed = state.cards[p.index()].host.trim();
    if typed.is_empty() {
        state
            .listing
            .as_ref()
            .and_then(|l| l.kinds.iter().find(|k| k.kind == p.key()))
            .map(|k| k.default_host.trim_end_matches('/').to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| p.default_host().to_string())
    } else {
        typed.trim_end_matches('/').to_string()
    }
}

/// The app a card is about on a host: the GitHub App (`app`), or an OAuth app the owner registered.
fn registration<'a>(
    state: &'a State,
    p: Platform,
    host: &str,
    app: bool,
) -> Option<&'a Registration> {
    state
        .listing
        .as_ref()?
        .registrations
        .iter()
        .find(|r| r.kind == p.key() && same_host(&r.host, host) && r.app.is_some() == app)
}

pub fn subscription(state: &State, ctx: &Ctx) -> Subscription<Msg> {
    let events = events::subscription(&ctx.client).map(Msg::Event);
    if state.flow.is_some() {
        Subscription::batch([events, iced::time::every(FLOW_POLL).map(|_| Msg::FlowTick)])
    } else {
        events
    }
}

// ---- View --------------------------------------------------------------------------------------------------

/// The line under the head: the state, in one sentence.
fn lede<'a>(line: String) -> Element<'a, Msg> {
    ui::prose(line).size(15.0).color(INK_2).into()
}

/// Rows with a hairline between each.
fn rules<'a>(rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::with_capacity(rows.len() * 2);
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            items.push(ui::hairline());
        }
        items.push(r);
    }
    column(items).spacing(10).into()
}

/// A line of warning text, in the warning tone.
fn warn_text<'a>(line: String) -> Element<'a, Msg> {
    ui::faint(line).color(theme::palette().warn).into()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    match &state.listing {
        Some(listing) => parts.push(lede(summary(listing, &state.weft))),
        None => parts.push(ui::loading()),
    }
    if let Some(e) = &state.load_error {
        parts.push(ui::error_line(e));
    }
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        parts.push(ui::faint(n.clone()).into());
    }
    if state.flow.is_some() {
        parts.push(
            row![
                ui::faint("Waiting for the platform…"),
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelFlow)),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    let cards: Vec<Element<'a, Msg>> = Platform::ALL
        .into_iter()
        .map(|p| add_card(state, p))
        .collect();
    parts.push(ui::card_grid(cards));
    if let Some(listing) = &state.listing
        && !listing.integrations.is_empty()
    {
        parts.push(authorizations(state, listing));
    }
    if let Some(grants) = grants_card(state) {
        parts.push(grants);
    }
    ui::sheet(
        ui::view_head("Git platforms", vec![]),
        column(parts).spacing(18).into(),
    )
}

/// One "Add" card: the host, then the platform's own way in (an app to create, or a sign-in), and a token.
fn add_card<'a>(state: &'a State, p: Platform) -> Element<'a, Msg> {
    let card = &state.cards[p.index()];
    let host = host_of(state, p);
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::label("Host").into(),
        text_input(p.default_host(), &card.host)
            .on_input(move |v| Msg::Host(p, v))
            .padding(8)
            .into(),
    ];
    if p == Platform::Github {
        items.extend(github_part(state, p, &host));
    } else {
        items.extend(oauth_part(state, p, &host));
    }
    items.push(token_part(state, p));
    if let Some(e) = &card.error {
        items.push(ui::error_line(e));
    }
    ui::card(
        &format!("Add {}", p.label()),
        column(items).spacing(10).into(),
    )
}

/// GitHub: the plaza's App, created once for the host, then installed on an account or an organisation.
fn github_part<'a>(state: &'a State, p: Platform, host: &str) -> Vec<Element<'a, Msg>> {
    let card = &state.cards[p.index()];
    match registration(state, p, host, true) {
        Some(reg) => {
            let owner = reg.app.as_ref().map(|a| {
                if a.owner.is_empty() {
                    a.name.clone()
                } else {
                    format!("{}, owned by {}", a.name, a.owner)
                }
            });
            vec![
                ui::faint(format!(
                    "The plaza's GitHub App: {}.",
                    owner.unwrap_or_default()
                ))
                .into(),
                ui::btn(
                    "Install on an account or organisation",
                    None,
                    Kind::Primary,
                    press(!state.busy, Msg::Install(reg.id.clone())),
                ),
            ]
        }
        None => vec![
            ui::faint(
                "Create the plaza's GitHub App once for this host. An organisation can own it.",
            )
            .into(),
            ui::label("Organisation (optional)").into(),
            text_input("Organisation", &card.org)
                .on_input(move |v| Msg::Org(p, v))
                .padding(8)
                .into(),
            ui::btn(
                "Create the GitHub App",
                None,
                Kind::Primary,
                press(!state.busy, Msg::CreateApp(p)),
            ),
        ],
    }
}

/// GitLab and Forgejo: an OAuth app the owner registers on the host (with the redirect URI), then a sign-in.
fn oauth_part<'a>(state: &'a State, p: Platform, host: &str) -> Vec<Element<'a, Msg>> {
    if let Some(reg) = registration(state, p, host, false) {
        return vec![ui::btn(
            "Sign in",
            None,
            Kind::Primary,
            press(!state.busy, Msg::SignIn(reg.id.clone())),
        )];
    }
    let card = &state.cards[p.index()];
    let callback = state.listing.as_ref().map_or("", |l| l.callback.as_str());
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::faint("Register an OAuth app on the host, with this redirect URI:").into(),
        // Read-only, and selectable: the owner copies it into the platform.
        text_input("", callback)
            .on_input(|_| Msg::Noop)
            .padding(8)
            .into(),
    ];
    if p == Platform::Gitlab {
        items.push(ui::faint("Tick the scopes api and read_user.").into());
    }
    let ready = !card.client_id.trim().is_empty() && !card.client_secret.trim().is_empty();
    items.push(ui::label("Client ID").into());
    items.push(
        text_input("Client ID", &card.client_id)
            .on_input(move |v| Msg::ClientId(p, v))
            .padding(8)
            .into(),
    );
    items.push(ui::label("Client secret").into());
    items.push(
        text_input("Client secret", &card.client_secret)
            .secure(true)
            .on_input(move |v| Msg::ClientSecret(p, v))
            .padding(8)
            .into(),
    );
    items.push(ui::btn(
        "Register",
        None,
        Kind::Primary,
        press(ready && !state.busy, Msg::Register(p)),
    ));
    items
}

/// Where a token's level says its target goes.
fn target_hint(p: Platform, scope: Scope) -> &'static str {
    match scope {
        Scope::User => "",
        Scope::Org if p == Platform::Gitlab => "Group path, such as jumphash/tools",
        Scope::Org => "Organisation, such as JumpHash",
        Scope::Repo => "Repository as owner/name",
    }
}

/// "Use a token": a secure input, the level, and the target when the level is not the owner's own account.
fn token_part<'a>(state: &'a State, p: Platform) -> Element<'a, Msg> {
    let card = &state.cards[p.index()];
    if !card.token_open {
        return ui::btn(
            "Use a token",
            None,
            Kind::Secondary,
            Some(Msg::ToggleToken(p)),
        );
    }
    let choices = scope_choices(p);
    let selected = choices.iter().copied().find(|c| c.scope == card.scope);
    let target_ok = card.scope == Scope::User || !card.target.trim().is_empty();
    let ready = !card.token.trim().is_empty() && target_ok;
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::label("Token").into(),
        text_input("Token", &card.token)
            .secure(true)
            .on_input(move |v| Msg::TokenValue(p, v))
            .padding(8)
            .into(),
        row![
            ui::label("Level"),
            pick_list(choices, selected, move |c: ScopeChoice| {
                Msg::TokenScope(p, c.scope)
            }),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    ];
    if card.scope != Scope::User {
        items.push(
            text_input(target_hint(p, card.scope), &card.target)
                .on_input(move |v| Msg::TokenTarget(p, v))
                .padding(8)
                .into(),
        );
    }
    items.push(
        row![
            ui::btn(
                "Save token",
                None,
                Kind::Primary,
                press(ready && !state.busy, Msg::SubmitToken(p)),
            ),
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::ToggleToken(p))),
        ]
        .spacing(10)
        .into(),
    );
    column(items).spacing(10).into()
}

/// The authorizations, one card per host.
fn authorizations<'a>(state: &'a State, listing: &'a Listing) -> Element<'a, Msg> {
    let counts = connected_by_authorization(&state.weft);
    let cards: Vec<Element<'a, Msg>> = by_host(&listing.integrations)
        .into_iter()
        .map(|(host, rows)| {
            let title = format!(
                "{} · {}",
                rows.first()
                    .and_then(|i| Platform::parse(&i.kind))
                    .map_or("Platform", Platform::label),
                host_name(host)
            );
            let items: Vec<Element<'a, Msg>> = rows
                .into_iter()
                .map(|i| auth_row(state, i, counts.get(&i.id).copied().unwrap_or(0)))
                .collect();
            ui::card(&title, rules(items))
        })
        .collect();
    column(cards).spacing(16).into()
}

/// One authorization: whose it is, its level and how it was given, what it connects, the identity exports carry, a
/// warning when there is one, and its actions.
fn auth_row<'a>(state: &'a State, i: &'a Integration, connected: usize) -> Element<'a, Msg> {
    let id = i.id.clone();
    let mut head: Vec<Element<'a, Msg>> = vec![
        ui::sans(i.account.login.clone(), 14.0, INK)
            .font(ui::SANS_SEMIBOLD)
            .into(),
    ];
    if !i.account.name.is_empty() {
        head.push(ui::faint(i.account.name.clone()).into());
    }
    head.push(ui::badge(level_label(&i.kind, &i.level), Tone::Muted));
    head.push(ui::faint(via_label(&i.via)).into());
    let mut lines: Vec<Element<'a, Msg>> =
        vec![row(head).spacing(12).align_y(Alignment::Center).into()];
    if i.level == "repo"
        && let Some(target) = i.target.as_deref().filter(|t| !t.is_empty())
    {
        lines.push(ui::faint(format!("Repository {target}")).into());
    }
    let mut facts = vec![connected_text(connected)];
    let selection = selection_text(i.repository_selection.as_deref().unwrap_or(""));
    if !selection.is_empty() {
        facts.push(selection.to_string());
    }
    lines.push(ui::faint(facts.join(" · ")).into());
    if i.identity.login.is_empty() {
        lines.push(warn_text(format!(
            "No account signed in on {}: exports need an author. Sign in on the platform.",
            host_name(&i.host)
        )));
    } else {
        lines.push(ui::faint(identity_text(&i.identity)).into());
    }
    if let Some(e) = warning(i) {
        lines.push(
            row![
                ui::badge("Needs attention", Tone::Warn),
                warn_text(e.to_string())
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    lines.push(
        row![
            ui::btn(
                "Choose what the plaza may use",
                None,
                Kind::Secondary,
                Some(Msg::OpenGrants(id.clone())),
            ),
            ui::btn(
                "Sync",
                None,
                Kind::Ghost,
                press(!state.busy, Msg::Sync(id.clone())),
            ),
            remove_part(state, &id),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
    );
    column(lines).spacing(8).into()
}

/// Remove: one press asks, the next forgets the authorization and its tokens.
fn remove_part<'a>(state: &'a State, id: &str) -> Element<'a, Msg> {
    if state.confirm_remove.as_deref() == Some(id) {
        row![
            ui::faint("Forget this authorization and its tokens?"),
            ui::btn(
                "Remove",
                None,
                Kind::Danger,
                press(!state.busy, Msg::Remove(id.to_string())),
            ),
            ui::btn("Keep", None, Kind::Ghost, Some(Msg::CancelRemove)),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    } else {
        ui::btn(
            "Remove",
            None,
            Kind::DangerOutline,
            Some(Msg::AskRemove(id.to_string())),
        )
    }
}

/// The grants view of the open authorization: its namespaces, each with "All repositories" and, when opened, its
/// repositories to tick. Its Save sends the whole map.
fn grants_card<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let view = state.grants.as_ref()?;
    let integration = state
        .listing
        .as_ref()?
        .integrations
        .iter()
        .find(|i| i.id == view.integration)?;
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::faint("Tick All repositories for a namespace, or open it and tick its repositories one by one.")
            .into(),
    ];
    if let Some(e) = &view.error {
        items.push(ui::error_line(e));
    }
    if !view.loaded {
        items.push(ui::loading());
    } else {
        let rows: Vec<Element<'a, Msg>> = view
            .namespaces
            .iter()
            .map(|ns| namespace_block(state, view, ns))
            .collect();
        items.push(rules(rows));
    }
    let dirty = grants_body(&view.draft) != grants_body(&view.saved);
    let note: Element<'a, Msg> = if dirty {
        ui::faint("Unsaved changes").into()
    } else {
        Space::new().into()
    };
    items.push(
        row![
            ui::btn(
                "Save",
                None,
                Kind::Primary,
                press(view.loaded && dirty && !state.busy, Msg::SaveGrants),
            ),
            note,
            Space::new().width(iced::Length::Fill),
            ui::btn("Close", None, Kind::Ghost, Some(Msg::CloseGrants)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    );
    Some(ui::card(
        &format!(
            "What the plaza may use · {} on {}",
            integration.account.login,
            host_name(&integration.host)
        ),
        column(items).spacing(12).into(),
    ))
}

/// One namespace: its disclosure, its kind, and "All repositories"; under it, when open, its repositories.
fn namespace_block<'a>(
    state: &'a State,
    view: &'a GrantsView,
    ns: &'a NamespaceRow,
) -> Element<'a, Msg> {
    let open = view.open.contains(&ns.name);
    let all = view.draft.get(&ns.name).is_some_and(|d| d.all);
    let name = ns.name.clone();
    let mut head: Vec<Element<'a, Msg>> = vec![ui::disclosure(
        ns.name.clone(),
        theme::palette().ink,
        open,
        Msg::ToggleNamespace(name.clone()),
    )];
    if !ns.display.is_empty() {
        head.push(ui::faint(ns.display.clone()).into());
    }
    head.push(ui::faint(namespace_kind(&ns.kind)).into());
    head.push(Space::new().width(iced::Length::Fill).into());
    head.push(
        checkbox(all)
            .label("All repositories")
            .text_size(13.0)
            .on_toggle(move |on| Msg::SetAll(name.clone(), on))
            .into(),
    );
    let head = row(head).spacing(12).align_y(Alignment::Center);
    let mut items: Vec<Element<'a, Msg>> = vec![head.into()];
    if open {
        items.push(repo_list(state, view, ns));
    }
    column(items).spacing(10).into()
}

/// A namespace's repositories: each ticked (or all, under All), with its marks, its connections, and its connect form.
fn repo_list<'a>(state: &'a State, view: &'a GrantsView, ns: &'a NamespaceRow) -> Element<'a, Msg> {
    match view.repos.get(&ns.name) {
        None | Some(RepoList::Pending) => ui::loading(),
        Some(RepoList::Failed(e)) => ui::error_line(e),
        Some(RepoList::Ready(list)) if list.is_empty() => ui::faint("No repositories here.").into(),
        Some(RepoList::Ready(list)) => {
            rules(list.iter().map(|r| repo_row(state, view, ns, r)).collect())
        }
    }
}

fn repo_row<'a>(
    state: &'a State,
    view: &'a GrantsView,
    ns: &'a NamespaceRow,
    r: &'a RepoRow,
) -> Element<'a, Msg> {
    let draft = view.draft.get(&ns.name);
    let all = draft.is_some_and(|d| d.all);
    let picked = all || draft.is_some_and(|d| d.repos.contains(&r.name));
    let mut check = checkbox(picked).label(r.name.clone()).text_size(13.0);
    if !all {
        let (ns_name, repo_name) = (ns.name.clone(), r.name.clone());
        check = check.on_toggle(move |on| Msg::SetRepo(ns_name.clone(), repo_name.clone(), on));
    }
    let mut marks: Vec<Element<'a, Msg>> = vec![check.into()];
    if r.private {
        marks.push(ui::badge("Private", Tone::Muted));
    }
    if !r.can_push {
        marks.push(ui::badge("Can't push", Tone::Warn));
    }
    let mut lines: Vec<Element<'a, Msg>> =
        vec![row(marks).spacing(12).align_y(Alignment::Center).into()];
    if r.granted {
        for c in &r.connections {
            let project = if c.project.is_empty() {
                c.weft_repo.as_str()
            } else {
                c.project.as_str()
            };
            lines.push(
                row![
                    ui::faint(format!(
                        "{project} · {}",
                        history::connection_label(&c.state)
                    )),
                    ui::btn_sm(
                        "Disconnect",
                        None,
                        Kind::Ghost,
                        press(
                            !state.busy,
                            Msg::Disconnect(c.weft_repo.clone(), c.connection.clone()),
                        ),
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .into(),
            );
        }
        match view.connect.as_ref().filter(|f| f.repo == r.full_name) {
            Some(form) => lines.push(connect_form(state, form)),
            None => lines.push(ui::btn_sm(
                "Connect to a project",
                None,
                Kind::Secondary,
                press(!state.busy, Msg::OpenConnect(r.full_name.clone())),
            )),
        }
    } else {
        lines.push(ui::faint("Not granted yet: save the choice above to connect it.").into());
    }
    if let Some((repo, note)) = &state.connected_note
        && repo == &r.full_name
    {
        lines.push(ui::faint(note.clone()).into());
    }
    column(lines).spacing(8).padding([10, 0]).into()
}

/// The connect form: the project to join, whether to register the hub's signing key (on by default), and whether to
/// push now (off by default).
fn connect_form<'a>(state: &'a State, form: &'a ConnectForm) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![
        row![
            ui::label("Project"),
            pick_list(
                state.projects.clone(),
                form.project.clone(),
                Msg::ConnectProject,
            )
            .placeholder("Choose a project"),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    ];
    if state.projects.is_empty() {
        items.push(ui::faint("No projects yet. Make one first.").into());
    }
    items.push(
        checkbox(form.sign)
            .label("Register the hub's signing key")
            .text_size(13.0)
            .on_toggle(Msg::ConnectSign)
            .into(),
    );
    items.push(
        checkbox(form.push)
            .label("Push now")
            .text_size(13.0)
            .on_toggle(Msg::ConnectPush)
            .into(),
    );
    items.push(
        row![
            ui::btn(
                "Connect",
                None,
                Kind::Primary,
                press(form.project.is_some() && !state.busy, Msg::Connect),
            ),
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CloseConnect)),
        ]
        .spacing(10)
        .into(),
    );
    column(items).spacing(10).padding([4, 0]).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn integration(id: &str, host: &str, error: Option<&str>) -> Integration {
        Integration {
            id: id.into(),
            kind: "github".into(),
            host: host.into(),
            level: "org".into(),
            error: error.map(str::to_string),
            ..Integration::default()
        }
    }

    #[test]
    fn the_state_line_says_what_is_authorized_and_what_needs_the_owner() {
        assert_eq!(
            state_line(0, 0, 0),
            "Nothing authorized yet: add GitHub, GitLab or Forgejo."
        );
        assert_eq!(
            state_line(2, 1, 0),
            "2 authorizations. 1 repository connected."
        );
        assert_eq!(
            state_line(1, 0, 1),
            "1 authorization. No repository connected. 1 needs attention."
        );
        assert_eq!(
            state_line(3, 2, 2),
            "3 authorizations. 2 repositories connected. 2 need attention."
        );
    }

    #[test]
    fn levels_and_ways_read_in_plain_words_per_platform() {
        assert_eq!(level_label("github", "user"), "Your account");
        assert_eq!(level_label("github", "org"), "Organisation");
        assert_eq!(level_label("gitlab", "org"), "Group");
        assert_eq!(level_label("forgejo", "repo"), "One repository");
        assert_eq!(Scope::Org.label(Platform::Gitlab), "Group");
        assert_eq!(via_label("app"), "GitHub App");
        assert_eq!(via_label("oauth"), "Signed in");
        assert_eq!(via_label("token"), "Token");
        assert_eq!(host_name("https://github.com/"), "github.com");
        assert!(same_host("https://GitLab.com/", "https://gitlab.com"));
    }

    #[test]
    fn authorizations_group_by_host_and_connections_count_per_authorization() {
        let list = vec![
            integration("github-1", "https://github.com", None),
            integration("github-2", "https://github.com", None),
            integration("forgejo-1", "https://codeberg.org", Some("expired")),
        ];
        let groups = by_host(&list);
        assert_eq!(
            groups.keys().copied().collect::<Vec<_>>(),
            vec!["https://codeberg.org", "https://github.com"]
        );
        assert_eq!(groups["https://github.com"].len(), 2);

        // Two repositories through github-1, one through forgejo-1 and one plain git remote (no platform).
        let weft: Vec<WeftRepo> = serde_json::from_value(json!([
            {"connections": [{"credential": "platform:github-1"}]},
            {"connections": [{"credential": "platform:github-1"}, {"credential": "platform:github-1"}]},
            {"connections": [{"credential": "platform:forgejo-1"}]},
            {"connections": [{"credential": null}]}
        ]))
        .expect("weft repos of the hub's shape");
        let counts = connected_by_authorization(&weft);
        assert_eq!(counts.get("github-1"), Some(&2));
        assert_eq!(counts.get("forgejo-1"), Some(&1));
        assert_eq!(counts.get("github-2"), None);
        assert_eq!(connected_repos(&weft), 3);
    }

    #[test]
    fn the_grants_body_is_built_from_the_toggles() {
        let namespaces: Vec<NamespaceRow> = serde_json::from_value(json!([
            {"name": "irfan", "kind": "user", "display": "Irfan S", "grant": {"all": false, "repos": ["notes"]}},
            {"name": "JumpHash", "kind": "org", "display": "JumpHash", "grant": {"all": true, "repos": []}},
            {"name": "other", "kind": "user", "display": "", "grant": null}
        ]))
        .expect("namespaces of the hub's shape");
        let saved = draft_of(&namespaces);
        assert_eq!(saved.len(), 2, "a namespace without a grant is not kept");

        // The owner ticks two repositories, unticks notes, and turns All off for JumpHash.
        let mut draft = saved.clone();
        set_repo(&mut draft, "irfan", "notes", false);
        set_repo(&mut draft, "irfan", "blog", true);
        set_repo(&mut draft, "irfan", "tools", true);
        set_all(&mut draft, "JumpHash", false);
        assert_eq!(
            grants_body(&draft),
            json!({"namespaces": {
                "irfan": {"all": false, "repos": ["blog", "tools"]}
            }})
        );

        // All on keeps no repository list; a namespace with nothing ticked is left out.
        let mut draft = saved;
        set_all(&mut draft, "irfan", true);
        set_repo(&mut draft, "JumpHash", "x", true);
        set_repo(&mut draft, "JumpHash", "x", false);
        assert_eq!(
            grants_body(&draft),
            json!({"namespaces": {
                "irfan": {"all": true, "repos": []},
                "JumpHash": {"all": true, "repos": []}
            }})
        );
        assert_eq!(grants_body(&BTreeMap::new()), json!({"namespaces": {}}));
    }

    #[test]
    fn the_connect_result_reads_in_plain_words() {
        let answer: ConnectAnswer = serde_json::from_value(json!({
            "weft_repo": "pr1-notes", "connection": "github", "remote": "https://github.com/irfan/notes.git",
            "branch": "main", "author": "Irfan S <irfan@x.test>",
            "imported": {"commits": 12, "releases": ["v0.1.0"]},
            "signing_key": "registered"
        }))
        .expect("a connect answer of the hub's shape");
        assert_eq!(
            connect_sentence("Plaza one", "irfan/notes", &answer, false),
            "irfan/notes is connected to Plaza one. Imported 12 commits and 1 release. The hub's signing key is registered on the account, so exports show as verified. Exports start at the next landing. Exports are by Irfan S."
        );

        let empty: ConnectAnswer = serde_json::from_value(json!({
            "author": "", "imported": {"commits": 0, "empty": true}, "signing_key": {"error": "no key"}
        }))
        .expect("a connect answer");
        assert_eq!(
            connect_sentence("Plaza one", "irfan/new", &empty, true),
            "irfan/new is connected to Plaza one. The repository is empty, so there was no history to import. The signing key was not registered: no key. Pushing now."
        );

        let failed: ConnectAnswer = serde_json::from_value(json!({
            "imported": {"error": "remote refused"}
        }))
        .expect("a connect answer");
        assert_eq!(
            connect_sentence("Plaza one", "irfan/old", &failed, false),
            "irfan/old is connected to Plaza one. Its history did not import: remote refused. Exports start at the next landing."
        );
    }

    #[test]
    fn a_flow_opens_only_a_secure_link_or_the_hub_this_console_talks_to() {
        let hub = "http://127.0.0.1:7700";
        assert!(browser_link(
            "http://127.0.0.1:7700/v1/platforms/flows/a/go",
            hub
        ));
        assert!(browser_link("https://github.com/apps/new", ""));
        assert!(!browser_link(
            "http://evil.test/v1/platforms/flows/a/go",
            hub
        ));
        assert!(!browser_link("file:///etc/passwd", hub));
        assert!(!browser_link("https://github.com/\nrm", hub));
        assert!(!browser_link("https://", hub));
    }

    #[test]
    fn a_finished_flow_says_how_many_authorizations_it_touched() {
        assert_eq!(
            flow_done_line(&json!({"integrations": ["github-2"]})),
            "Done. 1 authorization added or updated."
        );
        assert_eq!(
            flow_done_line(&json!({"integrations": ["a", "b"]})),
            "Done. 2 authorizations added or updated."
        );
        assert_eq!(flow_done_line(&Value::Null), "Done.");
    }

    #[test]
    fn the_listing_of_the_hub_parses_with_its_registrations_and_authorizations() {
        let listing: Listing = serde_json::from_value(json!({
            "kinds": [{"kind": "github", "label": "GitHub", "default_host": "https://github.com"}],
            "registrations": [{"id": "github-app-1", "kind": "github", "host": "https://github.com", "client_id": "Iv23li",
                "app": {"id": 42, "slug": "agentplaza-3f9c2a", "name": "AgentPlaza 3f9c2a", "owner": "irfan", "html_url": "https://github.com/apps/agentplaza-3f9c2a"}}],
            "integrations": [{"id": "github-2", "kind": "github", "host": "https://github.com", "via": "app", "level": "org",
                "repository_selection": "selected", "target": "JumpHash",
                "account": {"id": 9, "login": "JumpHash", "name": "JumpHash", "email": "", "avatar_url": ""},
                "grants": {"namespaces": {"JumpHash": {"all": true, "repos": []}}},
                "error": null, "label": "GitHub",
                "identity": {"id": 5, "login": "irfan", "name": "Irfan S", "email": "irfan@x.test", "avatar_url": ""}}],
            "callback": "http://127.0.0.1:7700/v1/platforms/callback"
        }))
        .expect("a listing of the hub's shape");
        assert_eq!(listing.integrations[0].account.login, "JumpHash");
        assert_eq!(
            identity_text(&listing.integrations[0].identity),
            "Exports as Irfan S <irfan@x.test>."
        );
        assert_eq!(
            listing.registrations[0]
                .app
                .as_ref()
                .map(|a| a.owner.as_str()),
            Some("irfan")
        );
        assert_eq!(
            summary(&listing, &[]),
            "1 authorization. No repository connected."
        );
    }

    #[test]
    fn a_card_never_shows_its_secrets() {
        let card = Card {
            client_secret: "hunter2".into(),
            token: "ghp_secret".into(),
            ..Card::default()
        };
        let shown = format!("{card:?}");
        assert!(!shown.contains("hunter2") && !shown.contains("ghp_secret"));
    }
}
