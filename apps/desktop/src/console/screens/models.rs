//! The Models screen (parity with the web console's `models` view). Two tabs. Providers: the scope line;
//! the Providers card, the API provider instances and coding engines beside the selected one's detail (its
//! name, setup, runtime, environment or headers, models, removal); the Usage providers card (CLIProxyAPI hubs);
//! the Advanced card (the health check interval); then the decisions. Policy & defaults: the orchestrator and
//! worker model lists, the defaults and per-role models, the limits (with the fetch blocklist: a domain and its
//! subdomains, or `*.domain` for subdomains only) and the prices, saved whole. A key is typed into a secure
//! field and is never echoed or logged. Adding a provider is the wizard's (`provider_wizard.rs`): this screen
//! only sets `wizard_open`.
//!
//! Captures: `APZ_MODELS_TAB=policy` opens the Policy & defaults tab at start.

use std::collections::{BTreeMap, BTreeSet};

use iced::font::{Font, Weight};
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, column, container, pick_list, row, text, text_editor, text_input, toggler,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task, Theme};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::Id;
use super::platforms::ProjectChoice;
use super::projects;
use super::providers_api::{
    self as api, ApiInstance, EngineChange, EngineEdit, EngineInstance, EngineUpdate, Health,
    UsageProvider, UsageReading,
};
use crate::console::client::{self, Client, enc};
use crate::console::ui::{Kind, Tone};
use crate::console::{Ctx, ui};
use crate::theme::{self, SANS, TYPE_BADGE, TYPE_ITEM, TYPE_META, TYPE_SECTION, TYPE_UI};

const SANS_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..SANS
};

const EFFORTS: [&str; 8] = [
    "default", "none", "minimal", "low", "medium", "high", "xhigh", "max",
];
/// The web's `--fs-sm`: the lede and the faint notes of the Models screens.
const FS_SM: f32 = 12.5;
const DECISIONS_LEDE: &str = "Models that answer typed questions with calibrated probabilities instead of writing text. The plaza uses them to decide; agents never chat with them.";
/// The accents an instance takes: the web's eight swatches (the accent popover also offers none).
const ACCENTS: [&str; 8] = [
    "#5cc8ff", "#7ee0b5", "#b69cff", "#ff9ac1", "#ffd36e", "#6fe3e1", "#a3b8ff", "#c5f27a",
];
/// The width of the Providers card's list, which the detail sits beside (the web's 300px).
const LIST_W: f32 = 300.0;
/// The health check interval the hub starts with (`health_interval_s`), in seconds.
const DEFAULT_INTERVAL_S: i64 = 300;
/// The largest health check interval the hub takes, in seconds (a day).
const MAX_INTERVAL_S: i64 = 86_400;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Providers,
    Policy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListedModel {
    pub id: String,
}
/// The plaza's decision model (`GET /v1/decisions/default`); both null when none is set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DecisionDefault {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// A decision answer (`POST /v1/decisions`): the model the plaza reports and one answer per question.
#[derive(Debug, Clone, Deserialize)]
pub struct DecisionReply {
    #[serde(default)]
    pub model: String,
    pub answers: Vec<Answer>,
}

/// One answer, by question type. Choice and score carry a distribution over their options or levels.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Predicate {
        name: String,
        probability: f64,
    },
    Choice {
        name: String,
        choice: Value,
        #[serde(default)]
        probabilities: Vec<Weighted>,
        #[serde(default)]
        confidence: Option<f64>,
    },
    Score {
        name: String,
        score: f64,
        #[serde(default)]
        probabilities: Vec<Weighted>,
        #[serde(default)]
        confidence: Option<f64>,
    },
    Refusal {
        name: String,
    },
}

/// One option or level with its probability.
#[derive(Debug, Clone, Deserialize)]
pub struct Weighted {
    pub value: Value,
    #[serde(default)]
    pub label: Option<String>,
    pub probability: f64,
}

/// The question kinds the Try-it form asks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QKind {
    #[default]
    Predicate,
    Choice,
    Score,
}

impl QKind {
    fn key(self) -> &'static str {
        match self {
            QKind::Predicate => "predicate",
            QKind::Choice => "choice",
            QKind::Score => "score",
        }
    }
}

impl std::fmt::Display for QKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            QKind::Predicate => "Predicate",
            QKind::Choice => "Choice",
            QKind::Score => "Score",
        })
    }
}

/// The Try-it form: the input, one question, and the last answer.
#[derive(Debug)]
pub struct TryIt {
    input: text_editor::Content,
    kind: QKind,
    instructions: String,
    /// Comma-separated choices (choice) or levels (score), lowest first.
    options: String,
    asking: bool,
    error: Option<String>,
    answer: Option<DecisionReply>,
}

