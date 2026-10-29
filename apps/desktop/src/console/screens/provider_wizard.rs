//! The Add provider dialog (the providers brief, "The Add provider dialog"): three steps, Provider, Identity and
//! Config, drawn with `ui::modal` at 760 wide. The Models screen opens it with [`open`], forwards its messages to
//! [`update`] and draws [`view`] over itself with `ui::with_modal`. [`update`] answers `Some(Outcome)` when the
//! dialog is done: `Added(id)` once the hub created the instance (an API instance gets its key set first), or
//! `Cancelled`.
//!
//! Creating: an API kind is `POST /v1/providers` with the preset's own auth, options and headers (read from the
//! catalogue), then `PUT /v1/providers/{id}/secret` when a key was typed. An engine is `POST /v1/engines`:
//! `acp:<registry id>` for a registry agent, `acp-local` for a command, a built-in engine id for a second
//! instance. An engine has no key field: no key ever goes into an engine.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use iced::widget::{Space, button, column, container, operation, row, text_editor, text_input};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::providers_api::{self, Catalog, CatalogApi, EngineEdit, Registry, RegistryAgent};
use crate::console::client::{self, Client};
use crate::console::picker;
use crate::console::ui::{self, Kind, Tone};

/// The dialog's width.
const WIDTH: f32 = 760.0;
/// The accents the swatch popover offers (`#rrggbb`), the design's eight.
const ACCENTS: [&str; 8] = [
    "#5cc8ff", "#7ee0b5", "#b69cff", "#ff9ac1", "#ffd36e", "#6fe3e1", "#a3b8ff", "#c5f27a",
];
/// The OpenAI-compatible kind (the hub's `ProviderKind`).
const COMPATIBLE: &str = "openai-compatible";
/// The Azure kind: its endpoint is the resource's own, so it has a Base URL with no preset.
const AZURE: &str = "azure-openai";
/// The hub's id length limit (`[a-z0-9_-]{1,40}`).
pub const MAX_ID: usize = 40;

static BASE_URL_ID: LazyLock<iced::widget::Id> = LazyLock::new(iced::widget::Id::unique);

/// The three steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Provider,
    Identity,
    Config,
}

impl Step {
    fn index(self) -> usize {
        match self {
            Step::Provider => 0,
            Step::Identity => 1,
            Step::Config => 2,
        }
    }
}

/// What the owner picked on the Provider step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    None,
    /// An API kind from the catalogue: its preset id (`openai`, `deepseek` …).
    Api(String),
    /// The OpenAI-compatible kind typed by hand (no preset): "Configure manually".
    Compatible,
    /// A built-in engine, a second instance of it: its engine id.
    Engine(String),
    /// An ACP registry agent: its registry id.
    Registry(String),
    /// A command the hub runs as an engine (`acp-local`).
    Local,
}

/// How the dialog ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The hub created the instance with this id.
    Added(String),
    Cancelled,
}

/// The dialog's state.
pub struct Wizard {
    pub step: Step,
    pick: Pick,
    catalog: Option<Catalog>,
    registry: Option<Registry>,
    /// A failed catalogue load (the tiles stay empty).
    load_error: Option<String>,
    /// A failed registry load.
    registry_error: Option<String>,
    /// The ids of the existing API instances and engines: a new one avoids them.
    taken: Vec<String>,
    search: String,
    label: String,
    label_touched: bool,
    id: String,
    id_touched: bool,
    accent: Option<String>,
    accent_open: bool,
    base_url: String,
    base_touched: bool,
    key: String,
    models: String,
    executable: String,
    version: String,
    home: String,
    args: text_editor::Content,
    variables: Vec<(String, String)>,
    command: String,
    route: String,
    busy: bool,
    error: Option<String>,
    /// The instance the hub created when its key failed: a retry sets the key only.
    created: Option<String>,
}

/// The dialog's messages.
#[derive(Debug, Clone)]
pub enum WizMsg {
    Catalog(Result<Catalog, String>),
    Registry(Result<Registry, String>),
    Taken(Result<Vec<String>, String>),
    Cancel,
    Back,
    Next,
    /// Step 1's "Configure manually": the OpenAI-compatible kind, straight to Config.
    ConfigureManually,
    /// "Enter manually" (acp-local): the command, straight to Identity.
    EnterManually,
    Submit,
    /// The hub created the instance (its id).
    Created(Result<String, String>),
    KeySet(Result<(), String>),
    PickApi(String),
    PickEngine(String),
    /// "Add" on a registry row: the agent is picked and Identity opens.
    PickRegistry(String),
    PickLocal,
    Search(String),
    OpenSite(String),
    Label(String),
    Id(String),
    AccentOpen,
    Accent(Option<String>),
    BaseUrl(String),
    Key(String),
    Models(String),
    Executable(String),
    Version(String),
    Home(String),
    Args(text_editor::Action),
    Command(String),
    Route(String),
    VarName(usize, String),
    VarValue(usize, String),
    VarRemove(usize),
    VarAdd,
}

/// What an API instance is created from, before the catalogue's own fields are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPlan {
    pub id: String,
    /// The ProviderKind string (`openai`, `openai-compatible` …).
    pub kind: String,
    /// The catalogue preset the kind came from, when there is one.
    pub preset: Option<String>,
    pub label: String,
    pub accent: Option<String>,
    pub base_url: Option<String>,
    /// The models typed (comma separated); empty takes the preset's list.
    pub models: Vec<String>,
    /// An optional program path for the two local provider kinds.
    pub executable: Option<String>,
}

/// Whether `id` is a valid instance id: `[a-z0-9_-]`, 1 to 40 characters (the hub's rule).
pub fn valid_instance_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `text` as an instance id: lower-case, with every character the hub refuses turned into a hyphen.
fn sanitize(text: &str) -> String {
    let s: String = text
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(MAX_ID)
        .collect();
    if s.is_empty() {
        "provider".to_string()
    } else {
        s
    }
}

/// The id a new instance starts with: `base`, or `base_2`, `base_3` … the first one no instance uses.
pub fn default_id(base: &str, taken: &[String]) -> String {
    let base = sanitize(base);
    if !taken.contains(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}_{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// `text` as the trimmed value, or None when it is blank.
fn non_empty(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The models a comma-separated field names: trimmed, without blanks or repeats.
pub fn parse_models(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in text.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        if !out.iter().any(|o| o == m) {
            out.push(m.to_string());
        }
    }
    out
}

/// The launch arguments: one per line, trimmed, without blanks.
fn arg_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The plain variables the engine gets: rows with a name; the name trimmed.
fn env_map(vars: &[(String, String)]) -> BTreeMap<String, String> {
    vars.iter()
        .filter_map(|(name, value)| non_empty(name).map(|n| (n, value.clone())))
        .collect()
}

/// The catalogue's entry for the preset `id`, from the raw `GET /v1/provider-catalog` body.
fn preset_entry<'a>(catalog: &'a Value, id: &str) -> Option<&'a Value> {
    catalog
        .get("api")?
        .as_array()?
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
}

