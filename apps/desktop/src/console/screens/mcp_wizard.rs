//! The MCP add and edit wizard (parity item 5): the desktop twin of the web's mcp-wizard.ts,
//! mcp-wizard-secrets.ts and mcp-wizard-test.ts, with the same six steps, labels and checks.
//! An env or header secret is a vault secret name bound to this server as `mcp:<id>`; a plain
//! setting is a literal. A secret value only passes through the create form, which clears it on
//! submit. Step 3 runs the test on the node the owner picks. Steps 4 and 5 edit the tool picks,
//! risk levels, roles and nodes, and Next saves them as the web does. Cancel asks before it drops
//! unsaved changes.

use std::collections::BTreeMap;

use iced::widget::{checkbox, column, row, text_input};
use iced::{Element, Length, Task};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::mcp::Preset;
use crate::console::{client, ui, Ctx};

const STEP_LABELS: [&str; 6] = ["Source", "Credentials", "Test", "Tools", "Access", "Enable"];
const SECRET_WORDS: [&str; 4] = ["TOKEN", "KEY", "SECRET", "PASSWORD"];
/// The fields this wizard does not edit; they go back to the hub as the hub had them.
const KEPT: [&str; 3] = ["limits", "allow_private", "storage"];
/// The risk levels a tool can be given; the empty value means auto (the hub's own).
const RISKS: [&str; 3] = ["read", "write", "irreversible"];
const TOOL_MODE: [(bool, &str); 2] = [(true, "Allow all tools"), (false, "Pick tools")];
const ROLE_MODE: [(bool, &str); 2] = [(true, "All roles"), (false, "Pick roles")];
const NODE_MODE: [(bool, &str); 2] = [(true, "All nodes"), (false, "Pick nodes")];
const ID_HINT: &str = "The ID must be 2 to 32 characters: lower-case letters, digits and hyphens, starting with a letter.";
const NAME_HINT: &str = "Use 2 to 64 characters: a capital letter, then capitals, digits or underscores.";

// ---- Shapes -----------------------------------------------------------------------------------

/// A credential or setting a preset needs (the hub's `McpSpec`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub name: String,
    pub secret: bool,
    pub required: bool,
    pub help: String,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Create,
    Edit,
}

/// A vault secret by name (a header's `format` wraps it, e.g. "Bearer {}"), or a plain literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    Secret { secret: String, format: Option<String> },
    Value(String),
}

impl Setting {
    fn to_json(&self) -> Value {
        match self {
            Setting::Secret { secret, format: Some(f) } => json!({ "secret": secret, "format": f }),
            Setting::Secret { secret, format: None } => json!({ "secret": secret }),
            Setting::Value(v) => json!({ "value": v }),
        }
    }

    fn from_json(v: &Value) -> Option<Setting> {
        if let Some(secret) = v.get("secret").and_then(Value::as_str) {
            let format = v.get("format").and_then(Value::as_str).map(str::to_string);
            return Some(Setting::Secret { secret: secret.to_string(), format });
        }
        v.get("value").and_then(Value::as_str).map(|s| Setting::Value(s.to_string()))
    }
}

/// Where the server comes from (step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Preset(String),
    Custom,
    Remote,
}

/// A tool, role or node pick as the hub has it: everything ("all"), or the named ones (an empty list is nobody).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    All,
    Only(Vec<String>),
}

impl Pick {
    fn from_json(v: Option<&Value>) -> Pick {
        match v {
            Some(Value::String(s)) if s == "all" => Pick::All,
            Some(Value::Array(list)) => Pick::Only(list.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            _ => Pick::Only(Vec::new()),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Pick::All => json!("all"),
            Pick::Only(names) => json!(names),
        }
    }

    fn has(&self, name: &str) -> bool {
        match self {
            Pick::All => true,
            Pick::Only(names) => names.iter().any(|n| n == name),
        }
    }

    fn nobody(&self) -> bool {
        matches!(self, Pick::Only(names) if names.is_empty())
    }

    /// The pick as the owner reads it: "all", the names, or "nobody" when empty.
    fn text(&self) -> String {
        match self {
            Pick::All => "all".to_string(),
            Pick::Only(names) if names.is_empty() => "nobody".to_string(),
            Pick::Only(names) => names.join(", "),
        }
    }
}

/// Adds or removes a name from a pick that lists names; an "all" pick is left alone.
fn pick_edit(pick: &mut Pick, name: String, on: bool) {
    if let Pick::Only(names) = pick {
        if on {
            if !names.contains(&name) {
                names.push(name);
            }
        } else {
            names.retain(|n| *n != name);
        }
    }
}

/// The config being built. Field names follow the hub's `McpServerConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// False for a container (stdio), true for a remote URL (http).
    pub http: bool,
    pub image: String,
    /// The command as typed; split on spaces when sent.
    pub command: String,
    pub workdir: Option<String>,
    pub url: String,
    pub env: BTreeMap<String, Setting>,
    pub headers: BTreeMap<String, Setting>,
    /// The tools agents may call (step 4).
    pub tools: Pick,
    /// A risk override per tool name; a missing name is auto.
    pub tool_risk: BTreeMap<String, String>,
    /// The roles that may use the server (step 5).
    pub roles: Pick,
    /// The nodes the server runs on (step 5).
    pub nodes: Pick,
    /// Limits, allow_private and storage as the hub has them.
    pub kept: Map<String, Value>,
}

impl Draft {
    /// A new server: disabled, nobody can use it until the owner says so (the web's blankConfig).
    pub fn blank() -> Draft {
        let mut kept = Map::new();
        kept.insert("limits".into(), json!({ "timeout_s": 60, "max_output_kb": 256, "idle_stop_min": 10 }));
        kept.insert("allow_private".into(), json!(false));
        kept.insert("storage".into(), json!({ "volume": false }));
        Draft {
            id: String::new(),
            name: String::new(),
            enabled: false,
            http: false,
            image: String::new(),
            command: String::new(),
            workdir: None,
            url: String::new(),
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            tools: Pick::All,
            tool_risk: BTreeMap::new(),
            roles: Pick::Only(Vec::new()),
            nodes: Pick::Only(Vec::new()),
            kept,
        }
    }

