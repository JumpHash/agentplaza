//! The Models screen (parity with the web console's `models` view). Two tabs. Providers: the
//! hub's providers as cards grouped by how they are credentialed, with keys, tests, their models,
//! edits, removal, and adding one from the catalogue. Policy & defaults: the orchestrator and
//! worker model lists, the defaults and per-role models, the limits (with the fetch blocklist:
//! a domain and its subdomains, or `*.domain` for subdomains only) and the prices, saved whole.
//! A key is typed into a secure field and is never echoed or logged.
//!
//! Captures: `APZ_MODELS_TAB=policy` opens the Policy & defaults tab at start.

use std::collections::{BTreeMap, BTreeSet};

use iced::font::{Font, Weight};
use iced::widget::text::{self as text_mod, Wrapping};
use iced::widget::{button, column, container, pick_list, rich_text, row, span, text, text_editor, text_input, Space};
use iced::{Alignment, Border, Element, Length, Padding, Task, Theme};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::usage::enc;
use super::Id;
use crate::console::client::{self, Client};
use crate::console::ui::{Kind, Tone};
use crate::console::{ui, Ctx};
use crate::theme::{self, MONO, SERIF};

const MONO_SEMIBOLD: Font = Font { weight: Weight::Semibold, ..MONO };
const SERIF_SEMIBOLD: Font = Font { weight: Weight::Semibold, ..SERIF };

const GROUPS: [&str; 3] = ["Configured with a key", "Needs a key", "Local"];
const EFFORTS: [&str; 6] = ["default", "low", "medium", "high", "xhigh", "max"];
const DECISIONS_LEDE: &str = "Models that answer typed questions with calibrated probabilities instead of writing text. The plaza uses them to decide; agents never chat with them.";