/// The `POST /v1/providers` body for `plan`. The preset's auth, options, headers and (when none were typed)
/// models come from the raw catalogue: the hub takes a whole config, and the typed catalogue does not carry
/// them. A kind without a preset has no auth (`none`).
pub fn api_body(plan: &ApiPlan, catalog: &Value) -> Value {
    let preset = plan
        .preset
        .as_deref()
        .and_then(|id| preset_entry(catalog, id));
    let auth = preset
        .and_then(|p| p.get("auth"))
        .cloned()
        .unwrap_or_else(|| json!("none"));
    let mut options = preset
        .and_then(|p| p.get("options"))
        .cloned()
        .unwrap_or(Value::Null);
    if matches!(plan.kind.as_str(), "codex-app-server" | "opencode-local")
        && let Some(executable) = plan
            .executable
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        if !options.is_object() {
            options = json!({});
        }
        options["executable"] = json!(executable);
    }
    let headers = preset
        .and_then(|p| p.get("extra_headers"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let models = if plan.models.is_empty() {
        preset
            .and_then(|p| p.get("models"))
            .cloned()
            .unwrap_or_else(|| json!([]))
    } else {
        json!(plan.models)
    };
    json!({
        "id": plan.id,
        "kind": plan.kind,
        "label": plan.label,
        "accent": plan.accent,
        "enabled": true,
        "base_url": plan.base_url,
        "auth": auth,
        "models": models,
        "options": options,
        "extra_headers": headers,
        "extra_body": Value::Null,
    })
}

/// Why the Identity step cannot go on, or None.
fn identity_problem(label: &str, id: &str, taken: &[String]) -> Option<&'static str> {
    if label.trim().is_empty() {
        Some("Give the provider a name.")
    } else if !valid_instance_id(id) {
        Some("The ID is 1 to 40 characters: lower-case letters, digits, hyphens and underscores.")
    } else if taken.iter().any(|t| t == id) {
        Some("Another provider or engine already uses this ID.")
    } else {
        None
    }
}

/// Why a Base URL cannot go to the hub, or None.
fn url_problem(url: &str) -> Option<&'static str> {
    let url = url.trim();
    if url.is_empty() {
        Some("Enter the base URL.")
    } else if !(url.starts_with("https://") || url.starts_with("http://")) {
        Some("The base URL starts with https:// or http://.")
    } else {
        None
    }
}

/// Why a route cannot go to the hub, or None: a route is `provider/model`, and an empty one is the default.
fn route_problem(route: &str) -> Option<&'static str> {
    let route = route.trim();
    match route.split_once('/') {
        _ if route.is_empty() => None,
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => None,
        _ => Some("A route is provider/model, for example anthropic/claude-sonnet-5-5."),
    }
}

impl Wizard {
    /// The dialog before anything is loaded: the Provider step, nothing picked.
    fn blank() -> Wizard {
        Wizard {
            step: Step::Provider,
            pick: Pick::None,
            catalog: None,
            registry: None,
            load_error: None,
            registry_error: None,
            taken: Vec::new(),
            search: String::new(),
            label: String::new(),
            label_touched: false,
            id: String::new(),
            id_touched: false,
            accent: None,
            accent_open: false,
            base_url: String::new(),
            base_touched: false,
            key: String::new(),
            models: String::new(),
            executable: String::new(),
            version: String::new(),
            home: String::new(),
            args: text_editor::Content::new(),
            variables: Vec::new(),
            command: String::new(),
            route: String::new(),
            busy: false,
            error: None,
            created: None,
        }
    }

    fn preset(&self) -> Option<&CatalogApi> {
        let Pick::Api(id) = &self.pick else {
            return None;
        };
        self.catalog.as_ref()?.api.iter().find(|a| a.id == *id)
    }

    /// Whether the pick creates an API instance (not an engine).
    fn is_api(&self) -> bool {
        matches!(self.pick, Pick::Api(_) | Pick::Compatible)
    }

    fn is_local_provider(&self) -> bool {
        matches!(
            self.api_kind().as_str(),
            "codex-app-server" | "opencode-local"
        )
    }

    /// The ProviderKind the instance is created with.
    fn api_kind(&self) -> String {
        match &self.pick {
            Pick::Api(id) => self.preset().map_or_else(|| id.clone(), |p| p.kind.clone()),
            Pick::Compatible => COMPATIBLE.to_string(),
            _ => String::new(),
        }
    }

    /// Whether the Config step shows a Base URL: the OpenAI-compatible kind, Azure, and a preset with an endpoint.
    fn shows_base(&self) -> bool {
        match &self.pick {
            Pick::Compatible => true,
            Pick::Api(_) => {
                self.api_kind() == AZURE || self.preset().is_some_and(|p| p.base_url.is_some())
            }
            _ => false,
        }
    }

    fn default_label(&self) -> String {
        match &self.pick {
            Pick::None => String::new(),
            Pick::Api(id) => self
                .preset()
                .map_or_else(|| id.clone(), |p| p.label.clone()),
            Pick::Compatible => "OpenAI-compatible".to_string(),
            Pick::Engine(id) => self
                .catalog
                .as_ref()
                .and_then(|c| c.engines.iter().find(|e| e.id == *id))
                .map_or_else(|| id.clone(), |e| e.label.clone()),
            Pick::Registry(id) => self
                .registry
                .as_ref()
                .and_then(|r| r.agents.iter().find(|a| a.id == *id))
                .map_or_else(|| id.clone(), |a| a.name.clone()),
            Pick::Local => "Local ACP command".to_string(),
        }
    }

    /// The base the default id is built from: the preset or engine id, `openai-compatible`, `acp-local`.
    fn default_base(&self) -> String {
        match &self.pick {
            Pick::None => String::new(),
            Pick::Api(id) | Pick::Engine(id) | Pick::Registry(id) => id.clone(),
            Pick::Compatible => COMPATIBLE.to_string(),
            Pick::Local => "acp-local".to_string(),
        }
    }

    /// Fills the label, the id and the Base URL from the pick, where the owner has not typed them.
    fn apply_defaults(&mut self) {
        if matches!(self.pick, Pick::None) {
            self.label = String::new();
            self.id = String::new();
            return;
        }
        if !self.label_touched {
            self.label = self.default_label();
        }
        if !self.id_touched {
            self.id = default_id(&self.default_base(), &self.taken);
        }
        if !self.base_touched {
            self.base_url = self
                .preset()
                .and_then(|p| p.base_url.clone())
                .unwrap_or_default();
        }
    }