impl Default for TryIt {
    fn default() -> Self {
        TryIt {
            input: text_editor::Content::new(),
            kind: QKind::default(),
            instructions: String::new(),
            options: String::new(),
            asking: false,
            error: None,
            answer: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestReport {
    pub ok: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub trains_on_data: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelsView {
    #[serde(default)]
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Allowlists {
    #[serde(default)]
    pub orchestrator: Vec<String>,
    #[serde(default)]
    pub worker: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

/// apid's `/v1/settings`. Fields the screen does not show travel back untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub default_model: String,
    pub orchestrator_model: String,
    #[serde(default)]
    pub role_models: BTreeMap<String, String>,
    #[serde(default)]
    pub model_policy: Allowlists,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub max_turns: i64,
    #[serde(default)]
    pub max_tokens: i64,
    /// Minutes an idle agent waits before it sleeps; 0 keeps every agent awake (apid's `idle_sleep_min`).
    #[serde(default = "default_idle_sleep_min")]
    pub idle_sleep_min: i64,
    /// Seconds an idle agent waits before it dozes (its VM paused and paged out); 0 never dozes (apid's `doze_idle_s`).
    #[serde(default = "default_doze_idle_s")]
    pub doze_idle_s: i64,
    /// The roles that never sleep (apid's `sleep_never_roles`).
    #[serde(default = "default_sleep_never_roles")]
    pub sleep_never_roles: Vec<String>,
    /// Whether the hub takes a machine checkpoint before risky tool calls (apid's `vm_auto_checkpoints`).
    #[serde(default = "default_vm_auto_checkpoints")]
    pub vm_auto_checkpoints: bool,
    #[serde(default)]
    pub fetch_block: Vec<String>,
    #[serde(default)]
    pub prices: BTreeMap<String, Price>,
    /// The owner's confirmation that contributor models may be defaults, role models and policy entries.
    #[serde(default)]
    pub allow_training_models: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_models: Option<bool>,
    /// The coding engines the owner has enabled. The hub never keeps none.
    #[serde(default = "default_engines")]
    pub engines_enabled: Vec<String>,
    /// Whether editors may connect to the hub's ACP server. Off until the owner turns it on.
    #[serde(default)]
    pub acp_enabled: bool,
    /// Whether an editor offers to apply an agent's change to the editor's project. Off until the owner turns it on.
    #[serde(default)]
    pub acp_write_back: bool,
    /// Whether editors and coding engines that ask for protocol 2 get the ACP v2 draft. Off until the owner turns it on.
    #[serde(default)]
    pub acp_v2: bool,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// Carries the hub's ACP switches over to a save from the Models tab, which does not own them: the MCP hub tab does.
fn keep_acp_switches(copy: &mut Settings, hub: &Settings) {
    copy.acp_enabled = hub.acp_enabled;
    copy.acp_write_back = hub.acp_write_back;
    copy.acp_v2 = hub.acp_v2;
}

/// The coding engines a `code_engine` call may name, in the hub's order (`ENGINE_IDS` in apid's settings).
const ENGINE_IDS: [&str; 12] = [
    "claude", "codex", "opencode", "gemini", "qwen", "kilo", "cline", "copilot", "goose", "kimi",
    "auggie", "vibe",
];

/// The engines enabled out of the box (`DEFAULT_ENGINE_IDS` in apid's settings): Gemini CLI, Qwen Code,
/// Kilo Code, Cline, GitHub Copilot, Goose, Kimi Code and Auggie are off until the owner turns them on.
const DEFAULT_ENGINE_IDS: [&str; 3] = ["claude", "codex", "opencode"];

/// The name an engine's switch shows.
fn engine_label(id: &'static str) -> &'static str {
    match id {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "gemini" => "Gemini CLI",
        "qwen" => "Qwen Code",
        "kilo" => "Kilo Code",
        "cline" => "Cline",
        "copilot" => "GitHub Copilot",
        "goose" => "Goose",
        "kimi" => "Kimi Code",
        "auggie" => "Auggie",
        "vibe" => "Mistral Vibe",
        other => other,
    }
}

fn default_engines() -> Vec<String> {
    DEFAULT_ENGINE_IDS.into_iter().map(String::from).collect()
}

/// The engine list after `id` is switched `on`, in the hub's order. None when that would leave no engine on:
/// the hub refuses an empty list, so the switch is not taken.
pub fn engines_after(enabled: &[String], id: &str, on: bool) -> Option<Vec<String>> {
    let next: Vec<String> = ENGINE_IDS
        .into_iter()
        .filter(|&engine| {
            if engine == id {
                on
            } else {
                enabled.iter().any(|e| e.as_str() == engine)
            }
        })
        .map(String::from)
        .collect();
    (!next.is_empty()).then_some(next)
}

/// apid's defaults for the sleep and checkpoint settings, read when a hub that predates them omits them.
fn default_idle_sleep_min() -> i64 {
    10
}

fn default_doze_idle_s() -> i64 {
    0
}

fn default_sleep_never_roles() -> Vec<String> {
    vec!["orchestrator".to_string()]
}

fn default_vm_auto_checkpoints() -> bool {
    true
}

/// Which tier a list belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Orchestrator,
    Worker,
}

/// Which default a model picker sets.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    Orchestrator,
    Worker,
    Role(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Input,
    Output,
}

/// A numeric or price field of the policy.
#[derive(Debug, Clone, PartialEq)]
pub enum Num {
    MaxTurns,
    MaxTokens,
    IdleSleep,
    DozeIdle,
    Price(String, Side),
}

/// One choice in a model picker. An empty value means "inherit the tier default".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opt {
    value: String,
    label: String,
}

impl std::fmt::Display for Opt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The row the Providers tab's detail shows: an API provider instance, or a coding engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sel {
    Api(String),
    Engine(String),
}

/// What the owner is typing into the selected instance's detail, until it is saved.
#[derive(Debug, Default)]
pub struct Draft {
    name: String,
    base_url: String,
    version: String,
    home: String,
    args: text_editor::Content,
    /// The engine's variables, one (name, value) row each.
    env: Vec<(String, String)>,
    header_name: String,
    header_value: String,
    /// The "+ Add header" row is showing.
    header_open: bool,
    model: String,
    /// A runtime save is in flight: the hub is pinning the version.
    pinning: bool,
}

/// The dialog open over the providers tab.
#[derive(Debug, Clone, Default)]
pub enum Dialog {
    #[default]
    Closed,
    /// Set or replace an API provider's key: write-only, the secure field never echoes it.
    Key {
        provider: String,
        replace: bool,
        value: String,
        saving: bool,
    },
    /// Remove an API provider or an added engine.
    Remove { target: Sel, busy: bool },
    /// The engines a dry run would change, for the owner to confirm (`title` names how many).
    Update {
        title: String,
        changes: Vec<EngineChange>,
        busy: bool,
    },
    /// Add a CLIProxyAPI hub as a usage provider.
    Hub {
        url: String,
        key: String,
        label: String,
        busy: bool,
    },
}

impl Dialog {
    fn unbusy(&mut self) {
        match self {
            Dialog::Key { saving, .. } => *saving = false,
            Dialog::Remove { busy, .. }
            | Dialog::Update { busy, .. }
            | Dialog::Hub { busy, .. } => *busy = false,
            Dialog::Closed => {}
        }
    }
}

/// The policy being edited: the saved settings, the working copy, and what is in flight.
#[derive(Debug)]
pub struct Policy {
    saved: Settings,
    copy: Settings,
    models: Vec<ModelInfo>,
    known: BTreeSet<String>,
    /// Fields that do not hold a valid value.
    errors: BTreeSet<String>,
    /// What the user typed into each text field, so a half-typed value stays on screen.
    raw: BTreeMap<String, String>,
    /// The fetch blocklist editor: one host per line.
    block: text_editor::Content,
    add_orchestrator: String,
    add_worker: String,
    saving: bool,
}

impl Policy {
    fn new(settings: Settings, view: ModelsView) -> Policy {
        let block = text_editor::Content::with_text(&settings.fetch_block.join("\n"));
        let known: BTreeSet<String> = view
            .models
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.id))
            .collect();
        Policy {
            saved: settings.clone(),
            copy: settings,
            models: view.models,
            known,
            errors: BTreeSet::new(),
            raw: BTreeMap::new(),
            block,
            add_orchestrator: String::new(),
            add_worker: String::new(),
            saving: false,
        }
    }

    fn dirty(&self) -> bool {
        self.copy != self.saved
    }

    fn add_text(&self, tier: Tier) -> &str {
        match tier {
            Tier::Orchestrator => &self.add_orchestrator,
            Tier::Worker => &self.add_worker,
        }
    }

    fn add_mut(&mut self, tier: Tier) -> &mut String {
        match tier {
            Tier::Orchestrator => &mut self.add_orchestrator,
            Tier::Worker => &mut self.add_worker,
        }
    }

    /// The text a field shows: what the user typed, else the stored value.
    fn shown(&self, key: &str, stored: String) -> String {
        self.raw.get(key).cloned().unwrap_or(stored)
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    pub notice: Option<String>,
    tab: Tab,
    /// The API provider instances, the enabled ones first.
    api: Vec<ApiInstance>,
    /// The coding engines, the built-in ones first, as the hub lists them.
    engines: Vec<EngineInstance>,
    /// The CLIProxyAPI hubs (usage providers).
    usage: Vec<UsageProvider>,
    /// The projects the scope menu offers.
    projects: Vec<ProjectChoice>,
    /// Whether the scope pill's menu is open.
    scope_open: bool,
    /// The `provider/model` keys the hub reports as training on what is sent to them.
    trains: BTreeSet<String>,
    /// The health check interval in seconds (0 is off), and the favourite models as `provider/model`.
    interval_s: i64,
    favourites: Vec<String>,
    /// The row the detail shows, and what is being typed into it.
    selected: Option<Sel>,
    draft: Option<Draft>,
    runtime_open: bool,
    accent_open: bool,
    /// The last Test's answer, under its button.
    test_note: Option<String>,
    /// Each API provider's listed models, by id.
    listed: BTreeMap<String, Vec<ListedModel>>,
    dialog: Dialog,
    policy: Option<Policy>,
    prices_open: bool,
    /// Each listed model's price per 1M tokens (input, output), keyed `provider/model`; absent when none is published.
    model_prices: std::collections::HashMap<String, (f64, f64)>,
    /// The plaza's decision model as `provider/model`, as the poll last read it.
    decision_default: Option<String>,
    /// A decision model just picked, while its save is in flight.
    decision_pending: Option<String>,
    try_it: TryIt,
    /// The Add provider wizard (`provider_wizard.rs`) opens over this tab when `wizard_open` is set. `wizard_preset`
    /// names the kind it starts on: `openai-compatible` for a CLIProxyAPI hub.
    pub wizard_open: bool,
    pub wizard_preset: Option<String>,
    /// The open Add provider dialog.
    pub wizard: Option<super::provider_wizard::Wizard>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Tab(Tab),
    Loaded(Result<ProvidersLoad, String>),
    Select(Sel),
    /// Turns an instance on or off.
    Toggle(Sel, bool),
    ToggleAccent,
    /// Opens or shuts the scope menu.
    ToggleScope,
    /// Sets an instance's accent (None clears it).
    Accent(Sel, Option<String>),
    NameInput(String),
    /// Saves the display name (on Enter).
    NameSave,
    ToggleRuntime,
    RuntimeVersion(String),
    RuntimeHome(String),
    RuntimeArgs(text_editor::Action),
    /// Saves an engine's version, home folder and launch arguments.
    RuntimeSave(String),
    /// Puts an engine back on its catalogue pin.
    RevertPin(String),
    EnvName(usize, String),
    EnvValue(usize, String),
    EnvRemove(usize),
    EnvAdd,
    /// Saves an engine's variables.
    EnvSave(String),
    /// Sets an engine's route (the model it runs through; the empty value is the plaza default).
    RouteModel(String, Opt),
    BaseUrlInput(String),
    BaseUrlSave(String),
    HeaderName(String),
    HeaderValue(String),
    HeaderAdd(String),
    /// Shows the one name/value row under "+ Add header".
    HeaderOpen,
    HeaderClose,
    HeaderRemove(String, String),
    ModelDraft(String),
    ModelAdd(String),
    /// Toggles a model's favourite star (`provider/model`).
    Favourite(String),
    /// The hub's settings were written: the interval and the favourites follow them.
    SettingsSaved(Result<Settings, String>),
    /// Sets the health check interval, in seconds.
    Interval(i64),
    /// The scope menu: a project opens its Settings tab; All projects does nothing here.
    Scope(Option<String>),
    /// The Add provider wizard opens.
    AddProvider,
    /// A frame of the hub's event stream: the providers and engines refresh on their health and changes.
    Event(crate::console::events::Event),
    /// A message of the open Add provider dialog.
    Wizard(super::provider_wizard::WizMsg),
    /// The wizard opens on the OpenAI-compatible kind with its base URL focused (the CLIProxyAPI route).
    AddCliProxy,
    /// Checks every provider and engine now.
    CheckNow,
    /// Dry-runs the engine updates; the changes wait in a confirm dialog.
    UpdateAll,
    UpdateChecked(Result<EngineUpdate, String>),
    ConfirmUpdate,
    Test(String),
    Tested(Result<TestReport, String>),
    Listed(String, Result<Vec<ListedModel>, String>),
    /// A model's price (`GET /v1/prices/<provider>/<model>`), keyed `provider/model`.
    Price(String, Result<Value, String>),
    OpenKey(String),
    KeyInput(String),
    SaveKey,
    OpenRemove(Sel),
    ConfirmRemove,
    HubOpen,
    HubUrl(String),
    HubKey(String),
    HubLabel(String),
    HubAdd,
    HubRead(String),
    HubReading(String, Result<UsageReading, String>),
    HubRemove(String),
    Close,
    /// A save or removal finished: its notice, or its error.
    Done(Result<String, String>),
    PolicyLoaded(Result<(Settings, ModelsView), String>),
    PolicyAddInput(Tier, String),
    PolicyAdd(Tier),
    PolicyRemove(Tier, usize),
    PolicyDefault(Slot, Opt),
    PolicyEffort(String),
    PolicyAllowTraining(bool),
    PolicyEngine(String, bool),
    PolicyAutoCheckpoints(bool),
    PolicyRoles(String),
    PolicyNumber(Num, String),
    PolicyFetchBlock(text_editor::Action),
    TogglePrices,
    PolicySave,
    PolicySaved(Result<Settings, String>),
    /// The plaza's decision model, as the poll read it.
    DefaultLoaded(Result<DecisionDefault, String>),
    /// A decision model was picked (`provider/model`); its save's result.
    DecisionPick(String),
    DecisionSaved(Result<String, String>),
    TryKind(QKind),
    TryInstructions(String),
    TryOptions(String),
    TryInput(text_editor::Action),
    TryAsk,
    Asked(Result<DecisionReply, String>),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Scope(Some(project)) => Some(Id::ProjectTab(project.clone(), projects::Tab::Settings)),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick). The providers tab
/// reloads every time; the policy reloads only when there are no unsaved edits to lose.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    // Snapshot knob: `APZ_MODELS_TAB=policy` opens the Policy & defaults tab once, at start.
    static START_TAB: std::sync::Once = std::sync::Once::new();
    START_TAB.call_once(|| {
        if std::env::var("APZ_MODELS_TAB").is_ok_and(|t| t == "policy") {
            state.tab = Tab::Policy;
        }
    });
    match state.tab {
        Tab::Providers => Task::batch(vec![providers_task(ctx), default_task(ctx)]),
        Tab::Policy => match &state.policy {
            Some(p) if p.dirty() || p.saving => Task::none(),
            _ => policy_task(ctx),
        },
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Tab(tab) => {
            state.tab = tab;
            load(state, ctx)
        }
        Msg::Loaded(Ok(loaded)) => {
            state.api = loaded.api;
            state.engines = loaded.engines;
            state.usage = loaded.usage;
            state.projects = loaded.projects;
            if let Ok(view) = loaded.listing {
                state.trains = view
                    .models
                    .iter()
                    .filter(|m| m.trains_on_data)
                    .map(|m| format!("{}/{}", m.provider, m.id))
                    .collect();
            }
            if let Ok(settings) = &loaded.settings {
                state.interval_s = interval_of(settings);
                state.favourites = favourites_of(settings);
            }
            state.error = None;
            settle_selection(state, ctx)
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Select(sel) => select(state, Some(sel), ctx),
        Msg::Toggle(sel, on) => match sel {
            Sel::Api(id) => match api_of(state, &id) {
                Some(inst) => save_api(ctx, &inst, move |b| b["enabled"] = json!(on), "Saved"),
                None => Task::none(),
            },
            Sel::Engine(id) => save_engine(
                ctx,
                &id,
                EngineEdit {
                    enabled: Some(on),
                    ..Default::default()
                },
                "Saved",
            ),
        },
        Msg::ToggleAccent => {
            state.accent_open = !state.accent_open;
            Task::none()
        }
        Msg::Accent(sel, accent) => {
            state.accent_open = false;
            match sel {
                Sel::Api(id) => match api_of(state, &id) {
                    Some(inst) => {
                        save_api(ctx, &inst, move |b| b["accent"] = json!(accent), "Saved")
                    }
                    None => Task::none(),
                },
                Sel::Engine(id) => save_engine(
                    ctx,
                    &id,
                    EngineEdit {
                        accent: Some(accent),
                        ..Default::default()
                    },
                    "Saved",
                ),
            }
        }
        Msg::NameInput(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.name = v;
            }
            Task::none()
        }
        Msg::NameSave => {
            let (Some(sel), Some(name)) = (
                state.selected.clone(),
                state.draft.as_ref().map(|d| d.name.trim().to_string()),
            ) else {
                return Task::none();
            };
            if name.is_empty() {
                state.error = Some("Give it a name".to_string());
                return Task::none();
            }
            match sel {
                Sel::Api(id) => match api_of(state, &id) {
                    Some(inst) => save_api(ctx, &inst, move |b| b["label"] = json!(name), "Saved"),
                    None => Task::none(),
                },
                Sel::Engine(id) => save_engine(
                    ctx,
                    &id,
                    EngineEdit {
                        label: Some(Some(name)),
                        ..Default::default()
                    },
                    "Saved",
                ),
            }
        }
        Msg::ToggleRuntime => {
            state.runtime_open = !state.runtime_open;
            Task::none()
        }
        Msg::RuntimeVersion(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.version = v;
            }
            Task::none()
        }
        Msg::RuntimeHome(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.home = v;
            }
            Task::none()
        }
        Msg::RuntimeArgs(action) => {
            if let Some(d) = state.draft.as_mut() {
                d.args.perform(action);
            }
            Task::none()
        }
        Msg::RuntimeSave(id) => {
            let Some(d) = state.draft.as_mut() else {
                return Task::none();
            };
            let version = d.version.trim().to_string();
            let home = d.home.trim().to_string();
            let args: Vec<String> = d
                .args
                .text()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            d.pinning = true;
            save_engine(
                ctx,
                &id,
                EngineEdit {
                    version: Some(version).filter(|v| !v.is_empty()),
                    home: Some(Some(home).filter(|h| !h.is_empty())),
                    args: Some(args),
                    ..Default::default()
                },
                "Runtime saved",
            )
        }
        Msg::RevertPin(id) => save_engine(
            ctx,
            &id,
            EngineEdit {
                version: Some("pinned".to_string()),
                ..Default::default()
            },
            "Back on the pinned version",
        ),
        Msg::EnvName(i, v) => {
            if let Some(row) = state.draft.as_mut().and_then(|d| d.env.get_mut(i)) {
                row.0 = v;
            }
            Task::none()
        }
        Msg::EnvValue(i, v) => {
            if let Some(row) = state.draft.as_mut().and_then(|d| d.env.get_mut(i)) {
                row.1 = v;
            }
            Task::none()
        }
        Msg::EnvRemove(i) => {
            if let Some(d) = state.draft.as_mut().filter(|d| i < d.env.len()) {
                d.env.remove(i);
            }
            Task::none()
        }
        Msg::EnvAdd => {
            if let Some(d) = state.draft.as_mut() {
                d.env.push((String::new(), String::new()));
            }
            Task::none()
        }
        Msg::EnvSave(id) => {
            let env: BTreeMap<String, String> = state
                .draft
                .as_ref()
                .map(|d| {
                    d.env
                        .iter()
                        .map(|(k, v)| (k.trim().to_string(), v.clone()))
                        .filter(|(k, _)| !k.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            save_engine(
                ctx,
                &id,
                EngineEdit {
                    env: Some(env),
                    ..Default::default()
                },
                "Variables saved",
            )
        }
        Msg::RouteModel(id, opt) => save_engine(
            ctx,
            &id,
            EngineEdit {
                route: Some(Some(opt.value).filter(|v| !v.is_empty())),
                ..Default::default()
            },
            "Route saved",
        ),
        Msg::BaseUrlInput(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.base_url = v;
            }
            Task::none()
        }
        Msg::BaseUrlSave(id) => {
            let base = state
                .draft
                .as_ref()
                .map(|d| d.base_url.trim().to_string())
                .unwrap_or_default();
            match api_of(state, &id) {
                Some(inst) => save_api(
                    ctx,
                    &inst,
                    move |b| b["base_url"] = json!(Some(base).filter(|s| !s.is_empty())),
                    "Saved",
                ),
                None => Task::none(),
            }
        }
        Msg::HeaderName(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.header_name = v;
            }
            Task::none()
        }
        Msg::HeaderValue(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.header_value = v;
            }
            Task::none()
        }
        Msg::HeaderAdd(id) => {
            let Some(d) = state.draft.as_mut() else {
                return Task::none();
            };
            let name = d.header_name.trim().to_string();
            let value = std::mem::take(&mut d.header_value);
            if name.is_empty() {
                state.error = Some("Give the header a name".to_string());
                return Task::none();
            }
            d.header_name.clear();
            d.header_open = false;
            match api_of(state, &id) {
                Some(inst) => save_api(
                    ctx,
                    &inst,
                    move |b| {
                        with_headers(b, |h| {
                            h.insert(name, json!(value));
                        })
                    },
                    "Header added",
                ),
                None => Task::none(),
            }
        }
        Msg::HeaderRemove(id, name) => match api_of(state, &id) {
            Some(inst) => save_api(
                ctx,
                &inst,
                move |b| {
                    with_headers(b, |h| {
                        h.remove(&name);
                    })
                },
                "Header removed",
            ),
            None => Task::none(),
        },
        Msg::ModelDraft(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.model = v;
            }
            Task::none()
        }
        Msg::ModelAdd(id) => {
            let Some(d) = state.draft.as_mut() else {
                return Task::none();
            };
            let model = d.model.trim().to_string();
            if model.is_empty() {
                return Task::none();
            }
            d.model.clear();
            match api_of(state, &id) {
                Some(inst) => save_api(
                    ctx,
                    &inst,
                    move |b| {
                        let mut models = b["models"].as_array().cloned().unwrap_or_default();
                        models.push(json!(model));
                        b["models"] = Value::Array(models);
                    },
                    "Model added",
                ),
                None => Task::none(),
            }
        }
        Msg::Favourite(key) => {
            let on = state.favourites.contains(&key);
            let c = ctx.client.clone();
            client::task(
                move || {
                    patch_settings(&c, |s| {
                        let mut favourites = favourites_of(s);
                        if on {
                            favourites.retain(|k| k != &key);
                        } else {
                            favourites.push(key);
                        }
                        s.rest
                            .insert("favourite_models".to_string(), json!(favourites));
                    })
                },
                Msg::SettingsSaved,
            )
        }
        Msg::SettingsSaved(Ok(saved)) => {
            state.interval_s = interval_of(&saved);
            state.favourites = favourites_of(&saved);
            // The policy tab keeps the settings it was loaded with: the fields it does not show follow the hub.
            if let Some(p) = state.policy.as_mut() {
                p.saved.rest = saved.rest.clone();
                p.copy.rest = saved.rest.clone();
            }
            state.error = None;
            Task::none()
        }
        Msg::SettingsSaved(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Interval(v) => {
            let v = v.clamp(0, MAX_INTERVAL_S);
            state.interval_s = v;
            let c = ctx.client.clone();
            client::task(
                move || {
                    patch_settings(&c, |s| {
                        s.rest.insert("health_interval_s".to_string(), json!(v));
                    })
                },
                Msg::SettingsSaved,
            )
        }
        Msg::ToggleScope => {
            state.scope_open = !state.scope_open;
            Task::none()
        }
        Msg::Scope(_) => {
            state.scope_open = false;
            Task::none()
        }
        Msg::HeaderOpen => {
            if let Some(d) = state.draft.as_mut() {
                d.header_open = true;
            }
            Task::none()
        }
        Msg::HeaderClose => {
            if let Some(d) = state.draft.as_mut() {
                d.header_open = false;
                d.header_name.clear();
                d.header_value.clear();
            }
            Task::none()
        }
        Msg::AddProvider | Msg::AddCliProxy => {
            let preset = matches!(msg, Msg::AddCliProxy).then(|| "openai-compatible".to_string());
            state.wizard_open = true;
            state.wizard_preset = preset.clone();
            let (wizard, task) = super::provider_wizard::open(&ctx.client, preset);
            state.wizard = Some(wizard);
            task.map(Msg::Wizard)
        }
        Msg::Event(event) => {
            let ours = matches!(
                event.kind.as_str(),
                "providers.health" | "engines.health" | "engines.changed" | "provider"
            );
            if ours && state.tab == Tab::Providers {
                load(state, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Wizard(m) => {
            let Some(wizard) = state.wizard.as_mut() else {
                return Task::none();
            };
            let (task, outcome) = super::provider_wizard::update(wizard, m, &ctx.client);
            let task = task.map(Msg::Wizard);
            match outcome {
                Some(super::provider_wizard::Outcome::Added(id)) => {
                    state.wizard = None;
                    state.wizard_open = false;
                    state.notice = Some(format!("Added {id}"));
                    Task::batch([task, load(state, ctx)])
                }
                Some(super::provider_wizard::Outcome::Cancelled) => {
                    state.wizard = None;
                    state.wizard_open = false;
                    task
                }
                None => task,
            }
        }
        Msg::CheckNow => {
            let c = ctx.client.clone();
            client::task(
                move || {
                    api::check_all(&c)?;
                    api::check_engines(&c)?;
                    load_providers(&c)
                },
                Msg::Loaded,
            )
        }
        Msg::UpdateAll => {
            let c = ctx.client.clone();
            client::task(move || api::update_engines(&c, true), Msg::UpdateChecked)
        }
        Msg::UpdateChecked(Ok(update)) => {
            if update.changes.is_empty() {
                state.notice = Some("Everything is up to date".to_string());
            } else {
                let n = update.changes.len();
                let noun = if n == 1 { "engine" } else { "engines" };
                state.dialog = Dialog::Update {
                    title: format!("Update {n} {noun}?"),
                    changes: update.changes,
                    busy: false,
                };
            }
            Task::none()
        }
        Msg::UpdateChecked(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::ConfirmUpdate => {
            let Dialog::Update { busy, .. } = &mut state.dialog else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let c = ctx.client.clone();
            client::task(
                move || {
                    api::update_engines(&c, false)
                        .map(|u| format!("Updated {} engines", u.changes.len()))
                },
                Msg::Done,
            )
        }
        Msg::Test(id) => {
            let c = ctx.client.clone();
            client::task(move || test_provider(&c, &id), Msg::Tested)
        }
        Msg::Tested(Ok(report)) => {
            if report.ok {
                state.test_note = Some(format!("OK · {} ms · {}", report.latency_ms, report.model));
                state.error = None;
            } else {
                state.test_note = None;
                state.error = Some(report.error.unwrap_or_else(|| "Test failed".to_string()));
            }
            Task::none()
        }
        Msg::Tested(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Listed(id, Ok(list)) => {
            let tasks: Vec<Task<Msg>> = list
                .iter()
                .map(|m| {
                    let c = ctx.client.clone();
                    let key = format!("{id}/{}", m.id);
                    let path = price_path(&key);
                    client::task(
                        move || c.get::<Value>(&path),
                        move |r| Msg::Price(key.clone(), r),
                    )
                })
                .collect();
            state.listed.insert(id, list);
            Task::batch(tasks)
        }
        Msg::Price(key, Ok(v)) => {
            let input = v.pointer("/price/input").and_then(Value::as_f64);
            let output = v.pointer("/price/output").and_then(Value::as_f64);
            if let (Some(i), Some(o)) = (input, output) {
                state.model_prices.insert(key, (i, o));
            }
            Task::none()
        }
        Msg::Price(_, Err(_)) => Task::none(),
        Msg::Listed(_, Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::OpenKey(id) => {
            let replace = api_of(state, &id)
                .is_some_and(|a| matches!(a.credential.as_str(), "secret" | "sealed"));
            state.dialog = Dialog::Key {
                provider: id,
                replace,
                value: String::new(),
                saving: false,
            };
            Task::none()
        }
        Msg::KeyInput(v) => {
            if let Dialog::Key { value, .. } = &mut state.dialog {
                *value = v;
            }
            Task::none()
        }
        Msg::SaveKey => {
            let Dialog::Key {
                provider,
                value,
                saving,
                ..
            } = &mut state.dialog
            else {
                return Task::none();
            };
            let key = value.trim().to_string();
            if key.is_empty() || *saving {
                return Task::none();
            }
            *saving = true;
            let provider = provider.clone();
            let c = ctx.client.clone();
            client::task(
                move || api::set_key(&c, &provider, &key).map(|_| "Key saved".to_string()),
                Msg::Done,
            )
        }
        Msg::OpenRemove(target) => {
            state.dialog = Dialog::Remove {
                target,
                busy: false,
            };
            Task::none()
        }
        Msg::ConfirmRemove => {
            let Dialog::Remove { target, busy } = &mut state.dialog else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let target = target.clone();
            let c = ctx.client.clone();
            match target {
                Sel::Api(id) => client::task(
                    move || api::remove(&c, &id).map(|_| format!("Removed {id}")),
                    Msg::Done,
                ),
                Sel::Engine(id) => client::task(
                    move || api::remove_engine(&c, &id).map(|_| format!("Removed {id}")),
                    Msg::Done,
                ),
            }
        }
        Msg::HubOpen => {
            state.dialog = Dialog::Hub {
                url: String::new(),
                key: String::new(),
                label: String::new(),
                busy: false,
            };
            Task::none()
        }
        Msg::HubUrl(v) => {
            if let Dialog::Hub { url, .. } = &mut state.dialog {
                *url = v;
            }
            Task::none()
        }
        Msg::HubKey(v) => {
            if let Dialog::Hub { key, .. } = &mut state.dialog {
                *key = v;
            }
            Task::none()
        }
        Msg::HubLabel(v) => {
            if let Dialog::Hub { label, .. } = &mut state.dialog {
                *label = v;
            }
            Task::none()
        }
        Msg::HubAdd => {
            let Dialog::Hub {
                url,
                key,
                label,
                busy,
            } = &mut state.dialog
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let url = url.trim().to_string();
            let key = key.trim().to_string();
            let label = Some(label.trim().to_string()).filter(|s| !s.is_empty());
            if url.is_empty() || key.is_empty() {
                state.error = Some("Give the hub's address and its management key".to_string());
                return Task::none();
            }
            *busy = true;
            let c = ctx.client.clone();
            client::task(
                move || {
                    api::add_usage(&c, &url, &key, label.as_deref())
                        .map(|_| "Hub added".to_string())
                },
                Msg::Done,
            )
        }
        Msg::HubRead(id) => {
            let c = ctx.client.clone();
            let id2 = id.clone();
            client::task(
                move || api::read_usage(&c, &id),
                move |r| Msg::HubReading(id2.clone(), r),
            )
        }
        Msg::HubReading(id, Ok(reading)) => {
            if let Some(hub) = state.usage.iter_mut().find(|h| h.id == id) {
                hub.last = Some(reading);
            }
            Task::none()
        }
        Msg::HubReading(_, Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::HubRemove(id) => {
            let c = ctx.client.clone();
            client::task(
                move || api::remove_usage(&c, &id).map(|_| "Hub removed".to_string()),
                Msg::Done,
            )
        }
        Msg::Close => {
            state.dialog = Dialog::Closed;
            Task::none()
        }
        Msg::Done(Ok(note)) => {
            state.dialog = Dialog::Closed;
            if let Some(d) = state.draft.as_mut() {
                d.pinning = false;
            }
            state.notice = Some(note);
            state.error = None;
            // The models of the selected provider may have changed: list them again.
            state.listed.clear();
            let mut tasks = vec![providers_task(ctx)];
            if let Some(Sel::Api(id)) = state.selected.clone() {
                tasks.push(listed_task(ctx, id));
            }
            Task::batch(tasks)
        }
        Msg::Done(Err(e)) => {
            state.dialog.unbusy();
            if let Some(d) = state.draft.as_mut() {
                d.pinning = false;
            }
            state.error = Some(e);
            Task::none()
        }
        Msg::PolicyLoaded(Ok((settings, view))) => {
            if state
                .policy
                .as_ref()
                .is_none_or(|p| !p.dirty() && !p.saving)
            {
                state.policy = Some(Policy::new(settings, view));
            }
            Task::none()
        }
        Msg::PolicyLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::PolicyAddInput(tier, v) => {
            if let Some(p) = state.policy.as_mut() {
                *p.add_mut(tier) = v;
            }
            Task::none()
        }
        Msg::PolicyAdd(tier) => {
            let Some(p) = state.policy.as_mut() else {
                return Task::none();
            };
            let value = p.add_text(tier).trim().to_string();
            if value.is_empty() {
                return Task::none();
            }
            if !valid_entry(&value) {
                state.error = Some("Use provider/model, with an optional @effort".to_string());
                return Task::none();
            }
            let list = tier_list_mut(&mut p.copy, tier);
            if !list.contains(&value) {
                list.push(value);
            }
            p.add_mut(tier).clear();
            state.error = None;
            Task::none()
        }
        Msg::PolicyRemove(tier, index) => {
            if let Some(p) = state.policy.as_mut() {
                let list = tier_list_mut(&mut p.copy, tier);
                if index < list.len() {
                    list.remove(index);
                }
            }
            Task::none()
        }
        Msg::PolicyDefault(slot, opt) => {
            if let Some(p) = state.policy.as_mut() {
                match slot {
                    Slot::Orchestrator => p.copy.orchestrator_model = opt.value,
                    Slot::Worker => p.copy.default_model = opt.value,
                    Slot::Role(role) => {
                        if opt.value.is_empty() {
                            p.copy.role_models.remove(&role);
                        } else {
                            p.copy.role_models.insert(role, opt.value);
                        }
                    }
                }
            }
            Task::none()
        }
        Msg::PolicyAllowTraining(on) => {
            if let Some(p) = state.policy.as_mut() {
                p.copy.allow_training_models = on;
            }
            Task::none()
        }
        Msg::PolicyEngine(id, on) => {
            // A switch that would leave no engine on is not taken; the checkbox keeps its place.
            if let Some(p) = state.policy.as_mut()
                && let Some(next) = engines_after(&p.copy.engines_enabled, &id, on)
            {
                p.copy.engines_enabled = next;
            }
            Task::none()
        }
        Msg::PolicyAutoCheckpoints(on) => {
            if let Some(p) = state.policy.as_mut() {
                p.copy.vm_auto_checkpoints = on;
            }
            Task::none()
        }
        Msg::PolicyRoles(text) => {
            let Some(p) = state.policy.as_mut() else {
                return Task::none();
            };
            let key = "sleep_never_roles".to_string();
            match role_names(&text) {
                Some(roles) => {
                    p.copy.sleep_never_roles = roles;
                    p.errors.remove(&key);
                }
                None => {
                    p.errors.insert(key.clone());
                }
            }
            p.raw.insert(key, text);
            Task::none()
        }
        Msg::PolicyEffort(v) => {
            if let Some(p) = state.policy.as_mut() {
                p.copy.effort = if v == "default" { None } else { Some(v) };
            }
            Task::none()
        }
        Msg::PolicyNumber(num, text) => {
            let Some(p) = state.policy.as_mut() else {
                return Task::none();
            };
            let key = match &num {
                Num::MaxTurns => "max_turns".to_string(),
                Num::MaxTokens => "max_tokens".to_string(),
                Num::IdleSleep => "idle_sleep_min".to_string(),
                Num::DozeIdle => "doze_idle_s".to_string(),
                Num::Price(k, side) => price_key(k, *side),
            };
            let ok = match &num {
                Num::MaxTurns => match text.trim().parse::<i64>() {
                    Ok(n) if (1..=200).contains(&n) => {
                        p.copy.max_turns = n;
                        true
                    }
                    _ => false,
                },
                Num::MaxTokens => match text.trim().parse::<i64>() {
                    Ok(n) if (0..=1_000_000_000).contains(&n) => {
                        p.copy.max_tokens = n;
                        true
                    }
                    _ => false,
                },
                Num::IdleSleep => match idle_minutes(&text) {
                    Some(n) => {
                        p.copy.idle_sleep_min = n;
                        true
                    }
                    None => false,
                },
                Num::DozeIdle => match doze_seconds(&text) {
                    Some(n) => {
                        p.copy.doze_idle_s = n;
                        true
                    }
                    None => false,
                },
                Num::Price(k, side) => match text.trim().parse::<f64>() {
                    Ok(n) if n.is_finite() && n >= 0.0 => {
                        let price = p.copy.prices.entry(k.clone()).or_default();
                        match side {
                            Side::Input => price.input = n,
                            Side::Output => price.output = n,
                        }
                        true
                    }
                    _ => false,
                },
            };
            p.raw.insert(key.clone(), text);
            if ok {
                p.errors.remove(&key);
            } else {
                p.errors.insert(key);
            }
            Task::none()
        }
        Msg::PolicyFetchBlock(action) => {
            if let Some(p) = state.policy.as_mut() {
                p.block.perform(action);
                p.copy.fetch_block = split_list(&p.block.text());
            }
            Task::none()
        }
        Msg::TogglePrices => {
            state.prices_open = !state.prices_open;
            Task::none()
        }
        Msg::PolicySave => {
            let Some(p) = state.policy.as_mut() else {
                return Task::none();
            };
            if p.saving || !p.errors.is_empty() || !p.dirty() {
                return Task::none();
            }
            p.saving = true;
            let mut s = p.copy.clone();
            let c = ctx.client.clone();
            client::task(
                move || {
                    // The ACP switch lives on the MCP hub tab, so this save keeps the hub's current value
                    // rather than the one this copy was loaded with.
                    keep_acp_switches(&mut s, &c.get::<Settings>("/v1/settings")?);
                    save_settings(&c, &s)
                },
                Msg::PolicySaved,
            )
        }
        Msg::PolicySaved(Ok(saved)) => {
            if let Some(p) = state.policy.as_mut() {
                p.copy = saved.clone();
                p.block = text_editor::Content::with_text(&saved.fetch_block.join("\n"));
                p.saved = saved;
                p.raw.clear();
                p.errors.clear();
                p.saving = false;
            }
            state.notice = Some("Policy saved".to_string());
            state.error = None;
            Task::none()
        }
        Msg::PolicySaved(Err(e)) => {
            if let Some(p) = state.policy.as_mut() {
                p.saving = false;
            }
            state.error = Some(e);
            Task::none()
        }
        Msg::DefaultLoaded(Ok(d)) => {
            state.decision_default = d.provider.zip(d.model).map(|(p, m)| format!("{p}/{m}"));
            Task::none()
        }
        Msg::DefaultLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::DecisionPick(id) => {
            let Some((provider, model)) = id.split_once('/') else {
                return Task::none();
            };
            let body = json!({ "provider": provider, "model": model });
            state.decision_pending = Some(id.clone());
            let c = ctx.client.clone();
            client::task(
                move || {
                    c.call("PUT", "/v1/decisions/default", Some(&body))
                        .map(|_| id)
                },
                Msg::DecisionSaved,
            )
        }
        Msg::DecisionSaved(Ok(id)) => {
            state.decision_default = Some(id);
            state.decision_pending = None;
            state.error = None;
            Task::none()
        }
        Msg::DecisionSaved(Err(e)) => {
            state.decision_pending = None;
            state.error = Some(e);
            Task::none()
        }
        Msg::TryKind(kind) => {
            state.try_it.kind = kind;
            Task::none()
        }
        Msg::TryInstructions(s) => {
            state.try_it.instructions = s;
            Task::none()
        }
        Msg::TryOptions(s) => {
            state.try_it.options = s;
            Task::none()
        }
        Msg::TryInput(action) => {
            state.try_it.input.perform(action);
            Task::none()
        }
        Msg::TryAsk => {
            let Some(target) = state.decision_default.clone() else {
                state.try_it.error = Some("Pick a decision model first".to_string());
                return Task::none();
            };
            let t = &mut state.try_it;
            if t.asking {
                return Task::none();
            }
            let input = t.input.text().trim().to_string();
            if input.is_empty() {
                t.error = Some("Write the input the question is about".to_string());
                return Task::none();
            }
            let question = match question_json(t.kind, &t.instructions, &t.options) {
                Ok(q) => q,
                Err(e) => {
                    t.error = Some(e);
                    return Task::none();
                }
            };
            let Some((provider, model)) = target.split_once('/') else {
                return Task::none();
            };
            let body = json!({ "provider": provider, "model": model, "input": input, "questions": [question] });
            t.asking = true;
            t.error = None;
            let c = ctx.client.clone();
            client::task(
                move || c.post::<DecisionReply>("/v1/decisions", &body),
                Msg::Asked,
            )
        }
        Msg::Asked(Ok(reply)) => {
            state.try_it.asking = false;
            state.try_it.error = None;
            state.try_it.answer = Some(reply);
            Task::none()
        }
        Msg::Asked(Err(e)) => {
            state.try_it.asking = false;
            state.try_it.error = Some(e);
            Task::none()
        }
    }
}

/// The hub's event stream while the screen is shown: health rounds and engine changes refresh the Providers tab.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let tabs: Element<'a, Msg> = tabs_strip(state);
    let mut parts: Vec<Element<'a, Msg>> = vec![tabs];
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        parts.push(ui::faint(n.as_str()).into());
    }
    parts.push(match state.tab {
        Tab::Providers => providers_view(state),
        Tab::Policy => policy_view(state),
    });
    // The head holds the screen's action on the policy tab (Save); the providers tab adds through its list.
    let actions: Vec<Element<'a, Msg>> = match state.tab {
        Tab::Providers => Vec::new(),
        Tab::Policy => {
            let save_enabled = state
                .policy
                .as_ref()
                .is_some_and(|p| !p.saving && p.errors.is_empty() && p.dirty());
            vec![ui::btn(
                "Save",
                Some("check"),
                Kind::Primary,
                save_enabled.then_some(Msg::PolicySave),
            )]
        }
    };
    let page = ui::sheet(
        ui::view_head("Models", actions),
        column(parts).spacing(20).into(),
    );
    let page = ui::with_modal(page, dialog_view(state));
    // The Add provider dialog lies over everything else on the screen.
    ui::with_modal(
        page,
        state
            .wizard
            .as_ref()
            .map(|w| super::provider_wizard::view(w).map(Msg::Wizard)),
    )
}

/// The tab strip: the kit's underline tabs, each underline as wide as its label.
fn tabs_strip<'a>(state: &'a State) -> Element<'a, Msg> {
    ui::tabs(
        &[
            (Tab::Providers, "Providers"),
            (Tab::Policy, "Policy & defaults"),
        ],
        state.tab,
        Msg::Tab,
    )
}

/// What the providers tab loads: the instances, the engines, the usage hubs, the listing (the providers whose models
/// could not be listed, or why the listing itself could not be read), the settings and the projects. Only the
/// instances, the engines and the hubs failing fails the load.
#[derive(Debug, Clone)]
pub struct ProvidersLoad {
    api: Vec<ApiInstance>,
    engines: Vec<EngineInstance>,
    usage: Vec<UsageProvider>,
    listing: Result<ModelsView, String>,
    settings: Result<Settings, String>,
    projects: Vec<ProjectChoice>,
}

fn load_providers(c: &Client) -> Result<ProvidersLoad, String> {
    Ok(ProvidersLoad {
        api: api::list(c)?,
        engines: api::engines(c)?,
        usage: api::usage(c)?,
        listing: c.get::<ModelsView>("/v1/models"),
        settings: c.get::<Settings>("/v1/settings"),
        projects: c
            .get::<Vec<ProjectChoice>>("/v1/projects")
            .unwrap_or_default(),
    })
}

fn providers_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(move || load_providers(&c), Msg::Loaded)
}

/// The API provider instance `id`, as the hub holds it.
fn api_of(state: &State, id: &str) -> Option<ApiInstance> {
    state.api.iter().find(|a| a.id == id).cloned()
}

/// The API provider instances in the list's order: the enabled ones first, the hub's order within each group.
fn api_rows(api: &[ApiInstance]) -> Vec<&ApiInstance> {
    let mut rows: Vec<&ApiInstance> = api.iter().collect();
    rows.sort_by_key(|a| !a.enabled);
    rows
}

/// The row the detail opens on at first: the first enabled API provider, else the first provider, else the first
/// engine.
fn first_sel(api: &[ApiInstance], engines: &[EngineInstance]) -> Option<Sel> {
    api.iter()
        .find(|a| a.enabled)
        .or(api.first())
        .map(|a| Sel::Api(a.id.clone()))
        .or_else(|| engines.first().map(|e| Sel::Engine(e.id.clone())))
}

/// Keeps the selection while its row exists; otherwise the first row is selected.
fn settle_selection(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let kept = match &state.selected {
        Some(Sel::Api(id)) => state.api.iter().any(|a| &a.id == id),
        Some(Sel::Engine(id)) => state.engines.iter().any(|e| &e.id == id),
        None => false,
    };
    if kept {
        return Task::none();
    }
    let first = first_sel(&state.api, &state.engines);
    select(state, first, ctx)
}

/// Shows `sel` in the detail: its drafts start from what the hub holds, and an API provider's models are listed.
fn select(state: &mut State, sel: Option<Sel>, ctx: &Ctx) -> Task<Msg> {
    let draft = sel.as_ref().and_then(|s| draft_for(state, s));
    state.draft = draft;
    state.selected = sel.clone();
    state.runtime_open = false;
    state.accent_open = false;
    state.test_note = None;
    match sel {
        Some(Sel::Api(id)) if !state.listed.contains_key(&id) => listed_task(ctx, id),
        _ => Task::none(),
    }
}

/// The drafts a row starts with: the values the hub holds.
fn draft_for(state: &State, sel: &Sel) -> Option<Draft> {
    match sel {
        Sel::Api(id) => state.api.iter().find(|a| &a.id == id).map(|a| Draft {
            name: a.label.clone(),
            base_url: a.base_url.clone().unwrap_or_default(),
            ..Draft::default()
        }),
        Sel::Engine(id) => state.engines.iter().find(|e| &e.id == id).map(|e| Draft {
            name: e.label.clone(),
            version: e.version.clone().unwrap_or_default(),
            home: e.home.clone(),
            args: text_editor::Content::with_text(&e.args.join("\n")),
            env: e.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ..Draft::default()
        }),
    }
}

/// Writes an API provider instance: `change` edits the whole PUT body (the hub restores the redacted values).
fn save_api(
    ctx: &Ctx,
    inst: &ApiInstance,
    change: impl FnOnce(&mut Value) + Send + 'static,
    note: &str,
) -> Task<Msg> {
    let c = ctx.client.clone();
    let inst = inst.clone();
    let note = note.to_string();
    client::task(
        move || {
            let body = inst.put_body(change);
            api::replace(&c, &inst.id, &body).map(|_| note)
        },
        Msg::Done,
    )
}

/// Writes an engine instance's fields; the hub pins the package first when the version changes.
fn save_engine(ctx: &Ctx, id: &str, edit: EngineEdit, note: &str) -> Task<Msg> {
    let c = ctx.client.clone();
    let id = id.to_string();
    let note = note.to_string();
    client::task(
        move || api::edit_engine(&c, &id, &edit).map(|_| note),
        Msg::Done,
    )
}

/// Changes the headers an API provider's PUT body carries.
fn with_headers(body: &mut Value, change: impl FnOnce(&mut Map<String, Value>)) {
    let mut headers = body["extra_headers"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    change(&mut headers);
    body["extra_headers"] = Value::Object(headers);
}

/// Reads the hub's settings, changes the fields this screen owns, and writes the whole object back.
fn patch_settings(c: &Client, change: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
    let mut settings = c.get::<Settings>("/v1/settings")?;
    change(&mut settings);
    save_settings(c, &settings)
}

/// The health check interval the settings hold (`health_interval_s`), in seconds.
fn interval_of(settings: &Settings) -> i64 {
    settings
        .rest
        .get("health_interval_s")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_INTERVAL_S)
}

/// The favourite models the settings hold (`favourite_models`), as `provider/model`.
fn favourites_of(settings: &Settings) -> Vec<String> {
    settings
        .rest
        .get("favourite_models")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Tests an API provider with one call (`POST /v1/providers/{id}/test`).
fn test_provider(c: &Client, id: &str) -> Result<TestReport, String> {
    c.post::<TestReport>(&format!("/v1/providers/{}/test", enc(id)), &json!({}))
}

/// The routes an engine can take: the plaza default (empty), then each enabled provider's models.
fn route_options(state: &State) -> Vec<Opt> {
    let mut out = vec![Opt {
        value: String::new(),
        label: "Plaza default".to_string(),
    }];
    for a in state.api.iter().filter(|a| a.enabled) {
        for m in &a.models {
            let key = format!("{}/{m}", a.id);
            out.push(Opt {
                value: key.clone(),
                label: key,
            });
        }
    }
    out
}

/// A hex accent (`#rrggbb`) as a colour.
fn accent_color(hex: &str) -> Option<Color> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(digits, 16).ok()?;
    Some(Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// "Checked just now", or how long ago the newest check was (`None`: not checked yet).
fn checked_label(checked: Option<u64>, now: u64) -> String {
    let Some(at) = checked else {
        return "Not checked yet".to_string();
    };
    let secs = now.saturating_sub(at) / 1000;
    match secs {
        0..=59 => "Checked just now".to_string(),
        60..=3_599 => format!("Checked {} min ago", secs / 60),
        3_600..=86_399 => format!("Checked {} h ago", secs / 3_600),
        _ => format!("Checked {} d ago", secs / 86_400),
    }
}

/// The current time in milliseconds since the epoch, as the hub's `checked_ms` counts.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The brand the kit draws for a provider kind or a built-in engine, where it has one.
fn brand_of(id: &str) -> &str {
    match id {
        "gemini" => "google",
        "claude" => "claude",
        "codex" => "openai",
        other => other,
    }
}

fn policy_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || -> Result<(Settings, ModelsView), String> {
            Ok((
                c.get::<Settings>("/v1/settings")?,
                c.get::<ModelsView>("/v1/models")?,
            ))
        },
        Msg::PolicyLoaded,
    )
}

fn listed_task(ctx: &Ctx, id: String) -> Task<Msg> {
    let c = ctx.client.clone();
    let id2 = id.clone();
    client::task(
        move || c.get::<Vec<ListedModel>>(&format!("/v1/providers/{id}/models")),
        move |r| Msg::Listed(id2.clone(), r),
    )
}

/// Writes the whole settings object. The MCP hub tab saves its ACP switch through this too.
pub(super) fn save_settings(c: &Client, s: &Settings) -> Result<Settings, String> {
    let body = serde_json::to_value(s).map_err(|e| e.to_string())?;
    let v = c.call("PUT", "/v1/settings", Some(&body))?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// A sub-heading in a card or a column, as the web's `.mdl-h3`: 13px semibold in ink-2.
pub(super) fn h3<'a, M: 'a>(title: &str) -> Element<'a, M> {
    text(title.to_string())
        .font(SANS_SEMIBOLD)
        .size(TYPE_UI)
        .color(crate::theme::palette().ink_2)
        .into()
}

/// The card surface of the Models screens, as the web's `.mdl-card`: the sheet ground, a rule, the skin's large radius.
pub(super) fn mdl_surface(_: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        snap: true,
        ..Default::default()
    }
}