/// The model group a provider view belongs to when the hub does not say ("chat" or "decisions").
fn chat_group() -> String {
    "chat".to_string()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Providers,
    Policy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderKind {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub brand: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth: Value,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth: Value,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub extra_headers: Map<String, Value>,
    pub credential: String,
    #[serde(default = "chat_group")]
    pub group: String,
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
    #[serde(default)]
    pub fetch_block: Vec<String>,
    #[serde(default)]
    pub prices: BTreeMap<String, Price>,
    /// The owner's confirmation that contributor models may be defaults, role models and policy entries.
    #[serde(default)]
    pub allow_training_models: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_models: Option<bool>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
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

/// What an add or edit form is for.
#[derive(Debug, Clone)]
pub enum Target {
    Edit(Provider),
    Add(ProviderKind),
}

#[derive(Debug, Clone)]
pub struct Form {
    target: Target,
    id: String,
    id_editable: bool,
    base_url: String,
    /// The auth env var, when the provider authenticates from one.
    env: Option<String>,
    /// Comma-separated models; None hides the field.
    models: Option<String>,
    /// The optional key, add only.
    key: Option<String>,
    saving: bool,
}

/// The dialog open over the providers tab.
#[derive(Debug, Clone, Default)]
pub enum Dialog {
    #[default]
    Closed,
    Key { provider: String, value: String, saving: bool },
    Remove { provider: String, busy: bool },
    Catalogue { search: String },
    Form(Form),
}

impl Dialog {
    fn unbusy(&mut self) {
        match self {
            Dialog::Key { saving, .. } => *saving = false,
            Dialog::Remove { busy, .. } => *busy = false,
            Dialog::Form(form) => form.saving = false,
            Dialog::Closed | Dialog::Catalogue { .. } => {}
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
        let known: BTreeSet<String> = view.models.iter().map(|m| format!("{}/{}", m.provider, m.id)).collect();
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
    kinds: Vec<ProviderKind>,
    providers: Vec<Provider>,
    expanded: BTreeSet<String>,
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
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Tab(Tab),
    Loaded(Result<(Vec<ProviderKind>, Vec<Provider>), String>),
    Test(String),
    Tested(Result<TestReport, String>),
    ToggleModels(String),
    Listed(String, Result<Vec<ListedModel>, String>),
    /// A model's price (`GET /v1/prices/<provider>/<model>`), keyed `provider/model`.
    Price(String, Result<Value, String>),
    OpenKey(String),
    KeyInput(String),
    SaveKey,
    OpenEdit(String),
    OpenRemove(String),
    ConfirmRemove,
    OpenCatalogue,
    CatalogueSearch(String),
    PickKind(String),
    FormId(String),
    FormUrl(String),
    FormEnv(String),
    FormModels(String),
    FormKey(String),
    FormSave,
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
        Msg::Loaded(Ok((kinds, providers))) => {
            state.kinds = kinds;
            state.providers = providers;
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Test(id) => {
            let c = ctx.client.clone();
            client::task(move || c.post::<TestReport>(&format!("/v1/providers/{id}/test"), &json!({})), Msg::Tested)
        }
        Msg::Tested(Ok(report)) => {
            if report.ok {
                state.notice = Some(format!("OK · {} ms · {}", report.latency_ms, report.model));
                state.error = None;
            } else {
                state.error = Some(report.error.unwrap_or_else(|| "Test failed".to_string()));
            }
            Task::none()
        }
        Msg::Tested(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::ToggleModels(id) => {
            if state.expanded.remove(&id) {
                return Task::none();
            }
            state.expanded.insert(id.clone());
            if state.listed.contains_key(&id) {
                Task::none()
            } else {
                listed_task(ctx, id)
            }
        }
        Msg::Listed(id, Ok(list)) => {
            let tasks: Vec<Task<Msg>> = list
                .iter()
                .map(|m| {
                    let c = ctx.client.clone();
                    let key = format!("{id}/{}", m.id);
                    let path = price_path(&key);
                    client::task(move || c.get::<Value>(&path), move |r| Msg::Price(key.clone(), r))
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
            state.dialog = Dialog::Key { provider: id, value: String::new(), saving: false };
            Task::none()
        }
        Msg::KeyInput(v) => {
            if let Dialog::Key { value, .. } = &mut state.dialog {
                *value = v;
            }
            Task::none()
        }
        Msg::SaveKey => {
            let Dialog::Key { provider, value, saving } = &mut state.dialog else { return Task::none() };
            let key = value.trim().to_string();
            if key.is_empty() || *saving {
                return Task::none();
            }
            *saving = true;
            let path = format!("/v1/providers/{provider}/secret");
            let c = ctx.client.clone();
            client::task(move || c.call("PUT", &path, Some(&json!({ "value": key }))).map(|_| "Key saved".to_string()), Msg::Done)
        }
        Msg::OpenEdit(id) => {
            if let Some(p) = state.providers.iter().find(|p| p.id == id).cloned() {
                state.dialog = Dialog::Form(Form {
                    id: p.id.clone(),
                    id_editable: false,
                    base_url: p.base_url.clone().unwrap_or_default(),
                    env: auth_env(&p.auth),
                    models: Some(p.models.join(", ")),
                    key: None,
                    saving: false,
                    target: Target::Edit(p),
                });
            }
            Task::none()
        }
        Msg::OpenRemove(id) => {
            state.dialog = Dialog::Remove { provider: id, busy: false };
            Task::none()
        }
        Msg::ConfirmRemove => {
            let Dialog::Remove { provider, busy } = &mut state.dialog else { return Task::none() };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let path = format!("/v1/providers/{provider}");
            let note = format!("Removed {provider}");
            let c = ctx.client.clone();
            client::task(move || c.delete(&path).map(|_| note), Msg::Done)
        }
        Msg::OpenCatalogue => {
            state.dialog = Dialog::Catalogue { search: String::new() };
            Task::none()
        }
        Msg::CatalogueSearch(s) => {
            if let Dialog::Catalogue { search } = &mut state.dialog {
                *search = s;
            }
            Task::none()
        }
        Msg::PickKind(id) => {
            if let Some(k) = state.kinds.iter().find(|k| k.id == id).cloned() {
                state.dialog = Dialog::Form(Form {
                    id: k.id.clone(),
                    id_editable: true,
                    base_url: k.base_url.clone().unwrap_or_default(),
                    env: auth_env(&k.auth),
                    models: None,
                    key: Some(String::new()),
                    saving: false,
                    target: Target::Add(k),
                });
            }
            Task::none()
        }
        Msg::FormId(s) => {
            if let Dialog::Form(f) = &mut state.dialog {
                f.id = s;
            }
            Task::none()
        }
        Msg::FormUrl(s) => {
            if let Dialog::Form(f) = &mut state.dialog {
                f.base_url = s;
            }
            Task::none()
        }
        Msg::FormEnv(s) => {
            if let Dialog::Form(f) = &mut state.dialog {
                f.env = Some(s);
            }
            Task::none()
        }
        Msg::FormModels(s) => {
            if let Dialog::Form(f) = &mut state.dialog {
                f.models = Some(s);
            }
            Task::none()
        }
        Msg::FormKey(s) => {
            if let Dialog::Form(f) = &mut state.dialog {
                f.key = Some(s);
            }
            Task::none()
        }
        Msg::FormSave => {
            let Dialog::Form(form) = &mut state.dialog else { return Task::none() };
            if form.saving {
                return Task::none();
            }
            let id = form.id.trim().to_string();
            if id.is_empty() {
                state.error = Some("Give the provider an id".to_string());
                return Task::none();
            }
            form.saving = true;
            let base = Some(form.base_url.trim().to_string()).filter(|s| !s.is_empty());
            let auth = form.env.as_ref().map(|e| json!({ "env": e.trim() }));
            let models = form.models.as_ref().map(|m| split_list(m));
            let key = form.key.as_deref().unwrap_or("").trim().to_string();
            let target = form.target.clone();
            let c = ctx.client.clone();
            match target {
                Target::Edit(p) => {
                    let mut body = json!({
                        "id": p.id.clone(),
                        "kind": p.kind.clone(),
                        "base_url": base,
                        "auth": auth.unwrap_or_else(|| p.auth.clone()),
                        "models": models.unwrap_or_else(|| p.models.clone()),
                        "extra_headers": p.extra_headers.clone(),
                    });
                    if let Some(o) = &p.options {
                        body["options"] = o.clone();
                    }
                    let path = format!("/v1/providers/{}", p.id);
                    client::task(move || c.call("PUT", &path, Some(&body)).map(|_| "Saved".to_string()), Msg::Done)
                }
                Target::Add(k) => {
                    let mut body = json!({
                        "id": id.clone(),
                        "kind": k.kind.clone(),
                        "base_url": base,
                        "auth": auth.unwrap_or_else(|| k.auth.clone()),
                        "models": k.models.clone(),
                    });
                    if let Some(o) = &k.options {
                        body["options"] = o.clone();
                    }
                    let note = format!("Added {id}");
                    client::task(
                        move || -> Result<String, String> {
                            c.post::<Value>("/v1/providers", &body)?;
                            if !key.is_empty() {
                                c.call("PUT", &format!("/v1/providers/{id}/secret"), Some(&json!({ "value": key })))?;
                            }
                            Ok(note)
                        },
                        Msg::Done,
                    )
                }
            }
        }
        Msg::Close => {
            state.dialog = Dialog::Closed;
            Task::none()
        }
        Msg::Done(Ok(note)) => {
            state.dialog = Dialog::Closed;
            state.notice = Some(note);
            state.error = None;
            providers_task(ctx)
        }
        Msg::Done(Err(e)) => {
            state.dialog.unbusy();
            state.error = Some(e);
            Task::none()
        }
        Msg::PolicyLoaded(Ok((settings, view))) => {
            if state.policy.as_ref().map_or(true, |p| !p.dirty() && !p.saving) {
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
            let Some(p) = state.policy.as_mut() else { return Task::none() };
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
        Msg::PolicyEffort(v) => {
            if let Some(p) = state.policy.as_mut() {
                p.copy.effort = if v == "default" { None } else { Some(v) };
            }
            Task::none()
        }
        Msg::PolicyNumber(num, text) => {
            let Some(p) = state.policy.as_mut() else { return Task::none() };
            let key = match &num {
                Num::MaxTurns => "max_turns".to_string(),
                Num::MaxTokens => "max_tokens".to_string(),
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
            let Some(p) = state.policy.as_mut() else { return Task::none() };
            if p.saving || !p.errors.is_empty() || !p.dirty() {
                return Task::none();
            }
            p.saving = true;
            let s = p.copy.clone();
            let c = ctx.client.clone();
            client::task(move || save_settings(&c, &s), Msg::PolicySaved)
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
            let Some((provider, model)) = id.split_once('/') else { return Task::none() };
            let body = json!({ "provider": provider, "model": model });
            state.decision_pending = Some(id.clone());
            let c = ctx.client.clone();
            client::task(move || c.call("PUT", "/v1/decisions/default", Some(&body)).map(|_| id), Msg::DecisionSaved)
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
            let Some((provider, model)) = target.split_once('/') else { return Task::none() };
            let body = json!({ "provider": provider, "model": model, "input": input, "questions": [question] });
            t.asking = true;
            t.error = None;
            let c = ctx.client.clone();
            client::task(move || c.post::<DecisionReply>("/v1/decisions", &body), Msg::Asked)
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
    ui::sheet(ui::view_head("Models", vec![]), column(parts).spacing(16).into())
}

/// The tab strip, as the web's `.tabs`: mono caps, the chosen tab underlined, a hairline under the
/// strip. The shared `ui::tabs` stretches each cell across the strip; these cells keep to their labels.
fn tabs_strip<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut strip = row(Vec::new()).spacing(22);
    for (tab, name) in [(Tab::Providers, "Providers"), (Tab::Policy, "Policy & defaults")] {
        let on = state.tab == tab;
        let spans: Vec<text_mod::Span<'a>> = vec![span(name.to_uppercase()).font(MONO_SEMIBOLD).size(11.5).color(if on { crate::theme::palette().ink } else { crate::theme::palette().ink_3 }).underline(on)];
        let label = rich_text(spans);
        let cell = button(container(label).padding(Padding { top: 10.0, right: 0.0, bottom: 8.0, left: 0.0 }))
            .padding(0.0)
            .style(move |_: &Theme, status: button::Status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: None,
                    text_color: if on || hovered { crate::theme::palette().ink } else { crate::theme::palette().ink_3 },
                    border: Border::default(),
                    ..Default::default()
                }
            })
            .on_press(Msg::Tab(tab));
        strip = strip.push(cell);
    }
    column(vec![strip.into(), ui::hairline()]).into()
}

fn providers_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || -> Result<(Vec<ProviderKind>, Vec<Provider>), String> {
            let kinds = c.get::<Vec<ProviderKind>>("/v1/provider-kinds")?;
            let providers = c.get::<Vec<Provider>>("/v1/providers")?;
            Ok((kinds, providers))
        },
        Msg::Loaded,
    )
}

fn policy_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || -> Result<(Settings, ModelsView), String> {
            Ok((c.get::<Settings>("/v1/settings")?, c.get::<ModelsView>("/v1/models")?))
        },
        Msg::PolicyLoaded,
    )
}

fn listed_task(ctx: &Ctx, id: String) -> Task<Msg> {
    let c = ctx.client.clone();
    let id2 = id.clone();
    client::task(move || c.get::<Vec<ListedModel>>(&format!("/v1/providers/{id}/models")), move |r| Msg::Listed(id2.clone(), r))
}

fn save_settings(c: &Client, s: &Settings) -> Result<Settings, String> {
    let body = serde_json::to_value(s).map_err(|e| e.to_string())?;
    let v = c.call("PUT", "/v1/settings", Some(&body))?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// The kind entry a configured provider was made from: same id, else the first of the same kind.
/// A policy sub-heading: serif semibold 13px in ink-3, as the web's `.models-policy h3`.
fn h3<'a>(title: &str) -> Element<'a, Msg> {
    text(title.to_string()).font(SERIF_SEMIBOLD).size(13.0).color(crate::theme::palette().ink_3).into()
}

/// The provider kinds that answer decision questions; the rest are chat (as the web's `isDecisionKind`).
fn is_decision_kind(kind: &str) -> bool {
    matches!(kind, "openai-decisions" | "systemone" | "azure-decisions")
}

fn kind_of<'a>(state: &'a State, p: &Provider) -> Option<&'a ProviderKind> {
    state.kinds.iter().find(|k| k.id == p.id).or_else(|| state.kinds.iter().find(|k| k.kind == p.kind))
}

fn group_of(p: &Provider) -> &'static str {
    match p.credential.as_str() {
        "missing" => "Needs a key",
        "none" => "Local",
        _ => "Configured with a key",
    }
}

fn credential_badge(credential: &str) -> (&'static str, Tone) {
    match credential {
        "env" | "secret" => ("Key set", Tone::Ok),
        "none" => ("No key needed", Tone::Muted),
        "command" => ("Command", Tone::Info),
        "missing" => ("Key missing", Tone::Warn),
        "sealed" => ("Sealed", Tone::Warn),
        "out_of_scope" => ("Out of scope", Tone::Muted),
        _ => ("Unknown", Tone::Muted),
    }
}

/// A provider's model count line. OpenRouter's list is fetched live from its API, so the line says so.
fn model_count_label(kind_id: &str, n: usize) -> String {
    let noun = if n == 1 { "model" } else { "models" };
    if kind_id == "openrouter" {
        format!("fetched live, {n} {noun}")
    } else {
        format!("{n} {noun}")
    }
}

fn auth_env(auth: &Value) -> Option<String> {
    auth.get("env")?.as_str().map(str::to_string)
}

/// Comma- or newline-separated values, trimmed, without blanks.
fn split_list(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c == '\n').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
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
        Some((p, m)) => !p.is_empty() && !m.is_empty() && !p.chars().any(char::is_whitespace) && !m.chars().any(char::is_whitespace),
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
        out.push(Opt { value: String::new(), label: label.to_string() });
    }
    let names: BTreeMap<String, String> = p
        .models
        .iter()
        .map(|m| {
            let name = m.name.clone().unwrap_or_else(|| m.id.clone());
            let label = if m.trains_on_data { format!("{name} · WARNING: trains on your data") } else { name };
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
        let label = names.get(current).cloned().unwrap_or_else(|| current.to_string());
        out.push(Opt { value: current.to_string(), label });
    }
    out
}

/// Whether the `provider/model` `base` is a contributor model the hub listed (`/v1/models`).
fn listed_trains(models: &[ModelInfo], base: &str) -> bool {
    models.iter().any(|m| m.trains_on_data && format!("{}/{}", m.provider, m.id) == base)
}

/// Whether a preset's `options.trains_on_data` names the model `id`.
fn options_trains(options: Option<&Value>, id: &str) -> bool {
    options.and_then(|o| o.get("trains_on_data")).and_then(Value::as_array).is_some_and(|names| names.iter().any(|n| n.as_str() == Some(id)))
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
    pick_list(options, selected, move |o: Opt| Msg::PolicyDefault(slot.clone(), o)).width(Length::Fill).into()
}

// ----- Providers tab -------------------------------------------------------------------------

/// Cards per row in a provider or kind grid, as the web's `minmax(300px, 1fr)` at the console's width.

fn providers_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![row(vec![spacer(), ui::btn("Add provider", None, Kind::Primary, Some(Msg::OpenCatalogue))]).into()];
    if let Some(d) = dialog_view(state) {
        parts.push(d);
    }
    if state.providers.is_empty() {
        parts.push(ui::empty("models", "No providers yet", ""));
    }
    parts.push(chat_section(state));
    parts.push(decisions_section(state));
    column(parts).spacing(24).into()
}

/// The providers tab's section heading: mono caps, as the web's `.prov-section-title`.
fn section_title<'a>(title: &str) -> Element<'a, Msg> {
    text(title.to_uppercase()).font(MONO_SEMIBOLD).size(16.0).color(crate::theme::palette().ink).into()
}

/// A credential group's heading: small mono caps over a hairline, as the web's `.prov-group-title`.
fn group_title<'a>(title: &str) -> Element<'a, Msg> {
    column(vec![text(title.to_uppercase()).font(MONO).size(11.0).color(crate::theme::palette().ink_3).into(), ui::hairline()]).spacing(6).into()
}

/// The chat providers, under their credential groups.
fn chat_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![section_title("Chat models")];
    for credential in GROUPS {
        let cards: Vec<Element<'a, Msg>> = state.providers.iter().filter(|p| p.group == "chat" && group_of(p) == credential).map(|p| provider_card(state, p)).collect();
        if cards.is_empty() {
            continue;
        }
        items.push(column(vec![group_title(credential), ui::card_grid(cards)]).spacing(12).into());
    }
    column(items).spacing(12).into()
}