    /// Picks `pick`: the defaults follow it, and an engine's version starts at `pinned`.
    fn set_pick(&mut self, pick: Pick) {
        if self.pick != pick {
            self.executable.clear();
            self.key.clear();
        }
        self.pick = pick;
        self.label_touched = false;
        self.id_touched = false;
        self.base_touched = false;
        self.error = None;
        if !matches!(self.pick, Pick::Api(_) | Pick::Compatible | Pick::None)
            && self.version.trim().is_empty()
        {
            self.version = "pinned".to_string();
        }
        self.apply_defaults();
    }

    fn identity_problem(&self) -> Option<&'static str> {
        identity_problem(&self.label, &self.id, &self.taken)
    }

    fn config_problem(&self) -> Option<&'static str> {
        match &self.pick {
            Pick::None => Some("Choose a provider first."),
            Pick::Api(_) | Pick::Compatible => {
                if self.api_kind() == "opencode-local"
                    && !self.executable.trim().is_empty()
                    && !std::path::Path::new(self.executable.trim()).is_absolute()
                {
                    return Some("Use an absolute path to the installed OpenCode executable.");
                }
                if self.shows_base() {
                    url_problem(&self.base_url)
                } else {
                    None
                }
            }
            Pick::Engine(_) | Pick::Registry(_) => route_problem(&self.route),
            Pick::Local => {
                if self.command.trim().is_empty() {
                    Some("Enter the command the engine starts with.")
                } else {
                    route_problem(&self.route)
                }
            }
        }
    }

    /// Whether the current step is complete, so the dialog may go on (or create the instance on Config).
    fn can_advance(&self) -> bool {
        match self.step {
            Step::Provider => self.pick != Pick::None,
            Step::Identity => self.identity_problem().is_none(),
            Step::Config => self.config_problem().is_none(),
        }
    }

    fn advance(&mut self) {
        if !self.can_advance() {
            return;
        }
        self.step = match self.step {
            Step::Provider => Step::Identity,
            Step::Identity | Step::Config => Step::Config,
        };
        self.error = None;
    }

    fn back(&mut self) {
        self.step = match self.step {
            Step::Provider | Step::Identity => Step::Provider,
            Step::Config => Step::Identity,
        };
        self.error = None;
    }

    /// The API instance the Config step describes.
    fn api_plan(&self) -> ApiPlan {
        ApiPlan {
            id: self.id.trim().to_string(),
            kind: self.api_kind(),
            preset: match &self.pick {
                Pick::Api(id) => Some(id.clone()),
                _ => None,
            },
            label: self.label.trim().to_string(),
            accent: self.accent.clone(),
            base_url: self.shows_base().then(|| self.base_url.trim().to_string()),
            models: parse_models(&self.models),
            executable: self
                .is_local_provider()
                .then(|| self.executable.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }

    /// The `POST /v1/engines` body the Config step describes. The engine takes no key.
    fn engine_edit(&self) -> EngineEdit {
        let version = non_empty(&self.version).unwrap_or_else(|| "pinned".to_string());
        let (engine, version, command) = match &self.pick {
            Pick::Engine(id) => (id.clone(), Some(version), None),
            Pick::Registry(id) => (format!("acp:{id}"), Some(version), None),
            _ => (
                "acp-local".to_string(),
                None,
                Some(self.command.trim().to_string()),
            ),
        };
        EngineEdit {
            id: Some(self.id.trim().to_string()),
            engine: Some(engine),
            label: Some(Some(self.label.trim().to_string())),
            accent: Some(self.accent.clone()),
            version,
            command,
            args: Some(arg_lines(&self.args.text())),
            env: Some(env_map(&self.variables)),
            home: non_empty(&self.home).map(Some),
            route: non_empty(&self.route).map(Some),
            ..Default::default()
        }
    }

    /// The error the foot shows: the last failed act, else a failed load.
    fn error_text(&self) -> Option<&str> {
        self.error
            .as_deref()
            .or(self.load_error.as_deref())
            .or(self.registry_error.as_deref())
    }
}

/// The ids of the API instances and the engines: the ones a new instance must avoid.
fn taken_ids(c: &Client) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = providers_api::list(c)?.into_iter().map(|a| a.id).collect();
    ids.extend(providers_api::engines(c)?.into_iter().map(|e| e.id));
    Ok(ids)
}

/// Opens the dialog. `kind` (the OpenAI-compatible kind, as the CLIProxyAPI hub's button asks) starts at Config
/// with that kind and its Base URL focused; None starts at the Provider step. The catalogue, the registry and
/// the taken ids load in the background.
pub fn open(client: &Client, kind: Option<String>) -> (Wizard, Task<WizMsg>) {
    let mut w = Wizard::blank();
    let c = client.clone();
    let catalog = client::task(move || providers_api::catalog(&c), WizMsg::Catalog);
    let c = client.clone();
    let registry = client::task(move || providers_api::registry(&c, false), WizMsg::Registry);
    let c = client.clone();
    let taken = client::task(move || taken_ids(&c), WizMsg::Taken);
    let mut tasks = vec![catalog, registry, taken];
    if let Some(kind) = kind {
        w.step = Step::Config;
        w.pick = if kind == COMPATIBLE {
            Pick::Compatible
        } else {
            Pick::Api(kind)
        };
        w.apply_defaults();
        tasks.push(operation::focus(BASE_URL_ID.clone()));
    }
    (w, Task::batch(tasks))
}