/// A card of the Models screens: its title in sentence case (13.5px semibold), then the body, padded 16.
pub(super) fn mdl_card<'a, M: 'a>(title: &str, body: Element<'a, M>) -> Element<'a, M> {
    container(
        column(vec![
            text(title.to_string())
                .font(SANS_SEMIBOLD)
                .size(TYPE_ITEM)
                .color(crate::theme::palette().ink)
                .into(),
            body,
        ])
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fill)
    .style(mdl_surface)
    .into()
}

fn auth_env(auth: &Value) -> Option<String> {
    auth.get("env")?.as_str().map(str::to_string)
}

/// Comma- or newline-separated values, trimmed, without blanks.
fn split_list(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// An idle-sleep time in whole minutes, 0 to a day (apid's `MAX_IDLE_SLEEP_MIN`); 0 keeps every agent awake.
fn idle_minutes(text: &str) -> Option<i64> {
    text.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| (0..=1440).contains(n))
}

/// A doze time in whole seconds, 0 to a day; 0 never dozes.
fn doze_seconds(text: &str) -> Option<i64> {
    text.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| (0..=86400).contains(n))
}

/// The roles that never sleep: comma-separated, each one name with no blanks or inner spaces. An empty field keeps
/// none awake. None when an entry is not a role name, so the save waits for it.
fn role_names(text: &str) -> Option<Vec<String>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    text.split(',')
        .map(|role| {
            let role = role.trim();
            (!role.is_empty() && !role.chars().any(char::is_whitespace)).then(|| role.to_string())
        })
        .collect()
}