/// The Decisions section: the decision providers, the decision model and the Try-it form.
fn decisions_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let cards: Vec<Element<'a, Msg>> = state.providers.iter().filter(|p| p.group == "decisions").map(|p| provider_card(state, p)).collect();
    let mut items: Vec<Element<'a, Msg>> = vec![section_title("Decisions"), text(DECISIONS_LEDE).font(theme::SERIF).size(16.0).color(crate::theme::palette().ink_2).into()];
    if !cards.is_empty() {
        items.push(ui::card_grid(cards));
    }
    items.push(decision_model_card(state));
    items.push(try_it_card(state));
    column(items).spacing(16).into()
}


/// The decision models the plaza may use: every decisions-group provider's models, as `provider/model`.
fn decision_options(providers: &[Provider]) -> Vec<String> {
    providers.iter().filter(|p| p.group == "decisions").flat_map(|p| p.models.iter().map(move |m| format!("{}/{m}", p.id))).collect()
}

fn decision_model_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let options = decision_options(&state.providers);
    let current = state.decision_pending.clone().or_else(|| state.decision_default.clone());
    let mut items: Vec<Element<'a, Msg>> = vec![ui::faint("The model the plaza decides with.").into()];
    if options.is_empty() {
        items.push(ui::faint("Add a decisions provider to choose one.").into());
    } else {
        items.push(pick_list(options, current, Msg::DecisionPick).width(Length::Fill).into());
    }
    if state.decision_pending.is_some() {
        items.push(ui::faint("Saving…").into());
    }
    ui::card("Decision model", column(items).spacing(12).into())
}