/// Applies a message: the step moves, the picks, the field edits, and the hub calls (their results come back as
/// messages). Answers the outcome when the dialog is done.
pub fn update(w: &mut Wizard, msg: WizMsg, client: &Client) -> (Task<WizMsg>, Option<Outcome>) {
    match msg {
        WizMsg::Catalog(Ok(catalog)) => {
            let known = match &w.pick {
                Pick::Api(id) => catalog.api.iter().any(|a| a.id == *id),
                _ => true,
            };
            if !known {
                w.pick = Pick::Compatible;
            }
            w.catalog = Some(catalog);
            w.apply_defaults();
            idle()
        }
        WizMsg::Catalog(Err(e)) => {
            w.load_error = Some(e);
            idle()
        }
        WizMsg::Registry(Ok(registry)) => {
            w.registry = Some(registry);
            idle()
        }
        WizMsg::Registry(Err(e)) => {
            w.registry_error = Some(e);
            idle()
        }
        WizMsg::Taken(Ok(ids)) => {
            w.taken = ids;
            w.apply_defaults();
            idle()
        }
        WizMsg::Taken(Err(e)) => {
            w.load_error = Some(e);
            idle()
        }
        WizMsg::Cancel => (Task::none(), Some(Outcome::Cancelled)),
        WizMsg::Back => {
            w.back();
            idle()
        }
        WizMsg::Next => {
            w.advance();
            idle()
        }
        WizMsg::ConfigureManually => {
            w.set_pick(Pick::Compatible);
            w.step = Step::Config;
            idle()
        }
        WizMsg::EnterManually => {
            w.set_pick(Pick::Local);
            w.step = Step::Identity;
            idle()
        }
        WizMsg::PickApi(id) => {
            w.set_pick(Pick::Api(id));
            idle()
        }
        WizMsg::PickEngine(id) => {
            w.set_pick(Pick::Engine(id));
            idle()
        }
        WizMsg::PickRegistry(id) => {
            w.set_pick(Pick::Registry(id));
            w.step = Step::Identity;
            idle()
        }
        WizMsg::PickLocal => {
            w.set_pick(Pick::Local);
            idle()
        }
        WizMsg::Search(q) => {
            w.search = q;
            idle()
        }
        WizMsg::OpenSite(url) => {
            if ui::openable(&url) {
                let _ = ui::open_url(&url);
            }
            idle()
        }
        WizMsg::Label(s) => {
            w.label = s;
            w.label_touched = true;
            idle()
        }
        WizMsg::Id(s) => {
            w.id = s;
            w.id_touched = true;
            idle()
        }
        WizMsg::AccentOpen => {
            w.accent_open = !w.accent_open;
            idle()
        }
        WizMsg::Accent(accent) => {
            w.accent = accent;
            w.accent_open = false;
            idle()
        }
        WizMsg::BaseUrl(s) => {
            w.base_url = s;
            w.base_touched = true;
            idle()
        }
        WizMsg::Key(s) => {
            w.key = s;
            idle()
        }
        WizMsg::Models(s) => {
            w.models = s;
            idle()
        }
        WizMsg::Executable(s) => {
            w.executable = s;
            idle()
        }
        WizMsg::Version(s) => {
            w.version = s;
            idle()
        }
        WizMsg::Home(s) => {
            w.home = s;
            idle()
        }
        WizMsg::Args(action) => {
            w.args.perform(action);
            idle()
        }
        WizMsg::Command(s) => {
            w.command = s;
            idle()
        }
        WizMsg::Route(s) => {
            w.route = s;
            idle()
        }
        WizMsg::VarName(i, s) => {
            if let Some(v) = w.variables.get_mut(i) {
                v.0 = s;
            }
            idle()
        }
        WizMsg::VarValue(i, s) => {
            if let Some(v) = w.variables.get_mut(i) {
                v.1 = s;
            }
            idle()
        }
        WizMsg::VarRemove(i) => {
            if i < w.variables.len() {
                w.variables.remove(i);
            }
            idle()
        }
        WizMsg::VarAdd => {
            w.variables.push((String::new(), String::new()));
            idle()
        }
        WizMsg::Submit => submit(w, client),
        WizMsg::Created(Ok(id)) => created(w, id, client),
        WizMsg::Created(Err(e)) => {
            w.busy = false;
            w.error = Some(e);
            idle()
        }
        WizMsg::KeySet(Ok(())) => {
            w.busy = false;
            (Task::none(), w.created.clone().map(Outcome::Added))
        }
        WizMsg::KeySet(Err(e)) => {
            w.busy = false;
            w.error = Some(format!(
                "The provider was added, but its key was not saved: {e}"
            ));
            idle()
        }
    }
}

fn idle() -> (Task<WizMsg>, Option<Outcome>) {
    (Task::none(), None)
}

/// Creates the instance (or, after a failed key, only sets the key).
fn submit(w: &mut Wizard, client: &Client) -> (Task<WizMsg>, Option<Outcome>) {
    if w.busy || w.step != Step::Config || !w.can_advance() {
        return idle();
    }
    let c = client.clone();
    if let Some(id) = w.created.clone() {
        if w.key.trim().is_empty() {
            return (Task::none(), Some(Outcome::Added(id)));
        }
        w.busy = true;
        w.error = None;
        return (key_task(c, id, w.key.trim().to_string()), None);
    }
    w.busy = true;
    w.error = None;
    let task = if w.is_api() {
        let plan = w.api_plan();
        client::task(
            move || {
                let raw: Value = c.get("/v1/provider-catalog")?;
                providers_api::create(&c, &api_body(&plan, &raw)).map(|i| i.id)
            },
            WizMsg::Created,
        )
    } else {
        let edit = w.engine_edit();
        client::task(
            move || providers_api::add_engine(&c, &edit).map(|e| e.id),
            WizMsg::Created,
        )
    };
    (task, None)
}

/// The instance exists: an API instance with a typed key sets it first, an engine is done.
fn created(w: &mut Wizard, id: String, client: &Client) -> (Task<WizMsg>, Option<Outcome>) {
    w.created = Some(id.clone());
    let key = w.key.trim().to_string();
    if w.is_api() && !key.is_empty() {
        return (key_task(client.clone(), id, key), None);
    }
    w.busy = false;
    (Task::none(), Some(Outcome::Added(id)))
}

fn key_task(c: Client, id: String, key: String) -> Task<WizMsg> {
    client::task(
        move || providers_api::set_key(&c, &id, &key).map(|_| ()),
        WizMsg::KeySet,
    )
}

/// The dialog: the step rail, the step, and the foot (the error, then the buttons).
pub fn view(w: &Wizard) -> Element<'_, WizMsg> {
    let step = match w.step {
        Step::Provider => provider_step(w),
        Step::Identity => identity_step(w),
        Step::Config => config_step(w),
    };
    let body = column![
        steps(w.step),
        ui::faint("Add an account or configure a provider on this plaza."),
        step,
    ]
    .spacing(14);

    let mut foot: Vec<Element<'_, WizMsg>> = Vec::new();
    if let Some(e) = w.error_text() {
        foot.push(ui::error_line(e));
    }
    foot.push(ui::btn("Cancel", None, Kind::Ghost, Some(WizMsg::Cancel)));
    if w.step != Step::Provider {
        foot.push(ui::btn(
            "Back",
            None,
            Kind::Secondary,
            (!w.busy).then_some(WizMsg::Back),
        ));
    } else {
        foot.push(ui::btn(
            "Configure manually",
            None,
            Kind::Secondary,
            Some(WizMsg::ConfigureManually),
        ));
    }
    foot.push(next_button(w));
    ui::modal("Add provider", body.into(), foot, WizMsg::Cancel, WIDTH)
}

/// The primary button: Next on the first two steps, Add instance (Adding… or Pinning…) on Config.
fn next_button(w: &Wizard) -> Element<'_, WizMsg> {
    match w.step {
        Step::Config => {
            let label = match (w.busy, w.is_api()) {
                (false, _) => "Add instance",
                (true, true) => "Adding…",
                (true, false) => "Pinning…",
            };
            ui::btn(
                label,
                None,
                Kind::Primary,
                (!w.busy && w.can_advance()).then_some(WizMsg::Submit),
            )
        }
        _ => ui::btn(
            "Next",
            None,
            Kind::Primary,
            w.can_advance().then_some(WizMsg::Next),
        ),
    }
}