/// Splits `provider/model[@effort]` into its base id and its pinned effort.
fn parse_entry(entry: &str) -> (&str, Option<&str>) {
    match entry.rfind('@') {
        Some(at) if at > 0 => (&entry[..at], Some(&entry[at + 1..])),
        _ => (entry, None),
    }
}

/// A policy entry must read `provider/model`, with no blanks.
fn valid_entry(value: &str) -> bool {
    let (base, _) = parse_entry(value);
    match base.split_once('/') {
        Some((p, m)) => {
            !p.is_empty()
                && !m.is_empty()
                && !p.chars().any(char::is_whitespace)
                && !m.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

fn tier_list(s: &Settings, tier: Tier) -> &Vec<String> {
    match tier {
        Tier::Orchestrator => &s.model_policy.orchestrator,
        Tier::Worker => &s.model_policy.worker,
    }
}

fn tier_list_mut(s: &mut Settings, tier: Tier) -> &mut Vec<String> {
    match tier {
        Tier::Orchestrator => &mut s.model_policy.orchestrator,
        Tier::Worker => &mut s.model_policy.worker,
    }
}

/// The hub's price route takes the model as one path segment, so the `provider/model` key is
/// percent-encoded (the slash goes as `%2F`), as the web does.
fn price_path(key: &str) -> String {
    format!("/v1/prices/{}", enc(key))
}

fn price_key(model: &str, side: Side) -> String {
    let side = match side {
        Side::Input => "input",
        Side::Output => "output",
    };
    format!("price:{model}:{side}")
}

/// The choices of a model picker: the tier's allowlist, the current value kept even if it has
/// left the list, and an inherit choice first when the picker has one.
fn model_options(p: &Policy, tier: Tier, current: &str, inherit: Option<&str>) -> Vec<Opt> {
    let mut out = Vec::new();
    if let Some(label) = inherit {
        out.push(Opt {
            value: String::new(),
            label: label.to_string(),
        });
    }
    let names: BTreeMap<String, String> = p
        .models
        .iter()
        .map(|m| {
            let name = m.name.clone().unwrap_or_else(|| m.id.clone());
            let label = if m.trains_on_data {
                format!("{name} · WARNING: trains on your data")
            } else {
                name
            };
            (format!("{}/{}", m.provider, m.id), label)
        })
        .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in tier_list(&p.copy, tier) {
        let base = parse_entry(entry).0.to_string();
        if seen.insert(base.clone()) {
            let label = names.get(&base).cloned().unwrap_or_else(|| base.clone());
            out.push(Opt { value: base, label });
        }
    }
    if !current.is_empty() && !seen.contains(current) {
        let label = names
            .get(current)
            .cloned()
            .unwrap_or_else(|| current.to_string());
        out.push(Opt {
            value: current.to_string(),
            label,
        });
    }
    out
}

/// Whether the `provider/model` `base` is a contributor model the hub listed (`/v1/models`).
fn listed_trains(models: &[ModelInfo], base: &str) -> bool {
    models
        .iter()
        .any(|m| m.trains_on_data && format!("{}/{}", m.provider, m.id) == base)
}

fn spacer<'a>() -> Element<'a, Msg> {
    Space::new().width(Length::Fill).into()
}

fn labelled<'a>(label: &str, input: Element<'a, Msg>) -> Element<'a, Msg> {
    let items: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), input];
    column(items).spacing(4).into()
}