fn try_it_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let t = &state.try_it;
    let input: Element<'a, Msg> = text_editor(&t.input).placeholder("The input the question is about").on_action(Msg::TryInput).height(120.0).into();
    let kinds: Vec<QKind> = vec![QKind::Predicate, QKind::Choice, QKind::Score];
    let mut items: Vec<Element<'a, Msg>> = vec![
        labelled("Input", input),
        labelled("Question", pick_list(kinds, Some(t.kind), Msg::TryKind).width(Length::Fill).into()),
        labelled("Instructions", text_input("What to decide", &t.instructions).on_input(Msg::TryInstructions).padding(8).into()),
    ];
    if t.kind != QKind::Predicate {
        let (label, hint) = if t.kind == QKind::Choice { ("Choices", "Comma-separated, at least two") } else { ("Levels", "Comma-separated, lowest first") };
        items.push(labelled(label, text_input(hint, &t.options).on_input(Msg::TryOptions).padding(8).into()));
    }
    items.push(row(vec![spacer(), ui::btn("Ask", None, Kind::Primary, (!t.asking).then_some(Msg::TryAsk))]).into());
    if let Some(e) = &t.error {
        items.push(ui::error_line(e));
    }
    if let Some(reply) = &t.answer {
        items.push(answers_view(reply));
    }
    ui::card("Try it", column(items).spacing(12).into())
}

