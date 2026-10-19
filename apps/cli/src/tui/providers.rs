//! The Providers tab: the model providers and the coding engines on the plaza, the selected one with its setup, runtime,
//! variables and models (favourites starred), the usage hubs, and the keys that check, update, enable, favourite, add,
//! edit and remove them. The calls are the ones `agentplaza models` makes; the hub's health and engine events refresh
//! the tab. Declared from `app.rs`, which holds the state, sends the calls and draws the tab.
//!
//! Secrets: an API key is typed into a masked prompt and sent once to the hub; it is never drawn, never kept after the
//! submit, and never part of a notice or a `Debug` print.

use std::collections::HashMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use serde_json::{Value, json};

use super::Palette;
use crate::cmd::models::{self, EngineFlags, Family};
use crate::ctx::{self, Client};
use crate::tui::screens::clip;

/// The endpoint the poller reads while the tab is shown: the API providers.
pub const PATH: &str = "/v1/providers";
const ENGINES: &str = "/v1/engines";
const USAGE: &str = "/v1/usage-providers";
const SETTINGS: &str = "/v1/settings";
const CATALOG: &str = "/v1/provider-catalog";
const REGISTRY: &str = "/v1/acp/registry";
const MODELS: &str = "/v1/models";
/// What the tab reads besides `PATH`: when it is shown, on its events and on a refresh.
pub const READS: [&str; 6] = [ENGINES, USAGE, SETTINGS, CATALOG, REGISTRY, MODELS];
/// The fields of the add form: the kind (or registry id), the label, the id, the local command and its arguments.
const ADD_FIELDS: usize = 5;
/// The fields of the usage hub form: the URL, the management key, the label.
const HUB_FIELDS: usize = 3;
/// The fields of the runtime form: version, home, route, arguments (lines), variables (lines).
const RUNTIME_FIELDS: usize = 5;
/// The sentence the engine section shows: engines reach models only through the plaza's gateway.
const CONTAINMENT: &str = "Engines use the plaza's model gateway. To use a ChatGPT or Claude subscription, run a CLIProxyAPI hub and add it as a provider.";

/// The data the tab draws and decides from: the endpoints' last answers, by path (the app's `data`).
pub type Data = HashMap<&'static str, Value>;

/// One row of the list: an API provider or an engine, borrowed from the answer it was read as.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    pub family: Family,
    pub value: &'a Value,
}

impl<'a> Entry<'a> {
    fn id(&self) -> &'a str {
        ctx::s(self.value, "id")
    }

    /// The name shown: the label, else the id.
    fn label(&self) -> &'a str {
        let label = ctx::s(self.value, "label");
        if label.is_empty() { self.id() } else { label }
    }

    /// The API kind, or the engine's built-in or registry id.
    fn kind(&self) -> &'a str {
        match self.family {
            Family::Api => ctx::s(self.value, "kind"),
            Family::Engine => ctx::s(self.value, "engine"),
        }
    }

    fn enabled(&self) -> bool {
        self.value.get("enabled").and_then(Value::as_bool) == Some(true)
    }

    /// Whether an engine is one the plaza ships (those are turned off, not removed).
    fn builtin(&self) -> bool {
        self.value.get("builtin").and_then(Value::as_bool) == Some(true)
    }

    fn health(&self) -> &'a Value {
        self.value.get("health").unwrap_or(&Value::Null)
    }

    fn status(&self) -> &'a str {
        ctx::s(self.health(), "status")
    }

    fn message(&self) -> &'a str {
        ctx::s(self.health(), "message")
    }

    /// An engine's version: the one it runs, else the catalogue's pin.
    fn version(&self) -> String {
        let version = ctx::text(self.value, "version");
        if version.is_empty() {
            ctx::text(self.value, "pinned")
        } else {
            version
        }
    }

    /// An API provider's models (none for an engine).
    fn models(&self) -> Vec<&'a str> {
        if self.family == Family::Engine {
            return Vec::new();
        }
        self.value
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    }
}

/// The array under `key` of `v` (none when it is not one).
fn sub<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    match v.get(key).and_then(Value::as_array) {
        Some(items) => items.as_slice(),
        None => &[],
    }
}

/// The array the endpoint `path` answered with (none until it has).
fn array<'a>(data: &'a Data, path: &str) -> &'a [Value] {
    match data.get(path).and_then(Value::as_array) {
        Some(items) => items.as_slice(),
        None => &[],
    }
}

/// The models `/v1/models` lists (none until it has been read).
fn models_of(data: &Data) -> &[Value] {
    match data
        .get(MODELS)
        .and_then(|m| m.get("models"))
        .and_then(Value::as_array)
    {
        Some(items) => items.as_slice(),
        None => &[],
    }
}

/// The favourites the settings hold: `provider/model` entries.
fn favourites(data: &Data) -> Vec<String> {
    data.get(SETTINGS)
        .and_then(|s| s.get("favourite_models"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect()
}

/// The list: the API providers (enabled first), then the engines (enabled first), filtered by the search text.
pub fn entries<'a>(data: &'a Data, query: &str) -> Vec<Entry<'a>> {
    let needle = query.trim().to_lowercase();
    let mut apis: Vec<Entry<'a>> = array(data, PATH)
        .iter()
        .map(|value| Entry {
            family: Family::Api,
            value,
        })
        .collect();
    let mut engines: Vec<Entry<'a>> = array(data, ENGINES)
        .iter()
        .map(|value| Entry {
            family: Family::Engine,
            value,
        })
        .collect();
    apis.sort_by_key(|e| !e.enabled());
    engines.sort_by_key(|e| !e.enabled());
    apis.into_iter()
        .chain(engines)
        .filter(|e| {
            needle.is_empty()
                || [e.id(), e.label(), e.kind()]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
        })
        .collect()
}

/// One model the picker lists: `provider/model`, the provider's name, and whether it is a favourite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRow {
    pub key: String,
    pub name: String,
    pub favourite: bool,
}

/// The models `/v1/models` lists, favourites first (the order the pickers use).
pub fn picker_rows(data: &Data) -> Vec<PickRow> {
    let favs = favourites(data);
    let mut rows: Vec<PickRow> = models_of(data)
        .iter()
        .map(|m| {
            let key = format!("{}/{}", ctx::s(m, "provider"), ctx::s(m, "id"));
            let name = match ctx::s(m, "name") {
                "" => ctx::s(m, "id").to_string(),
                name => name.to_string(),
            };
            PickRow {
                favourite: favs.contains(&key),
                name,
                key,
            }
        })
        .collect();
    rows.sort_by_key(|r| !r.favourite);
    rows
}

/// A hub call: its method, its path and its body.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
}

/// What a key asks the app to do. The app runs the calls on a thread and notes the outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Nothing to do but redraw.
    None,
    /// A line for the footer.
    Note(String),
    /// Run these hub calls in order; the notice names them by `label`.
    Run { label: String, calls: Vec<Call> },
    /// Read the engines' update dry run; its answer comes back to `Screen::on_plan`.
    Plan,
    /// Read the providers, engines, usage hubs, settings, catalogue, registry and models again.
    Refresh,
}

/// One engine the dry run would update: its id and its version now and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// The add form: what to add (a provider kind, an engine, a registry agent, or a local command), its label and id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Add {
    pub kind: String,
    pub label: String,
    pub id: String,
    pub command: String,
    pub args: String,
    /// The field that takes the typed keys (0 kind, 1 label, 2 id, 3 command, 4 arguments).
    pub focus: usize,
    /// Why the last Enter did not add it.
    pub error: Option<String>,
}

impl Add {
    fn field_mut(&mut self) -> &mut String {
        match self.focus {
            0 => &mut self.kind,
            1 => &mut self.label,
            2 => &mut self.id,
            3 => &mut self.command,
            _ => &mut self.args,
        }
    }
}

/// The masked key prompt for an API instance. The typed key is cleared once it is sent.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct KeyPrompt {
    pub id: String,
    pub text: String,
    pub error: Option<String>,
}

impl fmt::Debug for KeyPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyPrompt")
            .field("id", &self.id)
            .field("key", &"<redacted>")
            .field("error", &self.error)
            .finish()
    }
}

/// The usage hub form: the hub's URL, its management key (masked), its label.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct HubForm {
    pub url: String,
    pub key: String,
    pub label: String,
    pub focus: usize,
    pub error: Option<String>,
}

impl fmt::Debug for HubForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubForm")
            .field("url", &self.url)
            .field("key", &"<redacted>")
            .field("label", &self.label)
            .field("focus", &self.focus)
            .field("error", &self.error)
            .finish()
    }
}

/// The runtime form of an added or built-in engine: its version, home, route, arguments and variables.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeForm {
    pub id: String,
    pub version: String,
    pub home: String,
    pub route: String,
    /// One argument per line.
    pub args: String,
    /// One `NAME=VALUE` per line: plain values only.
    pub env: String,
    pub focus: usize,
    pub error: Option<String>,
}

impl RuntimeForm {
    fn field_mut(&mut self) -> &mut String {
        match self.focus {
            0 => &mut self.version,
            1 => &mut self.home,
            2 => &mut self.route,
            3 => &mut self.args,
            _ => &mut self.env,
        }
    }

    /// Whether the focused field takes lines (Enter adds one).
    fn multi(&self) -> bool {
        self.focus >= 3
    }
}

/// A yes/no question: `y` runs its calls, `n` drops them.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub question: String,
    pub label: String,
    pub calls: Vec<Call>,
}

/// The model picker popup: the cursor over `picker_rows`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Picker {
    pub cursor: usize,
}

/// The one dialog the tab shows at a time.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Add(Add),
    Key(KeyPrompt),
    Hub(HubForm),
    Runtime(RuntimeForm),
    Confirm(Confirm),
    Models(Picker),
}

/// What a dialog's key leaves: it stays open (with an action to run), or it closes (with one).
enum Step {
    Stay(Action),
    Close(Action),
}

impl Modal {
    fn key(&mut self, code: KeyCode, ctrl: bool, data: &Data) -> Step {
        match self {
            Modal::Add(form) => add_key(form, code, data),
            Modal::Key(prompt) => key_prompt_key(prompt, code),
            Modal::Hub(form) => hub_key(form, code),
            Modal::Runtime(form) => runtime_key(form, code, ctrl),
            Modal::Confirm(confirm) => match code {
                KeyCode::Char('y' | 'Y') => Step::Close(Action::Run {
                    label: confirm.label.clone(),
                    calls: confirm.calls.clone(),
                }),
                KeyCode::Char('n' | 'N') => Step::Close(Action::None),
                _ => Step::Stay(Action::None),
            },
            Modal::Models(picker) => picker_key(picker, code, data),
        }
    }