fn picker<'a>(options: Vec<Opt>, current: &str, slot: Slot) -> Element<'a, Msg> {
    let selected = options.iter().find(|o| o.value == current).cloned();
    pick_list(options, selected, move |o: Opt| {
        Msg::PolicyDefault(slot.clone(), o)
    })
    .width(Length::Fill)
    .into()
}

// ----- Providers tab -------------------------------------------------------------------------

/// The providers tab, as the web's models-providers view: the scope line, the Providers card,
/// the Usage providers card and the Advanced card; the decisions sit beneath. The dialogs draw over the screen.
fn providers_view<'a>(state: &'a State) -> Element<'a, Msg> {
    column(vec![
        scope_line(state),
        providers_card(state),
        usage_card(state),
        advanced_card(state),
        decisions_section(state),
    ])
    .spacing(20)
    .into()
}

/// The scope line under the head: "Applying settings for" and a pill (layers, All projects, up-down chevron) that
/// opens the menu: All projects, or a project whose Settings tab opens.
fn scope_line<'a>(state: &'a State) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let pill = button(
        row![
            ui::icon("layers", 14.0, p.ink_2),
            text("All projects").font(SANS).size(TYPE_UI).color(p.ink),
            ui::icon("chevrons-up-down", 13.0, p.ink_3),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([4, 10])
    .style(|_: &Theme, status: button::Status| button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                crate::theme::palette().hover
            } else {
                crate::theme::palette().raised
            }
            .into(),
        ),
        text_color: crate::theme::palette().ink,
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 14.0.into(),
        },
        ..Default::default()
    })
    .on_press(Msg::ToggleScope);
    let head = row![ui::faint("Applying settings for"), pill]
        .spacing(8)
        .align_y(Alignment::Center);
    if !state.scope_open {
        return head.into();
    }
    let mut options: Vec<(Option<String>, String)> = vec![(None, "All projects".to_string())];
    options.extend(
        state
            .projects
            .iter()
            .map(|pr| (Some(pr.id.clone()), pr.title.clone())),
    );
    let rows: Vec<Element<'a, Msg>> = options
        .into_iter()
        .map(|(project, label)| {
            button(text(label).font(SANS).size(TYPE_UI).color(p.ink))
                .width(Length::Fill)
                .padding([6, 10])
                .style(|_: &Theme, status: button::Status| button::Style {
                    background: hover_ground(status),
                    text_color: crate::theme::palette().ink,
                    border: Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(Msg::Scope(project))
                .into()
        })
        .collect();
    let menu = container(
        column(rows)
            .spacing(2)
            .padding(4)
            .width(Length::Fixed(240.0)),
    )
    .style(crate::theme::pop);
    column![head, menu].spacing(6).into()
}

/// A text field as the web's `.input`: 13.5px text in a 30px box.
fn text_field<'a>(
    placeholder: &'a str,
    value: &'a str,
) -> iced::widget::text_input::TextInput<'a, Msg> {
    text_input(placeholder, value)
        .size(TYPE_UI)
        .padding([5, 10])
}

/// The Providers card: the list of instances beside the selected one's detail, with Update all and the check.
fn providers_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let checked = api::last_checked(
        state
            .api
            .iter()
            .map(|a| &a.health)
            .chain(state.engines.iter().map(|e| &e.health)),
    );
    let actions = vec![
        ui::btn_sm("Update all", None, Kind::Ghost, Some(Msg::UpdateAll)),
        ui::btn_sm(
            &checked_label(checked, now_ms()),
            Some("refresh-cw"),
            Kind::Ghost,
            Some(Msg::CheckNow),
        ),
    ];
    // The rule between the list and the detail is an overlay: a Fill-height rule inside the row would collapse it.
    let split: Element<'a, Msg> = row![list_column(state), detail(state)]
        .width(Length::Fill)
        .into();
    let rule: Element<'a, Msg> = container(
        container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill)).style(
            |_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule.into()),
                ..Default::default()
            },
        ),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: LIST_W,
    })
    .into();
    let body: Element<'a, Msg> = iced::widget::stack![split, rule].into();
    bare_card("Providers", actions, body)
}

/// A card whose body runs to its edges, as the web's split cards: the head as `ui::card_with` draws it, the body
/// below a rule.
fn bare_card<'a>(
    title: &str,
    actions: Vec<Element<'a, Msg>>,
    body: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let mut head = row![ui::card_title(title)]
        .spacing(8)
        .align_y(Alignment::Center);
    head = head.push(Space::new().width(Length::Fill));
    for action in actions {
        head = head.push(action);
    }
    let head = container(head)
        .padding([8, 16])
        .width(Length::Fill)
        .center_y(Length::Fixed(44.0));
    container(column![head, ui::hairline(), body])
        .width(Length::Fill)
        .clip(true)
        .style(ui::card_style)
        .into()
}

/// The list: the API provider instances (enabled first), the coding engines, and Add provider. The rule beside it
/// belongs to the Providers card.
fn list_column<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    for a in api_rows(&state.api) {
        let selected = state.selected == Some(Sel::Api(a.id.clone()));
        rows.push(list_row(
            Sel::Api(a.id.clone()),
            selected,
            mark(brand_of(&a.kind), &a.label, a.accent.as_deref(), 22.0),
            a.label.clone(),
            None,
            a.enabled,
            &a.health,
        ));
    }
    for e in &state.engines {
        let selected = state.selected == Some(Sel::Engine(e.id.clone()));
        let version = e.version.clone().or_else(|| e.pinned.clone());
        rows.push(list_row(
            Sel::Engine(e.id.clone()),
            selected,
            mark(brand_of(&e.engine), &e.label, e.accent.as_deref(), 22.0),
            e.label.clone(),
            version,
            e.enabled,
            &e.health,
        ));
    }
    rows.push(
        container(ui::btn(
            "Add provider",
            Some("plus"),
            Kind::Ghost,
            Some(Msg::AddProvider),
        ))
        .padding(4)
        .into(),
    );
    let list = column(rows).spacing(2).padding(6).width(Length::Fill);
    container(list).width(Length::Fixed(LIST_W)).into()
}