    /// The draft of an existing server, from the hub's view of it.
    pub fn from_view(v: &Value) -> Draft {
        let t = v.get("transport").cloned().unwrap_or(Value::Null);
        let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let bag = |key: &str| -> BTreeMap<String, Setting> {
            v.get(key)
                .and_then(Value::as_object)
                .map(|o| o.iter().filter_map(|(k, s)| Setting::from_json(s).map(|x| (k.clone(), x))).collect())
                .unwrap_or_default()
        };
        let mut kept = Map::new();
        for key in KEPT {
            if let Some(x) = v.get(key) {
                kept.insert(key.to_string(), x.clone());
            }
        }
        let tools = v.get("tools");
        let access = v.get("access");
        Draft {
            id: text(v, "id"),
            name: text(v, "name"),
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            http: text(&t, "kind") == "http",
            image: text(&t, "image"),
            command: t
                .get("command")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
            workdir: t.get("workdir").and_then(Value::as_str).map(str::to_string),
            url: text(&t, "url"),
            env: bag("env"),
            headers: bag("headers"),
            tools: Pick::from_json(tools.and_then(|x| x.get("allow"))),
            tool_risk: tools
                .and_then(|x| x.get("risk"))
                .and_then(Value::as_object)
                .map(|o| o.iter().filter_map(|(k, r)| r.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                .unwrap_or_default(),
            roles: Pick::from_json(access.and_then(|x| x.get("roles"))),
            nodes: Pick::from_json(access.and_then(|x| x.get("nodes"))),
            kept,
        }
    }

    fn transport_json(&self) -> Value {
        if self.http {
            return json!({ "kind": "http", "url": self.url });
        }
        let parts: Vec<&str> = self.command.split_whitespace().collect();
        let mut t = json!({ "kind": "stdio", "image": self.image });
        if !parts.is_empty() {
            t["command"] = json!(parts);
        }
        if let Some(w) = &self.workdir {
            t["workdir"] = json!(w);
        }
        t
    }

    /// Settings as the hub takes them. A secret with no vault name yet is not sent, as on the web.
    fn settings_json(bag: &BTreeMap<String, Setting>) -> Value {
        Value::Object(
            bag.iter()
                .filter(|(_, s)| !matches!(s, Setting::Secret { secret, .. } if secret.is_empty()))
                .map(|(k, s)| (k.clone(), s.to_json()))
                .collect(),
        )
    }

    /// The body of PATCH /v1/mcp/servers/{id}.
    pub fn patch_body(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            "enabled": self.enabled,
            "transport": self.transport_json(),
            "env": Self::settings_json(&self.env),
            "headers": Self::settings_json(&self.headers),
            "tools": { "allow": self.tools.to_json(), "risk": self.tool_risk },
            "access": { "roles": self.roles.to_json(), "nodes": self.nodes.to_json() },
        });
        for (key, value) in &self.kept {
            body[key.as_str()] = value.clone();
        }
        body
    }

    /// The body of POST /v1/mcp/servers: the whole config, created disabled.
    pub fn create_body(&self) -> Value {
        let mut body = self.patch_body();
        body["id"] = json!(self.id);
        body["enabled"] = json!(false);
        body
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    /// True for a header, false for an env setting.
    pub header: bool,
    pub spec: Spec,
}

/// The vault as the wizard last loaded it.
#[derive(Debug, Clone)]
pub enum Vault {
    Loading,
    Note(String),
    Ready(Vec<VaultEntry>),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultEntry {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub bindings: Vec<String>,
}

/// A tool the hub reports, from a test (`agent_name` and the hub's own `auto_risk`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProbeTool {
    pub name: String,
    pub description: String,
    pub agent_name: String,
    pub auto_risk: String,
}

/// A tool row in step 4: from a passing test, or the hub's list of a saved server (`risk` there is its auto value).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ToolRow {
    pub name: String,
    pub tool: String,
    pub description: String,
    #[serde(rename = "risk")]
    pub auto: String,
}

/// A role in step 5 (the hub's `GET /v1/roles`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoleChoice {
    pub id: String,
    pub title: String,
}

/// A node in step 3 and step 5 (the hub's `GET /v1/nodes`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NodeChoice {
    pub id: String,
    pub name: String,
    pub revoked: bool,
}

/// The hub itself, which is always the first node.
fn hub_node() -> NodeChoice {
    NodeChoice { id: "n0".into(), name: "This hub".into(), revoked: false }
}

/// The hub's answer to a test: `ok: false` comes back as a 200 with `error`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TestResult {
    pub ok: bool,
    pub ms: f64,
    pub error: Option<String>,
    pub server_info: Value,
    pub tools: Vec<ProbeTool>,
}

/// What a save continues with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    Go(usize),
    Test,
    Finish,
}

#[derive(Debug)]
pub struct State {
    pub mode: Mode,
    pub draft: Draft,
    /// The draft as it was when the owner last saved or opened it; a difference is an unsaved change.
    pub baseline: Draft,
    /// True once the server exists on the hub.
    pub saved: bool,
    pub catalog: Vec<Preset>,
    pub source: Option<Source>,
    pub rows: Vec<Row>,
    pub step: usize,
    /// The furthest step reached; the steps up to it can be revisited.
    pub reached: usize,
    pub vault: Vault,
    pub show_all: bool,
    /// Row index to the vault entry the owner picked but has not yet bound to this server.
    pub picked: BTreeMap<usize, usize>,
    /// The row whose create-secret form is open.
    pub creating: Option<usize>,
    pub create_name: String,
    pub create_value: String,
    pub probe: Option<Vec<ProbeTool>>,
    pub test: Option<TestResult>,
    /// The tools step 4 lists; None until a test passes or a saved server's list loads.
    pub tool_rows: Option<Vec<ToolRow>>,
    pub loading_tools: bool,
    /// The node the test runs on ("n0" is this hub).
    pub node: String,
    /// This hub first, then the live nodes.
    pub nodes: Vec<NodeChoice>,
    pub roles: Vec<RoleChoice>,
    /// True while the owner is asked to discard unsaved changes.
    pub confirming: bool,
    pub error: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Vault(Result<Option<Vec<VaultEntry>>, String>),
    Roles(Result<Vec<RoleChoice>, String>),
    Nodes(Result<Vec<NodeChoice>, String>),
    Go(usize),
    Back,
    Next,
    Save(Then),
    Saved(Then, bool, Result<(), String>),
    RunTest,
    Tested(Result<TestResult, String>),
    ToolsLoaded(Result<Vec<ToolRow>, String>),
    PickNode(String),
    PickSource(Source),
    Id(String),
    Name(String),
    Image(String),
    Command(String),
    Url(String),
    Enabled(bool),
    Plain(usize, String),
    PickVault(usize, usize),
    Consent(usize),
    Bound(usize, Result<VaultEntry, String>),
    Clear(usize),
    ShowAll(bool),
    OpenCreate(usize),
    CreateName(String),
    CreateValue(String),
    CreateCancel,
    CreateSubmit,
    Created(usize, Result<VaultEntry, String>),
    ToolsAll(bool),
    ToolPick(String, bool),
    /// A risk for a tool; the empty value is auto.
    ToolRisk(String, String),
    RolesAll(bool),
    RolePick(String, bool),
    NodesAll(bool),
    NodePick(String, bool),
    Cancel,
    Discard,
    KeepEditing,
}

/// What an update leaves for the screen: keep the wizard open with a task, or close it after a save.
pub enum Outcome {
    Stay(Task<Msg>),
    Closed(String),
}

// ---- Helpers ----------------------------------------------------------------------------------

pub fn mcp_binding(id: &str) -> String {
    format!("mcp:{id}")
}

/// A server id: 2 to 32 characters, lower-case letters, digits and hyphens, starting with a letter.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=32).contains(&b.len()) && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A vault secret name: 2 to 64 characters, a capital letter, then capitals, digits or underscores.
pub fn valid_secret_name(name: &str) -> bool {
    let b = name.as_bytes();
    (2..=64).contains(&b.len()) && b[0].is_ascii_uppercase() && b.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

fn is_https(url: &str) -> bool {
    url.strip_prefix("https://").and_then(|r| r.chars().next()).is_some_and(|c| !c.is_whitespace())
}

fn preset_by_id<'a>(catalog: &'a [Preset], id: &str) -> Option<&'a Preset> {
    catalog.iter().find(|p| p.id == id)
}

fn source_for(d: &Draft, catalog: &[Preset]) -> Source {
    let hit = catalog.iter().find(|p| {
        if d.http {
            p.transport == "http" && p.url.as_deref() == Some(d.url.as_str())
        } else {
            p.transport == "stdio" && p.image.as_deref() == Some(d.image.as_str())
        }
    });
    match hit {
        Some(p) => Source::Preset(p.id.clone()),
        None if d.http => Source::Remote,
        None => Source::Custom,
    }
}