    /// Shows the hub's refusal in the dialog (the pickers have no field for it: the footer shows it).
    fn set_error(&mut self, error: String) {
        match self {
            Modal::Add(form) => form.error = Some(error),
            Modal::Key(prompt) => prompt.error = Some(error),
            Modal::Hub(form) => form.error = Some(error),
            Modal::Runtime(form) => form.error = Some(error),
            Modal::Confirm(_) | Modal::Models(_) => {}
        }
    }
}

/// The add form's keys: the fields, then Enter plans the add (a hub call, or a reason it cannot be added).
fn add_key(form: &mut Add, code: KeyCode, data: &Data) -> Step {
    match code {
        KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % ADD_FIELDS,
        KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + ADD_FIELDS - 1) % ADD_FIELDS,
        KeyCode::Backspace => {
            form.field_mut().pop();
        }
        KeyCode::Char(c) => form.field_mut().push(c),
        KeyCode::Enter => {
            return match plan_add(form, data) {
                Ok(action) => Step::Stay(action),
                Err(e) => {
                    form.error = Some(e);
                    Step::Stay(Action::None)
                }
            };
        }
        _ => {}
    }
    Step::Stay(Action::None)
}

/// The key prompt: typed characters, masked on screen; Enter sends them to the hub, once.
fn key_prompt_key(prompt: &mut KeyPrompt, code: KeyCode) -> Step {
    match code {
        KeyCode::Char(c) => prompt.text.push(c),
        KeyCode::Backspace => {
            prompt.text.pop();
        }
        KeyCode::Enter => {
            if prompt.text.is_empty() {
                prompt.error = Some("type the key first".into());
                return Step::Stay(Action::None);
            }
            let value = std::mem::take(&mut prompt.text);
            return Step::Stay(Action::Run {
                label: format!("key stored for {}", prompt.id),
                calls: vec![Call {
                    method: "PUT",
                    path: format!("/v1/providers/{}/secret", ctx::seg(&prompt.id)),
                    body: Some(json!({ "value": value })),
                }],
            });
        }
        _ => {}
    }
    Step::Stay(Action::None)
}

/// The usage hub form: Enter checks the URL and the key are there, then adds the hub.
fn hub_key(form: &mut HubForm, code: KeyCode) -> Step {
    match code {
        KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % HUB_FIELDS,
        KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + HUB_FIELDS - 1) % HUB_FIELDS,
        KeyCode::Backspace => {
            hub_field(form).pop();
        }
        KeyCode::Char(c) => hub_field(form).push(c),
        KeyCode::Enter => {
            let url = form.url.trim().to_string();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                form.error = Some("the hub's URL starts with https:// (or http://)".into());
                return Step::Stay(Action::None);
            }
            if form.key.is_empty() {
                form.error = Some("give the hub's management key".into());
                return Step::Stay(Action::None);
            }
            let key = std::mem::take(&mut form.key);
            let label = form.label.trim();
            let body = models::usage_body(&url, &key, (!label.is_empty()).then_some(label));
            return Step::Stay(Action::Run {
                label: "added usage hub".into(),
                calls: vec![Call {
                    method: "POST",
                    path: "/v1/usage-providers".into(),
                    body: Some(body),
                }],
            });
        }
        _ => {}
    }
    Step::Stay(Action::None)
}

fn hub_field(form: &mut HubForm) -> &mut String {
    match form.focus {
        0 => &mut form.url,
        1 => &mut form.key,
        _ => &mut form.label,
    }
}

/// The runtime form: Enter adds a line to a list field (or moves on), Ctrl+S saves the whole runtime.
fn runtime_key(form: &mut RuntimeForm, code: KeyCode, ctrl: bool) -> Step {
    if ctrl && code == KeyCode::Char('s') {
        return Step::Stay(runtime_save(form));
    }
    match code {
        KeyCode::Enter if form.multi() => form.field_mut().push('\n'),
        KeyCode::Enter | KeyCode::Tab | KeyCode::Down => {
            form.focus = (form.focus + 1) % RUNTIME_FIELDS;
        }
        KeyCode::BackTab | KeyCode::Up => {
            form.focus = (form.focus + RUNTIME_FIELDS - 1) % RUNTIME_FIELDS;
        }
        KeyCode::Backspace => {
            form.field_mut().pop();
        }
        KeyCode::Char(c) if !ctrl => form.field_mut().push(c),
        _ => {}
    }
    Step::Stay(Action::None)
}

/// The runtime as the hub takes it: the hub's own refusal (a secret-looking name, a bad version) is shown in the form.
fn runtime_save(form: &mut RuntimeForm) -> Action {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    };
    match models::runtime_body(
        form.version.trim(),
        form.home.trim(),
        form.route.trim(),
        &lines(&form.args),
        &lines(&form.env),
    ) {
        Ok(body) => Action::Run {
            label: format!("saved {}", form.id),
            calls: vec![Call {
                method: "PUT",
                path: format!("/v1/engines/{}", ctx::seg(&form.id)),
                body: Some(body),
            }],
        },
        Err(e) => {
            form.error = Some(e);
            Action::None
        }
    }
}

/// The picker: the cursor moves; `f` or Space toggles the favourite under it and saves the settings; Enter closes.
fn picker_key(picker: &mut Picker, code: KeyCode, data: &Data) -> Step {
    let rows = picker_rows(data);
    match code {
        KeyCode::Up => picker.cursor = step_index(picker.cursor, -1, rows.len()),
        KeyCode::Down => picker.cursor = step_index(picker.cursor, 1, rows.len()),
        KeyCode::Enter => return Step::Close(Action::None),
        KeyCode::Char('f' | ' ') => {
            let Some(row) = rows.get(picker.cursor) else {
                return Step::Stay(Action::None);
            };
            return Step::Stay(favourite_save(data, &row.key, !row.favourite));
        }
        _ => {}
    }
    Step::Stay(Action::None)
}

/// The settings saved with one model's favourite flag changed (the other settings go back as they are).
fn favourite_save(data: &Data, key: &str, on: bool) -> Action {
    let Some(mut settings) = data.get(SETTINGS).and_then(Value::as_object).cloned() else {
        return Action::Note("the settings are not read yet".into());
    };
    models::edit_favourites(&mut settings, key, on);
    Action::Run {
        label: format!("{} {key}", if on { "favourited" } else { "unfavourited" }),
        calls: vec![Call {
            method: "PUT",
            path: SETTINGS.into(),
            body: Some(Value::Object(settings)),
        }],
    }
}

/// The app's own keys on this tab: the tab numbers, `m`, `q`, Tab. Everything else is the tab's.
fn is_app_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Char('0'..='9' | 'm' | 'q') | KeyCode::Tab | KeyCode::BackTab
    )
}

/// A hub call that changes nothing on the plaza but the one it names: a PUT, POST or DELETE.
fn call(method: &'static str, path: String, body: Option<Value>) -> Call {
    Call { method, path, body }
}

/// The Providers tab's state: the selection, the search, the update plan, the open dialog and the usage section.
#[derive(Debug, Default, Clone)]
pub struct Screen {
    /// The selected row of the list (an index into the filtered entries).
    selected: usize,
    /// The selected model of the models list, and whether the keys move it (Enter gives them to the list).
    model: usize,
    models_focus: bool,
    /// The search text, and whether the `/` line is taking keys.
    query: String,
    searching: bool,
    /// The update plan the dry run found, waiting for `y` (or `n`).
    plan: Option<Vec<Change>>,
    /// The open dialog, if any.
    modal: Option<Modal>,
    /// The label of a submit the hub has not answered yet: the dialog waits for it and shows its refusal.
    pending: Option<String>,
    /// Whether the keys move through the usage hubs, and which one is selected.
    usage_focus: bool,
    usage: usize,
}

impl Screen {
    /// The search text the list is filtered by ("" for none).
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The selected row of the list, if there is one.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The update plan waiting for `y`, if there is one.
    pub fn plan(&self) -> Option<&[Change]> {
        self.plan.as_deref()
    }