/// One row of the list: the mark, the label (and the engine's version in mono), the status dot with its message,
/// and the enabled toggle. The selected row is on the signal-soft ground.
fn list_row<'a>(
    sel: Sel,
    selected: bool,
    mark: Element<'a, Msg>,
    label: String,
    version: Option<String>,
    enabled: bool,
    health: &Health,
) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let mut title = row![
        text(label)
            .font(theme::SANS_MEDIUM)
            .size(TYPE_UI)
            .color(if enabled { p.ink } else { p.ink_3 })
            .wrapping(Wrapping::None)
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if let Some(v) = version {
        title = title.push(
            text(v)
                .font(theme::MONO)
                .size(TYPE_META)
                .color(p.ink_3)
                .wrapping(Wrapping::None),
        );
    }
    let status = row![
        ui::dot(health.tone().color()),
        text(health.message.clone())
            .font(SANS)
            .size(TYPE_META)
            .color(p.ink_3)
            .wrapping(Wrapping::None),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let toggled = sel.clone();
    let toggle = toggler(enabled)
        .on_toggle(move |on| Msg::Toggle(toggled.clone(), on))
        .style(switch_style);
    let body = column![title, container(status).width(Length::Fill).clip(true)]
        .spacing(2)
        .width(Length::Fill);
    let content = row![mark, body, toggle]
        .spacing(10)
        .align_y(Alignment::Center)
        .padding([8, 10]);
    button(content)
        .width(Length::Fill)
        .padding(0)
        .style(move |_: &Theme, status: button::Status| row_style(selected, status))
        .on_press(Msg::Select(sel))
        .into()
}

/// The enabled switch: ink on its track when on, the strong rule when off, the paper knob.
fn switch_style(_: &Theme, status: iced::widget::toggler::Status) -> iced::widget::toggler::Style {
    use iced::widget::toggler::Status;
    let p = crate::theme::palette();
    let on = match status {
        Status::Active { is_toggled }
        | Status::Hovered { is_toggled }
        | Status::Disabled { is_toggled } => is_toggled,
    };
    iced::widget::toggler::Style {
        background: if on { p.ink } else { p.rule_strong }.into(),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: p.paper.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: None,
        border_radius: None,
        padding_ratio: 0.12,
    }
}

/// A row's button: the signal-soft ground when selected, the hover ground under the pointer.
fn row_style(selected: bool, status: button::Status) -> button::Style {
    let p = crate::theme::palette();
    button::Style {
        background: if selected {
            Some(p.signal_soft.into())
        } else {
            hover_ground(status)
        },
        text_color: p.ink,
        border: Border {
            radius: crate::theme::skin().radius.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A quiet button's ground: none at rest, the hover ground under the pointer.
fn hover_ground(status: button::Status) -> Option<iced::Background> {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    hovered.then(|| crate::theme::palette().hover.into())
}

/// The mark of an instance: the brand's logo when the kit has one, else a monogram tile in its accent.
fn mark<'a>(brand: &str, label: &str, accent: Option<&str>, size: f32) -> Element<'a, Msg> {
    if crate::console::brands::get(brand).is_some() {
        return ui::brand_logo(brand, size);
    }
    let letters: String = label
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let p = crate::theme::palette();
    let ground = accent.and_then(accent_color).unwrap_or(p.hover);
    container(
        text(letters)
            .font(theme::SANS_SEMIBOLD)
            .size(TYPE_BADGE)
            .color(p.ink),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .center(Length::Fixed(size))
    .style(move |_: &Theme| container::Style {
        background: Some(ground.into()),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// The detail of the selected row. Nothing selected (no providers yet) says so.
fn detail<'a>(state: &'a State) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match (state.selected.as_ref(), state.draft.as_ref()) {
        (Some(Sel::Api(id)), Some(draft)) => match state.api.iter().find(|a| &a.id == id) {
            Some(a) => api_detail(state, a, draft),
            None => no_selection(),
        },
        (Some(Sel::Engine(id)), Some(draft)) => match state.engines.iter().find(|e| &e.id == id) {
            Some(e) => engine_detail(state, e, draft),
            None => no_selection(),
        },
        _ => no_selection(),
    };
    container(body).width(Length::Fill).padding(16).into()
}

fn no_selection<'a>() -> Element<'a, Msg> {
    ui::empty("models", "No providers yet", "Add a provider to start.")
}

/// The detail's head: the mark and the label, and what sits at the right (an engine's version and its revert).
fn detail_head<'a>(
    mark: Element<'a, Msg>,
    label: &str,
    right: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut head = row![
        mark,
        text(label.to_string())
            .font(SANS_SEMIBOLD)
            .size(TYPE_SECTION)
            .color(crate::theme::palette().ink)
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    head = head.push(Space::new().width(Length::Fill));
    for item in right {
        head = head.push(item);
    }
    head.into()
}

/// One section of the detail: a rule, the heading, and the body.
fn section<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::hairline(), h3(title), body]
        .spacing(10)
        .width(Length::Fill)
        .into()
}

/// A health status line: the dot and the message.
fn status_line<'a>(health: &Health) -> Element<'a, Msg> {
    row![
        ui::dot(health.tone().color()),
        ui::faint(health.message.clone())
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Display name: the name (saved on Enter) with the accent swatch at its left, the status under it, and the accent
/// popover under that when open.
fn name_section<'a>(
    sel: &Sel,
    health: &Health,
    draft: &'a Draft,
    accent: Option<&str>,
    accent_open: bool,
) -> Element<'a, Msg> {
    let swatch = button(swatch_dot(accent.and_then(accent_color), 16.0))
        .padding(4)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: hover_ground(status),
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Msg::ToggleAccent);
    let field = text_field("Display name", &draft.name)
        .on_input(Msg::NameInput)
        .on_submit(Msg::NameSave);
    let mut items: Vec<Element<'a, Msg>> = vec![
        row![swatch, field]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        status_line(health),
    ];
    if accent_open {
        items.push(accent_row(sel, accent));
    }
    section("Display name", column(items).spacing(10).into())
}

/// A colour dot: the accent, or the ink-3 tone when there is none.
fn swatch_dot<'a>(color: Option<Color>, size: f32) -> Element<'a, Msg> {
    let fill = color.unwrap_or(crate::theme::palette().ink_3);
    container(Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_: &Theme| container::Style {
            background: Some(fill.into()),
            border: Border {
                radius: (size / 2.0).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// The accent popover's swatches: the eight accents and None, the chosen one ringed.
fn accent_row<'a>(sel: &Sel, current: Option<&str>) -> Element<'a, Msg> {
    let ring = crate::theme::palette().ink;
    let mut cells: Vec<Element<'a, Msg>> = Vec::new();
    for hex in ACCENTS {
        let on = current == Some(hex);
        cells.push(
            button(swatch_dot(accent_color(hex), 18.0))
                .padding(3)
                .style(move |_: &Theme, status: button::Status| button::Style {
                    background: hover_ground(status),
                    border: Border {
                        color: if on { ring } else { Color::TRANSPARENT },
                        width: if on { 2.0 } else { 0.0 },
                        radius: 10.0.into(),
                    },
                    ..Default::default()
                })
                .on_press(Msg::Accent(sel.clone(), Some(hex.to_string())))
                .into(),
        );
    }
    cells.push(ui::btn_sm(
        "None",
        None,
        Kind::Ghost,
        Some(Msg::Accent(sel.clone(), None)),
    ));
    row(cells)
        .spacing(6)
        .align_y(Alignment::Center)
        .wrap()
        .into()
}

/// The API instance's Setup: the credential state, Set or Replace key, Test (and its answer), and the base URL for
/// the OpenAI-compatible kind.
fn api_setup<'a>(state: &'a State, a: &'a ApiInstance, draft: &'a Draft) -> Element<'a, Msg> {
    let keyed = matches!(a.credential.as_str(), "secret" | "sealed");
    let mut credential = row![ui::faint(credential_text(a))]
        .spacing(8)
        .align_y(Alignment::Center);
    credential = credential.push(Space::new().width(Length::Fill));
    if !matches!(a.credential.as_str(), "none" | "command" | "out_of_scope") {
        credential = credential.push(ui::btn_sm(
            if keyed {
                "Replace key…"
            } else {
                "Set key…"
            },
            Some("key"),
            Kind::Secondary,
            Some(Msg::OpenKey(a.id.clone())),
        ));
    }
    credential = credential.push(ui::btn_sm(
        "Test",
        Some("play"),
        Kind::Secondary,
        Some(Msg::Test(a.id.clone())),
    ));
    let mut items: Vec<Element<'a, Msg>> = vec![credential.into()];
    if let Some(note) = &state.test_note {
        items.push(ui::faint(note.clone()).into());
    }
    if a.base_url.is_some() || a.kind == "openai-compatible" {
        items.push(labelled(
            "Base URL",
            text_field("https://", &draft.base_url)
                .on_input(Msg::BaseUrlInput)
                .on_submit(Msg::BaseUrlSave(a.id.clone()))
                .into(),
        ));
    }
    section("Setup", column(items).spacing(10).into())
}

/// What the credential is, in words: the vault, the environment variable it reads, or none.
fn credential_text(a: &ApiInstance) -> String {
    match a.credential.as_str() {
        "secret" => "Key set in the vault".to_string(),
        "sealed" => "Sealed vault".to_string(),
        "env" => format!("Read from ${}", auth_env(&a.auth).unwrap_or_default()),
        "missing" => "No key".to_string(),
        "none" => "No key needed".to_string(),
        "command" => "Read from a command".to_string(),
        _ => "Out of scope".to_string(),
    }
}

/// The engine's Setup: the route (the model it runs through, the plaza default when empty), and the containment
/// line with the CLIProxyAPI route.
fn engine_setup<'a>(state: &'a State, e: &'a EngineInstance) -> Element<'a, Msg> {
    let current = e.route.clone().unwrap_or_default();
    let mut options = route_options(state);
    if !current.is_empty() && !options.iter().any(|o| o.value == current) {
        options.push(Opt {
            value: current.clone(),
            label: current.clone(),
        });
    }
    let selected = options.iter().find(|o| o.value == current).cloned();
    let id = e.id.clone();
    let route = row![
        ui::faint("Runs through"),
        pick_list(options, selected, move |o: Opt| Msg::RouteModel(
            id.clone(),
            o
        )),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let containment = ui::faint(
        "Engines use the plaza's model gateway. To use a ChatGPT or Claude subscription, run a CLIProxyAPI hub and add it as a provider.",
    );
    let hub = ui::btn_sm(
        "Add a CLIProxyAPI hub",
        Some("plus"),
        Kind::Secondary,
        Some(Msg::AddCliProxy),
    );
    section("Setup", column![route, containment, hub].spacing(10).into())
}

/// The engine's Runtime, closed by default: the source, the version (a pin, `latest` or a version), the home folder
/// and the launch arguments (one per line). Saving a version can take a few seconds while the hub pins it.
fn runtime_section<'a>(
    state: &'a State,
    e: &'a EngineInstance,
    draft: &'a Draft,
) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let head = ui::disclosure(
        "Runtime".to_string(),
        p.ink_2,
        state.runtime_open,
        Msg::ToggleRuntime,
    );
    let mut items: Vec<Element<'a, Msg>> = vec![ui::hairline(), head];
    if state.runtime_open {
        let source = match &e.package {
            Some(package) => format!("{} · {package}", e.source),
            None => e.source.clone(),
        };
        let save = row![
            ui::btn_sm(
                "Save runtime",
                Some("check"),
                Kind::Primary,
                Some(Msg::RuntimeSave(e.id.clone())),
            ),
            if draft.pinning {
                ui::faint("Pinning…")
            } else {
                ui::faint("")
            },
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        items.push(labelled(
            "Source",
            ui::mono(source, TYPE_META, p.ink_2).into(),
        ));
        items.push(labelled(
            "Version",
            text_field("pinned, latest or x.y.z", &draft.version)
                .on_input(Msg::RuntimeVersion)
                .on_submit(Msg::RuntimeSave(e.id.clone()))
                .into(),
        ));
        items.push(labelled(
            "Home folder",
            text_field("", &draft.home)
                .on_input(Msg::RuntimeHome)
                .on_submit(Msg::RuntimeSave(e.id.clone()))
                .into(),
        ));
        items.push(labelled(
            "Launch arguments",
            text_editor(&draft.args)
                .placeholder("One argument per line")
                .on_action(Msg::RuntimeArgs)
                .height(72.0)
                .into(),
        ));
        items.push(save.into());
    }
    column(items).spacing(10).width(Length::Fill).into()
}

/// The engine's Environment: the variables (plain settings the agent can read: keys never go here), each a name and
/// a value with a remove, then Add variable and Save.
fn env_section<'a>(e: &'a EngineInstance, draft: &'a Draft) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    for (i, (name, value)) in draft.env.iter().enumerate() {
        rows.push(
            row![
                text_field("NAME", name)
                    .on_input(move |v| Msg::EnvName(i, v))
                    .width(Length::FillPortion(2)),
                text_field("value", value)
                    .on_input(move |v| Msg::EnvValue(i, v))
                    .width(Length::FillPortion(3)),
                ui::btn_sm("", Some("x"), Kind::Ghost, Some(Msg::EnvRemove(i))),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    let actions = row![
        ui::btn_sm("Add variable", Some("plus"), Kind::Ghost, Some(Msg::EnvAdd)),
        Space::new().width(Length::Fill),
        ui::btn_sm(
            "Save variables",
            Some("check"),
            Kind::Secondary,
            Some(Msg::EnvSave(e.id.clone())),
        ),
    ]
    .align_y(Alignment::Center);
    section(
        "Environment",
        column![
            ui::faint(
                "Plain settings for the engine. The agent can read them, so keys never go here."
            ),
            column(rows).spacing(6),
            actions,
        ]
        .spacing(10)
        .into(),
    )
}

/// The API instance's Headers: the names (the values stay hidden), each removable, then the add row.
fn headers_section<'a>(a: &'a ApiInstance, draft: &'a Draft) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    for name in a.extra_headers.keys() {
        rows.push(
            row![
                ui::mono(name.clone(), TYPE_META, p.ink),
                Space::new().width(Length::Fill),
                ui::mono("••••••", TYPE_META, p.ink_3),
                ui::btn_sm(
                    "",
                    Some("x"),
                    Kind::Ghost,
                    Some(Msg::HeaderRemove(a.id.clone(), name.clone())),
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    let add: Element<'a, Msg> = if draft.header_open {
        row![
            text_field("Header name", &draft.header_name)
                .on_input(Msg::HeaderName)
                .width(Length::FillPortion(2)),
            text_field("Value", &draft.header_value)
                .on_input(Msg::HeaderValue)
                .secure(true)
                .width(Length::FillPortion(3)),
            ui::btn_sm(
                "Add",
                Some("check"),
                Kind::Primary,
                Some(Msg::HeaderAdd(a.id.clone())),
            ),
            ui::btn_sm("", Some("x"), Kind::Ghost, Some(Msg::HeaderClose)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        ui::btn_sm(
            "Add header",
            Some("plus"),
            Kind::Ghost,
            Some(Msg::HeaderOpen),
        )
    };
    section(
        "Headers",
        column![
            ui::faint("Sent with every request. The values stay on the hub."),
            column(rows).spacing(6),
            add,
        ]
        .spacing(10)
        .into(),
    )
}

/// The API instance's Models: the hint, the count, each model (id, price when known, the trains-on-data chip and the
/// favourite star), then Add custom model.
fn models_section<'a>(state: &'a State, a: &'a ApiInstance, draft: &'a Draft) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let ids: Vec<String> = match state.listed.get(&a.id) {
        Some(list) => list.iter().map(|m| m.id.clone()).collect(),
        None => a.models.clone(),
    };
    let count = if ids.len() == 1 {
        "1 model".to_string()
    } else {
        format!("{} models", ids.len())
    };
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    for id in ids {
        let key = format!("{}/{id}", a.id);
        let on = state.favourites.contains(&key);
        let mut line = row![star_button(key.clone(), on), ui::mono(id, TYPE_META, p.ink)]
            .spacing(8)
            .align_y(Alignment::Center);
        if let Some((input, output)) = state.model_prices.get(&key) {
            line = line.push(ui::faint(format!("${input} in · ${output} out per 1M")));
        }
        if state.trains.contains(&key) {
            line = line.push(ui::badge("Trains on data", Tone::Warn));
        }
        line = line.push(Space::new().width(Length::Fill));
        rows.push(line.into());
        rows.push(ui::hairline());
    }
    let add = row![
        text_field("Add a custom model", &draft.model)
            .on_input(Msg::ModelDraft)
            .on_submit(Msg::ModelAdd(a.id.clone()))
            .width(Length::Fill),
        ui::btn_sm(
            "Add custom model",
            Some("plus"),
            Kind::Ghost,
            Some(Msg::ModelAdd(a.id.clone())),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    section(
        "Models",
        column![
            ui::faint("Model preferences are saved on the hub and shared across your devices."),
            ui::faint(count),
            column(rows).spacing(6),
            add,
        ]
        .spacing(10)
        .into(),
    )
}

/// A model's favourite star: the signal when it is a favourite.
fn star_button<'a>(key: String, on: bool) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let color = if on { p.signal } else { p.ink_3 };
    button(ui::icon("star", 14.0, color))
        .padding(4)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: hover_ground(status),
            border: Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Msg::Favourite(key))
        .into()
}

/// The Remove row at the foot of a detail: a rule, then the outline danger button.
fn remove_row<'a>(sel: Sel) -> Element<'a, Msg> {
    column![
        ui::hairline(),
        row![
            Space::new().width(Length::Fill),
            ui::btn(
                "Remove provider",
                Some("trash"),
                Kind::DangerOutline,
                Some(Msg::OpenRemove(sel)),
            ),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(12)
    .into()
}

/// An API provider's detail: the head, Display name, Setup, Headers, Models and Remove.
fn api_detail<'a>(state: &'a State, a: &'a ApiInstance, draft: &'a Draft) -> Element<'a, Msg> {
    let kind = crate::console::brands::get(&a.kind)
        .map(|b| b.title.to_string())
        .unwrap_or_else(|| a.kind.clone());
    let head: Element<'a, Msg> = column![
        detail_head(
            mark(brand_of(&a.kind), &a.label, a.accent.as_deref(), 28.0),
            &a.label,
            Vec::new(),
        ),
        ui::mono(
            format!("{kind} · {}", a.id),
            TYPE_META,
            crate::theme::palette().ink_3
        ),
    ]
    .spacing(4)
    .into();
    column![
        head,
        name_section(
            &Sel::Api(a.id.clone()),
            &a.health,
            draft,
            a.accent.as_deref(),
            state.accent_open,
        ),
        api_setup(state, a, draft),
        headers_section(a, draft),
        models_section(state, a, draft),
        remove_row(Sel::Api(a.id.clone())),
    ]
    .spacing(18)
    .width(Length::Fill)
    .into()
}

/// An engine's detail: the head with its version and revert, Display name, Setup, Runtime, Environment and Remove
/// (for an added engine only: the built-in ones are disabled, not removed).
fn engine_detail<'a>(
    state: &'a State,
    e: &'a EngineInstance,
    draft: &'a Draft,
) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let mut right: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(v) = e.version.clone().or_else(|| e.pinned.clone()) {
        right.push(ui::mono(v, TYPE_META, p.ink_3).into());
    }
    if e.version.is_some() && e.version != e.pinned {
        right.push(ui::btn_sm(
            "",
            Some("rotate-ccw"),
            Kind::Ghost,
            Some(Msg::RevertPin(e.id.clone())),
        ));
    }
    let head = detail_head(
        mark(brand_of(&e.engine), &e.label, e.accent.as_deref(), 28.0),
        &e.label,
        right,
    );
    let mut parts: Vec<Element<'a, Msg>> = vec![
        head,
        name_section(
            &Sel::Engine(e.id.clone()),
            &e.health,
            draft,
            e.accent.as_deref(),
            state.accent_open,
        ),
        engine_setup(state, e),
        runtime_section(state, e, draft),
        env_section(e, draft),
    ];
    if !e.builtin {
        parts.push(remove_row(Sel::Engine(e.id.clone())));
    }
    column(parts).spacing(18).width(Length::Fill).into()
}

/// The Usage providers card: each CLIProxyAPI hub's label, address, status, totals and pooled accounts, with a
/// refresh and a remove; Add hub opens its dialog.
fn usage_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let add = ui::btn_sm("Add hub", Some("plus"), Kind::Ghost, Some(Msg::HubOpen));
    let body: Element<'a, Msg> = if state.usage.is_empty() {
        ui::faint("No hubs configured.").into()
    } else {
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        for hub in &state.usage {
            rows.push(hub_row(hub));
            rows.push(ui::hairline());
        }
        column(rows).spacing(12).width(Length::Fill).into()
    };
    ui::card_with("Usage providers", vec![add], body)
}

/// A tone for a status word the hub reports (`ok`, `warn`, `crit`, `error`…).
fn status_tone(status: &str) -> Tone {
    match status {
        "ok" => Tone::Ok,
        "warn" => Tone::Warn,
        "crit" | "error" => Tone::Danger,
        _ => Tone::Muted,
    }
}

/// A usage hub's row: its label and address, its status, its totals, and its pooled accounts as chips.
fn hub_row<'a>(hub: &'a UsageProvider) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let (tone, message, totals, accounts) = match &hub.last {
        Some(r) => (
            status_tone(&r.status),
            r.message.clone(),
            r.totals.as_ref().map(|t| {
                format!(
                    "{} requests · {} tokens",
                    ui::thousands(t.requests as i64),
                    compact(t.tokens)
                )
            }),
            r.accounts.clone(),
        ),
        None => (Tone::Muted, "Not read yet".to_string(), None, Vec::new()),
    };
    let mut info: Vec<Element<'a, Msg>> = vec![
        row![
            text(hub.label.clone())
                .font(theme::SANS_SEMIBOLD)
                .size(TYPE_UI)
                .color(p.ink),
            ui::mono(hub.url.clone(), TYPE_META, p.ink_3),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        row![ui::dot(tone.color()), ui::faint(message)]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
    ];
    if let Some(t) = totals {
        info.push(ui::faint(t).into());
    }
    if !accounts.is_empty() {
        let chips: Vec<Element<'a, Msg>> = accounts
            .iter()
            .map(|a| {
                ui::badge(
                    &format!("{} · {} · {}", a.name, a.provider, a.status),
                    status_tone(&a.status),
                )
            })
            .collect();
        info.push(ui::chip_row(chips));
    }
    row![
        column(info).spacing(6).width(Length::Fill),
        ui::btn_sm(
            "",
            Some("refresh-cw"),
            Kind::Ghost,
            Some(Msg::HubRead(hub.id.clone())),
        ),
        ui::btn_sm(
            "",
            Some("trash"),
            Kind::Ghost,
            Some(Msg::HubRemove(hub.id.clone())),
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Start)
    .into()
}