fn rows_of(p: &Preset) -> Vec<Row> {
    p.env
        .iter()
        .map(|s| Row { header: false, spec: s.clone() })
        .chain(p.headers.iter().map(|s| Row { header: true, spec: s.clone() }))
        .collect()
}

/// A preset's credentials as empty references: nothing is set until the owner picks or creates one.
fn seed(specs: &[Spec]) -> BTreeMap<String, Setting> {
    specs
        .iter()
        .map(|s| {
            let setting = if s.secret {
                Setting::Secret { secret: String::new(), format: s.format.clone() }
            } else {
                Setting::Value(String::new())
            };
            (s.name.clone(), setting)
        })
        .collect()
}

fn bag_ref(d: &Draft, header: bool) -> &BTreeMap<String, Setting> {
    if header {
        &d.headers
    } else {
        &d.env
    }
}

fn bag_mut(d: &mut Draft, header: bool) -> &mut BTreeMap<String, Setting> {
    if header {
        &mut d.headers
    } else {
        &mut d.env
    }
}

fn set(st: &mut State, row: usize, setting: Option<Setting>) {
    let Some(r) = st.rows.get(row) else { return };
    let (header, name) = (r.header, r.spec.name.clone());
    let bag = bag_mut(&mut st.draft, header);
    match setting {
        Some(s) => {
            bag.insert(name, s);
        }
        None => {
            bag.remove(&name);
        }
    }
}

fn is_set(st: &State, row: &Row) -> bool {
    match bag_ref(&st.draft, row.header).get(&row.spec.name) {
        Some(Setting::Secret { secret, .. }) => !secret.is_empty(),
        Some(Setting::Value(v)) => !v.is_empty(),
        None => false,
    }
}

/// The name rule of the vault picker: the spec's own name, or one of its name tokens, or a secret word.
fn plausible(e: &VaultEntry, spec: &Spec) -> bool {
    if e.name == spec.name {
        return true;
    }
    let mut wants: Vec<&str> = spec.name.split('_').collect();
    wants.extend(SECRET_WORDS.iter().copied());
    e.name.split('_').any(|t| wants.contains(&t))
}

fn transport_text(d: &Draft) -> String {
    if d.http {
        format!("remote · {}", d.url)
    } else {
        format!("container · {}", d.image)
    }
}

fn tools_text(d: &Draft) -> String {
    match &d.tools {
        Pick::All => "all tools".to_string(),
        Pick::Only(names) => format!("{} picked", names.len()),
    }
}

/// The first problem with step 1, or "" when the owner can go on (the web's sourceProblem).
fn source_problem(st: &State) -> String {
    let d = &st.draft;
    let problem = match &st.source {
        None => "Pick a preset, a custom container image or a remote URL.",
        Some(Source::Custom) if d.http || d.image.trim().is_empty() => "Enter a container image.",
        Some(Source::Remote) if !d.http || !is_https(&d.url) => "Enter a remote URL that starts with https://.",
        _ if st.mode == Mode::Create && !valid_id(&d.id) => ID_HINT,
        _ if d.name.trim().is_empty() => "Enter a display name.",
        _ => "",
    };
    problem.to_string()
}

fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy {
        None
    } else {
        Some(msg)
    }
}

fn idle() -> Outcome {
    Outcome::Stay(Task::none())
}

/// Cancel: closes at once when nothing has changed, otherwise asks the owner to discard the changes.
fn cancel(st: &mut State) -> Outcome {
    if st.draft != st.baseline {
        st.confirming = true;
        return idle();
    }
    Outcome::Closed(String::new())
}

// ---- State ------------------------------------------------------------------------------------

impl State {
    /// A wizard for a new server (optionally started on a preset), or for editing `existing`, the hub's view.
    pub fn new(existing: Option<&Value>, preset: Option<&str>, catalog: &[Preset]) -> State {
        let mut st = State {
            mode: Mode::Create,
            draft: Draft::blank(),
            baseline: Draft::blank(),
            saved: false,
            catalog: catalog.to_vec(),
            source: None,
            rows: Vec::new(),
            step: 1,
            reached: 1,
            vault: Vault::Loading,
            show_all: false,
            picked: BTreeMap::new(),
            creating: None,
            create_name: String::new(),
            create_value: String::new(),
            probe: None,
            test: None,
            tool_rows: None,
            loading_tools: false,
            node: "n0".to_string(),
            nodes: vec![hub_node()],
            roles: Vec::new(),
            confirming: false,
            error: String::new(),
            busy: false,
        };
        match existing {
            Some(view) => {
                st.mode = Mode::Edit;
                st.draft = Draft::from_view(view);
                st.saved = true;
                st.source = Some(source_for(&st.draft, catalog));
                st.rows = preset_by_id(catalog, &st.draft.id).map(rows_of).unwrap_or_default();
            }
            None => {
                if let Some(id) = preset {
                    st.choose(Source::Preset(id.to_string()));
                }
            }
        }
        st.baseline = st.draft.clone();
        st
    }

    fn go(&mut self, step: usize) {
        self.step = step;
        self.reached = self.reached.max(step);
        self.error.clear();
    }

    /// Picks the source. A preset seeds its transport, its credentials and the id and name, if empty.
    fn choose(&mut self, source: Source) {
        if self.source.as_ref() == Some(&source) {
            return;
        }
        if matches!(self.source, Some(Source::Preset(_))) {
            // Leaving a preset drops the settings it seeded.
            self.draft.env.clear();
            self.draft.headers.clear();
            self.rows.clear();
        }
        match &source {
            Source::Preset(id) => {
                let Some(p) = preset_by_id(&self.catalog, id) else { return };
                let d = &mut self.draft;
                d.http = p.transport == "http";
                d.image = p.image.clone().unwrap_or_default();
                d.command = p.command.join(" ");
                d.workdir = None;
                d.url = p.url.clone().unwrap_or_default();
                d.env = seed(&p.env);
                d.headers = seed(&p.headers);
                if d.id.is_empty() {
                    d.id = p.id.clone();
                }
                if d.name.is_empty() {
                    d.name = p.name.clone();
                }
                self.rows = rows_of(p);
            }
            Source::Custom => {
                if self.draft.http {
                    self.draft.http = false;
                    self.draft.image.clear();
                }
            }
            Source::Remote => {
                if !self.draft.http {
                    self.draft.http = true;
                    self.draft.url.clear();
                }
            }
        }
        self.source = Some(source);
    }
}

// ---- Tasks ------------------------------------------------------------------------------------

/// Loads the vault for the credentials step (a locked vault is a note, not an error), and the roles and nodes for steps 3 and 5.
pub fn start(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let vault = client::task(
        move || -> Result<Option<Vec<VaultEntry>>, String> {
            let status = client.get::<Value>("/v1/vault")?;
            if status.get("state").and_then(Value::as_str) != Some("unsealed") {
                return Ok(None);
            }
            client.get::<Vec<VaultEntry>>("/v1/vault/secrets").map(Some)
        },
        Msg::Vault,
    );
    let client = ctx.client.clone();
    let roles = client::task(move || client.get::<Vec<RoleChoice>>("/v1/roles"), Msg::Roles);
    let client = ctx.client.clone();
    let nodes = client::task(move || client.get::<Vec<NodeChoice>>("/v1/nodes"), Msg::Nodes);
    Task::batch([vault, roles, nodes])
}