    /// The open dialog, if there is one.
    pub fn modal(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    fn current<'a>(&self, data: &'a Data) -> Option<Entry<'a>> {
        entries(data, &self.query).get(self.selected).copied()
    }

    /// Handles one key. None when the key is the app's (see `is_app_key`, and Esc with nothing to close).
    pub fn on_key(&mut self, key: KeyEvent, data: &Data) -> Option<Action> {
        let code = key.code;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.modal.is_some() {
            return Some(self.on_modal_key(code, ctrl, data));
        }
        if self.searching {
            return Some(self.on_search_key(code));
        }
        if self.plan.is_some() {
            return Some(self.on_plan_key(code));
        }
        if self.usage_focus {
            return self.on_usage_key(code, data);
        }
        self.on_list_key(code, ctrl, data)
    }

    fn on_list_key(&mut self, code: KeyCode, ctrl: bool, data: &Data) -> Option<Action> {
        if is_app_key(code) {
            return None;
        }
        if ctrl {
            return Some(Action::None);
        }
        let action = match code {
            KeyCode::Up => {
                self.step(-1, data);
                Action::None
            }
            KeyCode::Down => {
                self.step(1, data);
                Action::None
            }
            KeyCode::Enter | KeyCode::Right => {
                if self.current(data).is_some_and(|e| !e.models().is_empty()) {
                    self.models_focus = true;
                }
                Action::None
            }
            KeyCode::Left => {
                self.models_focus = false;
                Action::None
            }
            KeyCode::Esc => {
                if self.models_focus {
                    self.models_focus = false;
                } else if !self.query.is_empty() {
                    self.set_query(String::new());
                } else {
                    return None;
                }
                Action::None
            }
            KeyCode::Char(' ') => self.toggle(data),
            KeyCode::Char('c') => self.check(data),
            KeyCode::Char('C') => check_all(),
            KeyCode::Char('f') => self.favourite(data),
            KeyCode::Char('u') => Action::Plan,
            KeyCode::Char('a') => {
                self.modal = Some(Modal::Add(Add::default()));
                Action::None
            }
            KeyCode::Char('k') => self.key_prompt(data),
            KeyCode::Char('e') => self.runtime_prompt(data),
            KeyCode::Char('d') => self.remove_prompt(data),
            KeyCode::Char('h') => {
                self.modal = Some(Modal::Hub(HubForm::default()));
                Action::None
            }
            KeyCode::Char('p') => {
                self.modal = Some(Modal::Models(Picker::default()));
                Action::None
            }
            KeyCode::Char('U') => {
                if array(data, USAGE).is_empty() {
                    Action::Note("no usage hubs yet: h adds one".into())
                } else {
                    self.usage_focus = true;
                    self.usage = 0;
                    Action::None
                }
            }
            KeyCode::Char('/') => {
                self.searching = true;
                Action::None
            }
            KeyCode::Char('r') => Action::Refresh,
            _ => Action::None,
        };
        Some(action)
    }

    /// The usage section's keys: Up and Down choose a hub, `d` removes it (after a yes), Esc gives the keys back.
    fn on_usage_key(&mut self, code: KeyCode, data: &Data) -> Option<Action> {
        if is_app_key(code) {
            return None;
        }
        let hubs = array(data, USAGE);
        match code {
            KeyCode::Up => self.usage = step_index(self.usage, -1, hubs.len()),
            KeyCode::Down => self.usage = step_index(self.usage, 1, hubs.len()),
            KeyCode::Char('d') => {
                if let Some(hub) = hubs.get(self.usage) {
                    let label = ctx::s(hub, "label").to_string();
                    self.modal = Some(Modal::Confirm(Confirm {
                        question: format!("Remove usage hub {label}?"),
                        label: format!("removed usage hub {label}"),
                        calls: vec![call(
                            "DELETE",
                            format!("{USAGE}/{}", ctx::seg(ctx::s(hub, "id"))),
                            None,
                        )],
                    }));
                }
            }
            KeyCode::Char('h') => self.modal = Some(Modal::Hub(HubForm::default())),
            KeyCode::Char('r') => return Some(Action::Refresh),
            KeyCode::Char('U') | KeyCode::Esc | KeyCode::Left => self.usage_focus = false,
            _ => {}
        }
        Some(Action::None)
    }

    /// Moves the list's selection, or the models list's when the keys are there. The list stops at both ends.
    fn step(&mut self, delta: isize, data: &Data) {
        if self.models_focus {
            let count = self.current(data).map_or(0, |e| e.models().len());
            self.model = step_index(self.model, delta, count);
        } else {
            let count = entries(data, &self.query).len();
            self.selected = step_index(self.selected, delta, count);
            self.model = 0;
        }
    }

    /// A new search text: the list starts again at its top, and the models list is not focused.
    fn set_query(&mut self, query: String) {
        self.query = query;
        self.selected = 0;
        self.model = 0;
        self.models_focus = false;
    }

    fn on_search_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Char(c) => {
                let mut query = self.query.clone();
                query.push(c);
                self.set_query(query);
            }
            KeyCode::Backspace => {
                let mut query = self.query.clone();
                query.pop();
                self.set_query(query);
            }
            KeyCode::Enter => self.searching = false,
            KeyCode::Esc => {
                self.searching = false;
                self.set_query(String::new());
            }
            _ => {}
        }
        Action::None
    }

    fn on_plan_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Char('y' | 'Y') => {
                self.plan = None;
                Action::Run {
                    label: "update engines".into(),
                    calls: vec![call(
                        "POST",
                        "/v1/engines/update".into(),
                        Some(json!({ "dry_run": false })),
                    )],
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.plan = None;
                Action::Note("update cancelled".into())
            }
            _ => Action::None,
        }
    }

    /// The dry run's answer. Returns the line for the footer when there is one to say (an error, or nothing to update).
    pub fn on_plan(&mut self, result: Result<Value, String>) -> Option<String> {
        match result {
            Err(e) => {
                self.plan = None;
                Some(format!("update: {e}"))
            }
            Ok(report) => {
                let changes: Vec<Change> = models::items(&report["changes"])
                    .map(|c| Change {
                        id: ctx::s(c, "id").to_string(),
                        from: ctx::s(c, "from").to_string(),
                        to: ctx::s(c, "to").to_string(),
                    })
                    .collect();
                if changes.is_empty() {
                    self.plan = None;
                    Some("Everything is up to date".into())
                } else {
                    self.plan = Some(changes);
                    None
                }
            }
        }
    }

    /// The hub answered a submit the dialog sent (by its label): a success closes the dialog, a refusal stays in it.
    pub fn on_done(&mut self, label: &str, result: Result<(), String>) {
        if self.pending.as_deref() != Some(label) {
            return;
        }
        self.pending = None;
        let picker = matches!(self.modal, Some(Modal::Models(_)));
        match result {
            Ok(()) => {
                if !picker {
                    self.modal = None;
                }
            }
            Err(e) => {
                if let Some(modal) = self.modal.as_mut() {
                    modal.set_error(e);
                }
            }
        }
    }

    fn on_modal_key(&mut self, code: KeyCode, ctrl: bool, data: &Data) -> Action {
        if code == KeyCode::Esc {
            self.modal = None;
            self.pending = None;
            return Action::None;
        }
        if self.pending.is_some() {
            return Action::None;
        }
        let Some(modal) = self.modal.as_mut() else {
            return Action::None;
        };
        match modal.key(code, ctrl, data) {
            Step::Stay(action) => self.submitted(action),
            Step::Close(action) => {
                self.modal = None;
                action
            }
        }
    }

    /// A submit that runs calls waits for the hub's answer; the dialog keeps its keys until then.
    fn submitted(&mut self, action: Action) -> Action {
        if let Action::Run { label, .. } = &action {
            self.pending = Some(label.clone());
        }
        action
    }

    /// Turns the selected provider or engine on or off: an API provider is saved whole, an engine takes the flag.
    fn toggle(&self, data: &Data) -> Action {
        let Some(e) = self.current(data) else {
            return Action::None;
        };
        let on = !e.enabled();
        let (path, body) = match e.family {
            Family::Api => {
                let mut body = models::api_config(e.value);
                body["enabled"] = json!(on);
                (format!("/v1/providers/{}", ctx::seg(e.id())), body)
            }
            Family::Engine => (
                format!("/v1/engines/{}", ctx::seg(e.id())),
                json!({ "enabled": on }),
            ),
        };
        Action::Run {
            label: format!("{} {}", if on { "enabled" } else { "disabled" }, e.id()),
            calls: vec![call("PUT", path, Some(body))],
        }
    }

    /// Checks the selected provider (an engine's check covers all of them, the hub's way).
    fn check(&self, data: &Data) -> Action {
        let Some(e) = self.current(data) else {
            return Action::None;
        };
        let path = match e.family {
            Family::Api => format!("/v1/providers/{}/check", ctx::seg(e.id())),
            Family::Engine => "/v1/engines/check".to_string(),
        };
        Action::Run {
            label: format!("checked {}", e.id()),
            calls: vec![call("POST", path, None)],
        }
    }

    /// Favourites or unfavourites the model under the models cursor (`provider/model`), keeping the other settings.
    fn favourite(&self, data: &Data) -> Action {
        if !self.models_focus {
            return Action::Note("press Enter on a provider to pick a model first".into());
        }
        let Some(e) = self.current(data) else {
            return Action::None;
        };
        let models = e.models();
        let Some(model) = models.get(self.model) else {
            return Action::None;
        };
        let key = format!("{}/{}", e.id(), model);
        let on = !favourites(data).contains(&key);
        favourite_save(data, &key, on)
    }

    /// The masked key prompt for the selected API instance (an engine has no key).
    fn key_prompt(&mut self, data: &Data) -> Action {
        match self.current(data) {
            Some(e) if e.family == Family::Api => {
                self.modal = Some(Modal::Key(KeyPrompt {
                    id: e.id().to_string(),
                    ..Default::default()
                }));
                Action::None
            }
            Some(_) => Action::Note(
                "engines take no key: their models come through the plaza's gateway".into(),
            ),
            None => Action::None,
        }
    }

    /// The runtime form of the selected engine, filled from what the hub lists.
    fn runtime_prompt(&mut self, data: &Data) -> Action {
        let Some(e) = self.current(data) else {
            return Action::None;
        };
        if e.family != Family::Engine {
            return Action::Note(
                "an API instance's endpoint and headers are changed in the CLI: agentplaza models edit".into(),
            );
        }
        let args: Vec<String> = sub(e.value, "args")
            .iter()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect();
        let env: Vec<String> = e
            .value
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
            .collect();
        self.modal = Some(Modal::Runtime(RuntimeForm {
            id: e.id().to_string(),
            version: e.version(),
            home: ctx::text(e.value, "home"),
            route: ctx::text(e.value, "route"),
            args: args.join("\n"),
            env: env.join("\n"),
            ..Default::default()
        }));
        Action::None
    }

    /// Asks before removing the selected API instance or added engine. The built-in engines are turned off instead.
    fn remove_prompt(&mut self, data: &Data) -> Action {
        let Some(e) = self.current(data) else {
            return Action::None;
        };
        let (question, label, path) = match e.family {
            Family::Api => (
                format!("Remove provider {}?", e.label()),
                format!("removed {}", e.id()),
                format!("/v1/providers/{}", ctx::seg(e.id())),
            ),
            Family::Engine if e.builtin() => {
                return Action::Note(
                    "built-in engines are turned off, not removed: space turns them off".into(),
                );
            }
            Family::Engine => (
                format!("Remove engine {}?", e.label()),
                format!("removed engine {}", e.id()),
                format!("/v1/engines/{}", ctx::seg(e.id())),
            ),
        };
        self.modal = Some(Modal::Confirm(Confirm {
            question,
            label,
            calls: vec![call("DELETE", path, None)],
        }));
        Action::None
    }
}

/// The index one step from `index` in a list of `count`, held inside it (0 for an empty list).
fn step_index(index: usize, delta: isize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    (index as isize + delta).clamp(0, count as isize - 1) as usize
}

fn check_all() -> Action {
    Action::Run {
        label: "checked all".into(),
        calls: vec![
            call("POST", "/v1/providers/check".into(), None),
            call("POST", "/v1/engines/check".into(), None),
        ],
    }
}

/// Whether an id is taken by a provider or an engine.
fn taken(id: &str, data: &Data) -> bool {
    array(data, PATH)
        .iter()
        .chain(array(data, ENGINES))
        .any(|v| ctx::s(v, "id") == id)
}

/// The id an add takes: the one typed (it must be free), else the base, or `base_2`, `base_3` … while taken.
fn pick_id(typed: &str, base: &str, data: &Data) -> Result<String, String> {
    if !typed.is_empty() {
        models::checked_id(typed)?;
        if taken(typed, data) {
            return Err(format!(
                "{typed} is already a provider or an engine: pick another id"
            ));
        }
        return Ok(typed.to_string());
    }
    models::checked_id(base)?;
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            base.to_string()
        } else {
            format!("{base}_{n}")
        };
        if !taken(&candidate, data) {
            return Ok(candidate);
        }
        n += 1;
    }
}

fn create(label: String, path: &str, body: Value) -> Action {
    Action::Run {
        label,
        calls: vec![call("POST", path.to_string(), Some(body))],
    }
}