/// A token count as the hub's totals read it: millions with one decimal, else with separators.
fn compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else {
        ui::thousands(tokens as i64)
    }
}

/// The Advanced card: the health check interval, with a stepper of 60 seconds (0 turns the checks off).
fn advanced_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let s = state.interval_s;
    let stepper = row![
        ui::btn_sm(
            "−",
            None,
            Kind::Secondary,
            (s > 0).then_some(Msg::Interval((s - 60).max(0))),
        ),
        container(ui::mono(s.to_string(), TYPE_UI, p.ink))
            .width(Length::Fixed(56.0))
            .center_x(Length::Fixed(56.0)),
        ui::btn_sm(
            "+",
            None,
            Kind::Secondary,
            (s < MAX_INTERVAL_S).then_some(Msg::Interval((s + 60).min(MAX_INTERVAL_S))),
        ),
        ui::faint("seconds"),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let body = row![
        column![
            text("Health check interval")
                .font(theme::SANS_MEDIUM)
                .size(TYPE_UI)
                .color(p.ink),
            ui::faint(
                "Refresh provider status, versions and models in the background. Set to 0 to turn it off.",
            ),
        ]
        .spacing(4)
        .width(Length::Fill),
        stepper,
    ]
    .spacing(16)
    .align_y(Alignment::Center);
    ui::card("Advanced", body.into())
}

/// Cards in a grid two to a row, 12 apart, as the web's `.mdl-grid`; a card stretches to its row's height.
pub(super) fn card_grid<'a, M: 'a>(cards: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut out = column(Vec::<Element<'a, M>>::new())
        .spacing(12)
        .width(Length::Fill);
    let mut line: Vec<Element<'a, M>> = Vec::new();
    for card in cards {
        line.push(
            container(card)
                .width(Length::FillPortion(1))
                .height(Length::Fill)
                .into(),
        );
        if line.len() == 2 {
            out = out.push(
                row(std::mem::take(&mut line))
                    .spacing(12)
                    .width(Length::Fill),
            );
        }
    }
    if !line.is_empty() {
        line.push(Space::new().width(Length::FillPortion(1)).into());
        out = out.push(row(line).spacing(12).width(Length::Fill));
    }
    out.into()
}

/// A section heading, as the web's `.mdl-sec h2`.
fn section_title<'a>(title: &str) -> Element<'a, Msg> {
    text(title.to_string())
        .font(SANS_SEMIBOLD)
        .size(15.0)
        .color(crate::theme::palette().ink)
        .into()
}

/// The Decisions section: the decision model the plaza decides with, and the Try-it form. The decision providers
/// are rows of the Providers card above.
fn decisions_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let items: Vec<Element<'a, Msg>> = vec![
        section_title("Decisions"),
        text(DECISIONS_LEDE)
            .font(theme::SANS)
            .size(FS_SM)
            .color(crate::theme::palette().ink_3)
            .into(),
        decision_model_card(state),
        try_it_card(state),
    ];
    column(items).spacing(16).into()
}

/// The decision models the plaza may use: every decisions-group provider's models, as `provider/model`.
fn decision_options(providers: &[ApiInstance]) -> Vec<String> {
    providers
        .iter()
        .filter(|p| p.group == "decisions")
        .flat_map(|p| p.models.iter().map(move |m| format!("{}/{m}", p.id)))
        .collect()
}

fn decision_model_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let options = decision_options(&state.api);
    let current = state
        .decision_pending
        .clone()
        .or_else(|| state.decision_default.clone());
    let mut items: Vec<Element<'a, Msg>> =
        vec![ui::faint("The model the plaza decides with.").into()];
    if options.is_empty() {
        items.push(ui::faint("Add a decisions provider to choose one.").into());
    } else {
        items.push(
            pick_list(options, current, Msg::DecisionPick)
                .width(Length::Fill)
                .into(),
        );
    }
    if state.decision_pending.is_some() {
        items.push(ui::faint("Saving…").into());
    }
    mdl_card("Decision model", column(items).spacing(12).into())
}

fn try_it_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let t = &state.try_it;
    let input: Element<'a, Msg> = text_editor(&t.input)
        .placeholder("The input the question is about")
        .on_action(Msg::TryInput)
        .height(120.0)
        .into();
    let kinds: Vec<QKind> = vec![QKind::Predicate, QKind::Choice, QKind::Score];
    let mut items: Vec<Element<'a, Msg>> = vec![
        labelled("Input", input),
        labelled(
            "Question",
            pick_list(kinds, Some(t.kind), Msg::TryKind)
                .width(Length::Fill)
                .into(),
        ),
        labelled(
            "Instructions",
            text_field("What to decide", &t.instructions)
                .on_input(Msg::TryInstructions)
                .into(),
        ),
    ];
    if t.kind != QKind::Predicate {
        let (label, hint) = if t.kind == QKind::Choice {
            ("Choices", "Comma-separated, at least two")
        } else {
            ("Levels", "Comma-separated, lowest first")
        };
        items.push(labelled(
            label,
            text_field(hint, &t.options)
                .on_input(Msg::TryOptions)
                .into(),
        ));
    }
    items.push(
        row(vec![
            spacer(),
            ui::btn(
                "Ask",
                None,
                Kind::Primary,
                (!t.asking).then_some(Msg::TryAsk),
            ),
        ])
        .into(),
    );
    if let Some(e) = &t.error {
        items.push(ui::error_line(e));
    }
    if let Some(reply) = &t.answer {
        items.push(answers_view(reply));
    }
    mdl_card("Try it", column(items).spacing(12).into())
}

/// The one question the Try-it form asks, in the plaza's request shape.
fn question_json(kind: QKind, instructions: &str, options: &str) -> Result<Value, String> {
    let items = split_list(options);
    let mut question =
        json!({ "type": kind.key(), "name": "question", "instructions": instructions.trim() });
    match kind {
        QKind::Predicate => {}
        QKind::Choice => {
            if items.len() < 2 {
                return Err("A choice question needs at least two choices".to_string());
            }
            question["choices"] = items
                .iter()
                .map(|v| json!({ "value": v }))
                .collect::<Vec<Value>>()
                .into();
        }
        QKind::Score => {
            if items.len() < 2 {
                return Err("A score question needs at least two levels".to_string());
            }
            question["levels"] = items
                .iter()
                .map(|v| json!({ "label": v }))
                .collect::<Vec<Value>>()
                .into();
        }
    }
    Ok(question)
}

fn answers_view<'a>(reply: &'a DecisionReply) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if !reply.model.is_empty() {
        items.push(ui::faint(format!("Answered by {}", reply.model)).into());
    }
    items.extend(reply.answers.iter().map(answer_view));
    column(items).spacing(12).into()
}

fn answer_view<'a>(answer: &'a Answer) -> Element<'a, Msg> {
    match answer {
        Answer::Predicate { name, probability } => bar_line(name.clone(), *probability),
        Answer::Choice {
            name,
            choice,
            probabilities,
            confidence,
        } => {
            let head: Element<'a, Msg> = row(vec![
                ui::sans(name.clone(), 12.0, crate::theme::palette().ink).into(),
                spacer(),
                ui::badge(&value_text(choice), Tone::Signal),
            ])
            .spacing(8)
            .into();
            let mut items: Vec<Element<'a, Msg>> = vec![head];
            items.extend(
                probabilities
                    .iter()
                    .map(|w| bar_line(weighted_label(w), w.probability)),
            );
            items.extend(confidence.map(confidence_line));
            column(items).spacing(6).into()
        }
        Answer::Score {
            name,
            score,
            probabilities,
            confidence,
        } => {
            let head: Element<'a, Msg> = row(vec![
                ui::sans(name.clone(), 12.0, crate::theme::palette().ink).into(),
                spacer(),
                ui::badge(&format!("{score:.2}"), Tone::Signal),
            ])
            .spacing(8)
            .into();
            let mut items: Vec<Element<'a, Msg>> = vec![head];
            items.extend(
                probabilities
                    .iter()
                    .map(|w| bar_line(weighted_label(w), w.probability)),
            );
            items.extend(confidence.map(confidence_line));
            column(items).spacing(6).into()
        }
        Answer::Refusal { name } => row(vec![
            ui::sans(name.clone(), 12.0, crate::theme::palette().ink).into(),
            spacer(),
            ui::badge("Refused", Tone::Warn),
        ])
        .spacing(8)
        .into(),
    }
}

/// A label with its percentage, and the probability as a bar under it.
fn bar_line<'a>(label: String, probability: f64) -> Element<'a, Msg> {
    let head: Element<'a, Msg> = row(vec![
        ui::sans(label, 12.0, crate::theme::palette().ink).into(),
        spacer(),
        ui::sans(
            format!("{:.1}%", probability * 100.0),
            11.0,
            crate::theme::palette().ink_3,
        )
        .into(),
    ])
    .spacing(8)
    .into();
    column(vec![head, bar(probability)]).spacing(4).into()
}

/// A probability bar: the signal colour to its share, the faint rule for the rest.
fn bar<'a>(probability: f64) -> Element<'a, Msg> {
    let filled = ((probability.clamp(0.0, 1.0) * 1000.0).round() as u16).clamp(1, 999);
    row(vec![
        container(Space::new())
            .width(Length::FillPortion(filled))
            .height(6.0)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().signal.into()),
                ..Default::default()
            })
            .into(),
        container(Space::new())
            .width(Length::FillPortion(1000 - filled))
            .height(6.0)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule_faint.into()),
                ..Default::default()
            })
            .into(),
    ])
    .width(Length::Fill)
    .into()
}

fn confidence_line<'a>(confidence: f64) -> Element<'a, Msg> {
    ui::faint(format!("Confidence {:.0}%", confidence * 100.0)).into()
}

/// A decision value as text: a string as it is, anything else as JSON.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn weighted_label(w: &Weighted) -> String {
    w.label.clone().unwrap_or_else(|| value_text(&w.value))
}

fn default_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<DecisionDefault>("/v1/decisions/default"),
        Msg::DefaultLoaded,
    )
}

/// The dialog over the providers tab, as the web's `modal()`: set or replace a key, remove a provider, confirm the
/// engine updates, or add a CLIProxyAPI hub.
fn dialog_view<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    match &state.dialog {
        Dialog::Closed => None,
        Dialog::Key {
            provider,
            replace,
            value,
            saving,
        } => {
            let body = column![
                ui::faint(format!(
                    "{provider}: the key goes to the hub's vault. It is never shown again."
                )),
                text_field("API key", value)
                    .secure(true)
                    .on_input(Msg::KeyInput)
                    .on_submit(Msg::SaveKey),
            ]
            .spacing(12);
            let foot = vec![
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)),
                ui::btn(
                    "Save key",
                    None,
                    Kind::Primary,
                    (!*saving && !value.trim().is_empty()).then_some(Msg::SaveKey),
                ),
            ];
            Some(ui::modal(
                if *replace { "Replace key" } else { "Set key" },
                body.into(),
                foot,
                Msg::Close,
                440.0,
            ))
        }
        Dialog::Remove { target, busy } => {
            let name = match target {
                Sel::Api(id) | Sel::Engine(id) => id.clone(),
            };
            let body = ui::prose(format!(
                "{name} is removed from the hub. This cannot be undone."
            ));
            let foot = vec![
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)),
                ui::btn(
                    "Remove provider",
                    None,
                    Kind::Danger,
                    (!*busy).then_some(Msg::ConfirmRemove),
                ),
            ];
            Some(ui::modal(
                "Remove provider",
                body.into(),
                foot,
                Msg::Close,
                440.0,
            ))
        }
        Dialog::Update {
            title,
            changes,
            busy,
        } => {
            let p = crate::theme::palette();
            let lines: Vec<Element<'a, Msg>> = changes
                .iter()
                .map(|c| {
                    row![
                        ui::mono(c.id.clone(), TYPE_META, p.ink),
                        Space::new().width(Length::Fill),
                        ui::mono(format!("{} → {}", c.from, c.to), TYPE_META, p.ink_2),
                    ]
                    .align_y(Alignment::Center)
                    .into()
                })
                .collect();
            let foot = vec![
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)),
                ui::btn(
                    "Update",
                    None,
                    Kind::Primary,
                    (!*busy).then_some(Msg::ConfirmUpdate),
                ),
            ];
            Some(ui::modal(
                title.as_str(),
                column(lines).spacing(8).into(),
                foot,
                Msg::Close,
                480.0,
            ))
        }
        Dialog::Hub {
            url,
            key,
            label,
            busy,
        } => {
            let body = column![
                ui::faint(
                    "Show the quota of every account the hub pools, next to the providers. The key stays on the hub.",
                ),
                labelled(
                    "Hub URL",
                    text_field("https://hub.example.ts.net:8318", url)
                        .on_input(Msg::HubUrl)
                        .into(),
                ),
                labelled(
                    "Management key",
                    text_field("", key)
                        .secure(true)
                        .on_input(Msg::HubKey)
                        .into(),
                ),
                labelled(
                    "Label",
                    text_field("Defaults to the hub's host name", label)
                        .on_input(Msg::HubLabel)
                        .on_submit(Msg::HubAdd)
                        .into(),
                ),
            ]
            .spacing(12);
            let foot = vec![
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)),
                ui::btn(
                    "Add hub",
                    None,
                    Kind::Primary,
                    (!*busy).then_some(Msg::HubAdd),
                ),
            ];
            Some(ui::modal(
                "Add a CLIProxyAPI hub",
                body.into(),
                foot,
                Msg::Close,
                480.0,
            ))
        }
    }
}

// ----- Policy & defaults tab -----------------------------------------------------------------

fn policy_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(p) = state.policy.as_ref() else {
        return if state.error.is_some() {
            ui::empty(
                "alert",
                "Models unavailable",
                "apid did not answer. Reload to try again.",
            )
        } else {
            ui::loading()
        };
    };
    let s = &p.copy;
    let trains = s
        .model_policy
        .orchestrator
        .iter()
        .chain(&s.model_policy.worker)
        .any(|e| listed_trains(&p.models, parse_entry(e).0))
        || [&s.default_model, &s.orchestrator_model]
            .into_iter()
            .chain(s.role_models.values())
            .any(|e| listed_trains(&p.models, parse_entry(e).0));
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if trains {
        items.push(ui::faint(ui::TRAINS_NOTE).into());
    }
    items.extend([
        policy_card(p),
        defaults_card(p),
        limits_card(p),
        engines_card(p),
        prices_card(state, p),
    ]);
    column(items).spacing(16).into()
}

fn policy_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if p.saved.dev_models == Some(true) {
        body.push(ui::badge(
            "Development models enabled — policy relaxed",
            Tone::Warn,
        ));
    }
    body.push(ui::prose("Agents may only run the models on their tier's list.").into());
    let cols: Vec<Element<'a, Msg>> = vec![
        tier_column(p, Tier::Orchestrator, "Orchestrator"),
        tier_column(p, Tier::Worker, "Workers"),
    ];
    body.push(row(cols).spacing(16).into());
    mdl_card("Model policy", column(body).spacing(12).into())
}