/// Writes the draft: POST when not yet saved (created disabled), then PATCH the editable fields.
/// Returns whether the server now exists, with the outcome of the write.
fn write(client: &client::Client, draft: &Draft, saved: bool) -> (bool, Result<(), String>) {
    let mut now = saved;
    if !saved {
        if let Err(e) = client.post::<Value>("/v1/mcp/servers", &draft.create_body()) {
            return (false, Err(e));
        }
        now = true;
        if !draft.enabled {
            return (true, Ok(()));
        }
    }
    let res = client.patch::<Value>(&format!("/v1/mcp/servers/{}", draft.id), &draft.patch_body()).map(|_| ());
    (now, res)
}

fn save(st: &mut State, ctx: &Ctx, then: Then) -> Outcome {
    st.error.clear();
    st.busy = true;
    let client = ctx.client.clone();
    let (draft, saved) = (st.draft.clone(), st.saved);
    Outcome::Stay(client::task(
        move || -> Result<(bool, Result<(), String>), String> { Ok(write(&client, &draft, saved)) },
        move |r| match r {
            Ok((now, res)) => Msg::Saved(then, now, res),
            Err(e) => Msg::Saved(then, saved, Err(e)),
        },
    ))
}

fn test(st: &mut State, ctx: &Ctx) -> Outcome {
    st.error.clear();
    st.busy = true;
    let id = st.draft.id.clone();
    let body = json!({ "node": st.node });
    let client = ctx.client.clone();
    Outcome::Stay(client::task(
        move || client.post::<TestResult>(&format!("/v1/mcp/servers/{id}/test"), &body),
        Msg::Tested,
    ))
}

/// Step 4 lists a saved server's tools when no test has run in this session.
fn tools_on_enter(st: &mut State, ctx: &Ctx) -> Outcome {
    if st.step != 4 || st.tool_rows.is_some() || st.loading_tools || !st.saved {
        return idle();
    }
    st.loading_tools = true;
    let client = ctx.client.clone();
    let id = st.draft.id.clone();
    Outcome::Stay(client::task(
        move || client.get::<Vec<ToolRow>>(&format!("/v1/mcp/servers/{id}/tools")),
        Msg::ToolsLoaded,
    ))
}

fn bind_task(ctx: &Ctx, entry: VaultEntry, row: usize, binding: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let mut bindings = entry.bindings.clone();
    if !bindings.contains(&binding) {
        bindings.push(binding);
    }
    let path = format!("/v1/vault/secrets/{}", entry.id);
    client::task(move || client.patch::<VaultEntry>(&path, &json!({ "bindings": bindings })), move |r| Msg::Bound(row, r))
}

fn create_task(ctx: &Ctx, name: String, value: String, binding: String, row: usize) -> Task<Msg> {
    let client = ctx.client.clone();
    let body = json!({
        "name": name,
        "kind": "api_key",
        "value": value,
        "scope": { "kind": "plaza" },
        "bindings": [binding],
    });
    client::task(move || client.post::<VaultEntry>("/v1/vault/secrets", &body), move |r| Msg::Created(row, r))
}

// ---- Update -----------------------------------------------------------------------------------

/// Picks a vault entry for a row. A secret already bound to this server is set at once; otherwise
/// the owner is asked to allow the binding first.
fn pick(st: &mut State, row: usize, idx: usize) {
    let Vault::Ready(list) = &st.vault else { return };
    let Some(entry) = list.get(idx).cloned() else { return };
    if entry.bindings.contains(&mcp_binding(&st.draft.id)) {
        st.picked.remove(&row);
        let format = st.rows.get(row).and_then(|r| r.spec.format.clone());
        set(st, row, Some(Setting::Secret { secret: entry.name, format }));
    } else {
        st.picked.insert(row, idx);
    }
}

fn picked_entry(st: &State, row: usize) -> Option<VaultEntry> {
    let idx = *st.picked.get(&row)?;
    match &st.vault {
        Vault::Ready(list) => list.get(idx).cloned(),
        _ => None,
    }
}

fn create(st: &mut State, ctx: &Ctx) -> Outcome {
    let Some(row) = st.creating else { return idle() };
    let name = st.create_name.trim().to_string();
    // The value is cleared on submit, whether or not the check passes.
    let value = std::mem::take(&mut st.create_value);
    if !valid_secret_name(&name) {
        st.error = NAME_HINT.to_string();
        return idle();
    }
    if value.is_empty() {
        st.error = "Enter a value.".to_string();
        return idle();
    }
    st.error.clear();
    st.busy = true;
    Outcome::Stay(create_task(ctx, name, value, mcp_binding(&st.draft.id), row))
}

pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Outcome {
    match msg {
        Msg::Vault(res) => {
            st.vault = match res {
                Ok(Some(list)) => Vault::Ready(list),
                Ok(None) => Vault::Note("Unseal the vault to use secrets.".to_string()),
                Err(e) => Vault::Note(e),
            };
        }
        Msg::Roles(res) => match res {
            Ok(list) => st.roles = list,
            Err(e) => st.error = e,
        },
        Msg::Nodes(res) => match res {
            Ok(list) => {
                st.nodes = std::iter::once(hub_node()).chain(list.into_iter().filter(|n| !n.revoked && n.id != "n0")).collect();
            }
            Err(e) => st.error = e,
        },
        Msg::Go(n) => st.go(n),
        Msg::Back => {
            let n = st.step - 1;
            st.go(n);
        }
        Msg::Next => {
            if st.step == 1 {
                let problem = source_problem(st);
                if !problem.is_empty() {
                    st.error = problem;
                    return idle();
                }
                st.go(2);
            } else if st.step == 4 || st.step == 5 {
                // Steps 4 and 5 save the draft on the way out, as the web does.
                let then = Then::Go(st.step + 1);
                return save(st, ctx, then);
            } else {
                let n = st.step + 1;
                st.go(n);
            }
        }
        Msg::Save(then) => return save(st, ctx, then),
        Msg::Saved(then, now, res) => {
            st.busy = false;
            st.saved = now;
            match res {
                Err(e) => st.error = e,
                Ok(()) => {
                    st.baseline = st.draft.clone();
                    match then {
                        Then::Go(n) => st.go(n),
                        Then::Test => return test(st, ctx),
                        Then::Finish => return Outcome::Closed(format!("Saved {}.", st.draft.id)),
                    }
                }
            }
        }
        Msg::RunTest => {
            if !st.saved {
                return save(st, ctx, Then::Test);
            }
            return test(st, ctx);
        }
        Msg::Tested(res) => {
            st.busy = false;
            let result = res.unwrap_or_else(|e| TestResult { error: Some(e), ..TestResult::default() });
            st.probe = result.ok.then(|| result.tools.clone());
            st.tool_rows = st.probe.as_ref().map(|list| {
                list.iter()
                    .map(|t| ToolRow { name: t.name.clone(), tool: t.agent_name.clone(), description: t.description.clone(), auto: t.auto_risk.clone() })
                    .collect()
            });
            st.test = Some(result);
        }
        Msg::ToolsLoaded(res) => {
            st.loading_tools = false;
            match res {
                Ok(list) => st.tool_rows = Some(list),
                Err(e) => {
                    // An empty list keeps the step from asking again on every message.
                    st.error = e;
                    st.tool_rows = Some(Vec::new());
                }
            }
        }
        Msg::PickNode(id) => st.node = id,
        Msg::PickSource(source) => {
            st.choose(source);
            st.error.clear();
        }
        Msg::Id(id) => st.draft.id = id.trim().to_string(),
        Msg::Name(name) => st.draft.name = name,
        Msg::Image(image) => st.draft.image = image,
        Msg::Command(command) => st.draft.command = command,
        Msg::Url(url) => st.draft.url = url,
        Msg::Enabled(on) => st.draft.enabled = on,
        Msg::Plain(row, value) => set(st, row, Some(Setting::Value(value))),
        Msg::PickVault(row, idx) => pick(st, row, idx),
        Msg::Consent(row) => {
            let Some(entry) = picked_entry(st, row) else { return idle() };
            st.busy = true;
            return Outcome::Stay(bind_task(ctx, entry, row, mcp_binding(&st.draft.id)));
        }
        Msg::Bound(row, res) => {
            st.busy = false;
            match res {
                Ok(entry) => {
                    st.picked.remove(&row);
                    if let Vault::Ready(list) = &mut st.vault {
                        if let Some(slot) = list.iter_mut().find(|e| e.id == entry.id) {
                            *slot = entry.clone();
                        }
                    }
                    let format = st.rows.get(row).and_then(|r| r.spec.format.clone());
                    set(st, row, Some(Setting::Secret { secret: entry.name, format }));
                }
                Err(e) => st.error = e,
            }
        }
        Msg::Clear(row) => {
            set(st, row, None);
            st.picked.remove(&row);
        }
        Msg::ShowAll(on) => st.show_all = on,
        Msg::OpenCreate(row) => {
            st.creating = Some(row);
            st.create_name = st
                .rows
                .get(row)
                .map(|r| r.spec.name.to_uppercase().replace(|c: char| c == '-' || c == '.', "_"))
                .unwrap_or_default();
            st.create_value.clear();
            st.error.clear();
        }
        Msg::CreateName(name) => st.create_name = name.to_uppercase(),
        Msg::CreateValue(value) => st.create_value = value,
        Msg::CreateCancel => {
            st.creating = None;
            st.create_name.clear();
            st.create_value.clear();
        }
        Msg::CreateSubmit => return create(st, ctx),
        Msg::Cancel => return cancel(st),
        Msg::Discard => return Outcome::Closed(String::new()),
        Msg::KeepEditing => st.confirming = false,
        Msg::Created(row, res) => {
            st.busy = false;
            match res {
                Ok(entry) => {
                    st.creating = None;
                    st.create_name.clear();
                    let mut list = match std::mem::replace(&mut st.vault, Vault::Loading) {
                        Vault::Ready(l) => l,
                        _ => Vec::new(),
                    };
                    list.push(entry.clone());
                    st.vault = Vault::Ready(list);
                    let format = st.rows.get(row).and_then(|r| r.spec.format.clone());
                    set(st, row, Some(Setting::Secret { secret: entry.name, format }));
                }
                Err(e) => st.error = e,
            }
        }
        Msg::ToolsAll(all) => {
            st.draft.tools = if all {
                Pick::All
            } else {
                // Switching to a pick starts with every listed tool picked, as the web does.
                Pick::Only(st.tool_rows.as_deref().unwrap_or(&[]).iter().map(|t| t.name.clone()).collect())
            };
        }
        Msg::ToolPick(name, on) => pick_edit(&mut st.draft.tools, name, on),
        Msg::ToolRisk(name, risk) => {
            if risk.is_empty() {
                st.draft.tool_risk.remove(&name);
            } else {
                st.draft.tool_risk.insert(name, risk);
            }
        }
        Msg::RolesAll(all) => st.draft.roles = if all { Pick::All } else { Pick::Only(Vec::new()) },
        Msg::RolePick(id, on) => pick_edit(&mut st.draft.roles, id, on),
        Msg::NodesAll(all) => st.draft.nodes = if all { Pick::All } else { Pick::Only(Vec::new()) },
        Msg::NodePick(id, on) => pick_edit(&mut st.draft.nodes, id, on),
    }
    tools_on_enter(st, ctx)
}

// ---- View -------------------------------------------------------------------------------------

/// A labelled input. `on` is the message the input sends; the placeholder and value live as long as the state.
fn field_input<'a>(placeholder: &'a str, value: &'a str, on: impl Fn(String) -> Msg + 'a) -> Element<'a, Msg> {
    text_input(placeholder, value).on_input(on).padding(8).style(crate::theme::input).into()
}

fn field<'a>(label: &str, control: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), control];
    if !hint.is_empty() {
        items.push(ui::faint(hint.to_string()).into());
    }
    column(items).spacing(6).width(Length::Fill).into()
}

fn option<'a>(on: bool, title: &str, body: &str, note: &str, msg: Msg) -> Element<'a, Msg> {
    let kind = if on { ui::Kind::Primary } else { ui::Kind::Secondary };
    let mut items: Vec<Element<'a, Msg>> = vec![ui::btn_sm(title, None, kind, Some(msg)), ui::prose(body.to_string()).into()];
    if !note.is_empty() {
        items.push(ui::faint(note.to_string()).into());
    }
    column(items).spacing(6).into()
}

/// A credential's name, required badge, help and format.
fn spec_head<'a>(row: &Row) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![ui::mono(row.spec.name.clone(), 12.0, crate::theme::palette().ink).into()];
    if row.spec.required {
        items.push(ui::badge("Required", ui::Tone::Warn));
    }
    if !row.spec.help.is_empty() {
        items.push(ui::faint(row.spec.help.clone()).into());
    }
    if let Some(f) = &row.spec.format {
        items.push(ui::faint(format!("Format: {f}")).into());
    }
    column(items).spacing(4).into()
}

fn plain_card<'a>(st: &'a State, i: usize, row: &'a Row) -> Element<'a, Msg> {
    let value: &'a str = match bag_ref(&st.draft, row.header).get(&row.spec.name) {
        Some(Setting::Value(v)) => v.as_str(),
        _ => "",
    };
    let items: Vec<Element<'a, Msg>> = vec![spec_head(row), field_input("", value, move |v| Msg::Plain(i, v))];
    column(items).spacing(8).width(Length::Fill).into()
}