/// What the add form asks for, as the hub call that adds it. The kind is looked up in the catalogue: an API preset
/// (`/v1/provider-catalog` `api`), a built-in engine, or an ACP registry agent that runs in a VM. `acp-local` is a
/// local command the form names (its program and arguments), run in the agent's VM like the others.
pub fn plan_add(form: &Add, data: &Data) -> Result<Action, String> {
    let kind = form.kind.trim();
    if kind.is_empty() {
        return Err("type a provider kind, an engine, a registry agent or acp-local".into());
    }
    let label = form.label.trim();
    let typed_id = form.id.trim();
    if kind == "acp-local" {
        let command = form.command.trim();
        if command.is_empty() {
            return Err("give the program to run (command)".into());
        }
        let id = pick_id(typed_id, "local", data)?;
        let flags = EngineFlags {
            label: (!label.is_empty()).then(|| label.to_string()),
            command: Some(command.to_string()),
            args: form.args.split_whitespace().map(str::to_string).collect(),
            ..Default::default()
        };
        let body = models::engine_body(&id, None, &flags)?;
        return Ok(create(format!("added engine {id}"), "/v1/engines", body));
    }
    let catalog = data.get(CATALOG).unwrap_or(&Value::Null);
    if let Some(preset) = sub(catalog, "api").iter().find(|p| ctx::s(p, "id") == kind) {
        let id = pick_id(typed_id, kind, data)?;
        let mut body = models::preset_body(&id, preset);
        body["label"] = json!(if label.is_empty() {
            ctx::s(preset, "label")
        } else {
            label
        });
        return Ok(create(format!("added {id}"), "/v1/providers", body));
    }
    if sub(catalog, "engines")
        .iter()
        .any(|e| ctx::s(e, "id") == kind)
    {
        let id = pick_id(typed_id, kind, data)?;
        let mut body = json!({ "id": id, "engine": kind });
        if !label.is_empty() {
            body["label"] = json!(label);
        }
        return Ok(create(format!("added engine {id}"), "/v1/engines", body));
    }
    let registry = data.get(REGISTRY).unwrap_or(&Value::Null);
    if let Some(agent) = sub(registry, "agents")
        .iter()
        .find(|a| ctx::s(a, "id") == kind)
    {
        if agent.get("runs_in_vm").and_then(Value::as_bool) != Some(true) {
            return Err(format!(
                "{kind} has no Linux build, so it cannot run in an agent's VM"
            ));
        }
        let id = pick_id(typed_id, kind, data)?;
        let flags = EngineFlags {
            label: (!label.is_empty()).then(|| label.to_string()),
            ..Default::default()
        };
        let body = models::engine_body(&id, Some(kind), &flags)?;
        return Ok(create(format!("added engine {id}"), "/v1/engines", body));
    }
    Err(format!(
        "no provider, engine or registry agent named {kind}"
    ))
}

/// Runs the calls in order, on the app's thread. The answer is the last call's.
pub fn run_calls(client: &Client, calls: &[Call]) -> Result<Value, String> {
    let mut last = Value::Null;
    for call in calls {
        last = client.call(call.method, &call.path, call.body.as_ref())?;
    }
    Ok(last)
}

/// The engines' update dry run (its answer goes to `Screen::on_plan`).
pub fn dry_run(client: &Client) -> Result<Value, String> {
    client.call(
        "POST",
        "/v1/engines/update",
        Some(&json!({ "dry_run": true })),
    )
}

/// Whether an event means the tab's data changed (the health checks, the engines, the providers).
pub fn is_change(kind: &str) -> bool {
    kind.starts_with("providers.") || kind.starts_with("engines.")
}

// ---- Drawing ----------------------------------------------------------------------------------

pub fn render(frame: &mut Frame, area: Rect, screen: &Screen, data: &Data, palette: Palette) {
    let body = match screen.plan() {
        Some(plan) => {
            let height = (plan.len() as u16 + 3)
                .min(8)
                .min(area.height.saturating_sub(4));
            let [body, panel] =
                Layout::vertical([Constraint::Min(4), Constraint::Length(height)]).areas(area);
            draw_plan(frame, panel, plan, palette);
            body
        }
        None => area,
    };
    let [left, right] =
        Layout::horizontal([Constraint::Length(46), Constraint::Min(30)]).areas(body);
    let [list, usage] = Layout::vertical([
        Constraint::Min(4),
        Constraint::Length(
            (2 + 2 * array(data, USAGE).len() as u16)
                .max(3)
                .min(left.height / 2),
        ),
    ])
    .areas(left);
    draw_list(frame, list, screen, data, palette);
    draw_usage(frame, usage, screen, data, palette);
    match screen.modal() {
        Some(Modal::Models(picker)) => draw_picker(frame, body, picker, data, palette),
        Some(Modal::Add(form)) => draw_add(frame, right, form, data, palette),
        Some(Modal::Key(prompt)) => draw_key(frame, right, prompt, palette),
        Some(Modal::Hub(form)) => draw_hub(frame, right, form, palette),
        Some(Modal::Runtime(form)) => draw_runtime(frame, right, form, palette),
        Some(Modal::Confirm(confirm)) => draw_confirm(frame, right, confirm, palette),
        None => draw_detail(frame, right, screen, data, palette),
    }
}

fn status_color(status: &str, palette: Palette) -> Color {
    match status {
        "ok" => palette.ok,
        "crit" => palette.accent,
        "warn" => Color::Yellow,
        _ => palette.muted,
    }
}

fn draw_list(frame: &mut Frame, area: Rect, screen: &Screen, data: &Data, palette: Palette) {
    let list = entries(data, screen.query());
    let title = if screen.searching || !screen.query().is_empty() {
        format!(
            " / {}{} ",
            screen.query(),
            if screen.searching { "_" } else { "" }
        )
    } else {
        format!(" providers · {} ", list.len())
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if list.is_empty() {
        let text = if screen.query().is_empty() {
            "no providers or engines yet: press a to add one"
        } else {
            "nothing matches the search"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::new().fg(palette.muted),
            ))),
            inner,
        );
        return;
    }
    let visible = (usize::from(inner.height) / 2).max(1);
    let start = if screen.selected() < visible {
        0
    } else {
        screen.selected() + 1 - visible
    };
    let mut lines = Vec::new();
    for (i, e) in list.iter().enumerate().skip(start).take(visible) {
        lines.extend(row_lines(e, i == screen.selected(), inner.width, palette));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One row of the list: the status dot, the label, the version and the toggle; then the health message.
fn row_lines(e: &Entry, selected: bool, width: u16, palette: Palette) -> [Line<'static>; 2] {
    let width = usize::from(width);
    let mark = if selected { "▌ " } else { "  " };
    let name_style = if selected {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let mut head = vec![
        Span::styled(mark, Style::new().fg(palette.accent)),
        Span::styled("● ", Style::new().fg(status_color(e.status(), palette))),
        Span::styled(clip(e.label(), width.saturating_sub(24).max(8)), name_style),
    ];
    if e.family == Family::Engine {
        head.push(Span::styled(
            format!("  {}", clip(&e.version(), 10)),
            Style::new().fg(palette.muted),
        ));
    }
    let (toggle, tone) = if e.enabled() {
        ("  on", palette.ok)
    } else {
        ("  off", palette.muted)
    };
    head.push(Span::styled(toggle, Style::new().fg(tone)));
    let message = if e.message().is_empty() {
        e.status().to_string()
    } else {
        e.message().to_string()
    };
    let second = Line::from(Span::styled(
        format!("    {}", clip(&message, width.saturating_sub(6))),
        Style::new().fg(palette.muted),
    ));
    [Line::from(head), second]
}

fn draw_usage(frame: &mut Frame, area: Rect, screen: &Screen, data: &Data, palette: Palette) {
    let hubs = array(data, USAGE);
    let block = Block::bordered().title(format!(
        " usage hubs · {}{} ",
        hubs.len(),
        if screen.usage_focus {
            " · U back"
        } else {
            " · U select"
        }
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if hubs.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No hubs configured. Press h to add one.",
                Style::new().fg(palette.muted),
            ))),
            inner,
        );
        return;
    }
    let mut lines = Vec::new();
    for (i, hub) in hubs.iter().enumerate() {
        let last = hub.get("last").unwrap_or(&Value::Null);
        let status = ctx::s(last, "status");
        let selected = screen.usage_focus && i == screen.usage;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "▌" } else { " " },
                Style::new().fg(palette.accent),
            ),
            Span::styled("● ", Style::new().fg(status_color(status, palette))),
            Span::raw(ctx::s(hub, "label").to_string()),
            Span::styled(
                format!("  {}", ctx::s(hub, "url")),
                Style::new().fg(palette.muted),
            ),
        ]));
        let totals = last.get("totals").unwrap_or(&Value::Null);
        let summary = if last.is_null() {
            "    not read yet".to_string()
        } else {
            format!(
                "    {}  ·  {} requests · {} tokens",
                ctx::s(last, "message"),
                ctx::text(totals, "requests"),
                ctx::text(totals, "tokens")
            )
        };
        lines.push(Line::from(Span::styled(
            clip(&summary, usize::from(inner.width)),
            Style::new().fg(palette.muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_plan(frame: &mut Frame, area: Rect, plan: &[Change], palette: Palette) {
    let block = Block::bordered().title(format!(" update {} engine(s)? ", plan.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines: Vec<Line> = plan
        .iter()
        .map(|c| {
            Line::from(vec![
                Span::raw(format!("  {}  ", c.id)),
                Span::styled(
                    format!("{} → {}", c.from, c.to),
                    Style::new().fg(palette.muted),
                ),
            ])
        })
        .collect();
    lines.push(Line::from(Span::styled(
        "  y to update · n to cancel",
        Style::new().fg(palette.accent),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn section(title: &str, palette: Palette) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::new().fg(palette.muted).add_modifier(Modifier::BOLD),
    ))
}

fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![Span::raw(format!("  {label:<12}")), Span::raw(value)])
}

/// The credential as the setup line names it: where the key comes from, never the key.
fn credential_text(e: &Entry) -> String {
    match ctx::s(e.value, "credential") {
        "env" => {
            let name = e
                .value
                .get("auth")
                .and_then(|a| a.get("env"))
                .and_then(Value::as_str)
                .unwrap_or("");
            format!("Read from ${name}")
        }
        "secret" => "Key set in the vault".into(),
        "none" => "No key needed".into(),
        "missing" => "No key".into(),
        "command" => "Token from a command".into(),
        "sealed" => "Sealed vault".into(),
        other => other.to_string(),
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, screen: &Screen, data: &Data, palette: Palette) {
    let Some(e) = screen.current(data) else {
        let block = Block::bordered().title(" details ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Nothing selected. Press a to add a provider or an engine.",
                Style::new().fg(palette.muted),
            ))),
            inner,
        );
        return;
    };
    let block = Block::bordered().title(format!(" {} ", e.label()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = detail_lines(&e, screen, data, palette, usize::from(inner.height));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The selected provider or engine: its head and status, its setup, runtime, variables and models, and the keys.
fn detail_lines(
    e: &Entry,
    screen: &Screen,
    data: &Data,
    palette: Palette,
    height: usize,
) -> Vec<Line<'static>> {
    let muted = Style::new().fg(palette.muted);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            e.label().to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", e.kind()), muted),
    ])];
    let message = if e.message().is_empty() {
        e.status().to_string()
    } else {
        e.message().to_string()
    };
    lines.push(Line::from(vec![
        Span::styled("● ", Style::new().fg(status_color(e.status(), palette))),
        Span::raw(message),
    ]));
    lines.push(Line::raw(""));
    lines.push(section("Setup", palette));
    match e.family {
        Family::Api => {
            lines.push(field("key", credential_text(e)));
            lines.push(field("endpoint", none_if(ctx::text(e.value, "base_url"))));
            lines.push(Line::from(Span::styled(
                "  k sets or replaces the key (masked, sent once to the hub)",
                muted,
            )));
        }
        Family::Engine => {
            let route = ctx::text(e.value, "route");
            lines.push(field(
                "runs through",
                if route.is_empty() {
                    "the plaza's default model".into()
                } else {
                    route
                },
            ));
            lines.push(Line::from(Span::styled(format!("  {CONTAINMENT}"), muted)));
            lines.push(Line::raw(""));
            lines.push(section("Runtime", palette));
            lines.push(field("source", ctx::text(e.value, "source")));
            lines.push(field("version", none_if(e.version())));
            lines.push(field("home", none_if(ctx::text(e.value, "home"))));
            let args: Vec<String> = sub(e.value, "args")
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect();
            lines.push(field("arguments", none_if(args.join(" "))));
            lines.push(Line::raw(""));
            lines.push(section("Environment", palette));
            let env = e.value.get("env").and_then(Value::as_object);
            let pairs: Vec<String> = env
                .into_iter()
                .flatten()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                .collect();
            if pairs.is_empty() {
                lines.push(Line::from(Span::styled("  none", muted)));
            }
            for pair in pairs {
                lines.push(Line::raw(format!("  {pair}")));
            }
        }
    }
    if e.family == Family::Api {
        lines.push(Line::raw(""));
        let models = e.models();
        lines.push(section(&format!("Models · {}", models.len()), palette));
        let favs = favourites(data);
        let head = lines.len();
        // The models list scrolls under the cursor when it is taller than the pane.
        let room = height.saturating_sub(head + 4).max(1);
        let cursor = if screen.models_focus { screen.model } else { 0 };
        let skip = (cursor + 1).saturating_sub(room);
        for (i, model) in models.iter().enumerate().skip(skip).take(room) {
            let on_cursor = screen.models_focus && i == cursor;
            let starred = favs.contains(&format!("{}/{}", e.id(), model));
            lines.push(Line::from(vec![
                Span::styled(
                    if on_cursor { "▌" } else { " " },
                    Style::new().fg(palette.accent),
                ),
                Span::styled(
                    if starred { "★ " } else { "☆ " },
                    Style::new().fg(if starred {
                        palette.accent
                    } else {
                        palette.muted
                    }),
                ),
                Span::styled(
                    (*model).to_string(),
                    if on_cursor {
                        Style::new().add_modifier(Modifier::BOLD)
                    } else {
                        Style::new()
                    },
                ),
            ]));
        }
        if models.is_empty() {
            lines.push(Line::from(Span::styled("  no models listed", muted)));
        }
    }
    if e.family == Family::Engine {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  e edits the runtime and variables · d removes it (built-in: space turns it off)",
            muted,
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "space on/off · c check · C all · Enter models · f favourite · a add · d remove",
        muted,
    )));
    lines.push(Line::from(Span::styled(
        "h usage hub · U usage hubs · p models · u update · / search · r refresh",
        muted,
    )));
    lines
}