fn tier_column<'a>(p: &'a Policy, tier: Tier, title: &str) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = tier_list(&p.copy, tier)
        .iter()
        .enumerate()
        .map(|(i, entry)| policy_row(p, tier, i, entry))
        .collect();
    if rows.is_empty() {
        rows.push(ui::faint("No models").into());
    }
    let add: Element<'a, Msg> = text_input("provider/model", p.add_text(tier))
        .on_input(move |v| Msg::PolicyAddInput(tier, v))
        .on_submit(Msg::PolicyAdd(tier))
        .padding(8)
        .into();
    let add_row: Element<'a, Msg> = row(vec![
        add,
        ui::btn(
            "Add model",
            None,
            Kind::Secondary,
            Some(Msg::PolicyAdd(tier)),
        ),
    ])
    .spacing(8)
    .into();
    let items: Vec<Element<'a, Msg>> = vec![h3(title), column(rows).spacing(6).into(), add_row];
    column(items)
        .spacing(10)
        .width(Length::FillPortion(1))
        .into()
}

fn policy_row<'a>(p: &'a Policy, tier: Tier, index: usize, entry: &'a str) -> Element<'a, Msg> {
    let (base, effort) = parse_entry(entry);
    let listed = p.known.contains(base);
    // The availability square: filled when the provider lists the model, hollow when it does not.
    let marker: Element<'a, Msg> = container(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_: &Theme| container::Style {
            background: if listed {
                Some(crate::theme::palette().ok.into())
            } else {
                None
            },
            border: Border {
                color: if listed {
                    crate::theme::palette().ok
                } else {
                    crate::theme::palette().ink_3
                },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .into();
    // The whole id, nested slashes included: it wraps rather than being cut off.
    let mut items: Vec<Element<'a, Msg>> = vec![
        marker,
        ui::sans(base.to_string(), 12.0, crate::theme::palette().ink)
            .wrapping(Wrapping::WordOrGlyph)
            .into(),
    ];
    if effort.is_some() {
        items.push(ui::badge("effort pinned", Tone::Signal));
    }
    if listed_trains(&p.models, base) {
        items.push(ui::trains_badge());
    }
    items.push(spacer());
    items.push(ui::btn(
        "",
        Some("x"),
        Kind::Ghost,
        Some(Msg::PolicyRemove(tier, index)),
    ));
    column(vec![
        container(row(items).spacing(8).align_y(Alignment::Center))
            .padding(Padding {
                top: 8.0,
                right: 0.0,
                bottom: 8.0,
                left: 0.0,
            })
            .into(),
        ui::hairline(),
    ])
    .into()
}

fn defaults_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let mut items: Vec<Element<'a, Msg>> = vec![
        labelled(
            "Orchestrator model",
            picker(
                model_options(p, Tier::Orchestrator, &s.orchestrator_model, None),
                &s.orchestrator_model,
                Slot::Orchestrator,
            ),
        ),
        labelled(
            "Worker default",
            picker(
                model_options(p, Tier::Worker, &s.default_model, None),
                &s.default_model,
                Slot::Worker,
            ),
        ),
        h3("Per role"),
    ];
    if s.role_models.is_empty() {
        items.push(ui::empty("agents", "No roles connected", ""));
    }
    for (role, current) in &s.role_models {
        let orchestrator = role == "orchestrator";
        let (tier, inherit) = if orchestrator {
            (Tier::Orchestrator, "Orchestrator default")
        } else {
            (Tier::Worker, "Worker default")
        };
        let options = model_options(p, tier, current, Some(inherit));
        let role_row: Element<'a, Msg> = row(vec![
            ui::sans(role.clone(), 12.0, crate::theme::palette().ink).into(),
            picker(options, current, Slot::Role(role.clone())),
        ])
        .spacing(12)
        .into();
        items.push(role_row);
    }
    items.push(h3("Contributor models"));
    items.push(
        iced::widget::checkbox(s.allow_training_models)
            .label("Allow as defaults, role models and policy entries")
            .on_toggle(Msg::PolicyAllowTraining)
            .into(),
    );
    items.push(ui::faint(ui::TRAINS_NOTE).into());
    mdl_card("Defaults", column(items).spacing(12).into())
}

fn limits_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let turns: Element<'a, Msg> =
        text_input("1–200", &p.shown("max_turns", s.max_turns.to_string()))
            .on_input(|v| Msg::PolicyNumber(Num::MaxTurns, v))
            .padding(8)
            .into();
    let tokens: Element<'a, Msg> =
        text_input("0", &p.shown("max_tokens", s.max_tokens.to_string()))
            .on_input(|v| Msg::PolicyNumber(Num::MaxTokens, v))
            .padding(8)
            .into();
    let effort_current = s.effort.clone().unwrap_or_else(|| "default".to_string());
    let effort_options: Vec<String> = EFFORTS.iter().map(|e| e.to_string()).collect();
    let effort: Element<'a, Msg> = pick_list(effort_options, Some(effort_current), |v: String| {
        Msg::PolicyEffort(v)
    })
    .width(Length::Fill)
    .into();
    let block: Element<'a, Msg> = text_editor(&p.block)
        .placeholder("example.com\n*.example.net")
        .on_action(Msg::PolicyFetchBlock)
        .height(120.0)
        .into();
    let idle: Element<'a, Msg> = text_input(
        "0–1440",
        &p.shown("idle_sleep_min", s.idle_sleep_min.to_string()),
    )
    .on_input(|v| Msg::PolicyNumber(Num::IdleSleep, v))
    .padding(8)
    .into();
    let doze: Element<'a, Msg> = text_input(
        "0–86400",
        &p.shown("doze_idle_s", s.doze_idle_s.to_string()),
    )
    .on_input(|v| Msg::PolicyNumber(Num::DozeIdle, v))
    .padding(8)
    .into();
    let roles: Element<'a, Msg> = text_input(
        "orchestrator, dev",
        &p.shown("sleep_never_roles", s.sleep_never_roles.join(", ")),
    )
    .on_input(Msg::PolicyRoles)
    .padding(8)
    .into();
    let checkpoints: Element<'a, Msg> = iced::widget::checkbox(s.vm_auto_checkpoints)
        .label("Take machine checkpoints before risky tool calls")
        .on_toggle(Msg::PolicyAutoCheckpoints)
        .into();
    let items: Vec<Element<'a, Msg>> = vec![
        labelled("Max turns", turns),
        labelled("Max tokens (0 = provider default)", tokens),
        labelled("Sleep an idle agent after (minutes, 0 = never)", idle),
        labelled("Doze after (seconds idle, 0 = never)", doze),
        labelled("Roles that never sleep (comma-separated)", roles),
        checkpoints,
        labelled("Effort", effort),
        labelled("Fetch blocklist (every other host is allowed)", block),
    ];
    mdl_card("Limits", column(items).spacing(12).into())
}

/// The coding engines a `code_engine` call may name, and how they run. At least one stays on.
fn engines_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let items: Vec<Element<'a, Msg>> = ENGINE_IDS
        .into_iter()
        .map(|id| {
            iced::widget::checkbox(s.engines_enabled.iter().any(|e| e == id))
                .label(engine_label(id))
                .on_toggle(move |on| Msg::PolicyEngine(id.to_string(), on))
                .into()
        })
        .collect();
    mdl_card("Engines", column(items).spacing(12).into())
}

fn prices_card<'a>(state: &'a State, p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for entry in s
        .model_policy
        .orchestrator
        .iter()
        .chain(s.model_policy.worker.iter())
    {
        keys.insert(parse_entry(entry).0.to_string());
    }
    keys.extend(s.prices.keys().cloned());

    let toggle = ui::link("Models", Msg::TogglePrices, false);
    if !state.prices_open {
        return mdl_card("Prices", toggle);
    }
    let header: Element<'a, Msg> = row(vec![
        ui::label("Model").into(),
        ui::label("Input (credits / 1M)").into(),
        ui::label("Output (credits / 1M)").into(),
    ])
    .spacing(8)
    .into();
    let mut table: Vec<Element<'a, Msg>> = vec![header];
    if keys.is_empty() {
        table.push(ui::empty("models", "No models listed", ""));
    }
    for key in keys {
        let price = s.prices.get(&key);
        let side_input = |side: Side| -> Element<'a, Msg> {
            let stored = price.map_or(0.0, |pr| match side {
                Side::Input => pr.input,
                Side::Output => pr.output,
            });
            let k = key.clone();
            text_input("0", &p.shown(&price_key(&key, side), stored.to_string()))
                .on_input(move |v| Msg::PolicyNumber(Num::Price(k.clone(), side), v))
                .padding(6)
                .into()
        };
        let key_cell: Element<'a, Msg> = if listed_trains(&p.models, &key) {
            row(vec![
                ui::sans(key.clone(), 12.0, crate::theme::palette().ink).into(),
                ui::trains_badge(),
            ])
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            ui::sans(key.clone(), 12.0, crate::theme::palette().ink).into()
        };
        let line: Element<'a, Msg> = row(vec![
            key_cell,
            side_input(Side::Input),
            side_input(Side::Output),
        ])
        .spacing(8)
        .into();
        table.push(line);
    }
    let items: Vec<Element<'a, Msg>> = vec![toggle, column(table).spacing(8).into()];
    mdl_card("Prices", column(items).spacing(12).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_line_reads_how_old_the_newest_check_is() {
        assert_eq!(checked_label(None, 1_000), "Not checked yet");
        assert_eq!(checked_label(Some(10_000), 40_000), "Checked just now");
        assert_eq!(checked_label(Some(0), 5 * 60_000), "Checked 5 min ago");
        assert_eq!(checked_label(Some(0), 3 * 3_600_000), "Checked 3 h ago");
    }

    #[test]
    fn accents_are_hex_colours_and_nothing_else() {
        assert_eq!(
            accent_color("#5cc8ff"),
            Some(Color::from_rgb8(0x5c, 0xc8, 0xff))
        );
        assert_eq!(accent_color("5cc8ff"), None);
        assert_eq!(accent_color("#5cc8f"), None);
    }

    #[test]
    fn an_instance_list_puts_the_enabled_providers_first() {
        let instance = |id: &str, enabled: bool| ApiInstance {
            id: id.to_string(),
            enabled,
            ..ApiInstance::default()
        };
        let api = vec![
            instance("a", false),
            instance("b", true),
            instance("c", true),
        ];
        let order: Vec<&str> = api_rows(&api).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(order, ["b", "c", "a"]);
        assert_eq!(first_sel(&api, &[]), Some(Sel::Api("b".to_string())));
    }

    #[test]
    fn compact_tokens_read_in_millions() {
        assert_eq!(compact(4_100_000), "4.1M");
        assert_eq!(compact(12_340), "12,340");
    }

    #[test]
    fn settings_without_engine_fields_take_the_hubs_defaults() {
        let bare = json!({ "default_model": "a", "orchestrator_model": "b" });
        let s: Settings = serde_json::from_value(bare).expect("settings parse");
        assert_eq!(s.engines_enabled, ["claude", "codex", "opencode"]);
        assert!(!s.acp_enabled);
    }

    #[test]
    fn policy_save_keeps_the_hubs_acp_switches() {
        let hub: Settings = serde_json::from_value(json!({
            "default_model": "a",
            "orchestrator_model": "b",
            "acp_enabled": true,
            "acp_write_back": true,
            "acp_v2": true
        }))
        .expect("settings parse");
        let mut copy: Settings = serde_json::from_value(json!({
            "default_model": "a",
            "orchestrator_model": "b",
            "acp_enabled": false,
            "acp_write_back": false,
            "acp_v2": false
        }))
        .expect("settings parse");
        keep_acp_switches(&mut copy, &hub);
        assert!(copy.acp_enabled);
        assert!(copy.acp_write_back);
        assert!(copy.acp_v2);
    }

    #[test]
    fn acp_switches_are_off_until_the_owner_turns_them_on() {
        let bare = json!({ "default_model": "a", "orchestrator_model": "b" });
        let s: Settings = serde_json::from_value(bare).expect("settings parse");
        for (case, on) in [
            (
                "write-back is off until the owner turns it on",
                s.acp_write_back,
            ),
            ("acp v2 is off until the owner turns it on", s.acp_v2),
        ] {
            assert!(!on, "{case}");
        }
    }

    #[test]
    fn engine_fields_round_trip_to_the_hub() {
        let raw = json!({
            "default_model": "a",
            "orchestrator_model": "b",
            "engines_enabled": ["opencode"],
            "acp_enabled": true,
            "some_future_field": 3
        });
        let s: Settings = serde_json::from_value(raw).expect("settings parse");
        let back = serde_json::to_value(&s).expect("settings serialize");
        assert_eq!(back["engines_enabled"], json!(["opencode"]));
        assert_eq!(back["acp_enabled"], true);
        assert_eq!(back["some_future_field"], 3);
    }

    #[test]
    fn price_path_encodes_the_provider_model_key() {
        assert_eq!(
            price_path("anthropic/claude-opus-5-5"),
            "/v1/prices/anthropic%2Fclaude-opus-5-5"
        );
    }

    #[test]
    fn the_effort_list_is_the_hubs_levels_after_default() {
        assert_eq!(
            EFFORTS,
            [
                "default", "none", "minimal", "low", "medium", "high", "xhigh", "max"
            ]
        );
    }

    #[test]
    fn settings_without_the_sleep_fields_take_the_hubs_defaults() {
        let bare = json!({"default_model": "a/b", "orchestrator_model": "a/c"});
        let settings: Settings = serde_json::from_value(bare).unwrap();
        assert_eq!(settings.idle_sleep_min, 10);
        assert_eq!(settings.doze_idle_s, 0);
        assert_eq!(settings.sleep_never_roles, vec!["orchestrator".to_string()]);
        assert!(settings.vm_auto_checkpoints);
    }

    #[test]
    fn the_sleep_fields_are_read_and_kept_for_the_save() {
        let saved = json!({
            "default_model": "a/b", "orchestrator_model": "a/c",
            "idle_sleep_min": 0, "doze_idle_s": 0, "sleep_never_roles": ["orchestrator", "dev"],
            "vm_auto_checkpoints": false
        });
        let settings: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(settings.idle_sleep_min, 0);
        assert_eq!(settings.doze_idle_s, 0);
        assert_eq!(settings.sleep_never_roles.len(), 2);
        assert!(!settings.vm_auto_checkpoints);
        let back = serde_json::to_value(&settings).unwrap();
        assert_eq!(back["idle_sleep_min"], json!(0));
        assert_eq!(back["doze_idle_s"], json!(0));
        assert_eq!(back["vm_auto_checkpoints"], json!(false));
    }

    #[test]
    fn a_doze_time_is_whole_seconds_up_to_a_day() {
        assert_eq!(doze_seconds("0"), Some(0));
        assert_eq!(doze_seconds(" 86400 "), Some(86400));
        assert_eq!(doze_seconds("86401"), None);
        assert_eq!(doze_seconds("-1"), None);
        assert_eq!(doze_seconds("2.5"), None);
        assert_eq!(doze_seconds(""), None);
    }
}