fn secret_card<'a>(st: &'a State, i: usize, row: &'a Row) -> Element<'a, Msg> {
    let current: Option<&'a str> = match bag_ref(&st.draft, row.header).get(&row.spec.name) {
        Some(Setting::Secret { secret, .. }) if !secret.is_empty() => Some(secret.as_str()),
        _ => None,
    };
    let mut items: Vec<Element<'a, Msg>> = vec![spec_head(row)];
    items.push(match current {
        Some(name) => ui::badge(&format!("Set to {name}"), ui::Tone::Ok),
        None => ui::badge("Not set", ui::Tone::Muted),
    });
    match &st.vault {
        Vault::Loading => items.push(ui::faint("Loading vault secrets").into()),
        Vault::Note(note) => items.push(ui::faint(note.clone()).into()),
        Vault::Ready(entries) => {
            items.push(ui::btn_sm(
                if st.show_all { "Show fewer" } else { "Show all" },
                None,
                ui::Kind::Ghost,
                Some(Msg::ShowAll(!st.show_all)),
            ));
            let shown: Vec<usize> = (0..entries.len())
                .filter(|&idx| st.show_all || plausible(&entries[idx], &row.spec) || current == Some(entries[idx].name.as_str()))
                .collect();
            if entries.is_empty() {
                items.push(ui::faint("The vault has no secrets yet. Create one instead.").into());
            } else if shown.is_empty() {
                items.push(ui::faint("No secret has a plausible name. Tick Show all.").into());
            }
            for idx in shown {
                let entry = &entries[idx];
                let on = current == Some(entry.name.as_str());
                let kind = if on { ui::Kind::Primary } else { ui::Kind::Secondary };
                items.push(ui::btn_sm(
                    &format!("{} · {}", entry.name, entry.kind),
                    None,
                    kind,
                    gate(st.busy, Msg::PickVault(i, idx)),
                ));
            }
            if let Some(entry) = st.picked.get(&i).and_then(|idx| entries.get(*idx)) {
                items.push(ui::btn_sm(
                    &format!("Allow the {} server to use {}", st.draft.id, entry.name),
                    None,
                    ui::Kind::Secondary,
                    gate(st.busy, Msg::Consent(i)),
                ));
            }
        }
    }
    if current.is_some() {
        items.push(ui::btn_sm("Clear", None, ui::Kind::Ghost, gate(st.busy, Msg::Clear(i))));
    }
    items.push(ui::btn_sm("Create a new secret", None, ui::Kind::Secondary, gate(st.busy, Msg::OpenCreate(i))));
    if st.creating == Some(i) {
        items.push(create_form(st));
    }
    column(items).spacing(10).width(Length::Fill).into()
}

fn create_form(st: &State) -> Element<'_, Msg> {
    let value_in: Element<'_, Msg> = text_input("", st.create_value.as_str())
        .on_input(Msg::CreateValue)
        .secure(true)
        .padding(8)
        .style(crate::theme::input)
        .into();
    let items: Vec<Element<'_, Msg>> = vec![
        ui::faint(format!("Creates a vault secret named {} and binds it to this server.", st.create_name)).into(),
        field("Name", field_input("", st.create_name.as_str(), Msg::CreateName), NAME_HINT),
        field(
            "Value",
            value_in,
            "The value is never shown again. It is cleared on save, so type it again after an error.",
        ),
        row(vec![
            ui::btn_sm("Create secret", None, ui::Kind::Primary, gate(st.busy, Msg::CreateSubmit)),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CreateCancel)),
        ])
        .spacing(12)
        .into(),
    ];
    column(items).spacing(10).width(Length::Fill).into()
}

fn step1(st: &State) -> Element<'_, Msg> {
    let mut options: Vec<Element<'_, Msg>> = Vec::new();
    for p in &st.catalog {
        let source = Source::Preset(p.id.clone());
        let on = st.source.as_ref() == Some(&source);
        let note = if !p.status.is_empty() && p.status != "verified" {
            if p.note.is_empty() {
                format!("Not verified: {}", p.status)
            } else {
                p.note.clone()
            }
        } else {
            String::new()
        };
        options.push(option(on, &p.name, &p.description, &note, Msg::PickSource(source)));
    }
    options.push(option(
        st.source == Some(Source::Custom),
        "Custom container image",
        "Any image the hub can run, with an optional command.",
        "",
        Msg::PickSource(Source::Custom),
    ));
    options.push(option(
        st.source == Some(Source::Remote),
        "Remote URL",
        "A server the hub reaches over https.",
        "",
        Msg::PickSource(Source::Remote),
    ));
    let mut items: Vec<Element<'_, Msg>> = vec![
        ui::faint("Pick a preset, or bring your own container image or remote URL.").into(),
        column(options).spacing(12).into(),
    ];
    match &st.source {
        Some(Source::Custom) => {
            items.push(field(
                "Container image",
                field_input("ghcr.io/owner/server:1.0", st.draft.image.as_str(), Msg::Image),
                "The hub pulls and runs the image. Agents never see its credentials.",
            ));
            items.push(field(
                "Command",
                field_input("serve --stdio", st.draft.command.as_str(), Msg::Command),
                "Optional. Separate the arguments with spaces.",
            ));
        }
        Some(Source::Remote) => items.push(field(
            "HTTPS URL",
            field_input("https://example.com/mcp", st.draft.url.as_str(), Msg::Url),
            "Must start with https://.",
        )),
        _ => {}
    }
    let (id_control, id_hint): (Element<'_, Msg>, &str) = if st.mode == Mode::Edit {
        (
            text_input("", st.draft.id.as_str()).padding(8).style(crate::theme::input).into(),
            "The ID cannot change.",
        )
    } else {
        (field_input("", st.draft.id.as_str(), Msg::Id), "2 to 32 characters: lower-case letters, digits and hyphens.")
    };
    items.push(field("ID", id_control, id_hint));
    items.push(field("Display name", field_input("", st.draft.name.as_str(), Msg::Name), ""));
    column(items).spacing(16).width(Length::Fill).into()
}

fn step2(st: &State) -> Element<'_, Msg> {
    let mut items: Vec<Element<'_, Msg>> = Vec::new();
    if st.rows.is_empty() {
        items.push(ui::faint("This server needs no credentials.").into());
    }
    for (i, row) in st.rows.iter().enumerate() {
        items.push(if row.spec.secret { secret_card(st, i, row) } else { plain_card(st, i, row) });
    }
    let missing: Vec<&str> = st
        .rows
        .iter()
        .filter(|r| r.spec.required && !is_set(st, r))
        .map(|r| r.spec.name.as_str())
        .collect();
    if !missing.is_empty() {
        items.push(ui::error_line(&format!("Required and not set: {}", missing.join(", "))));
    }
    column(items).spacing(18).width(Length::Fill).into()
}

fn step3(st: &State) -> Element<'_, Msg> {
    let label = if st.test.is_some() { "Run test again" } else { "Run test" };
    let nodes: Vec<Element<'_, Msg>> = st
        .nodes
        .iter()
        .map(|n| {
            let kind = if n.id == st.node { ui::Kind::Primary } else { ui::Kind::Secondary };
            ui::btn_sm(&n.name, None, kind, gate(st.busy, Msg::PickNode(n.id.clone())))
        })
        .collect();
    let mut items: Vec<Element<'_, Msg>> = vec![
        ui::faint("Run the server once to check that it starts and lists its tools. Unsaved changes are saved first.").into(),
        ui::label("Node").into(),
        row(nodes).spacing(6).into(),
        ui::btn_sm(label, None, ui::Kind::Primary, gate(st.busy, Msg::RunTest)),
    ];
    if st.busy {
        items.push(ui::faint("Testing the server. This can take a minute.").into());
    }
    if let Some(t) = &st.test {
        if t.ok {
            let name = t.server_info.get("name").and_then(Value::as_str).unwrap_or("Server");
            let version = t.server_info.get("version").and_then(Value::as_str).unwrap_or("");
            items.push(ui::badge("Test passed", ui::Tone::Ok));
            items.push(ui::faint(format!("{name} {version} · {:.0} ms", t.ms)).into());
            if t.tools.is_empty() {
                items.push(ui::faint("The server reports no tools.").into());
            }
        } else {
            items.push(ui::badge("Test failed", ui::Tone::Danger));
            items.push(ui::error_line(t.error.as_deref().unwrap_or("The server did not start.")));
        }
    }
    column(items).spacing(12).width(Length::Fill).into()
}