/// The one question the Try-it form asks, in the plaza's request shape.
fn question_json(kind: QKind, instructions: &str, options: &str) -> Result<Value, String> {
    let items = split_list(options);
    let mut question = json!({ "type": kind.key(), "name": "question", "instructions": instructions.trim() });
    match kind {
        QKind::Predicate => {}
        QKind::Choice => {
            if items.len() < 2 {
                return Err("A choice question needs at least two choices".to_string());
            }
            question["choices"] = items.iter().map(|v| json!({ "value": v })).collect::<Vec<Value>>().into();
        }
        QKind::Score => {
            if items.len() < 2 {
                return Err("A score question needs at least two levels".to_string());
            }
            question["levels"] = items.iter().map(|v| json!({ "label": v })).collect::<Vec<Value>>().into();
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
        Answer::Choice { name, choice, probabilities, confidence } => {
            let head: Element<'a, Msg> = row(vec![ui::mono(name.clone(), 12.0, crate::theme::palette().ink).into(), spacer(), ui::badge(&value_text(choice), Tone::Signal)]).spacing(8).into();
            let mut items: Vec<Element<'a, Msg>> = vec![head];
            items.extend(probabilities.iter().map(|w| bar_line(weighted_label(w), w.probability)));
            items.extend(confidence.map(confidence_line));
            column(items).spacing(6).into()
        }
        Answer::Score { name, score, probabilities, confidence } => {
            let head: Element<'a, Msg> = row(vec![ui::mono(name.clone(), 12.0, crate::theme::palette().ink).into(), spacer(), ui::badge(&format!("{score:.2}"), Tone::Signal)]).spacing(8).into();
            let mut items: Vec<Element<'a, Msg>> = vec![head];
            items.extend(probabilities.iter().map(|w| bar_line(weighted_label(w), w.probability)));
            items.extend(confidence.map(confidence_line));
            column(items).spacing(6).into()
        }
        Answer::Refusal { name } => row(vec![ui::mono(name.clone(), 12.0, crate::theme::palette().ink).into(), spacer(), ui::badge("Refused", Tone::Warn)]).spacing(8).into(),
    }
}

/// A label with its percentage, and the probability as a bar under it.
fn bar_line<'a>(label: String, probability: f64) -> Element<'a, Msg> {
    let head: Element<'a, Msg> = row(vec![ui::mono(label, 12.0, crate::theme::palette().ink).into(), spacer(), ui::mono(format!("{:.1}%", probability * 100.0), 11.0, crate::theme::palette().ink_3).into()])
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
            .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().signal.into()), ..Default::default() })
            .into(),
        container(Space::new())
            .width(Length::FillPortion(1000 - filled))
            .height(6.0)
            .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().rule_faint.into()), ..Default::default() })
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
    client::task(move || c.get::<DecisionDefault>("/v1/decisions/default"), Msg::DefaultLoaded)
}