/// The three step pills: the current one on the signal ground, the done ones with a check.
fn steps(current: Step) -> Element<'static, WizMsg> {
    let names = [
        (Step::Provider, "Provider"),
        (Step::Identity, "Identity"),
        (Step::Config, "Config"),
    ];
    let p = crate::theme::palette();
    let mut rail = row![].spacing(8).align_y(Alignment::Center);
    for (step, name) in names {
        let on = step == current;
        let marker: Element<'static, WizMsg> = if step.index() < current.index() {
            ui::icon("check", 12.0, p.ok)
        } else {
            ui::mono(
                (step.index() + 1).to_string(),
                11.0,
                if on { p.ink } else { p.ink_3 },
            )
            .into()
        };
        rail = rail.push(
            container(
                row![marker, ui::sans(name, 12.0, p.ink)]
                    .spacing(6)
                    .align_y(Alignment::Center),
            )
            .padding([4, 10])
            .style(pill(on)),
        );
    }
    rail.into()
}

fn pill(on: bool) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| {
        let p = crate::theme::palette();
        let fill = if on { p.signal_soft } else { p.raised };
        container::Style {
            background: Some(fill.into()),
            border: Border {
                color: if on { p.signal } else { p.rule },
                width: 1.0,
                radius: crate::theme::skin().radius.into(),
            },
            ..Default::default()
        }
    }
}

/// Step 1: the tiles (the API kinds, then the built-in engines), the registry search and its rows.
fn provider_step(w: &Wizard) -> Element<'_, WizMsg> {
    let mut tiles: Vec<Element<'_, WizMsg>> = Vec::new();
    if let Some(catalog) = &w.catalog {
        for a in &catalog.api {
            tiles.push(tile(
                picker::mark(&a.brand, None, 26.0),
                &a.label,
                None,
                w.pick == Pick::Api(a.id.clone()),
                WizMsg::PickApi(a.id.clone()),
            ));
        }
        for e in &catalog.engines {
            tiles.push(tile(
                picker::mark(engine_brand(&e.id), None, 26.0),
                &e.label,
                Some("Engine"),
                w.pick == Pick::Engine(e.id.clone()),
                WizMsg::PickEngine(e.id.clone()),
            ));
        }
    }

    let mut col = column![grid(tiles)].spacing(12);
    col = col.push(
        row![
            ui::hairline(),
            ui::label("Or choose from ACP Registry"),
            ui::hairline(),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    );
    col = col.push(
        row![
            ui::btn_sm(
                "Enter manually",
                Some("terminal"),
                Kind::Ghost,
                Some(WizMsg::EnterManually)
            ),
            ui::btn_sm(
                "Local ACP command",
                Some("terminal"),
                Kind::Ghost,
                Some(WizMsg::PickLocal)
            ),
        ]
        .spacing(8),
    );
    col = col.push(field("Search the ACP Registry", &w.search, WizMsg::Search));
    col = col.push(registry_list(w));
    col.into()
}

/// Two tiles to a row, each filling half the width.
fn grid(tiles: Vec<Element<'_, WizMsg>>) -> Element<'_, WizMsg> {
    let mut rows = column![].spacing(8);
    let mut it = tiles.into_iter();
    while let Some(first) = it.next() {
        let second: Element<'_, WizMsg> = match it.next() {
            Some(s) => s,
            None => Space::new().width(Length::Fill).into(),
        };
        rows = rows.push(row![first, second].spacing(8));
    }
    rows.into()
}

/// One provider tile: the mark, the label, the "Engine" chip, and the check when it is the pick.
fn tile<'a>(
    mark: Element<'a, WizMsg>,
    label: &str,
    chip: Option<&str>,
    on: bool,
    msg: WizMsg,
) -> Element<'a, WizMsg> {
    let p = crate::theme::palette();
    let mut text_col = column![ui::sans(label.to_string(), 13.0, p.ink)].spacing(4);
    if let Some(chip) = chip {
        text_col = text_col.push(ui::badge(chip, Tone::Muted));
    }
    let mut inner = row![mark, text_col].spacing(10).align_y(Alignment::Center);
    if on {
        inner = inner
            .push(Space::new().width(Length::Fill))
            .push(ui::icon("check", 14.0, p.signal));
    }
    button(inner)
        .on_press(msg)
        .width(Length::Fill)
        .padding([10, 12])
        .style(tile_style(on))
        .into()
}

fn tile_style(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, status: button::Status| {
        let p = crate::theme::palette();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let fill = if on {
            p.signal_soft
        } else if hovered {
            p.hover
        } else {
            p.raised
        };
        button::Style {
            background: Some(fill.into()),
            text_color: p.ink,
            border: Border {
                color: if on { p.signal } else { p.rule_strong },
                width: if on { 2.0 } else { 1.0 },
                radius: crate::theme::skin().radius.into(),
            },
            ..Default::default()
        }
    }
}

/// The registry rows: the monogram, the name, the description on one line, the website link, and "Add" (or
/// "No Linux build" for an agent that cannot run in a VM). Empty while loading.
fn registry_list(w: &Wizard) -> Element<'_, WizMsg> {
    let Some(registry) = &w.registry else {
        return if w.registry_error.is_some() {
            Space::new().into()
        } else {
            ui::loading()
        };
    };
    let q = w.search.trim().to_lowercase();
    let rows: Vec<Element<'_, WizMsg>> = registry
        .agents
        .iter()
        .filter(|a| {
            q.is_empty()
                || a.name.to_lowercase().contains(&q)
                || a.id.to_lowercase().contains(&q)
                || a.description.to_lowercase().contains(&q)
        })
        .map(registry_row)
        .collect();
    if rows.is_empty() {
        return ui::faint("No agents match").into();
    }
    ui::scroll(column(rows).spacing(0))
        .height(Length::Fixed(240.0))
        .into()
}

fn registry_row(a: &RegistryAgent) -> Element<'_, WizMsg> {
    let p = crate::theme::palette();
    let info = column![
        ui::sans(a.name.clone(), 13.0, p.ink),
        ui::faint(ui::ellipsize(&a.description, 90)),
    ]
    .spacing(2)
    .width(Length::Fill);
    let mut line = row![picker::mark(&a.id, None, 28.0), info]
        .spacing(10)
        .align_y(Alignment::Center);
    if let Some(site) = &a.website {
        line = line.push(ui::btn_sm(
            "",
            Some("external-link"),
            Kind::Ghost,
            Some(WizMsg::OpenSite(site.clone())),
        ));
    }
    line = line.push(if a.runs_in_vm {
        ui::btn_sm(
            "Add",
            None,
            Kind::Secondary,
            Some(WizMsg::PickRegistry(a.id.clone())),
        )
    } else {
        ui::faint("No Linux build").into()
    });
    column![container(line).padding([8, 10]), ui::hairline(),].into()
}