fn step4(st: &State) -> Element<'_, Msg> {
    let d = &st.draft;
    let mut items: Vec<Element<'_, Msg>> = vec![ui::faint(format!("Agents may call: {}.", tools_text(d))).into()];
    match &st.tool_rows {
        None => items.push(ui::faint("Run the test in step 3 to list the tools this server reports.").into()),
        Some(rows) if rows.is_empty() => items.push(ui::faint("The server reports no tools.").into()),
        Some(rows) => {
            let all = d.tools == Pick::All;
            items.push(ui::seg(&TOOL_MODE, all, Msg::ToolsAll).into());
            items.push(ui::faint("Risk sets how a call is treated. Auto uses the risk the hub derives from each tool's hints.").into());
            for tool in rows {
                let mut lines: Vec<Element<'_, Msg>> = Vec::new();
                if all {
                    lines.push(ui::mono(tool.name.clone(), 12.0, crate::theme::palette().ink).into());
                } else {
                    let name = tool.name.clone();
                    lines.push(checkbox(d.tools.has(&tool.name)).label(tool.name.clone()).on_toggle(move |on| Msg::ToolPick(name.clone(), on)).into());
                }
                lines.push(ui::faint(format!("Agent name: {}", tool.tool)).into());
                if !tool.description.is_empty() {
                    lines.push(ui::prose(tool.description.clone()).into());
                }
                let current = d.tool_risk.get(&tool.name).cloned().unwrap_or_default();
                let mut options: Vec<(String, String)> = vec![(String::new(), format!("auto ({})", tool.auto))];
                options.extend(RISKS.iter().map(|r| (r.to_string(), r.to_string())));
                let buttons: Vec<Element<'_, Msg>> = options
                    .into_iter()
                    .map(|(value, label)| {
                        let kind = if value == current { ui::Kind::Primary } else { ui::Kind::Ghost };
                        ui::btn_sm(&label, None, kind, Some(Msg::ToolRisk(tool.name.clone(), value)))
                    })
                    .collect();
                lines.push(row(buttons).spacing(6).into());
                items.push(column(lines).spacing(6).into());
            }
        }
    }
    column(items).spacing(10).width(Length::Fill).into()
}

/// A pick of named options (roles or nodes): all or some, with a checkbox per option when some.
fn pick_group<'a>(
    title: &str,
    pick: &Pick,
    modes: &'static [(bool, &'static str); 2],
    options: Vec<(String, String)>,
    all: fn(bool) -> Msg,
    one: fn(String, bool) -> Msg,
) -> Element<'a, Msg> {
    let is_all = *pick == Pick::All;
    let mut items: Vec<Element<'a, Msg>> = vec![ui::label(title).into(), ui::seg(modes, is_all, all).into()];
    if !is_all {
        for (id, name) in options {
            let on = pick.has(&id);
            items.push(checkbox(on).label(name).on_toggle(move |v| one(id.clone(), v)).into());
        }
    }
    column(items).spacing(8).width(Length::Fill).into()
}

fn step5(st: &State) -> Element<'_, Msg> {
    let d = &st.draft;
    let roles: Vec<(String, String)> = st
        .roles
        .iter()
        .map(|r| (r.id.clone(), if r.title.is_empty() { r.id.clone() } else { r.title.clone() }))
        .collect();
    let nodes: Vec<(String, String)> = st.nodes.iter().map(|n| (n.id.clone(), n.name.clone())).collect();
    let mut items: Vec<Element<'_, Msg>> = vec![
        ui::faint("Only the roles and nodes picked here can reach this server.").into(),
        pick_group("Roles", &d.roles, &ROLE_MODE, roles, Msg::RolesAll, Msg::RolePick),
        pick_group("Nodes", &d.nodes, &NODE_MODE, nodes, Msg::NodesAll, Msg::NodePick),
    ];
    if d.roles.nobody() || d.nodes.nobody() {
        items.push(ui::error_line("Nobody can use this server until a role and a node are picked."));
    }
    column(items).spacing(14).width(Length::Fill).into()
}

fn step6(st: &State) -> Element<'_, Msg> {
    let d = &st.draft;
    let total = d.env.len() + d.headers.len();
    let secrets = d
        .env
        .values()
        .chain(d.headers.values())
        .filter(|s| matches!(s, Setting::Secret { .. }))
        .count();
    let rows: Vec<(&str, String)> = vec![
        ("Name", if d.name.is_empty() { "—".to_string() } else { d.name.clone() }),
        ("ID", d.id.clone()),
        ("Transport", transport_text(d)),
        ("Secrets", secrets.to_string()),
        ("Settings", (total - secrets).to_string()),
        ("Tools", tools_text(d)),
        ("Roles", d.roles.text()),
        ("Nodes", d.nodes.text()),
    ];
    let mut items: Vec<Element<'_, Msg>> = rows
        .into_iter()
        .map(|(k, v)| {
            let cells: Vec<Element<'_, Msg>> = vec![ui::label(k).into(), ui::faint(v).into()];
            row(cells).spacing(12).into()
        })
        .collect();
    items.push(checkbox(d.enabled).label("Enabled").on_toggle(Msg::Enabled).into());
    items.push(
        ui::faint("A disabled server stays configured and cannot run. Agents use it only when it is enabled, has a role and a node.").into(),
    );
    column(items).spacing(10).width(Length::Fill).into()
}

fn footer(st: &State) -> Element<'_, Msg> {
    let mut items: Vec<Element<'_, Msg>> = vec![ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Cancel))];
    if st.step > 1 {
        items.push(ui::btn("Back", None, ui::Kind::Ghost, gate(st.busy, Msg::Back)));
    }
    if st.step < 6 {
        items.push(ui::btn("Next", None, ui::Kind::Primary, gate(st.busy, Msg::Next)));
    } else {
        let label = if st.draft.enabled { "Enable and save" } else { "Save" };
        items.push(ui::btn(label, None, ui::Kind::Primary, gate(st.busy, Msg::Save(Then::Finish))));
    }
    row(items).spacing(12).into()
}