fn provider_card<'a>(state: &'a State, p: &'a Provider) -> Element<'a, Msg> {
    let kind = kind_of(state, p);
    let (credential, tone) = credential_badge(&p.credential);
    let brand = kind.map_or("mock", |k| k.brand.as_str());
    let title = kind.map_or(p.id.as_str(), |k| k.label.as_str());
    let n = p.models.len();
    let key_label = if p.credential == "missing" { "Add key" } else { "Key" };
    let sub = format!("{} · {}", p.id, p.base_url.clone().unwrap_or_else(|| "—".to_string()));

    // The brand in a ruled 48px tile, then the kind's name and the id and URL in mono.
    let logo: Element<'a, Msg> = container(ui::brand_logo::<Msg>(brand, 26.0))
        .padding(10)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().paper.into()),
            border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into();
    let names: Element<'a, Msg> = column(vec![
        text(title.to_string()).font(SERIF_SEMIBOLD).size(16.5).color(crate::theme::palette().ink).wrapping(Wrapping::None).into(),
        ui::mono(ui::ellipsize(&sub, 28), 11.5, crate::theme::palette().ink_3).wrapping(Wrapping::None).into(),
    ])
    .spacing(2)
    .width(Length::Fill)
    .into();
    let head: Element<'a, Msg> = row(vec![logo, names]).spacing(12).align_y(Alignment::Center).into();

    let kind_id = kind.map_or(p.id.as_str(), |k| k.id.as_str());
    let n_label = ui::faint(model_count_label(kind_id, n));
    let meta: Element<'a, Msg> = row(vec![ui::badge(credential, tone), spacer(), n_label.into()]).align_y(Alignment::Center).into();

    let actions: Element<'a, Msg> = row(vec![
        ui::btn_sm("Test", None, Kind::Secondary, Some(Msg::Test(p.id.clone()))),
        ui::btn_sm(key_label, None, Kind::Secondary, Some(Msg::OpenKey(p.id.clone()))),
        spacer(),
        ui::btn_sm("", Some("models"), Kind::Ghost, Some(Msg::ToggleModels(p.id.clone()))),
        ui::btn_sm("", Some("edit"), Kind::Ghost, Some(Msg::OpenEdit(p.id.clone()))),
        ui::btn_sm("", Some("trash"), Kind::Ghost, Some(Msg::OpenRemove(p.id.clone()))),
    ])
    .spacing(4)
    .align_y(Alignment::Center)
    .into();

    let mut body: Vec<Element<'a, Msg>> = vec![head, meta, column(vec![ui::hairline(), container(actions).padding(Padding { top: 10.0, right: 0.0, bottom: 0.0, left: 0.0 }).into()]).into()];
    if state.expanded.contains(&p.id) {
        body.push(
            column(vec![ui::hairline(), container(model_list(state, p)).padding(Padding { top: 12.0, right: 0.0, bottom: 0.0, left: 0.0 }).into()]).into(),
        );
    }
    container(column(body).spacing(12))
        .padding(Padding { top: 16.0, right: 16.0, bottom: 12.0, left: 16.0 })
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

fn model_list<'a>(state: &'a State, p: &'a Provider) -> Element<'a, Msg> {
    let kind = kind_of(state, p);
    let trains = |id: &str| options_trains(p.options.as_ref(), id) || options_trains(kind.and_then(|k| k.options.as_ref()), id);
    match state.listed.get(&p.id) {
        None => ui::faint("Loading…").into(),
        Some(list) if list.is_empty() => ui::faint("No models listed").into(),
        Some(list) => {
            let mut items: Vec<Element<'a, Msg>> = list
                .iter()
                .map(|m| {
                    let mut line: Vec<Element<'a, Msg>> = vec![ui::mono(m.id.clone(), 12.0, crate::theme::palette().ink).into()];
                    if trains(&m.id) {
                        line.push(ui::trains_badge());
                    }
                    if let Some((i, o)) = state.model_prices.get(&format!("{}/{}", p.id, m.id)) {
                        line.push(ui::faint(format!("${i} in · ${o} out per 1M")).into());
                    }
                    row(line).spacing(8).align_y(Alignment::Center).into()
                })
                .collect();
            if list.iter().any(|m| trains(&m.id)) {
                items.push(ui::faint(ui::TRAINS_NOTE).into());
            }
            column(items).spacing(4).into()
        }
    }
}