fn none_if(text: String) -> String {
    if text.is_empty() { "-".into() } else { text }
}

/// A field of a dialog: its label, its value (masked when the value is a secret), the cursor when it has focus, and
/// the hint shown while it is empty.
fn dialog_field(
    label: &str,
    value: &str,
    focused: bool,
    hint: &str,
    masked: bool,
    palette: Palette,
) -> Line<'static> {
    let shown = if masked {
        "•".repeat(value.chars().count())
    } else {
        value.to_string()
    };
    let text = if focused {
        format!("{shown}_")
    } else if value.is_empty() {
        hint.to_string()
    } else {
        shown
    };
    let style = if focused {
        Style::new().fg(palette.accent)
    } else if value.is_empty() {
        Style::new().fg(palette.muted)
    } else {
        Style::new()
    };
    Line::from(vec![
        Span::styled(format!("  {label:<12}"), Style::new().fg(palette.muted)),
        Span::styled(text, style),
    ])
}

/// A list field of a dialog: one line per entry (the cursor on the last line when it has focus).
fn lines_field(
    label: &str,
    value: &str,
    focused: bool,
    hint: &str,
    palette: Palette,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut parts: Vec<&str> = value.split('\n').collect();
    if value.is_empty() {
        parts.clear();
    }
    if parts.is_empty() {
        return vec![dialog_field(label, "", focused, hint, false, palette)];
    }
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        let head = if i == 0 {
            format!("  {label:<12}")
        } else {
            " ".repeat(14)
        };
        let text = if focused && i == last {
            format!("{part}_")
        } else {
            (*part).to_string()
        };
        lines.push(Line::from(vec![
            Span::styled(head, Style::new().fg(palette.muted)),
            Span::raw(text),
        ]));
    }
    lines
}