/// The wizard: the steps, the open step, any error, the discard prompt when asked, and the footer.
pub fn view(st: &State) -> Element<'_, Msg> {
    let title = match st.mode {
        Mode::Create => "Add MCP server".to_string(),
        Mode::Edit => format!("Edit {}", if st.draft.name.is_empty() { &st.draft.id } else { &st.draft.name }),
    };
    let stepper: Vec<Element<'_, Msg>> = STEP_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let n = i + 1;
            let on = n == st.step;
            let press = if n <= st.reached && !on { gate(st.busy, Msg::Go(n)) } else { None };
            ui::btn_sm(&format!("{n} {label}"), None, if on { ui::Kind::Primary } else { ui::Kind::Ghost }, press)
        })
        .collect();
    let body: Element<'_, Msg> = match st.step {
        1 => step1(st),
        2 => step2(st),
        3 => step3(st),
        4 => step4(st),
        5 => step5(st),
        _ => step6(st),
    };
    let mut items: Vec<Element<'_, Msg>> = vec![row(stepper).spacing(6).into(), body];
    if !st.error.is_empty() {
        items.push(ui::error_line(&st.error));
    }
    if st.confirming {
        let prompt: Vec<Element<'_, Msg>> = vec![
            ui::faint("Discard the unsaved changes?").into(),
            ui::btn_sm("Keep editing", None, ui::Kind::Ghost, Some(Msg::KeepEditing)),
            ui::btn_sm("Discard", None, ui::Kind::Primary, Some(Msg::Discard)),
        ];
        items.push(row(prompt).spacing(12).into());
    }
    items.push(footer(st));
    ui::card(&title, column(items).spacing(18).width(Length::Fill).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"{
        "id":"fs","name":"Files","enabled":true,
        "transport":{"kind":"stdio","image":"ghcr.io/x/fs:1","command":["serve","/data"]},
        "env":{"TOKEN":{"secret":"fs_token"},"ROOT":{"value":"/data"}},
        "headers":{"Authorization":{"secret":"fs_auth","format":"Bearer {}"}},
        "tools":{"allow":["read_file"],"risk":{"read_file":"read"}},
        "access":{"roles":["owner"],"nodes":"all"},
        "limits":{"timeout_s":30,"max_output_kb":128,"idle_stop_min":5},
        "allow_private":false,"storage":{"volume":true},
        "status":"stopped","tools_count":0,"last_error":null,"created_ms":1,"updated_ms":2
    }"#;

    fn stdio_draft() -> Draft {
        let mut d = Draft::blank();
        d.id = "fs".into();
        d.name = "Files".into();
        d.image = "ghcr.io/x/fs:1".into();
        d.command = "serve  /data".into();
        d.env.insert("ROOT".into(), Setting::Value("/data".into()));
        d
    }

    #[test]
    fn mcp_wizard_stdio_request_body() {
        let body = stdio_draft().create_body();
        assert_eq!(body["id"], "fs");
        assert_eq!(body["enabled"], false);
        assert_eq!(body["transport"], json!({ "kind": "stdio", "image": "ghcr.io/x/fs:1", "command": ["serve", "/data"] }));
        assert_eq!(body["env"], json!({ "ROOT": { "value": "/data" } }));
        assert_eq!(body["tools"], json!({ "allow": "all", "risk": {} }));
        assert_eq!(body["access"], json!({ "roles": [], "nodes": [] }));
        assert!(body["transport"].get("url").is_none());
    }

    #[test]
    fn mcp_wizard_tool_and_access_picks_go_in_the_request_body() {
        let mut d = stdio_draft();
        d.tools = Pick::Only(vec!["read_file".into()]);
        d.tool_risk.insert("read_file".into(), "irreversible".into());
        d.roles = Pick::Only(vec!["ops".into()]);
        d.nodes = Pick::All;
        let body = d.patch_body();
        assert_eq!(body["tools"], json!({ "allow": ["read_file"], "risk": { "read_file": "irreversible" } }));
        assert_eq!(body["access"], json!({ "roles": ["ops"], "nodes": "all" }));
        assert_eq!(body["limits"]["timeout_s"], 60);
    }

    #[test]
    fn mcp_wizard_http_request_body() {
        let mut d = Draft::blank();
        d.id = "web".into();
        d.name = "Search".into();
        d.http = true;
        d.url = "https://mcp.example.com/sse".into();
        d.headers.insert("Authorization".into(), Setting::Secret { secret: "WEB_KEY".into(), format: Some("Bearer {}".into()) });
        let body = d.patch_body();
        assert_eq!(body["transport"], json!({ "kind": "http", "url": "https://mcp.example.com/sse" }));
        assert_eq!(body["headers"], json!({ "Authorization": { "secret": "WEB_KEY", "format": "Bearer {}" } }));
        assert!(body["transport"].get("command").is_none());
    }

    #[test]
    fn mcp_wizard_secret_field_is_a_binding_not_a_value() {
        let mut d = stdio_draft();
        d.env.insert("TOKEN".into(), Setting::Secret { secret: "FS_TOKEN".into(), format: None });
        d.env.insert("UNSET".into(), Setting::Secret { secret: String::new(), format: None });
        let body = d.patch_body();
        assert_eq!(body["env"]["TOKEN"], json!({ "secret": "FS_TOKEN" }));
        assert!(body["env"]["TOKEN"].get("value").is_none());
        assert!(body["env"].get("UNSET").is_none(), "an unset secret is not sent");
    }

    #[test]
    fn mcp_wizard_edit_prefill_round_trips() {
        let view: Value = serde_json::from_str(VIEW).unwrap();
        let draft = Draft::from_view(&view);
        assert_eq!(draft.command, "serve /data");
        assert_eq!(draft.env.get("TOKEN"), Some(&Setting::Secret { secret: "fs_token".into(), format: None }));
        assert_eq!(draft.env.get("ROOT"), Some(&Setting::Value("/data".into())));
        assert_eq!(draft.tools, Pick::Only(vec!["read_file".into()]));
        assert_eq!(draft.nodes, Pick::All);
        let body = draft.patch_body();
        for key in ["name", "enabled", "transport", "env", "headers", "tools", "access", "limits", "allow_private", "storage"] {
            assert_eq!(body[key], view[key], "{key} round-trips");
        }
        assert_eq!(draft.create_body()["id"], "fs");
    }

    #[test]
    fn mcp_wizard_edit_finds_the_preset_source() {
        let view: Value = serde_json::from_str(VIEW).unwrap();
        let catalog = vec![Preset {
            id: "fs".into(),
            name: "Files".into(),
            transport: "stdio".into(),
            image: Some("ghcr.io/x/fs:1".into()),
            env: vec![Spec { name: "TOKEN".into(), secret: true, ..Spec::default() }],
            ..Preset::default()
        }];
        let st = State::new(Some(&view), None, &catalog);
        assert_eq!(st.mode, Mode::Edit);
        assert!(st.saved);
        assert_eq!(st.source, Some(Source::Preset("fs".into())));
        assert_eq!(st.rows.len(), 1);
    }

    #[test]
    fn mcp_wizard_cancel_asks_only_when_there_are_changes() {
        let mut st = State::new(None, None, &[]);
        assert!(matches!(cancel(&mut st), Outcome::Closed(_)), "no changes: close at once");
        assert!(!st.confirming);
        st.draft.name = "Files".into();
        assert!(matches!(cancel(&mut st), Outcome::Stay(_)), "a change: stay and ask");
        assert!(st.confirming);
    }

    #[test]
    fn mcp_wizard_source_problem_matches_the_web() {
        let mut st = State::new(None, None, &[]);
        assert_eq!(source_problem(&st), "Pick a preset, a custom container image or a remote URL.");
        st.source = Some(Source::Custom);
        assert_eq!(source_problem(&st), "Enter a container image.");
        st.draft.image = "ghcr.io/x/fs:1".into();
        st.draft.id = "F".into();
        st.draft.name = "Files".into();
        assert_eq!(source_problem(&st), ID_HINT);
        st.draft.id = "fs".into();
        st.draft.name = " ".into();
        assert_eq!(source_problem(&st), "Enter a display name.");
        st.draft.name = "Files".into();
        assert_eq!(source_problem(&st), "");
        st.source = Some(Source::Remote);
        st.draft.http = true;
        st.draft.url = "http://example.com".into();
        assert_eq!(source_problem(&st), "Enter a remote URL that starts with https://.");
    }

    #[test]
    fn mcp_wizard_id_and_secret_name_rules() {
        assert!(valid_id("fs"));
        assert!(valid_id("files-2"));
        assert!(!valid_id("1fs"));
        assert!(!valid_id("Fs"));
        assert!(!valid_id("f"));
        assert!(valid_secret_name("FS_TOKEN"));
        assert!(!valid_secret_name("fs_token"));
        assert!(!valid_secret_name("A"));
        assert_eq!(mcp_binding("fs"), "mcp:fs");
    }
}