fn dialog_view<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    match &state.dialog {
        Dialog::Closed => None,
        Dialog::Key { provider, value, saving } => {
            let input: Element<'a, Msg> = text_input("API key", value).secure(true).on_input(Msg::KeyInput).on_submit(Msg::SaveKey).padding(8).into();
            let save = ui::btn("Save key", None, Kind::Primary, (!*saving).then_some(Msg::SaveKey));
            let actions: Element<'a, Msg> = row(vec![spacer(), ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)), save]).spacing(8).into();
            let items: Vec<Element<'a, Msg>> = vec![input, actions];
            Some(ui::card(&format!("Set key · {provider}"), column(items).spacing(12).into()))
        }
        Dialog::Remove { provider, busy } => {
            let actions: Element<'a, Msg> = row(vec![
                spacer(),
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)),
                ui::btn("Remove provider", None, Kind::Danger, (!*busy).then_some(Msg::ConfirmRemove)),
            ])
            .spacing(8)
            .into();
            Some(ui::card(&format!("Remove {provider}?"), actions))
        }
        Dialog::Catalogue { search } => {
            let taken: BTreeSet<&str> = state.providers.iter().map(|p| p.id.as_str()).collect();
            let free: Vec<&'a ProviderKind> = state.kinds.iter().filter(|k| !taken.contains(k.id.as_str())).collect();
            let q = search.trim().to_lowercase();
            let shown: Vec<&'a ProviderKind> = free
                .iter()
                .copied()
                .filter(|k| q.is_empty() || format!("{} {} {}", k.label, k.id, k.kind).to_lowercase().contains(&q))
                .collect();
            let catalogue: Element<'a, Msg> = if shown.is_empty() {
                let none = if free.is_empty() { "Every provider is already configured" } else { "No matches" };
                ui::faint(none).into()
            } else {
                let mut parts: Vec<Element<'a, Msg>> = Vec::new();
                for (title, decisions) in [("Chat", false), ("Decisions", true)] {
                    let tiles: Vec<Element<'a, Msg>> = shown.iter().copied().filter(|k| is_decision_kind(&k.kind) == decisions).map(kind_tile).collect();
                    if !tiles.is_empty() {
                        parts.push(group_title(title));
                        parts.push(ui::card_grid(tiles));
                    }
                }
                column(parts).spacing(12).into()
            };
            let items: Vec<Element<'a, Msg>> = vec![text_input("Search providers", search).on_input(Msg::CatalogueSearch).padding(8).into(), catalogue];
            Some(ui::card("Add provider", column(items).spacing(12).into()))
        }
        Dialog::Form(form) => Some(form_view(state, form)),
    }
}

/// One provider kind in the catalogue: the brand and its label, in a ruled tile.
fn kind_tile<'a>(k: &'a ProviderKind) -> Element<'a, Msg> {
    let content: Vec<Element<'a, Msg>> = vec![
        ui::brand_logo::<Msg>(&k.brand, 18.0),
        text(k.label.as_str()).font(theme::SERIF).size(14.0).color(crate::theme::palette().ink).wrapping(Wrapping::None).width(Length::Fill).into(),
    ];
    button(row(content).spacing(8).align_y(Alignment::Center))
        .on_press(Msg::PickKind(k.id.clone()))
        .padding(12)
        .width(Length::Fill)
        .style(|_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(if hovered { crate::theme::palette().hover } else { crate::theme::palette().sheet }.into()),
                text_color: crate::theme::palette().ink,
                border: Border { color: if hovered { crate::theme::palette().ink_3 } else { crate::theme::palette().rule }, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            }
        })
        .into()
}

/// The shared add and edit form.
fn form_view<'a>(state: &'a State, form: &'a Form) -> Element<'a, Msg> {
    let (title, submit, notes) = match &form.target {
        Target::Edit(p) => (format!("Edit {}", p.id), "Save", kind_of(state, p).map(|k| k.notes.clone())),
        Target::Add(k) => (k.label.clone(), "Add provider", Some(k.notes.clone())),
    };
    let mut fields: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(n) = notes.filter(|n| !n.is_empty()) {
        fields.push(ui::faint(n).into());
    }
    let mut id_input = text_input("provider id", &form.id).padding(8);
    if form.id_editable {
        id_input = id_input.on_input(Msg::FormId);
    }
    fields.push(labelled("Provider id", id_input.into()));
    fields.push(labelled("Base URL", text_input("https://", &form.base_url).on_input(Msg::FormUrl).padding(8).into()));
    if let Some(env) = &form.env {
        fields.push(labelled("Auth env var", text_input("ENV_VAR_NAME", env).on_input(Msg::FormEnv).padding(8).into()));
    }
    if let Some(models) = &form.models {
        fields.push(labelled("Models", text_input("Comma-separated", models).on_input(Msg::FormModels).padding(8).into()));
    }
    if let Some(key) = &form.key {
        fields.push(labelled("API key", text_input("Optional", key).secure(true).on_input(Msg::FormKey).padding(8).into()));
    }
    let actions: Element<'a, Msg> = row(vec![
        spacer(),
        ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Close)),
        ui::btn(submit, None, Kind::Primary, (!form.saving).then_some(Msg::FormSave)),
    ])
    .spacing(8)
    .into();
    fields.push(actions);
    ui::card(&title, column(fields).spacing(12).into())
}

// ----- Policy & defaults tab -----------------------------------------------------------------

fn policy_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(p) = state.policy.as_ref() else {
        return if state.error.is_some() {
            ui::empty("alert", "Models unavailable", "apid did not answer. Reload to try again.")
        } else {
            ui::loading()
        };
    };
    let save_enabled = !p.saving && p.errors.is_empty() && p.dirty();
    let bar: Element<'a, Msg> = row(vec![spacer(), ui::btn("Save", Some("check"), Kind::Primary, save_enabled.then_some(Msg::PolicySave))]).into();
    let s = &p.copy;
    let trains = s.model_policy.orchestrator.iter().chain(&s.model_policy.worker).any(|e| listed_trains(&p.models, parse_entry(e).0))
        || [&s.default_model, &s.orchestrator_model].into_iter().chain(s.role_models.values()).any(|e| listed_trains(&p.models, parse_entry(e).0));
    let mut items: Vec<Element<'a, Msg>> = vec![bar];
    if trains {
        items.push(ui::faint(ui::TRAINS_NOTE).into());
    }
    items.extend([policy_card(p), defaults_card(p), limits_card(p), prices_card(state, p)]);
    column(items).spacing(16).into()
}