fn draw_add(frame: &mut Frame, area: Rect, form: &Add, data: &Data, palette: Palette) {
    let block = Block::bordered().title(" add provider ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        dialog_field(
            "kind",
            &form.kind,
            form.focus == 0,
            "openai, claude-code, acp-local",
            false,
            palette,
        ),
        dialog_field(
            "label",
            &form.label,
            form.focus == 1,
            "defaults to the kind's name",
            false,
            palette,
        ),
        dialog_field(
            "id",
            &form.id,
            form.focus == 2,
            "defaults to the kind, or kind_2",
            false,
            palette,
        ),
        dialog_field(
            "command",
            &form.command,
            form.focus == 3,
            "acp-local only: the program",
            false,
            palette,
        ),
        dialog_field(
            "arguments",
            &form.args,
            form.focus == 4,
            "acp-local only: space separated",
            false,
            palette,
        ),
        Line::raw(""),
    ];
    let api: Vec<&str> = sub(data.get(CATALOG).unwrap_or(&Value::Null), "api")
        .iter()
        .take(5)
        .map(|p| ctx::s(p, "id"))
        .collect();
    lines.push(Line::from(Span::styled(
        format!("  providers: {}", api.join(", ")),
        Style::new().fg(palette.muted),
    )));
    let agents: Vec<&str> = sub(data.get(REGISTRY).unwrap_or(&Value::Null), "agents")
        .iter()
        .filter(|a| a.get("runs_in_vm").and_then(Value::as_bool) == Some(true))
        .take(4)
        .map(|a| ctx::s(a, "id"))
        .collect();
    lines.push(Line::from(Span::styled(
        format!("  registry: {}", agents.join(", ")),
        Style::new().fg(palette.muted),
    )));
    lines.push(Line::raw(""));
    push_error(&mut lines, form.error.as_deref(), palette);
    lines.push(Line::from(Span::styled(
        "  Tab next field · Enter adds · Esc cancels",
        Style::new().fg(palette.muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn push_error(lines: &mut Vec<Line<'static>>, error: Option<&str>, palette: Palette) {
    if let Some(error) = error {
        lines.push(Line::from(Span::styled(
            format!("  {error}"),
            Style::new().fg(palette.accent),
        )));
    }
}

fn draw_key(frame: &mut Frame, area: Rect, prompt: &KeyPrompt, palette: Palette) {
    let block = Block::bordered().title(format!(" key for {} ", prompt.id));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        dialog_field("key", &prompt.text, true, "", true, palette),
        Line::raw(""),
        Line::from(Span::styled(
            "  Shown masked. Sent once to the hub, never kept here.",
            Style::new().fg(palette.muted),
        )),
    ];
    push_error(&mut lines, prompt.error.as_deref(), palette);
    lines.push(Line::from(Span::styled(
        "  Enter saves · Esc cancels",
        Style::new().fg(palette.muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_hub(frame: &mut Frame, area: Rect, form: &HubForm, palette: Palette) {
    let block = Block::bordered().title(" add a CLIProxyAPI hub ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "  Show the quota of every account the hub pools. The key stays on the hub.",
            Style::new().fg(palette.muted),
        )),
        Line::raw(""),
        dialog_field(
            "hub URL",
            &form.url,
            form.focus == 0,
            "https://hub.example.ts.net:8318",
            false,
            palette,
        ),
        dialog_field(
            "management key",
            &form.key,
            form.focus == 1,
            "",
            true,
            palette,
        ),
        dialog_field(
            "label",
            &form.label,
            form.focus == 2,
            "defaults to the hub's host name",
            false,
            palette,
        ),
        Line::raw(""),
    ];
    push_error(&mut lines, form.error.as_deref(), palette);
    lines.push(Line::from(Span::styled(
        "  Tab next field · Enter adds the hub · Esc cancels",
        Style::new().fg(palette.muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_runtime(frame: &mut Frame, area: Rect, form: &RuntimeForm, palette: Palette) {
    let block = Block::bordered().title(format!(" runtime · {} ", form.id));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        dialog_field(
            "version",
            &form.version,
            form.focus == 0,
            "pinned, latest, or x.y.z",
            false,
            palette,
        ),
        dialog_field(
            "home",
            &form.home,
            form.focus == 1,
            "the engine's folder",
            false,
            palette,
        ),
        dialog_field(
            "route",
            &form.route,
            form.focus == 2,
            "provider/model, empty for the plaza's default",
            false,
            palette,
        ),
    ];
    lines.extend(lines_field(
        "arguments",
        &form.args,
        form.focus == 3,
        "one per line",
        palette,
    ));
    lines.extend(lines_field(
        "variables",
        &form.env,
        form.focus == 4,
        "NAME=VALUE per line",
        palette,
    ));
    lines.push(Line::from(Span::styled(
        "  Plain values only: the agent can read them, so keys never go here.",
        Style::new().fg(palette.muted),
    )));
    push_error(&mut lines, form.error.as_deref(), palette);
    lines.push(Line::from(Span::styled(
        "  Tab next · Enter adds a line in a list · Ctrl+S saves · Esc cancels",
        Style::new().fg(palette.muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm(frame: &mut Frame, area: Rect, confirm: &Confirm, palette: Palette) {
    let block = Block::bordered().title(" confirm ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = vec![
        Line::raw(format!("  {}", confirm.question)),
        Line::raw(""),
        Line::from(Span::styled(
            "  y yes · n no",
            Style::new().fg(palette.accent),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The model picker popup, drawn over the whole tab: the models `/v1/models` lists, favourites first.
fn draw_picker(frame: &mut Frame, area: Rect, picker: &Picker, data: &Data, palette: Palette) {
    let [_, popup_row, _] = Layout::vertical([
        Constraint::Percentage(10),
        Constraint::Min(8),
        Constraint::Percentage(10),
    ])
    .areas(area);
    let [_, popup, _] = Layout::horizontal([
        Constraint::Percentage(12),
        Constraint::Min(40),
        Constraint::Percentage(12),
    ])
    .areas(popup_row);
    let rows = picker_rows(data);
    frame.render_widget(Clear, popup);
    let block = Block::bordered().title(format!(" models · {} ", rows.len()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no models read yet",
            Style::new().fg(palette.muted),
        )));
    }
    let visible = usize::from(inner.height).saturating_sub(2).max(1);
    let start = if picker.cursor < visible {
        0
    } else {
        picker.cursor + 1 - visible
    };
    for (i, row) in rows.iter().enumerate().skip(start).take(visible) {
        let on_cursor = i == picker.cursor;
        lines.push(Line::from(vec![
            Span::styled(
                if on_cursor { "▌" } else { " " },
                Style::new().fg(palette.accent),
            ),
            Span::styled(
                if row.favourite { "★ " } else { "☆ " },
                Style::new().fg(if row.favourite {
                    palette.accent
                } else {
                    palette.muted
                }),
            ),
            Span::styled(
                clip(
                    &row.key,
                    usize::from(inner.width).saturating_sub(30).max(12),
                ),
                if on_cursor {
                    Style::new().add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                },
            ),
            Span::styled(format!("  {}", row.name), Style::new().fg(palette.muted)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "  Up/Down move · f or Space favourite · Esc closes",
        Style::new().fg(palette.muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn palette() -> Palette {
        Palette {
            accent: Color::Red,
            paper: Color::White,
            ok: Color::Green,
            muted: Color::DarkGray,
        }
    }

    /// The providers (two API instances, one enabled with two models), the engines (a built-in, a disabled one, and an
    /// added one), a usage hub, the settings with a favourite, the catalogue, the registry and the models.
    fn data() -> Data {
        let mut data = Data::new();
        data.insert(
            PATH,
            json!([
                {
                    "id": "anthropic", "family": "api", "kind": "anthropic", "label": "Anthropic",
                    "enabled": false, "credential": "missing", "models": [],
                    "health": { "status": "off", "message": "Disabled" }
                },
                {
                    "id": "openai", "family": "api", "kind": "openai", "label": "OpenAI",
                    "enabled": true, "credential": "env", "base_url": "https://api.openai.com/v1",
                    "auth": { "env": "OPENAI_API_KEY" }, "models": ["gpt-a", "gpt-b"],
                    "health": { "status": "ok", "message": "Ready · 2 models" }
                }
            ]),
        );
        data.insert(
            ENGINES,
            json!([
                {
                    "id": "claude-code", "family": "engine", "engine": "claude-code", "builtin": true,
                    "label": "Claude Code", "enabled": true, "source": "npm", "version": "1.2.3",
                    "pinned": "1.2.3", "route": "anthropic/claude-sonnet-5-5", "args": ["--fast"],
                    "env": { "MODE": "fast" }, "home": "/home/agent",
                    "health": { "status": "ok", "message": "Ready" }
                },
                {
                    "id": "codex", "family": "engine", "engine": "codex", "builtin": true,
                    "label": "Codex", "enabled": false, "source": "npm", "version": null,
                    "health": { "status": "off", "message": "Disabled" }
                },
                {
                    "id": "mine", "family": "engine", "engine": "acp:claude-code", "builtin": false,
                    "label": "Mine", "enabled": true, "source": "npm", "version": "2.0.0",
                    "route": null, "args": [], "env": {}, "home": "",
                    "health": { "status": "ok", "message": "Ready" }
                }
            ]),
        );
        data.insert(
            USAGE,
            json!([{
                "id": "hub1", "kind": "cliproxyapi", "url": "https://hub.example:8318", "label": "Home hub",
                "key": "secret",
                "last": { "status": "ok", "message": "Reading", "totals": { "requests": 12, "tokens": 40 } }
            }]),
        );
        data.insert(
            SETTINGS,
            json!({ "favourite_models": ["openai/gpt-b"], "health_interval_s": 300 }),
        );
        data.insert(
            CATALOG,
            json!({
                "api": [
                    { "id": "openai", "kind": "openai", "label": "OpenAI", "base_url": "https://api.openai.com/v1",
                      "auth": { "env": "OPENAI_API_KEY" }, "models": ["gpt-a"] },
                    { "id": "gemini", "kind": "gemini", "label": "Gemini", "base_url": null,
                      "auth": "none", "models": [] }
                ],
                "engines": [{ "id": "gemini-cli", "label": "Gemini CLI", "source": "npm",
                              "package": "@google/gemini-cli", "pinned": "0.5.0" }],
                "registry": "https://registry.example/agents.json"
            }),
        );
        data.insert(
            REGISTRY,
            json!({
                "fetched_ms": 1,
                "agents": [
                    { "id": "claude-code", "name": "Claude Code", "version": "2.0.0", "description": "A coding agent",
                      "runs_in_vm": true, "distribution": "npx" },
                    { "id": "mac-only", "name": "Mac Only", "version": "1.0.0", "description": "Needs macOS",
                      "runs_in_vm": false, "distribution": "binary" }
                ]
            }),
        );
        data.insert(
            MODELS,
            json!({
                "models": [
                    { "provider": "anthropic", "provider_label": "Anthropic", "id": "claude-x", "name": "Claude X" },
                    { "provider": "openai", "provider_label": "OpenAI", "id": "gpt-a", "name": "GPT A" },
                    { "provider": "openai", "provider_label": "OpenAI", "id": "gpt-b", "name": "GPT B" }
                ],
                "errors": []
            }),
        );
        data
    }

    fn press(screen: &mut Screen, data: &Data, code: KeyCode) -> Option<Action> {
        screen.on_key(KeyEvent::new(code, KeyModifiers::NONE), data)
    }

    fn ctrl(screen: &mut Screen, data: &Data, c: char) -> Option<Action> {
        screen.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL), data)
    }

    fn type_text(screen: &mut Screen, data: &Data, text: &str) {
        for c in text.chars() {
            press(screen, data, KeyCode::Char(c));
        }
    }

    /// Moves the list's selection to the row with `id`.
    fn select(screen: &mut Screen, data: &Data, id: &str) {
        for _ in 0..10 {
            press(screen, data, KeyCode::Up);
        }
        for _ in 0..10 {
            if screen.current(data).is_some_and(|e| e.id() == id) {
                return;
            }
            press(screen, data, KeyCode::Down);
        }
        panic!("no row {id}");
    }

    fn ids(data: &Data, query: &str) -> Vec<String> {
        entries(data, query)
            .iter()
            .map(|e| e.id().to_string())
            .collect()
    }

    fn run_of(action: Option<Action>) -> (String, Vec<Call>) {
        match action {
            Some(Action::Run { label, calls }) => (label, calls),
            other => panic!("expected a run, got {other:?}"),
        }
    }

    #[test]
    fn the_list_puts_enabled_apis_first_then_the_engines() {
        assert_eq!(
            ids(&data(), ""),
            vec!["openai", "anthropic", "claude-code", "mine", "codex"]
        );
    }

    #[test]
    fn selection_moves_and_stops_at_both_ends_and_only_the_arrows_move_it() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Down);
        assert_eq!(screen.selected(), 1);
        press(&mut screen, &data, KeyCode::Char('j'));
        assert_eq!(
            screen.selected(),
            1,
            "j is not a movement key: k and j are the prompts' letters"
        );
        for _ in 0..10 {
            press(&mut screen, &data, KeyCode::Down);
        }
        assert_eq!(screen.selected(), 4, "the last row is the last");
        for _ in 0..10 {
            press(&mut screen, &data, KeyCode::Up);
        }
        assert_eq!(screen.selected(), 0);
    }

    #[test]
    fn space_toggles_an_api_provider_whole_and_an_engine_by_its_flag() {
        let data = data();
        let mut screen = Screen::default();
        let (_, calls) = run_of(press(&mut screen, &data, KeyCode::Char(' ')));
        assert_eq!(calls[0].method, "PUT");
        assert_eq!(calls[0].path, "/v1/providers/openai");
        let body = calls[0].body.clone().expect("a body");
        assert_eq!(body["enabled"], json!(false));
        assert_eq!(body["auth"], json!({ "env": "OPENAI_API_KEY" }));
        select(&mut screen, &data, "claude-code");
        let (_, calls) = run_of(press(&mut screen, &data, KeyCode::Char(' ')));
        assert_eq!(calls[0].path, "/v1/engines/claude-code");
        assert_eq!(calls[0].body, Some(json!({ "enabled": false })));
    }

    #[test]
    fn check_asks_the_selected_one_and_check_all_asks_both_families() {
        let data = data();
        let mut screen = Screen::default();
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Char('c')));
        assert_eq!(label, "checked openai");
        assert_eq!(calls[0].path, "/v1/providers/openai/check");
        let (_, calls) = run_of(press(&mut screen, &data, KeyCode::Char('C')));
        let paths: Vec<&str> = calls.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["/v1/providers/check", "/v1/engines/check"]);
    }

    #[test]
    fn search_filters_the_list_and_esc_clears_it_before_it_quits() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('/'));
        assert!(screen.searching);
        type_text(&mut screen, &data, "clau");
        assert_eq!(
            ids(&data, screen.query()),
            vec!["claude-code", "mine"],
            "mine runs claude-code"
        );
        press(&mut screen, &data, KeyCode::Enter);
        assert!(!screen.searching);
        assert_eq!(screen.query(), "clau", "Enter keeps the filter");
        assert_eq!(press(&mut screen, &data, KeyCode::Esc), Some(Action::None));
        assert_eq!(screen.query(), "");
        assert_eq!(ids(&data, screen.query()).len(), 5);
        assert_eq!(
            press(&mut screen, &data, KeyCode::Esc),
            None,
            "with nothing open, Esc is the app's"
        );
    }

    #[test]
    fn update_shows_the_dry_run_then_y_applies_it_and_n_drops_it() {
        let data = data();
        let mut screen = Screen::default();
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('u')),
            Some(Action::Plan)
        );
        let report = json!({
            "dry_run": true,
            "changes": [{ "id": "claude-code", "from": "1.2.3", "to": "1.3.0" }]
        });
        assert_eq!(screen.on_plan(Ok(report)), None);
        assert_eq!(screen.plan().map(<[Change]>::len), Some(1));
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Char('y')));
        assert_eq!(label, "update engines");
        assert_eq!(calls[0].body, Some(json!({ "dry_run": false })));
        assert!(screen.plan().is_none(), "y takes the plan");

        press(&mut screen, &data, KeyCode::Char('u'));
        screen.on_plan(Ok(
            json!({ "dry_run": true, "changes": [{ "id": "codex", "from": "1", "to": "2" }] }),
        ));
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('n')),
            Some(Action::Note("update cancelled".into()))
        );
        assert!(screen.plan().is_none());
    }

    #[test]
    fn an_update_with_nothing_to_do_says_so_and_a_failed_dry_run_says_why() {
        let mut screen = Screen::default();
        assert_eq!(
            screen.on_plan(Ok(json!({ "dry_run": true, "changes": [] }))),
            Some("Everything is up to date".into())
        );
        assert_eq!(
            screen.on_plan(Err("the registry is down".into())),
            Some("update: the registry is down".into())
        );
    }

    #[test]
    fn favourite_needs_the_models_list_then_toggles_the_star_and_keeps_the_rest() {
        let data = data();
        let mut screen = Screen::default();
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('f')),
            Some(Action::Note(
                "press Enter on a provider to pick a model first".into()
            ))
        );
        press(&mut screen, &data, KeyCode::Enter);
        press(&mut screen, &data, KeyCode::Down);
        let (_, calls) = run_of(press(&mut screen, &data, KeyCode::Char('f')));
        assert_eq!(calls[0].path, "/v1/settings");
        let body = calls[0].body.clone().expect("the settings");
        assert_eq!(
            body["favourite_models"],
            json!([]),
            "gpt-b was a favourite: it leaves"
        );
        assert_eq!(
            body["health_interval_s"],
            json!(300),
            "the other settings go back too"
        );
        press(&mut screen, &data, KeyCode::Up);
        let (_, calls) = run_of(press(&mut screen, &data, KeyCode::Char('f')));
        assert_eq!(
            calls[0]
                .body
                .as_ref()
                .map(|b| b["favourite_models"].clone()),
            Some(json!(["openai/gpt-b", "openai/gpt-a"]))
        );
    }

    #[test]
    fn digits_tabs_and_m_and_q_are_the_apps_and_r_refreshes() {
        let data = data();
        let mut screen = Screen::default();
        for code in [
            KeyCode::Char('3'),
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Char('m'),
            KeyCode::Char('q'),
        ] {
            assert_eq!(press(&mut screen, &data, code), None, "{code:?}");
        }
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('r')),
            Some(Action::Refresh)
        );
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('z')),
            Some(Action::None),
            "other letters are the tab's"
        );
    }

    #[test]
    fn the_add_form_plans_a_preset_an_engine_or_a_registry_agent_and_refuses_the_rest() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('a'));
        assert!(matches!(screen.modal(), Some(Modal::Add(_))));
        type_text(&mut screen, &data, "openai");
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Enter));
        assert_eq!(
            label, "added openai_2",
            "the taken id gets the next free one"
        );
        assert_eq!(calls[0].path, "/v1/providers");
        assert_eq!(
            calls[0].body.as_ref().map(|b| b["kind"].clone()),
            Some(json!("openai"))
        );
        assert_eq!(
            calls[0].body.as_ref().map(|b| b["label"].clone()),
            Some(json!("OpenAI"))
        );
        assert!(
            screen.modal().is_some(),
            "the form waits for the hub's answer"
        );
        screen.on_done(&label, Ok(()));
        assert!(screen.modal().is_none(), "a success closes it");

        let registry = Add {
            kind: "claude-code".into(),
            ..Default::default()
        };
        let Ok(action) = plan_add(&registry, &data) else {
            panic!("a registry agent that runs in a VM adds");
        };
        let (_, calls) = run_of(Some(action));
        assert_eq!(
            calls[0].body.as_ref().map(|b| b["engine"].clone()),
            Some(json!("acp:claude-code"))
        );

        let built_in = Add {
            kind: "gemini-cli".into(),
            id: "gemini-2".into(),
            ..Default::default()
        };
        assert!(matches!(plan_add(&built_in, &data), Ok(Action::Run { .. })));
        let mac = Add {
            kind: "mac-only".into(),
            ..Default::default()
        };
        assert!(
            plan_add(&mac, &data)
                .unwrap_err()
                .contains("no Linux build")
        );
        let unknown = Add {
            kind: "nothing".into(),
            ..Default::default()
        };
        assert!(
            plan_add(&unknown, &data)
                .unwrap_err()
                .contains("no provider, engine or registry agent")
        );
        assert!(plan_add(&Add::default(), &data).is_err());
        let taken_id = Add {
            kind: "gemini".into(),
            id: "openai".into(),
            ..Default::default()
        };
        assert!(plan_add(&taken_id, &data).unwrap_err().contains("already"));
    }

    #[test]
    fn a_local_acp_command_is_added_with_its_program_and_arguments() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('a'));
        type_text(&mut screen, &data, "acp-local");
        assert_eq!(
            press(&mut screen, &data, KeyCode::Enter),
            Some(Action::None),
            "no command: refused"
        );
        assert!(matches!(&screen.modal, Some(Modal::Add(f)) if f.error.is_some()));
        press(&mut screen, &data, KeyCode::Tab);
        press(&mut screen, &data, KeyCode::Tab);
        press(&mut screen, &data, KeyCode::Tab);
        type_text(&mut screen, &data, "/usr/bin/agent");
        press(&mut screen, &data, KeyCode::Tab);
        type_text(&mut screen, &data, "--stdio  --fast");
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Enter));
        assert_eq!(label, "added engine local");
        assert_eq!(calls[0].path, "/v1/engines");
        let body = calls[0].body.clone().expect("a body");
        assert_eq!(body["engine"], json!("acp-local"));
        assert_eq!(body["command"], json!("/usr/bin/agent"));
        assert_eq!(body["args"], json!(["--stdio", "--fast"]));
    }

    #[test]
    fn a_refused_add_keeps_the_form_open_with_the_reason_and_the_hubs_refusal_too() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('a'));
        type_text(&mut screen, &data, "mac-only");
        assert_eq!(
            press(&mut screen, &data, KeyCode::Enter),
            Some(Action::None)
        );
        assert!(screen.modal().is_some(), "the form stays open");
        assert!(
            matches!(&screen.modal, Some(Modal::Add(f)) if f.error.as_deref().is_some_and(|e| e.contains("Linux")))
        );

        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('a'));
        type_text(&mut screen, &data, "openai");
        let (label, _) = run_of(press(&mut screen, &data, KeyCode::Enter));
        screen.on_done(&label, Err("id openai_2 is taken".into()));
        assert!(
            matches!(&screen.modal, Some(Modal::Add(f)) if f.error.as_deref() == Some("id openai_2 is taken"))
        );
        press(&mut screen, &data, KeyCode::Esc);
        assert!(screen.modal().is_none());
    }

    #[test]
    fn k_prompts_for_an_api_key_masked_and_sends_it_once_to_the_secret_route() {
        let data = data();
        let mut screen = Screen::default();
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('k')),
            Some(Action::None)
        );
        type_text(&mut screen, &data, "sk-test-123");
        let shown = format!("{:?}", screen.modal);
        assert!(
            !shown.contains("sk-test-123"),
            "a Debug print never shows the key"
        );
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Enter));
        assert_eq!(label, "key stored for openai");
        assert_eq!(calls[0].method, "PUT");
        assert_eq!(calls[0].path, "/v1/providers/openai/secret");
        assert_eq!(calls[0].body, Some(json!({ "value": "sk-test-123" })));
        assert!(
            matches!(&screen.modal, Some(Modal::Key(p)) if p.text.is_empty()),
            "the typed key is cleared once it is sent"
        );
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('x')),
            Some(Action::None),
            "the sent key waits for the answer"
        );
        screen.on_done(&label, Err("the vault is sealed".into()));
        assert!(
            matches!(&screen.modal, Some(Modal::Key(p)) if p.error.as_deref() == Some("the vault is sealed"))
        );
        screen.on_done(&label, Ok(()));
        assert!(
            screen.modal().is_some(),
            "a stale answer for the same label is the one waited for"
        );
        press(&mut screen, &data, KeyCode::Esc);
        assert!(screen.modal().is_none());
    }

    #[test]
    fn k_refuses_an_empty_key_and_engines_have_no_key() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('k'));
        assert_eq!(
            press(&mut screen, &data, KeyCode::Enter),
            Some(Action::None)
        );
        assert!(matches!(&screen.modal, Some(Modal::Key(p)) if p.error.is_some()));
        press(&mut screen, &data, KeyCode::Esc);
        select(&mut screen, &data, "claude-code");
        assert!(matches!(
            press(&mut screen, &data, KeyCode::Char('k')),
            Some(Action::Note(_))
        ));
        assert!(screen.modal().is_none());
    }

    #[test]
    fn h_adds_a_usage_hub_with_its_key_masked_and_refuses_a_bad_url_or_no_key() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('h'));
        press(&mut screen, &data, KeyCode::Enter);
        assert!(
            matches!(&screen.modal, Some(Modal::Hub(f)) if f.error.is_some()),
            "no URL refused"
        );
        type_text(&mut screen, &data, "hub.example:8318");
        press(&mut screen, &data, KeyCode::Enter);
        assert!(
            matches!(&screen.modal, Some(Modal::Hub(f)) if f.error.as_deref().is_some_and(|e| e.contains("https")))
        );
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('h'));
        type_text(&mut screen, &data, "https://hub.example:8318");
        press(&mut screen, &data, KeyCode::Tab);
        type_text(&mut screen, &data, "mgmt-key");
        assert!(!format!("{:?}", screen.modal).contains("mgmt-key"));
        press(&mut screen, &data, KeyCode::Tab);
        type_text(&mut screen, &data, "Home");
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Enter));
        assert_eq!(label, "added usage hub");
        assert_eq!(calls[0].path, "/v1/usage-providers");
        assert_eq!(
            calls[0].body,
            Some(
                json!({ "kind": "cliproxyapi", "url": "https://hub.example:8318", "key": "mgmt-key", "label": "Home" })
            )
        );
    }

    #[test]
    fn u_selects_a_usage_hub_and_d_removes_it_after_a_yes() {
        let data = data();
        let mut screen = Screen::default();
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('U')),
            Some(Action::None)
        );
        assert!(screen.usage_focus);
        press(&mut screen, &data, KeyCode::Down);
        assert_eq!(screen.usage, 0, "one hub: the cursor stays on it");
        press(&mut screen, &data, KeyCode::Char('d'));
        assert!(
            matches!(&screen.modal, Some(Modal::Confirm(c)) if c.question.contains("Home hub"))
        );
        assert_eq!(
            press(&mut screen, &data, KeyCode::Char('n')),
            Some(Action::None)
        );
        assert!(screen.modal().is_none(), "n drops the question");
        press(&mut screen, &data, KeyCode::Char('d'));
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Char('y')));
        assert_eq!(label, "removed usage hub Home hub");
        assert_eq!(calls[0].method, "DELETE");
        assert_eq!(calls[0].path, "/v1/usage-providers/hub1");
        press(&mut screen, &data, KeyCode::Esc);
        assert!(!screen.usage_focus, "Esc gives the keys back to the list");
    }

    #[test]
    fn d_removes_an_api_instance_or_an_added_engine_after_a_yes_and_turns_a_built_in_off() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('d'));
        assert!(
            matches!(&screen.modal, Some(Modal::Confirm(c)) if c.calls[0].path == "/v1/providers/openai")
        );
        let (_, calls) = run_of(press(&mut screen, &data, KeyCode::Char('y')));
        assert_eq!(calls[0].method, "DELETE");
        assert_eq!(calls[0].path, "/v1/providers/openai");

        select(&mut screen, &data, "mine");
        press(&mut screen, &data, KeyCode::Char('d'));
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Char('y')));
        assert_eq!(label, "removed engine mine");
        assert_eq!(calls[0].path, "/v1/engines/mine");

        select(&mut screen, &data, "claude-code");
        assert!(matches!(
            press(&mut screen, &data, KeyCode::Char('d')),
            Some(Action::Note(_))
        ));
        assert!(
            screen.modal().is_none(),
            "a built-in engine is never asked to be removed"
        );
    }

    /// Opens the runtime form on the built-in Claude Code engine.
    fn open_runtime(screen: &mut Screen, data: &Data) {
        select(screen, data, "claude-code");
        press(screen, data, KeyCode::Char('e'));
        assert!(
            matches!(&screen.modal, Some(Modal::Runtime(f)) if f.version == "1.2.3" && f.args == "--fast" && f.env == "MODE=fast")
        );
    }

    fn runtime(screen: &Screen) -> RuntimeForm {
        match &screen.modal {
            Some(Modal::Runtime(form)) => form.clone(),
            other => panic!("no runtime form: {other:?}"),
        }
    }

    #[test]
    fn e_saves_the_engines_runtime_whole_and_the_hub_answers_show_in_the_form() {
        let data = data();
        let mut screen = Screen::default();
        open_runtime(&mut screen, &data);
        // Version, then the arguments list (Enter adds a line there), then save.
        for _ in 0..3 {
            press(&mut screen, &data, KeyCode::Tab);
        }
        press(&mut screen, &data, KeyCode::Enter);
        type_text(&mut screen, &data, "--verbose");
        let (label, calls) = run_of(ctrl(&mut screen, &data, 's'));
        assert_eq!(label, "saved claude-code");
        assert_eq!(calls[0].method, "PUT");
        assert_eq!(calls[0].path, "/v1/engines/claude-code");
        assert_eq!(
            calls[0].body,
            Some(json!({
                "version": "1.2.3",
                "home": "/home/agent",
                "route": "anthropic/claude-sonnet-5-5",
                "args": ["--fast", "--verbose"],
                "env": { "MODE": "fast" }
            }))
        );
        // The hub refuses a secret-looking name: its words stay in the form until the user fixes it.
        screen.on_done(
            &label,
            Err("MY_API_KEY looks like a secret: engines take plain values".into()),
        );
        assert_eq!(
            runtime(&screen).error.as_deref(),
            Some("MY_API_KEY looks like a secret: engines take plain values")
        );
        // The refused save is over: a second Ctrl+S sends it again, and its success closes the form.
        let (again, _) = run_of(ctrl(&mut screen, &data, 's'));
        screen.on_done(&again, Ok(()));
        assert!(screen.modal().is_none(), "a saved runtime closes the form");
    }

    #[test]
    fn e_refuses_a_version_that_is_not_pinned_latest_or_dotted_and_a_variable_without_a_value_sign()
    {
        let data = data();
        let mut screen = Screen::default();
        open_runtime(&mut screen, &data);
        // Replace the version with a word.
        for _ in 0.."1.2.3".len() {
            press(&mut screen, &data, KeyCode::Backspace);
        }
        type_text(&mut screen, &data, "soon");
        assert_eq!(ctrl(&mut screen, &data, 's'), Some(Action::None));
        assert_eq!(
            runtime(&screen).error.as_deref(),
            Some("soon: a version is pinned, latest, or x.y.z")
        );
        // Fix the version, then break a variable line.
        for _ in 0.."soon".len() {
            press(&mut screen, &data, KeyCode::Backspace);
        }
        type_text(&mut screen, &data, "latest");
        for _ in 0..4 {
            press(&mut screen, &data, KeyCode::Tab);
        }
        press(&mut screen, &data, KeyCode::Enter);
        type_text(&mut screen, &data, "BROKEN");
        assert_eq!(ctrl(&mut screen, &data, 's'), Some(Action::None));
        assert!(
            runtime(&screen)
                .error
                .is_some_and(|e| e.contains("NAME=VALUE"))
        );
    }

    #[test]
    fn a_runtime_clears_home_and_route_when_they_are_left_empty() {
        let data = data();
        let mut screen = Screen::default();
        select(&mut screen, &data, "mine");
        press(&mut screen, &data, KeyCode::Char('e'));
        let form = runtime(&screen);
        assert_eq!(form.version, "2.0.0");
        assert_eq!(form.home, "");
        let (_, calls) = run_of(ctrl(&mut screen, &data, 's'));
        let body = calls[0].body.clone().expect("a body");
        assert_eq!(body["home"], Value::Null);
        assert_eq!(body["route"], Value::Null);
        assert_eq!(body["args"], json!([]));
    }

    #[test]
    fn p_opens_the_model_picker_with_favourites_first_and_f_saves_the_star() {
        let data = data();
        let mut screen = Screen::default();
        press(&mut screen, &data, KeyCode::Char('p'));
        assert!(matches!(screen.modal(), Some(Modal::Models(_))));
        let rows = picker_rows(&data);
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["openai/gpt-b", "anthropic/claude-x", "openai/gpt-a"],
            "the favourite leads"
        );
        assert!(rows[0].favourite && !rows[1].favourite);

        // The cursor's favourite comes off, and the settings save; the picker stays open for more.
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Char('f')));
        assert_eq!(label, "unfavourited openai/gpt-b");
        assert_eq!(
            calls[0]
                .body
                .as_ref()
                .map(|b| b["favourite_models"].clone()),
            Some(json!([]))
        );
        assert!(
            matches!(screen.modal(), Some(Modal::Models(_))),
            "the picker waits, then stays"
        );
        screen.on_done(&label, Ok(()));
        assert!(
            matches!(screen.modal(), Some(Modal::Models(_))),
            "a saved star keeps the picker open"
        );

        // The cursor moves down; Space stars the model there.
        press(&mut screen, &data, KeyCode::Down);
        let (label, calls) = run_of(press(&mut screen, &data, KeyCode::Char(' ')));
        assert_eq!(label, "favourited anthropic/claude-x");
        assert_eq!(
            calls[0]
                .body
                .as_ref()
                .map(|b| b["favourite_models"].clone()),
            Some(json!(["openai/gpt-b", "anthropic/claude-x"]))
        );
        screen.on_done(&label, Ok(()));

        press(&mut screen, &data, KeyCode::Enter);
        assert!(screen.modal().is_none(), "Enter closes the picker");
    }

    #[test]
    fn the_key_is_never_in_a_debug_print_of_the_dialog_or_the_hub_form() {
        let prompt = Modal::Key(KeyPrompt {
            id: "openai".into(),
            text: "sk-secret".into(),
            error: None,
        });
        assert!(!format!("{prompt:?}").contains("sk-secret"));
        let hub = Modal::Hub(HubForm {
            key: "mgmt-secret".into(),
            ..Default::default()
        });
        assert!(!format!("{hub:?}").contains("mgmt-secret"));
    }

    #[test]
    fn the_screen_draws_its_lists_its_dialogs_and_its_popup_and_never_the_key() {
        let data = data();
        let mut screen = Screen::default();
        let draw = |screen: &Screen, data: &Data| -> String {
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("a test terminal");
            terminal
                .draw(|frame| render(frame, frame.area(), screen, data, palette()))
                .expect("the tab draws");
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect()
        };
        let text = draw(&screen, &data);
        assert!(text.contains("OpenAI"), "the list shows the providers");
        assert!(text.contains("Claude Code"), "and the engines");
        assert!(text.contains("Home hub"), "and the usage hubs");
        assert!(text.contains("gpt-b"), "and the models");

        press(&mut screen, &data, KeyCode::Char('u'));
        screen.on_plan(Ok(json!({ "dry_run": true, "changes": [{ "id": "claude-code", "from": "1.2.3", "to": "1.3.0" }] })));
        assert!(
            draw(&screen, &data).contains("update 1 engine"),
            "the plan panel shows"
        );
        press(&mut screen, &data, KeyCode::Char('n'));

        select(&mut screen, &data, "openai");
        press(&mut screen, &data, KeyCode::Char('k'));
        type_text(&mut screen, &data, "sk-visible");
        let masked = draw(&screen, &data);
        assert!(
            masked.contains("••••••••••_"),
            "the key shows as one bullet per character"
        );
        assert!(!masked.contains("sk-visible"), "and never as text");
        press(&mut screen, &data, KeyCode::Esc);

        press(&mut screen, &data, KeyCode::Char('a'));
        assert!(
            draw(&screen, &data).contains("add provider"),
            "the add form shows"
        );
        press(&mut screen, &data, KeyCode::Esc);

        press(&mut screen, &data, KeyCode::Char('p'));
        let popup = draw(&screen, &data);
        assert!(popup.contains("models · 3"), "the picker shows its count");
        assert!(popup.contains("openai/gpt-b"), "and the models");
        press(&mut screen, &data, KeyCode::Esc);

        let empty = Data::new();
        assert!(draw(&Screen::default(), &empty).contains("no providers or engines yet"));
    }
}