/// Step 2: the label, the instance id (with its problem in crit when there is one), and the accent.
fn identity_step(w: &Wizard) -> Element<'_, WizMsg> {
    let p = crate::theme::palette();
    let id_note = match w.identity_problem() {
        Some(problem) if w.id_touched || !w.id.is_empty() => ui::sans(problem, 12.0, p.crit),
        _ => ui::faint("Lower-case letters, digits, hyphens and underscores."),
    };
    let accent_now = w.accent.as_deref();
    let mut accent_block = column![
        button(
            row![
                chip_swatch(accent_now.and_then(picker::accent_color), 18.0),
                ui::faint(accent_now.unwrap_or("None").to_string()),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .on_press(WizMsg::AccentOpen)
        .padding([4, 8])
        .style(tile_style(false))
    ]
    .spacing(8);
    if w.accent_open {
        let mut swatches = row![].spacing(6).align_y(Alignment::Center);
        for hex in ACCENTS {
            swatches = swatches.push(
                button(chip_swatch(picker::accent_color(hex), 20.0))
                    .on_press(WizMsg::Accent(Some(hex.to_string())))
                    .padding(2.0)
                    .style(tile_style(accent_now == Some(hex))),
            );
        }
        swatches = swatches.push(
            button(chip_swatch(None, 20.0))
                .on_press(WizMsg::Accent(None))
                .padding(2.0)
                .style(tile_style(accent_now.is_none())),
        );
        accent_block = accent_block.push(swatches);
    }
    column![
        setting(
            "Label",
            "Shown in the model picker and the providers list.",
            field("Label", &w.label, WizMsg::Label),
        ),
        ui::hairline(),
        setting(
            "Instance ID",
            "The hub and the vault name the instance by it.",
            column![field("instance-id", &w.id, WizMsg::Id), id_note]
                .spacing(4)
                .into(),
        ),
        ui::hairline(),
        setting(
            "Accent colour",
            "Optional. Tints the instance in the model picker.",
            accent_block.into(),
        ),
    ]
    .into()
}

/// A square swatch: the colour, or a raised square with a rule for None.
fn chip_swatch<'a>(color: Option<Color>, size: f32) -> Element<'a, WizMsg> {
    container(Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_: &Theme| {
            let p = crate::theme::palette();
            container::Style {
                background: Some(color.unwrap_or(p.raised).into()),
                border: Border {
                    color: p.rule,
                    width: 1.0,
                    radius: (size * 0.3).into(),
                },
                ..Default::default()
            }
        })
        .into()
}

/// Step 3: the fields of the family the pick belongs to.
fn config_step(w: &Wizard) -> Element<'_, WizMsg> {
    match &w.pick {
        Pick::Api(_) | Pick::Compatible => api_config(w),
        Pick::Engine(_) | Pick::Registry(_) => engine_config(w, false),
        Pick::Local => engine_config(w, true),
        Pick::None => ui::faint("Choose a provider on the first step.").into(),
    }
}

/// An API instance: the Base URL (when the kind has one), the key (optional, write-only), the models (optional).
fn api_config(w: &Wizard) -> Element<'_, WizMsg> {
    let mut col = column![].spacing(0);
    if w.is_local_provider() {
        let (placeholder, hint) = if w.api_kind() == "codex-app-server" {
            (
                "/path/to/codex",
                "The installed Codex program on this hub. Empty uses codex on PATH.",
            )
        } else {
            (
                "/usr/bin/opencode",
                "Absolute path to the installed OpenCode program on this hub. Empty uses /usr/bin/opencode.",
            )
        };
        col = col.push(setting(
            "Local executable",
            hint,
            field(placeholder, &w.executable, WizMsg::Executable),
        ));
        col = col.push(ui::hairline());
    }
    if w.shows_base() {
        let hint = if matches!(w.pick, Pick::Compatible) {
            "The endpoint the API answers on. Required for the OpenAI-compatible kind."
        } else {
            "The endpoint the API answers on."
        };
        col = col.push(setting("Base URL", hint, base_url_input(&w.base_url)));
        col = col.push(ui::hairline());
    }
    if w.api_kind() == "codex-app-server" {
        col = col.push(container(ui::faint("Uses the existing Codex login on this hub. Sign in with Codex before checking the provider.")).padding([10, 0]));
    } else {
        let key = text_input("Optional", &w.key)
            .on_input(WizMsg::Key)
            .secure(true)
            .padding(8)
            .size(13.0)
            .font(crate::theme::SANS)
            .style(crate::theme::input);
        col = col.push(setting(
            "Key",
            "Optional. Set it now or later. It goes into the hub's vault, never into an engine.",
            key.into(),
        ));
    }
    col = col.push(ui::hairline());
    let preset_models = w.preset().map(|p| p.models.join(", ")).unwrap_or_default();
    col = col.push(setting(
        "Models",
        "Optional, comma separated. Leave it empty to use the provider's list.",
        field(&preset_models, &w.models, WizMsg::Models),
    ));
    col.into()
}

fn base_url_input(value: &str) -> Element<'_, WizMsg> {
    text_input("https://", value)
        .id(BASE_URL_ID.clone())
        .on_input(WizMsg::BaseUrl)
        .padding(8)
        .size(13.0)
        .font(crate::theme::SANS)
        .style(crate::theme::input)
        .into()
}

/// An engine (a second built-in instance or a registry agent), or a local command: the version, the home folder,
/// the launch arguments, the variables, the route. The engine notes it runs only inside the VM.
fn engine_config(w: &Wizard, local: bool) -> Element<'_, WizMsg> {
    let p = crate::theme::palette();
    let mut col = column![].spacing(0);
    if local {
        col = col.push(setting(
            "Command",
            "One program the hub starts. Its arguments go below.",
            field("program", &w.command, WizMsg::Command),
        ));
    } else {
        col = col.push(setting(
            "Version",
            "pinned keeps the version the hub pinned, latest follows the newest, or type x.y.z.",
            field("pinned", &w.version, WizMsg::Version),
        ));
    }
    col = col.push(ui::hairline());
    if !local {
        col = col.push(setting(
            "Home folder",
            "A folder name for the engine's files. Empty for the default.",
            field("default", &w.home, WizMsg::Home),
        ));
        col = col.push(ui::hairline());
    }
    col = col.push(
        column![
            ui::label(if local {
                "Arguments"
            } else {
                "Launch arguments"
            }),
            text_editor(&w.args)
                .placeholder("One per line")
                .on_action(WizMsg::Args)
                .height(Length::Fixed(72.0))
                .font(crate::theme::MONO),
        ]
        .spacing(6)
        .padding([10, 0]),
    );
    col = col.push(ui::hairline());
    col = col.push(variables(w));
    col = col.push(ui::hairline());
    col = col.push(setting(
        "Route",
        "The model the engine runs through, provider/model. Empty uses the plaza default.",
        field("provider/model", &w.route, WizMsg::Route),
    ));
    column![
        col,
        ui::faint("An engine runs inside the agent's VM and reaches models only through the plaza gateway.")
            .color(p.ink_3),
    ]
    .spacing(10)
    .into()
}