fn policy_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if p.saved.dev_models == Some(true) {
        body.push(ui::badge("Development models enabled — policy relaxed", Tone::Warn));
    }
    body.push(ui::prose("Agents may only run the models on their tier's list.").into());
    let cols: Vec<Element<'a, Msg>> = vec![tier_column(p, Tier::Orchestrator, "Orchestrator"), tier_column(p, Tier::Worker, "Workers")];
    body.push(row(cols).spacing(16).into());
    ui::card("Model policy", column(body).spacing(12).into())
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
    let add_row: Element<'a, Msg> = row(vec![add, ui::btn("Add model", None, Kind::Secondary, Some(Msg::PolicyAdd(tier)))]).spacing(8).into();
    let items: Vec<Element<'a, Msg>> = vec![h3(title), column(rows).spacing(6).into(), add_row];
    column(items).spacing(10).width(Length::FillPortion(1)).into()
}

fn policy_row<'a>(p: &'a Policy, tier: Tier, index: usize, entry: &'a str) -> Element<'a, Msg> {
    let (base, effort) = parse_entry(entry);
    let listed = p.known.contains(base);
    // The availability square: filled when the provider lists the model, hollow when it does not.
    let marker: Element<'a, Msg> = container(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_: &Theme| container::Style {
            background: if listed { Some(crate::theme::palette().ok.into()) } else { None },
            border: Border { color: if listed { crate::theme::palette().ok } else { crate::theme::palette().ink_3 }, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into();
    // The whole id, nested slashes included: it wraps rather than being cut off.
    let mut items: Vec<Element<'a, Msg>> = vec![marker, ui::mono(base.to_string(), 12.0, crate::theme::palette().ink).wrapping(Wrapping::WordOrGlyph).into()];
    if effort.is_some() {
        items.push(ui::badge("effort pinned", Tone::Signal));
    }
    if listed_trains(&p.models, base) {
        items.push(ui::trains_badge());
    }
    items.push(spacer());
    items.push(ui::btn("", Some("x"), Kind::Ghost, Some(Msg::PolicyRemove(tier, index))));
    column(vec![
        container(row(items).spacing(8).align_y(Alignment::Center)).padding(Padding { top: 8.0, right: 0.0, bottom: 8.0, left: 0.0 }).into(),
        ui::hairline(),
    ])
    .into()
}

fn defaults_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let mut items: Vec<Element<'a, Msg>> = vec![
        labelled("Orchestrator model", picker(model_options(p, Tier::Orchestrator, &s.orchestrator_model, None), &s.orchestrator_model, Slot::Orchestrator)),
        labelled("Worker default", picker(model_options(p, Tier::Worker, &s.default_model, None), &s.default_model, Slot::Worker)),
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
            ui::mono(role.clone(), 12.0, crate::theme::palette().ink).into(),
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
    ui::card("Defaults", column(items).spacing(12).into())
}

fn limits_card<'a>(p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let turns: Element<'a, Msg> = text_input("1–200", &p.shown("max_turns", s.max_turns.to_string()))
        .on_input(|v| Msg::PolicyNumber(Num::MaxTurns, v))
        .padding(8)
        .into();
    let tokens: Element<'a, Msg> = text_input("0", &p.shown("max_tokens", s.max_tokens.to_string()))
        .on_input(|v| Msg::PolicyNumber(Num::MaxTokens, v))
        .padding(8)
        .into();
    let effort_current = s.effort.clone().unwrap_or_else(|| "default".to_string());
    let effort_options: Vec<String> = EFFORTS.iter().map(|e| e.to_string()).collect();
    let effort: Element<'a, Msg> = pick_list(effort_options, Some(effort_current), |v: String| Msg::PolicyEffort(v))
        .width(Length::Fill)
        .into();
    let block: Element<'a, Msg> = text_editor(&p.block)
        .placeholder("example.com\n*.example.net")
        .on_action(Msg::PolicyFetchBlock)
        .height(120.0)
        .into();
    let items: Vec<Element<'a, Msg>> = vec![
        labelled("Max turns", turns),
        labelled("Max tokens (0 = provider default)", tokens),
        labelled("Effort", effort),
        labelled("Fetch blocklist (every other host is allowed)", block),
    ];
    ui::card("Limits", column(items).spacing(12).into())
}

fn prices_card<'a>(state: &'a State, p: &'a Policy) -> Element<'a, Msg> {
    let s = &p.copy;
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for entry in s.model_policy.orchestrator.iter().chain(s.model_policy.worker.iter()) {
        keys.insert(parse_entry(entry).0.to_string());
    }
    keys.extend(s.prices.keys().cloned());

    let toggle = ui::link("Models", Msg::TogglePrices, false);
    if !state.prices_open {
        return ui::card("Prices", toggle);
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
            row(vec![ui::mono(key.clone(), 12.0, crate::theme::palette().ink).into(), ui::trains_badge()]).spacing(8).align_y(Alignment::Center).into()
        } else {
            ui::mono(key.clone(), 12.0, crate::theme::palette().ink).into()
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
    ui::card("Prices", column(items).spacing(12).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_path_encodes_the_provider_model_key() {
        assert_eq!(price_path("anthropic/claude-opus-5-5"), "/v1/prices/anthropic%2Fclaude-opus-5-5");
    }
}