/// The variables editor: a name and a value per row, a remove button, and "+ Add variable".
fn variables(w: &Wizard) -> Element<'_, WizMsg> {
    let mut rows = column![].spacing(6);
    for (i, (name, value)) in w.variables.iter().enumerate() {
        rows = rows.push(
            row![
                field("NAME", name, move |s| WizMsg::VarName(i, s)),
                field("value", value, move |s| WizMsg::VarValue(i, s)),
                ui::btn_sm("", Some("x"), Kind::Ghost, Some(WizMsg::VarRemove(i))),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    column![
        ui::label("Variables"),
        ui::faint("Plain settings for the engine. The agent can read them, so keys never go here."),
        rows,
        ui::btn_sm(
            "Add variable",
            Some("plus"),
            Kind::Secondary,
            Some(WizMsg::VarAdd)
        ),
    ]
    .spacing(6)
    .padding([10, 0])
    .into()
}

/// A settings row: the name and its hint on the left, the control at the right.
fn setting<'a>(name: &str, hint: &str, control: Element<'a, WizMsg>) -> Element<'a, WizMsg> {
    let p = crate::theme::palette();
    row![
        column![
            ui::sans(name.to_string(), crate::theme::TYPE_ITEM, p.ink),
            ui::faint(hint.to_string()),
        ]
        .spacing(3)
        .width(Length::Fill),
        container(control).width(Length::Fixed(300.0)),
    ]
    .spacing(24)
    .align_y(Alignment::Center)
    .padding([10, 0])
    .into()
}

/// A plain text field: the console's input, 13 px, the placeholder and the value borrowed from the dialog.
fn field<'a>(
    placeholder: &str,
    value: &'a str,
    on: impl Fn(String) -> WizMsg + 'a,
) -> Element<'a, WizMsg> {
    text_input(placeholder, value)
        .on_input(on)
        .padding(8)
        .size(13.0)
        .font(crate::theme::SANS)
        .style(crate::theme::input)
        .into()
}

impl std::fmt::Debug for Wizard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wizard")
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

/// The mark a built-in engine draws: its maker's (Claude Code is Claude's, Codex is OpenAI's).
fn engine_brand(id: &str) -> &str {
    match id {
        "codex" => "openai",
        "gemini" => "google",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog {
            api: vec![
                CatalogApi {
                    id: "openai".into(),
                    kind: "openai-responses".into(),
                    label: "OpenAI".into(),
                    brand: "openai".into(),
                    base_url: Some("https://api.openai.com/v1".into()),
                    auth: json!({"env": "OPENAI_API_KEY"}),
                    notes: String::new(),
                    models: vec!["gpt-6-luna".into()],
                },
                CatalogApi {
                    id: "azure-openai".into(),
                    kind: "azure-openai".into(),
                    label: "Azure OpenAI".into(),
                    brand: "azure".into(),
                    base_url: None,
                    auth: json!({"env": "AZURE_OPENAI_API_KEY"}),
                    notes: String::new(),
                    models: vec![],
                },
            ],
            engines: vec![providers_api::CatalogEngine {
                id: "codex".into(),
                label: "Codex".into(),
                source: "npm".into(),
                package: "@openai/codex".into(),
                pinned: "0.4.0".into(),
            }],
            registry: String::new(),
        }
    }

    fn wizard_with(pick: Pick) -> Wizard {
        let mut w = Wizard::blank();
        w.catalog = Some(catalog());
        w.set_pick(pick);
        w
    }

    #[test]
    fn instance_ids_follow_the_hub_rule() {
        assert!(valid_instance_id("openai"));
        assert!(valid_instance_id("openai_2"));
        assert!(valid_instance_id("my-hub"));
        assert!(valid_instance_id(&"a".repeat(MAX_ID)));
        assert!(!valid_instance_id(""));
        assert!(!valid_instance_id("OpenAI"));
        assert!(!valid_instance_id("a b"));
        assert!(!valid_instance_id("a.b"));
        assert!(!valid_instance_id(&"a".repeat(MAX_ID + 1)));
        assert!(!valid_instance_id("é"));
    }

    #[test]
    fn the_default_id_is_the_kind_then_kind_2_when_taken() {
        assert_eq!(default_id("openai", &[]), "openai");
        assert_eq!(default_id("openai", &["openai".to_string()]), "openai_2");
        assert_eq!(
            default_id("openai", &["openai".to_string(), "openai_2".to_string()]),
            "openai_3"
        );
        // A taken `openai_2` alone does not move the base id.
        assert_eq!(default_id("openai", &["openai_2".to_string()]), "openai");
        assert_eq!(default_id("Open AI", &[]), "open-ai");
    }

    #[test]
    fn identity_names_its_problem() {
        let taken = vec!["openai".to_string()];
        assert_eq!(
            identity_problem("", "x", &[]),
            Some("Give the provider a name.")
        );
        assert!(identity_problem("OpenAI", "Bad Id", &[]).is_some());
        assert_eq!(
            identity_problem("OpenAI", "openai", &taken),
            Some("Another provider or engine already uses this ID.")
        );
        assert_eq!(identity_problem("OpenAI", "openai_2", &taken), None);
    }

    #[test]
    fn the_provider_step_needs_a_pick_and_the_identity_step_a_valid_id() {
        let mut w = Wizard::blank();
        w.catalog = Some(catalog());
        assert!(!w.can_advance());
        w.set_pick(Pick::Api("openai".into()));
        assert!(w.can_advance());
        assert_eq!(w.id, "openai");
        assert_eq!(w.label, "OpenAI");
        w.advance();
        assert_eq!(w.step, Step::Identity);
        w.taken = vec!["openai".to_string()];
        w.apply_defaults();
        assert_eq!(w.id, "openai_2", "the default id skips a taken one");
        assert!(w.can_advance());
        w.id = "Bad ID".into();
        w.id_touched = true;
        assert!(!w.can_advance());
        w.advance();
        assert_eq!(w.step, Step::Identity, "an invalid id does not advance");
        w.back();
        assert_eq!(w.step, Step::Provider);
    }

    #[test]
    fn the_openai_compatible_kind_needs_a_base_url() {
        let mut w = Wizard::blank();
        w.catalog = Some(catalog());
        w.set_pick(Pick::Compatible);
        w.step = Step::Config;
        assert!(w.shows_base());
        assert!(!w.can_advance(), "no base URL yet");
        w.base_url = "api.example.test".into();
        w.base_touched = true;
        assert!(!w.can_advance(), "a base URL needs http or https");
        w.base_url = "https://api.example.test/v1".into();
        assert!(w.can_advance());
        assert_eq!(w.api_kind(), "openai-compatible");
        assert_eq!(w.default_base(), "openai-compatible");
        assert_eq!(w.id, "openai-compatible");
    }

    #[test]
    fn configure_manually_and_the_hub_button_open_the_compatible_kind() {
        let mut w = Wizard::blank();
        w.set_pick(Pick::Api("openai".into()));
        w.set_pick(Pick::Compatible);
        w.step = Step::Config;
        assert_eq!(w.step, Step::Config);
        assert!(matches!(w.pick, Pick::Compatible));
        assert_eq!(w.label, "OpenAI-compatible");
    }

    #[test]
    fn an_api_body_takes_the_presets_own_fields() {
        let raw = json!({"api": [{
            "id": "deepseek", "kind": "openai-compatible", "label": "DeepSeek",
            "auth": {"env": "DEEPSEEK_API_KEY"}, "options": {"replay_reasoning": true},
            "extra_headers": {"x-app": "plaza"}, "models": ["deepseek-flash", "deepseek-v4-pro"]
        }]});
        let plan = ApiPlan {
            id: "deepseek".into(),
            kind: "openai-compatible".into(),
            preset: Some("deepseek".into()),
            label: "DeepSeek".into(),
            accent: Some("#7ee0b5".into()),
            base_url: Some("https://api.deepseek.com".into()),
            models: vec![],
            executable: None,
        };
        let body = api_body(&plan, &raw);
        assert_eq!(body["auth"], json!({"env": "DEEPSEEK_API_KEY"}));
        assert_eq!(body["options"], json!({"replay_reasoning": true}));
        assert_eq!(body["extra_headers"], json!({"x-app": "plaza"}));
        assert_eq!(body["models"], json!(["deepseek-flash", "deepseek-v4-pro"]));
        assert_eq!(body["enabled"], json!(true));
        assert_eq!(body["accent"], json!("#7ee0b5"));

        // Typed models replace the preset's; a kind with no preset has no auth.
        let typed = ApiPlan {
            models: vec!["m1".into()],
            preset: None,
            ..plan.clone()
        };
        let body = api_body(&typed, &raw);
        assert_eq!(body["models"], json!(["m1"]));
        assert_eq!(body["auth"], json!("none"));
        assert_eq!(body["options"], Value::Null);
        let no_base = ApiPlan {
            base_url: None,
            ..typed
        };
        assert_eq!(api_body(&no_base, &raw)["base_url"], Value::Null);
    }

    #[test]
    fn local_executable_is_scoped_and_preserves_the_presets_options() {
        let raw = json!({"api": [{"id":"codex-local", "kind":"codex-app-server",
            "auth":"none", "options":{"working_directory":"/work"}}]});
        let plan = ApiPlan {
            id: "codex-local".into(),
            kind: "codex-app-server".into(),
            preset: Some("codex-local".into()),
            label: "Codex".into(),
            accent: None,
            base_url: None,
            models: vec![],
            executable: Some(" /opt/codex-desktop/resources/codex ".into()),
        };
        let body = api_body(&plan, &raw);
        assert_eq!(body["auth"], "none");
        assert_eq!(
            body["options"],
            json!({"working_directory":"/work", "executable":"/opt/codex-desktop/resources/codex"})
        );
        let remote = ApiPlan {
            kind: "openai".into(),
            preset: None,
            ..plan
        };
        assert_eq!(api_body(&remote, &raw)["options"], Value::Null);
        let mut w = Wizard::blank();
        w.set_pick(Pick::Api("opencode-local".into()));
        w.executable = "opencode".into();
        assert!(w.config_problem().is_some());
        w.executable = "/usr/bin/opencode".into();
        assert_eq!(w.config_problem(), None);
        w.key = "do-not-reuse".into();
        w.set_pick(Pick::Api("codex-app-server".into()));
        assert!(w.key.is_empty());
        assert!(w.executable.is_empty());
    }

    #[test]
    fn engines_and_commands_build_their_edit() {
        let mut w = wizard_with(Pick::Registry("claude-acp".into()));
        w.step = Step::Config;
        w.version.clear();
        w.id = "claude-acp".into();
        w.label = "Claude".into();
        w.home = "  ".into();
        w.variables = vec![
            (" FOO ".into(), "1".into()),
            (String::new(), "dropped".into()),
        ];
        let edit = w.engine_edit();
        assert_eq!(edit.engine.as_deref(), Some("acp:claude-acp"));
        assert_eq!(edit.version.as_deref(), Some("pinned"));
        assert_eq!(edit.command, None);
        assert_eq!(edit.home, None, "a blank home is left out");
        assert_eq!(edit.env.unwrap().get("FOO").map(String::as_str), Some("1"));

        let mut local = wizard_with(Pick::Local);
        local.step = Step::Config;
        assert_eq!(
            local.config_problem(),
            Some("Enter the command the engine starts with.")
        );
        local.command = " opencode acp ".into();
        assert!(local.can_advance());
        let edit = local.engine_edit();
        assert_eq!(edit.engine.as_deref(), Some("acp-local"));
        assert_eq!(edit.command.as_deref(), Some("opencode acp"));
        assert_eq!(edit.version, None);

        local.route = "anthropic".into();
        assert!(!local.can_advance(), "a route is provider/model");
    }

    #[test]
    fn models_and_lines_are_trimmed_and_deduplicated() {
        assert_eq!(parse_models(" a, b ,a,, c "), vec!["a", "b", "c"]);
        assert!(parse_models("  ").is_empty());
        assert_eq!(arg_lines("--x\n\n  --y \n"), vec!["--x", "--y"]);
    }

    #[test]
    fn the_base_url_shows_when_the_kind_has_an_endpoint() {
        let openai = wizard_with(Pick::Api("openai".into()));
        assert!(
            openai.shows_base(),
            "a preset with an endpoint shows it, prefilled"
        );
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
        let azure = wizard_with(Pick::Api("azure-openai".into()));
        assert!(azure.shows_base(), "Azure needs its own endpoint");
        assert_eq!(azure.base_url, "");
        let engine = wizard_with(Pick::Engine("codex".into()));
        assert!(!engine.shows_base());
    }
}
