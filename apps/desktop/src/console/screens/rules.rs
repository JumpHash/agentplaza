//! The Rules screen (parity with the web console's `rules` view): the plaza's laws (JHRL) by tier
//! with filters, each built-in statute with its override, the owner's own rules in a checked JHRL
//! editor, the violations by law beside the latest infractions, and the strict permits switch.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_editor, text_input, Space};
use iced::{Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme};
use serde_json::{json, Value};

use super::tools::encode;
use super::Id;
use crate::console::ui::{self, Kind, Tone, INK, INK_2, INK_3, RULE, RULE_FAINT, SHEET, SIGNAL};
use crate::console::{client, events, Ctx};
use crate::theme::{MONO, SERIF};

/// The tier of the constitution: switching one of its laws off needs its id typed in.
const CONSTITUTION: &str = "constitution";

const SEMIBOLD_SERIF: Font = Font { weight: font::Weight::Semibold, ..SERIF };
const BOLD_MONO: Font = Font { weight: font::Weight::Bold, ..MONO };

const DAY_MS: i64 = 86_400_000;
const INFRACTION_FETCH: usize = 200;
const LATEST_SHOWN: usize = 20;
const VIOLATION_LIMIT: usize = 50;
const INTRO: &str = "These are the plaza's laws. Every action an agent takes is checked against them before it happens; a law that forbids an action stops it, and the attempt is recorded as a violation.";
const STRICT_HELP: &str = "Strict permits: when on, an operation that has permit laws is refused unless one of those permits allows it. Off (the default): only forbids refuse, and a permit only allows.";
const JHRL_SHAPE: &str = "law \"ID\" \"Title\" {\n  tier statute | operations | constitution\n  enforcement preventive | detective\n  severity info | minor | major | critical\n  statement \"What the law says.\"\n  permit OP | forbid OP\n  principal is Agent\n  resource is Tool\n  when CONDITION\n}";
const JHRL_EXAMPLE: &str = r#"law "R-100" "No reading files with read_file" {
  tier statute
  enforcement preventive
  severity major
  statement "Agents do not read files with the read_file tool."
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.name == "read_file"
}"#;

/// The tiers in the order the web groups them: key, label, description.
const TIERS: [(&str, &str, &str); 5] = [
    ("constitution", "Constitution", "The plaza's fundamental rules; they override everything else."),
    ("statutes", "Statutes", "Standing rules for roles and everyday work."),
    ("tools", "Tools", "Who may use which tool."),
    ("operations", "Operations", "The kinds of actions the laws talk about."),
    ("other", "Other", ""),
];
const SEVERITIES: [&str; 4] = ["info", "minor", "major", "critical"];

/// The effect filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effect {
    #[default]
    All,
    Permit,
    Forbid,
}

/// An agent named by a law's usage.
#[derive(Debug, Clone)]
pub struct LawAgent {
    cid: u64,
    name: String,
    count: i64,
}

/// The owner's override of a built-in statute: who switched it off, why, and when.
#[derive(Debug, Clone)]
pub struct Override {
    disabled_by: String,
    reason: String,
    at_ms: i64,
}

/// One law, as the hub describes it (`GET /v1/laws`).
#[derive(Debug, Clone)]
pub struct Law {
    id: String,
    title: String,
    tier: String,
    enforcement: String,
    severity: String,
    statement: String,
    rationale: String,
    effect: String,
    ops: Vec<String>,
    principal: String,
    resource: String,
    conditions: Vec<String>,
    explanation: String,
    file: String,
    source: String,
    origin: String,
    rule_id: String,
    enabled: bool,
    overridden: Option<Override>,
    refusals: i64,
    agents: Vec<LawAgent>,
    last_at_ms: Option<i64>,
}

/// An owner rule (`GET /v1/laws/rules`).
#[derive(Debug, Clone)]
pub struct Rule {
    id: String,
    title: String,
    source: String,
    enabled: bool,
    updated_ms: i64,
}

/// A diagnostic from the JHRL checker: a position and the message.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    line: i64,
    column: i64,
    message: String,
}

/// An action the rule would have refused, as the check reports it.
#[derive(Debug, Clone)]
pub struct Refusal {
    cid: u64,
    role: String,
    tool: String,
    at_ms: i64,
    laws: Vec<String>,
}

/// The answer to a check: nothing is applied.
#[derive(Debug, Clone)]
pub struct Checked {
    ok: bool,
    diagnostics: Vec<Diagnostic>,
    laws: Vec<String>,
    would_refuse: Vec<Refusal>,
}

/// A save that did not go through: the hub's message and, for a refused source, its diagnostics.
#[derive(Debug, Clone)]
pub struct SaveFailure {
    message: String,
    diagnostics: Vec<Diagnostic>,
}

/// One violation of a law (`GET /v1/laws/violations`).
#[derive(Debug, Clone)]
pub struct Violation {
    cid: u64,
    at_ms: i64,
    tool: String,
    detail: String,
    agent_name: String,
}

/// One infraction (`GET /v1/infractions`).
#[derive(Debug, Clone)]
pub struct Infraction {
    cid: u64,
    law: String,
    at_ms: i64,
    tool: String,
    detail: String,
    law_title: String,
    agent_name: String,
}

/// What a refresh reads: the laws, the owner's rules, the strict permits switch and the infractions.
#[derive(Debug, Clone)]
pub struct Snapshot {
    laws: Vec<Law>,
    rules: Vec<Rule>,
    strict: bool,
    recent: Vec<Infraction>,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    loaded: bool,
    /// An action is in flight; the buttons that start another wait for it.
    busy: bool,
    laws: Vec<Law>,
    rules: Vec<Rule>,
    strict: bool,
    recent: Vec<Infraction>,
    /// The statute filters: the search text, the effect, the severity and the tier.
    query: String,
    effect: Effect,
    severity: Option<&'static str>,
    tier: Option<&'static str>,
    /// The statutes whose JHRL source is open.
    open_source: HashSet<String>,
    /// The reason typed for each built-in statute, by id.
    reasons: HashMap<String, String>,
    /// The line confirming the last action, and that action's error.
    notice: Option<String>,
    action_error: Option<String>,
    /// The rule open in the editor (None: a new rule), its title and source.
    editing: Option<String>,
    title: String,
    source: text_editor::Content,
    /// The last check's answer, and the last failed save.
    check: Option<Checked>,
    save_failure: Option<SaveFailure>,
    confirm_delete: bool,
    /// The law whose violations are open, and its rows (None while they load).
    selected_law: Option<String>,
    violations: Option<Result<Vec<Violation>, String>>,
    /// The constitutional law whose switch-off dialog is open, and the id typed into it so far.
    switch_off: Option<SwitchOff>,
}

/// A constitutional law being switched off: its id, and the text typed to confirm it.
#[derive(Debug, Clone, Default)]
pub struct SwitchOff {
    id: String,
    typed: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Snapshot, String>),
    /// An action finished: the line confirming it, or its error.
    Done(Result<String, String>),
    /// A rule was deleted: the editor clears.
    Deleted(Result<String, String>),
    Saved(Result<Rule, SaveFailure>),
    Checked(Result<Checked, String>),
    ViolationsLoaded(String, Result<Vec<Violation>, String>),
    Query(String),
    Effect(Effect),
    Severity(Option<&'static str>),
    Tier(Option<&'static str>),
    /// Clears every statute filter.
    Reset,
    /// Opens or closes a statute's JHRL source.
    ToggleSource(String),
    Reason(String, String),
    /// Switches a built-in statute off (with its typed reason) or back on.
    SetOverride(String, bool),
    /// Opens the switch-off dialog of a constitutional law, or closes it.
    AskSwitchOff(String),
    CancelSwitchOff,
    /// The id typed into the open switch-off dialog.
    TypeConfirm(String),
    /// A hub event: a `law` change reloads the screen.
    Event(events::Event),
    SelectLaw(String),
    NewRule,
    SelectRule(String),
    Title(String),
    Source(text_editor::Action),
    Check,
    Save,
    /// Enables or disables the rule open in the editor.
    SetRuleEnabled(bool),
    ArmDelete(bool),
    DeleteRule,
    Strict(bool),
}

/// A severity choice in the picker: `None` is any severity.
#[derive(Debug, Clone, PartialEq)]
struct SevChoice {
    value: Option<&'static str>,
    label: String,
}

impl fmt::Display for SevChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the laws, the owner's rules and the latest infractions (called when it opens and on every
/// poll tick). An open law's violations reload with them.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let snapshot = client::task(
        move || {
            let laws = client.get::<Value>("/v1/laws")?;
            let rules = client.get::<Value>("/v1/laws/rules")?;
            let infractions = client.get::<Value>(&format!("/v1/infractions?limit={INFRACTION_FETCH}"))?;
            Ok(Snapshot {
                laws: parse_laws(&laws),
                rules: parse_rules(&rules),
                strict: rules.get("strict_permits").and_then(Value::as_bool).unwrap_or(false),
                recent: parse_infractions(&infractions),
            })
        },
        Msg::Loaded,
    );
    match state.selected_law.clone() {
        Some(law) => Task::batch([snapshot, fetch_violations(ctx, law)]),
        None => snapshot,
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => {}
        Msg::Loaded(Ok(snapshot)) => {
            state.laws = snapshot.laws;
            state.rules = snapshot.rules;
            state.strict = snapshot.strict;
            state.recent = snapshot.recent;
            state.loaded = true;
            state.error = None;
        }
        Msg::Loaded(Err(e)) => state.error = Some(e),
        Msg::Done(result) => {
            state.busy = false;
            match result {
                Ok(line) => {
                    state.notice = Some(line);
                    state.action_error = None;
                    state.switch_off = None;
                    return load(state, ctx);
                }
                Err(e) => state.action_error = Some(e),
            }
        }
        Msg::Deleted(result) => {
            state.busy = false;
            state.confirm_delete = false;
            match result {
                Ok(id) => {
                    clear_editor(state);
                    state.notice = Some(format!("Deleted {id}."));
                    state.action_error = None;
                    return load(state, ctx);
                }
                Err(e) => state.action_error = Some(e),
            }
        }
        Msg::Saved(result) => {
            state.busy = false;
            match result {
                Ok(rule) => {
                    state.editing = Some(rule.id.clone());
                    state.save_failure = None;
                    state.notice = Some(format!("Saved {}.", rule.id));
                    state.action_error = None;
                    return load(state, ctx);
                }
                Err(failure) => state.save_failure = Some(failure),
            }
        }
        Msg::Checked(result) => {
            state.busy = false;
            match result {
                Ok(checked) => state.check = Some(checked),
                Err(e) => state.action_error = Some(e),
            }
        }
        Msg::ViolationsLoaded(law, result) => {
            if state.selected_law.as_deref() == Some(law.as_str()) {
                state.violations = Some(result);
            }
        }
        Msg::Query(q) => state.query = q,
        Msg::Effect(e) => state.effect = e,
        Msg::Severity(s) => state.severity = s,
        Msg::Tier(t) => state.tier = t,
        Msg::Reset => {
            state.query.clear();
            state.effect = Effect::All;
            state.severity = None;
            state.tier = None;
        }
        Msg::ToggleSource(id) => {
            if !state.open_source.remove(&id) {
                state.open_source.insert(id);
            }
        }
        Msg::Reason(id, text) => {
            state.reasons.insert(id, text);
        }
        Msg::AskSwitchOff(id) => state.switch_off = Some(SwitchOff { id, typed: String::new() }),
        Msg::CancelSwitchOff => state.switch_off = None,
        Msg::TypeConfirm(typed) => {
            if let Some(dialog) = state.switch_off.as_mut() {
                dialog.typed = typed;
            }
        }
        Msg::Event(event) => {
            if event.kind == "law" {
                return load(state, ctx);
            }
        }
        Msg::SetOverride(id, disabled) => {
            if state.busy {
                return Task::none();
            }
            let reason = state.reasons.get(&id).map(|r| r.trim().to_string()).unwrap_or_default();
            if disabled && reason.is_empty() {
                state.action_error = Some(format!("Give a reason before switching off {id}."));
                return Task::none();
            }
            // A constitutional law goes off only with its id typed in the dialog (the hub checks it too).
            let constitutional = state.laws.iter().any(|l| l.id == id && l.tier == CONSTITUTION);
            let typed = state.switch_off.as_ref().filter(|d| d.id == id).map(|d| d.typed.trim().to_string()).unwrap_or_default();
            if disabled && constitutional && typed != id {
                state.action_error = Some(format!("Type {id} to confirm before switching it off."));
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            let line = if disabled { format!("Switched off {id}.") } else { format!("Switched on {id}.") };
            let path = format!("/v1/laws/overrides/{}", encode(&id));
            let body = if constitutional { json!({"disabled": disabled, "reason": reason, "confirm": typed}) } else { json!({"disabled": disabled, "reason": reason}) };
            let client = ctx.client.clone();
            return client::task(move || client.call("PUT", &path, Some(&body)).map(|_| line), Msg::Done);
        }
        Msg::SelectLaw(id) => {
            if state.selected_law.as_deref() == Some(id.as_str()) {
                state.selected_law = None;
                state.violations = None;
            } else {
                state.selected_law = Some(id.clone());
                state.violations = None;
                return fetch_violations(ctx, id);
            }
        }
        Msg::NewRule => clear_editor(state),
        Msg::SelectRule(id) => {
            if let Some(rule) = state.rules.iter().find(|r| r.id == id).cloned() {
                state.editing = Some(rule.id);
                state.title = rule.title;
                state.source = text_editor::Content::with_text(&rule.source);
                state.confirm_delete = false;
                state.check = None;
                state.save_failure = None;
                state.notice = None;
                state.action_error = None;
            }
        }
        Msg::Title(title) => {
            state.title = title;
            clear_verdict(state);
        }
        Msg::Source(action) => {
            let edit = matches!(action, text_editor::Action::Edit(_));
            state.source.perform(action);
            if edit {
                clear_verdict(state);
            }
        }
        Msg::Check => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            let body = check_body(state.editing.clone(), state.source.text());
            let client = ctx.client.clone();
            return client::task(
                move || {
                    let value = client.post::<Value>("/v1/laws/check", &body)?;
                    Ok(parse_checked(&value))
                },
                Msg::Checked,
            );
        }
        Msg::Save => {
            if state.busy {
                return Task::none();
            }
            let title = state.title.trim().to_string();
            if title.is_empty() {
                state.save_failure = Some(SaveFailure { message: "Give the rule a title.".into(), diagnostics: Vec::new() });
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            state.save_failure = None;
            let editing = state.editing.clone();
            let source = state.source.text();
            let client = ctx.client.clone();
            return Task::perform(async move { save_rule(&client, editing.as_deref(), &title, &source) }, Msg::Saved);
        }
        Msg::SetRuleEnabled(enable) => {
            let Some(id) = state.editing.clone() else { return Task::none() };
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            let verb = if enable { "enable" } else { "disable" };
            let path = format!("/v1/laws/rules/{}/{verb}", encode(&id));
            let line = format!("{} {id}.", if enable { "Enabled" } else { "Disabled" });
            let client = ctx.client.clone();
            return client::task(move || client.post::<Value>(&path, &json!({})).map(|_| line), Msg::Done);
        }
        Msg::ArmDelete(armed) => state.confirm_delete = armed,
        Msg::DeleteRule => {
            let Some(id) = state.editing.clone() else { return Task::none() };
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            let path = format!("/v1/laws/rules/{}", encode(&id));
            let client = ctx.client.clone();
            return client::task(move || client.delete(&path).map(|_| id), Msg::Deleted);
        }
        Msg::Strict(on) => {
            if state.busy || on == state.strict {
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            let line = format!("Strict permits {}.", if on { "on" } else { "off" });
            let client = ctx.client.clone();
            return client::task(move || set_strict(&client, on).map(|_| line), Msg::Done);
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let now = now_ms();
    let mut body: Vec<Element<'a, Msg>> = vec![container(ui::prose(INTRO)).max_width(620).into()];
    if !state.loaded {
        body.push(match &state.error {
            Some(e) => ui::error_line(&format!("The rules could not be loaded: {e}")),
            None => ui::loading(),
        });
    } else {
        if let Some(e) = &state.error {
            body.push(ui::error_line(&format!("The rules could not be refreshed: {e}")));
        }
        if let Some(e) = &state.action_error {
            body.push(ui::error_line(e));
        }
        if let Some(line) = &state.notice {
            body.push(ui::faint(line.clone()).into());
        }
        if let Some(banner) = constitution_banner(state) {
            body.insert(0, banner);
        }
        if let Some(dialog) = state.switch_off.as_ref().and_then(|d| switch_off_dialog(state, d)) {
            body.push(dialog);
        }
        body.push(stats(state, now));
        body.push(ui::card("Built-in statutes", statutes(state, now)));
        body.push(ui::card("Owner rules", owner_rules(state, now)));
        body.push(ui::card("Violations by law", violations(state, now)));
        body.push(ui::card("Strict permits", strict_permits(state)));
    }
    ui::sheet(ui::view_head("Rules", vec![]), scrollable(column(body).spacing(22)).height(Length::Fill).into())
}

/// The banner while any constitutional law is switched off, with a link to each one.
fn constitution_banner<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let off: Vec<&Law> = state.laws.iter().filter(|l| l.tier == CONSTITUTION && !l.enabled).collect();
    if off.is_empty() {
        return None;
    }
    let n = off.len();
    let headline = format!("{n} constitutional law{} {} switched off", if n == 1 { "" } else { "s" }, if n == 1 { "is" } else { "are" });
    let links: Vec<Element<'a, Msg>> = off.iter().map(|l| ui::link(&l.id, Msg::SelectLaw(l.id.clone()), true)).collect();
    Some(
        container(column![text(headline).size(14.0).color(crate::theme::palette().warn), row(links).spacing(14).align_y(Alignment::Center)].spacing(8))
            .padding(Padding { top: 12.0, right: 16.0, bottom: 12.0, left: 16.0 })
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(Color { a: 0.08, ..crate::theme::palette().warn }.into()),
                border: Border { color: crate::theme::palette().warn, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            })
            .into(),
    )
}

/// The switch-off dialog of a constitutional law: what it protects, a reason, and its id typed in.
/// The switch stays disabled until both are given.
fn switch_off_dialog<'a>(state: &'a State, dialog: &'a SwitchOff) -> Option<Element<'a, Msg>> {
    let law = state.laws.iter().find(|l| l.id == dialog.id)?;
    let reason = state.reasons.get(&law.id).map(String::as_str).unwrap_or("");
    let ready = !state.busy && dialog.typed.trim() == law.id && !reason.trim().is_empty();
    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!("{} is a constitutional law. It protects:", law.id)).into(),
        ui::prose(law.statement.clone()).into(),
    ];
    if !law.rationale.is_empty() {
        body.push(ui::faint(law.rationale.clone()).into());
    }
    body.push(
        ui::faint("While it is off, agents may do what it forbids. The change is recorded with the time, and this screen flags it until the law is switched back on.").into(),
    );
    let reason_id = law.id.clone();
    body.push(
        column![
            ui::label("Reason (required)"),
            text_input("Why this law is off", reason)
                .on_input(move |t| Msg::Reason(reason_id.clone(), t))
                .padding([7, 10])
                .size(13.0)
                .width(Length::Fill)
                .style(crate::theme::input),
        ]
        .spacing(6)
        .into(),
    );
    body.push(
        column![
            ui::label(&format!("Type {} to confirm", law.id)),
            text_input(&law.id, &dialog.typed)
                .on_input(Msg::TypeConfirm)
                .padding([7, 10])
                .size(13.0)
                .width(Length::Fill)
                .style(crate::theme::input),
        ]
        .spacing(6)
        .into(),
    );
    body.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelSwitchOff)),
            ui::btn("Switch off constitutional law", Some("ban"), Kind::Danger, ready.then(|| Msg::SetOverride(law.id.clone(), true))),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    Some(ui::card(&format!("Switch off \"{}\"?", law.title), column(body).spacing(12).into()))
}

/// The hub's events for this screen: a constitutional law changed on the hub, so it reloads.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

// ---- requests ----

/// One call that keeps the hub's answer whatever its status: a refused save carries diagnostics.
fn send(client: &client::Client, method: &str, path: &str, body: &Value) -> Result<(u16, Value), String> {
    apz_http::client::request_json(&client.target, method, path, Some(body), client.timeout).map_err(|e| e.to_string())
}

fn save_rule(client: &client::Client, editing: Option<&str>, title: &str, source: &str) -> Result<Rule, SaveFailure> {
    let body = json!({"title": title, "source": source});
    let (method, path) = match editing {
        Some(id) => ("PUT", format!("/v1/laws/rules/{}", encode(id))),
        None => ("POST", "/v1/laws/rules".to_string()),
    };
    let (status, value) = send(client, method, &path, &body).map_err(|message| SaveFailure { message, diagnostics: Vec::new() })?;
    if (200..300).contains(&status) {
        return Ok(parse_rule(&value));
    }
    let message = value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("The hub answered {status}."));
    Err(SaveFailure { message, diagnostics: parse_diagnostics(&value) })
}

fn check_body(id: Option<String>, source: String) -> Value {
    let mut body = json!({"source": source});
    if let Some(id) = id {
        body["id"] = json!(id);
    }
    body
}

fn set_strict(client: &client::Client, on: bool) -> Result<(), String> {
    let mut settings = client.get::<Value>("/v1/settings")?;
    let root = settings.as_object_mut().ok_or("The settings did not load.")?;
    // The settings write takes the whole object, minus the key the hub only reports.
    root.remove("dev_models");
    let laws = root.entry("laws").or_insert_with(|| json!({}));
    laws.as_object_mut().ok_or("The laws settings are not an object.")?.insert("strict_permits".into(), json!(on));
    client.call("PUT", "/v1/settings", Some(&settings)).map(|_| ())
}

fn fetch_violations(ctx: &Ctx, law: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/laws/violations?law={}&limit={VIOLATION_LIMIT}", encode(&law));
    let key = law.clone();
    client::task(move || client.get::<Value>(&path).map(|v| parse_violations(&v)), move |r| Msg::ViolationsLoaded(key.clone(), r))
}

fn clear_editor(state: &mut State) {
    state.editing = None;
    state.title.clear();
    state.source = text_editor::Content::new();
    state.confirm_delete = false;
    clear_verdict(state);
}

/// The check and the save's failure describe the text as it was; an edit makes them stale.
fn clear_verdict(state: &mut State) {
    state.check = None;
    state.save_failure = None;
}

// ---- data ----

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn int_of(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn strings_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn parse_laws(v: &Value) -> Vec<Law> {
    v.as_array().map(|a| a.iter().map(parse_law).collect()).unwrap_or_default()
}

fn parse_law(v: &Value) -> Law {
    let agents = v
        .get("agents")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| LawAgent {
                    cid: x.get("cid").and_then(Value::as_u64).unwrap_or(0),
                    name: text_of(x, "name"),
                    count: int_of(x, "count"),
                })
                .collect()
        })
        .unwrap_or_default();
    Law {
        id: text_of(v, "id"),
        title: text_of(v, "title"),
        tier: text_of(v, "tier"),
        enforcement: text_of(v, "enforcement"),
        severity: text_of(v, "severity"),
        statement: text_of(v, "statement"),
        rationale: text_of(v, "rationale"),
        effect: text_of(v, "effect"),
        ops: strings_of(v, "ops"),
        principal: text_of(v, "principal"),
        resource: text_of(v, "resource"),
        conditions: strings_of(v, "conditions"),
        explanation: text_of(v, "explanation"),
        file: text_of(v, "file"),
        source: text_of(v, "source"),
        origin: text_of(v, "origin"),
        rule_id: text_of(v, "rule_id"),
        enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        overridden: v.get("overridden").filter(|o| !o.is_null()).map(|o| Override {
            disabled_by: text_of(o, "disabled_by"),
            reason: text_of(o, "reason"),
            at_ms: int_of(o, "at_ms"),
        }),
        refusals: int_of(v, "refusals"),
        agents,
        last_at_ms: v.get("last_at_ms").and_then(Value::as_i64),
    }
}

fn parse_rules(v: &Value) -> Vec<Rule> {
    v.get("rules").and_then(Value::as_array).map(|a| a.iter().map(parse_rule).collect()).unwrap_or_default()
}

fn parse_rule(v: &Value) -> Rule {
    Rule {
        id: text_of(v, "id"),
        title: text_of(v, "title"),
        source: text_of(v, "source"),
        enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        updated_ms: int_of(v, "updated_ms"),
    }
}

fn parse_diagnostics(v: &Value) -> Vec<Diagnostic> {
    v.get("diagnostics")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Diagnostic { line: int_of(x, "line"), column: int_of(x, "column"), message: text_of(x, "message") })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_checked(v: &Value) -> Checked {
    let would_refuse = v
        .get("would_refuse")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Refusal {
                    cid: x.get("cid").and_then(Value::as_u64).unwrap_or(0),
                    role: text_of(x, "role"),
                    tool: text_of(x, "tool"),
                    at_ms: int_of(x, "at_ms"),
                    laws: strings_of(x, "laws"),
                })
                .collect()
        })
        .unwrap_or_default();
    Checked {
        ok: v.get("ok").and_then(Value::as_bool).unwrap_or(false),
        diagnostics: parse_diagnostics(v),
        laws: strings_of(v, "laws"),
        would_refuse,
    }
}

fn parse_violations(v: &Value) -> Vec<Violation> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| Violation {
                    cid: x.get("cid").and_then(Value::as_u64).unwrap_or(0),
                    at_ms: int_of(x, "at_ms"),
                    tool: text_of(x, "tool"),
                    detail: text_of(x, "detail"),
                    agent_name: text_of(x, "agent_name"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_infractions(v: &Value) -> Vec<Infraction> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| Infraction {
                    cid: x.get("cid").and_then(Value::as_u64).unwrap_or(0),
                    law: text_of(x, "law"),
                    at_ms: int_of(x, "at_ms"),
                    tool: text_of(x, "tool"),
                    detail: text_of(x, "detail"),
                    law_title: text_of(x, "law_title"),
                    agent_name: text_of(x, "agent_name"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tier a law belongs to, read from its tier field, then from the file it lives in.
fn tier_of(l: &Law) -> &'static str {
    for probe in [l.tier.to_lowercase(), l.file.to_lowercase()] {
        if probe.contains("const") {
            return "constitution";
        }
        if probe.contains("statut") {
            return "statutes";
        }
        if probe.contains("tool") {
            return "tools";
        }
        if probe.contains("operat") {
            return "operations";
        }
    }
    "other"
}

fn visible(state: &State, l: &Law) -> bool {
    let forbid = l.effect == "forbid";
    match state.effect {
        Effect::Permit if forbid => return false,
        Effect::Forbid if !forbid => return false,
        _ => {}
    }
    if let Some(s) = state.severity {
        if l.severity != s {
            return false;
        }
    }
    if let Some(t) = state.tier {
        if tier_of(l) != t {
            return false;
        }
    }
    let q = state.query.trim().to_lowercase();
    if !q.is_empty() {
        let hay = format!("{} {} {} {}", l.title, l.statement, l.id, l.ops.join(" ")).to_lowercase();
        if !hay.contains(&q) {
            return false;
        }
    }
    true
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// "Just now", "12m ago", "3h ago"; an em dash for no time.
fn ago(ms: i64, now: i64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let s = ((now - ms) / 1000).max(0);
    if s < 5 {
        "just now".into()
    } else if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn or(s: &str, fallback: &str) -> String {
    if s.is_empty() {
        fallback.to_string()
    } else {
        s.to_string()
    }
}

fn times(n: i64) -> &'static str {
    if n == 1 {
        "time"
    } else {
        "times"
    }
}

fn enforcement_label(e: &str) -> String {
    match e {
        "preventive" => "Stops it before it happens".into(),
        "detective" => "Recorded after the fact".into(),
        "" => "Enforcement not stated".into(),
        other => cap(other),
    }
}

fn severity_tone(s: &str) -> Tone {
    match s {
        "critical" => Tone::Danger,
        "major" => Tone::Warn,
        "minor" => Tone::Info,
        _ => Tone::Muted,
    }
}

// ---- view ----

fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn sheet_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SHEET.into()),
        border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

/// A row that opens something: the selected one (and the one under the pointer) in the hover fill.
fn pick_style(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: (selected || hovered).then_some(crate::theme::palette().hover).map(Into::into),
            text_color: INK,
            border: Border { color: if selected { INK } else { RULE }, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }
    }
}

fn link_style(_: &Theme, _: button::Status) -> button::Style {
    button::Style { text_color: INK, ..Default::default() }
}

/// A mono text link to an agent or a law's source toggle.
fn link<'a>(label: &str, msg: Msg) -> Element<'a, Msg> {
    button(ui::mono(label.to_string(), 12.0, INK))
        .padding([0, 0])
        .style(link_style)
        .on_press(msg)
        .into()
}

/// A segmented control's segment (the web's `.rules-chip`): ink-filled when active.
fn seg<'a>(label: &str, active: bool, msg: Msg) -> Element<'a, Msg> {
    let (color, bg) = if active { (crate::theme::palette().paper, INK) } else { (INK_2, SHEET) };
    button(text(label.to_string()).font(SERIF).size(13.0).color(color))
        .padding([7, 14])
        .style(move |_: &Theme, _| button::Style { background: Some(bg.into()), text_color: color, ..Default::default() })
        .on_press(msg)
        .into()
}

/// A tier tab (the web's `tabs`): mono caps, the active one ink with a signal underline.
fn tab<'a>(label: &str, active: bool, msg: Msg) -> Element<'a, Msg> {
    let color = if active { INK } else { INK_3 };
    let underline = if active { SIGNAL } else { Color::TRANSPARENT };
    let face = button(text(label.to_uppercase()).font(BOLD_MONO).size(12.0).color(color))
        .padding([8, 12])
        .style(move |_: &Theme, _| button::Style { text_color: color, ..Default::default() })
        .on_press(msg);
    column![
        face,
        container(Space::new().height(2.0))
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style { background: Some(underline.into()), ..Default::default() }),
    ]
    .into()
}

fn stat_cell<'a>(label: &'static str, value: String) -> Element<'a, Msg> {
    container(column![
        text(label.to_uppercase()).font(SERIF).size(12.0).color(INK_3),
        text(value).font(SEMIBOLD_SERIF).size(26.0).color(INK),
    ]
    .spacing(4))
    .padding(Padding { top: 14.0, right: 18.0, bottom: 14.0, left: 18.0 })
    .width(Length::FillPortion(1))
    .style(|_: &Theme| container::Style { background: Some(SHEET.into()), ..Default::default() })
    .into()
}

fn stats<'a>(state: &State, now: i64) -> Element<'a, Msg> {
    let ready = state.loaded;
    let figure = |v: String| if ready { v } else { "—".to_string() };
    let forbids = state.laws.iter().filter(|l| l.effect == "forbid").count();
    let cut = now - 30 * DAY_MS;
    let n30 = state.recent.iter().filter(|i| i.at_ms >= cut).count();
    let n30_text = if state.recent.len() >= INFRACTION_FETCH { format!("{n30}+") } else { n30.to_string() };
    // The rule colour shows through the 1px gaps between the cells: the dividers.
    container(
        row![
            stat_cell("Laws", figure(state.laws.len().to_string())),
            stat_cell("Allows", figure((state.laws.len() - forbids).to_string())),
            stat_cell("Forbids", figure(forbids.to_string())),
            stat_cell("Infractions, 30 days", figure(n30_text)),
        ]
        .spacing(1),
    )
    .width(Length::Fill)
    .style(|_: &Theme| container::Style { background: Some(RULE.into()), border: Border { color: RULE, width: 1.0, radius: 0.0.into() }, ..Default::default() })
    .into()
}

fn toolbar<'a>(state: &'a State) -> Element<'a, Msg> {
    let search = text_input("Search by title, statement, id or operation", &state.query)
        .on_input(Msg::Query)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(crate::theme::input);

    let mut sev_options = vec![SevChoice { value: None, label: "Any severity".to_string() }];
    sev_options.extend(SEVERITIES.iter().map(|s| SevChoice { value: Some(*s), label: cap(s) }));
    let selected = sev_options.iter().find(|o| o.value == state.severity).cloned();
    let severity = column![
        ui::label("Severity"),
        pick_list(sev_options, selected, |c: SevChoice| Msg::Severity(c.value)),
    ]
    .spacing(6);
    let search_row = row![search, severity].spacing(14).align_y(Alignment::End);

    let effects: Vec<Element<'a, Msg>> = [(Effect::All, "All"), (Effect::Permit, "Allows"), (Effect::Forbid, "Forbids")]
        .into_iter()
        .map(|(e, label)| seg(label, state.effect == e, Msg::Effect(e)))
        .collect();
    // The ink shows through the 1px gaps between the segments: their dividers.
    let segmented = container(row(effects).spacing(1))
        .style(|_: &Theme| container::Style { background: Some(INK.into()), border: Border { color: INK, width: 1.0, radius: 0.0.into() }, ..Default::default() });

    let mut tabs: Vec<Element<'a, Msg>> = vec![tab("All", state.tier.is_none(), Msg::Tier(None))];
    for &(key, label, _) in TIERS.iter().take(4) {
        tabs.push(tab(label, state.tier == Some(key), Msg::Tier(Some(key))));
    }

    container(
        column![
            ui::faint("The filters apply to the built-in statutes."),
            search_row,
            row![segmented, row(tabs).spacing(4)].spacing(18).align_y(Alignment::Center),
        ]
        .spacing(14),
    )
    .padding(Padding { top: 16.0, right: 18.0, bottom: 16.0, left: 18.0 })
    .width(Length::Fill)
    .style(sheet_box)
    .into()
}

/// The built-in statutes: the filters, then the statutes grouped by tier.
fn statutes<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    let all: Vec<&Law> = state.laws.iter().filter(|l| l.origin == "built-in").collect();
    let shown: Vec<&Law> = all.iter().copied().filter(|l| visible(state, l)).collect();
    let noun = if all.len() == 1 { "statute" } else { "statutes" };
    let mut col: Vec<Element<'a, Msg>> = vec![toolbar(state), ui::faint(format!("Showing {} of {} {}", shown.len(), all.len(), noun)).into()];
    if all.is_empty() {
        col.push(ui::faint("No built-in statutes.").into());
    } else if shown.is_empty() {
        col.push(
            column![
                ui::faint("No statutes match these filters."),
                ui::btn("Clear filters", None, Kind::Secondary, Some(Msg::Reset)),
            ]
            .spacing(10)
            .into(),
        );
    } else {
        for &(key, label, desc) in TIERS.iter() {
            let items: Vec<&Law> = shown.iter().copied().filter(|l| tier_of(l) == key).collect();
            if items.is_empty() {
                continue;
            }
            col.push(group_head(label, desc, items.len()));
            for l in items {
                col.push(statute(state, l, state.open_source.contains(&l.id), now));
            }
        }
    }
    column(col).spacing(14).width(Length::Fill).into()
}

fn group_head<'a>(label: &'static str, desc: &'static str, n: usize) -> Element<'a, Msg> {
    let head = row![
        text(label.to_uppercase()).font(BOLD_MONO).size(20.0).color(INK),
        text(n.to_string()).font(MONO).size(12.0).color(INK_3),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(6);
    if !desc.is_empty() {
        col = col.push(text(desc).font(SERIF).size(14.0).color(INK_2));
    }
    col.push(Space::new().height(8.0))
        .push(container(Space::new().height(1.0)).width(Length::Fill).style(|_: &Theme| container::Style {
            background: Some(INK.into()),
            ..Default::default()
        }))
        .into()
}

fn labelled<'a>(name: &str, value: &str) -> Element<'a, Msg> {
    row![
        text(format!("{name}: ")).font(SEMIBOLD_SERIF).size(15.0).color(INK),
        text(value.to_string()).font(SERIF).size(15.0).color(INK_2).width(Length::Fill),
    ]
    .into()
}

fn op_tag<'a>(op: &str) -> Element<'a, Msg> {
    container(ui::mono(op.to_string(), 11.5, INK_2))
        .padding([2, 6])
        .style(boxed)
        .into()
}

fn applies<'a>(l: &Law) -> Element<'a, Msg> {
    let scope = format!("{} → {}", or(&l.principal, "any principal"), or(&l.resource, "any resource"));
    let mut col: Vec<Element<'a, Msg>> = vec![ui::mono(scope, 12.0, INK).into()];
    if !l.ops.is_empty() {
        let tags: Vec<Element<'a, Msg>> = l.ops.iter().map(|o| op_tag(o)).collect();
        col.push(row(tags).spacing(6).into());
    }
    row![
        text("APPLIES TO").font(SERIF).size(12.0).color(INK_3).width(Length::Fixed(96.0)),
        column(col).spacing(6).width(Length::Fill),
    ]
    .spacing(12)
    .into()
}

fn usage<'a>(l: &Law, now: i64) -> Element<'a, Msg> {
    let n = l.refusals;
    let mut line = format!("Enforced {} {}", n, times(n));
    if let Some(at) = l.last_at_ms {
        line.push_str(&format!(" · last {}", ago(at, now)));
    }
    if !l.agents.is_empty() {
        line.push_str(" · by");
    }
    let mut items: Vec<Element<'a, Msg>> = vec![ui::faint(line).into()];
    for a in &l.agents {
        items.push(link(&format!("{} ({})", a.name, a.count), Msg::Go(Id::Agent(a.cid))));
    }
    row(items).spacing(6).align_y(Alignment::Center).into()
}

fn source_block<'a>(l: &Law, open: bool) -> Element<'a, Msg> {
    let toggle = ui::btn("How it is written (JHRL)", None, Kind::Ghost, Some(Msg::ToggleSource(l.id.clone())));
    if !open {
        return toggle;
    }
    let conds: Element<'a, Msg> = if l.conditions.is_empty() {
        ui::faint("No extra conditions.").into()
    } else {
        let lines: Vec<Element<'a, Msg>> = l.conditions.iter().map(|c| ui::faint(format!("· {c}")).into()).collect();
        column(lines).spacing(4).into()
    };
    column![
        toggle,
        ui::faint(format!("File: {}", or(&l.file, "—"))),
        conds,
        container(ui::mono(or(&l.source, "—"), 12.0, INK)).padding(10).width(Length::Fill).style(sheet_box),
    ]
    .spacing(10)
    .into()
}

/// One built-in statute: its card (as the statutes always showed), then its override control.
fn statute<'a>(state: &'a State, l: &'a Law, open: bool, now: i64) -> Element<'a, Msg> {
    let forbid = l.effect == "forbid";
    let mut tags: Vec<Element<'a, Msg>> = vec![
        ui::badge(if forbid { "Forbids" } else { "Allows" }, if forbid { Tone::Danger } else { Tone::Ok }),
        ui::badge(&format!("Severity: {}", cap(&l.severity)), severity_tone(&l.severity)),
        ui::badge(
            &enforcement_label(&l.enforcement),
            if l.enforcement == "preventive" { Tone::Info } else { Tone::Muted },
        ),
    ];
    if !l.enabled {
        tags.push(ui::badge("Switched off", Tone::Warn));
    }
    let head = row![
        column![
            text(l.title.clone()).font(SEMIBOLD_SERIF).size(18.0).color(INK),
            ui::mono(l.id.clone(), 12.0, INK_3),
        ]
        .spacing(4)
        .width(Length::Fill),
        row(tags).spacing(6).align_y(Alignment::Center),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut body: Vec<Element<'a, Msg>> = vec![head.into(), ui::prose(l.statement.clone()).into(), labelled("In short", &l.explanation)];
    if !l.rationale.is_empty() {
        body.push(labelled("Why", &l.rationale));
    }
    body.push(applies(l));
    if l.refusals > 0 {
        body.push(usage(l, now));
    }
    body.push(override_control(state, l, now));
    body.push(source_block(l, open));

    // The web's 4px left border: the accent shows in the strip left of the card.
    let accent = if forbid { crate::theme::palette().crit } else { crate::theme::palette().ok };
    let card = container(column(body).spacing(12))
        .padding(Padding { top: 20.0, right: 22.0, bottom: 18.0, left: 22.0 })
        .width(Length::Fill)
        .style(sheet_box);
    container(card)
        .padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 4.0 })
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style { background: Some(accent.into()), ..Default::default() })
        .into()
}

/// The switch for a statute: a reason field and "Switch off" while it is on; who switched it off,
/// why, and "Switch on" while it is off.
fn override_control<'a>(state: &'a State, l: &'a Law, now: i64) -> Element<'a, Msg> {
    match &l.overridden {
        Some(o) => row![
            ui::faint(format!("Switched off by {} {}: {}", or(&o.disabled_by, "the owner"), ago(o.at_ms, now), o.reason)).width(Length::Fill),
            ui::btn("Switch on", None, Kind::Secondary, (!state.busy).then(|| Msg::SetOverride(l.id.clone(), false))),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
        // A constitutional law goes off through its dialog: the reason and the typed id are asked there.
        None if l.tier == CONSTITUTION => row![
            ui::faint("Constitutional: switching it off asks for a reason and for its id to be typed.").width(Length::Fill),
            ui::btn("Switch off", None, Kind::Secondary, (!state.busy).then(|| Msg::AskSwitchOff(l.id.clone()))),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
        None => {
            let reason = state.reasons.get(&l.id).map(String::as_str).unwrap_or("");
            let id = l.id.clone();
            let ready = !reason.trim().is_empty() && !state.busy;
            row![
                text_input("Reason for switching this statute off", reason)
                    .on_input(move |t| Msg::Reason(id.clone(), t))
                    .padding([7, 10])
                    .size(13.0)
                    .width(Length::Fill)
                    .style(crate::theme::input),
                ui::btn("Switch off", None, Kind::Secondary, ready.then(|| Msg::SetOverride(l.id.clone(), true))),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        }
    }
}

/// Items with a hairline between them.
fn divided<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut col = column![].spacing(14).width(Length::Fill);
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            col = col.push(ui::hairline::<Msg>());
        }
        col = col.push(item);
    }
    col.into()
}

fn owner_rules<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    let list: Element<'a, Msg> = if state.rules.is_empty() {
        ui::faint("No rules of your own yet. Write one below, check it, then save it.").into()
    } else {
        divided(state.rules.iter().map(|r| rule_row(state, r, now)).collect())
    };
    column![
        row![
            ui::faint("Your own rules are checked as they are written, and apply like the built-in statutes once enabled."),
            Space::new().width(Length::Fill),
            ui::btn("New rule", None, Kind::Secondary, Some(Msg::NewRule)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        list,
        ui::hairline::<Msg>(),
        editor(state, now),
    ]
    .spacing(16)
    .into()
}

fn rule_row<'a>(state: &'a State, r: &'a Rule, now: i64) -> Element<'a, Msg> {
    let declared: Vec<&str> = state.laws.iter().filter(|l| l.rule_id == r.id).map(|l| l.id.as_str()).collect();
    let declares = if declared.is_empty() { "No laws declared".to_string() } else { format!("Declares {}", declared.join(", ")) };
    let state_tag = if r.enabled { ui::badge("Enabled", Tone::Ok) } else { ui::badge("Disabled", Tone::Muted) };
    let body = column![
        row![text(r.title.clone()).font(SEMIBOLD_SERIF).size(16.0).color(INK), state_tag].spacing(12).align_y(Alignment::Center),
        row![
            ui::mono(r.id.clone(), 12.0, INK_3),
            ui::faint(declares),
            ui::faint(format!("Updated {}", ago(r.updated_ms, now))),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    ]
    .spacing(6);
    let selected = state.editing.as_deref() == Some(r.id.as_str());
    button(body)
        .on_press(Msg::SelectRule(r.id.clone()))
        .width(Length::Fill)
        .padding([10, 12])
        .style(pick_style(selected))
        .into()
}

fn editor<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    let current = state.editing.as_deref().and_then(|id| state.rules.iter().find(|r| r.id == id));
    let heading = match &state.editing {
        Some(id) => format!("Editing {id}"),
        None => "New rule".to_string(),
    };
    let title = column![
        ui::label("Title"),
        text_input("What the rule is for", &state.title)
            .on_input(Msg::Title)
            .padding([8, 10])
            .size(13.0)
            .style(crate::theme::input),
    ]
    .spacing(6);
    let source = column![
        ui::label("Source (JHRL)"),
        text_editor(&state.source)
            .placeholder("law \"R-100\" \"Title\" { ... }")
            .font(MONO)
            .size(13.0)
            .height(Length::Fixed(240.0))
            .on_action(Msg::Source),
    ]
    .spacing(6);
    let reference = container(
        column![
            ui::label("JHRL reference"),
            ui::mono(JHRL_SHAPE, 12.0, INK_2),
            ui::label("Example"),
            ui::mono(JHRL_EXAMPLE, 12.0, INK_2),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fill)
    .style(sheet_box);

    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::btn("Check", None, Kind::Secondary, (!state.busy).then_some(Msg::Check)),
        ui::btn("Save", None, Kind::Primary, (!state.busy).then_some(Msg::Save)),
    ];
    if let Some(rule) = current {
        let (label, enable) = if rule.enabled { ("Disable rule", false) } else { ("Enable rule", true) };
        actions.push(ui::btn(label, None, Kind::Secondary, (!state.busy).then_some(Msg::SetRuleEnabled(enable))));
    }
    if state.editing.is_some() {
        if state.confirm_delete {
            actions.push(ui::btn("Cancel", None, Kind::Ghost, Some(Msg::ArmDelete(false))));
            actions.push(ui::btn("Confirm delete", None, Kind::Danger, (!state.busy).then_some(Msg::DeleteRule)));
        } else {
            actions.push(ui::btn("Delete", None, Kind::Ghost, Some(Msg::ArmDelete(true))));
        }
    }

    let mut items: Vec<Element<'a, Msg>> = vec![
        text(heading).font(SEMIBOLD_SERIF).size(15.0).color(INK).into(),
        title.into(),
        source.into(),
        reference.into(),
        row(actions).spacing(8).align_y(Alignment::Center).into(),
    ];
    if let Some(failure) = &state.save_failure {
        items.push(ui::error_line(&failure.message));
        items.extend(diagnostics(&failure.diagnostics));
    }
    if let Some(checked) = &state.check {
        items.push(check_result(checked, now));
    }
    column(items).spacing(14).into()
}

fn diagnostics<'a>(list: &[Diagnostic]) -> Vec<Element<'a, Msg>> {
    list.iter()
        .map(|d| ui::mono(format!("Line {}, column {}: {}", d.line, d.column, d.message), 12.0, crate::theme::palette().crit).into())
        .collect()
}

fn check_result<'a>(c: &Checked, now: i64) -> Element<'a, Msg> {
    if !c.ok {
        let mut items: Vec<Element<'a, Msg>> = vec![ui::badge("Not valid yet", Tone::Danger)];
        items.extend(diagnostics(&c.diagnostics));
        return column(items).spacing(6).into();
    }
    let declared = if c.laws.is_empty() { "No laws declared.".to_string() } else { format!("Declares {}.", c.laws.join(", ")) };
    let mut items: Vec<Element<'a, Msg>> = vec![ui::badge("Valid", Tone::Ok), ui::faint(declared).into()];
    if c.would_refuse.is_empty() {
        items.push(ui::faint("None of the recent actions would have been refused.").into());
    } else {
        items.push(ui::label("Would have refused, recently").into());
        items.extend(c.would_refuse.iter().map(|w| refusal_row(w, now)));
    }
    column(items).spacing(6).into()
}

fn refusal_row<'a>(w: &Refusal, now: i64) -> Element<'a, Msg> {
    let who = if w.role.is_empty() { format!("Agent {}", w.cid) } else { format!("Agent {} as {}", w.cid, w.role) };
    column![
        ui::mono(who, 12.0, INK_2),
        ui::faint(format!("{} · {} · {}", or(&w.tool, "a tool"), ago(w.at_ms, now), w.laws.join(", "))),
    ]
    .spacing(2)
    .into()
}

/// The violations by law (each law's refusals, and its rows when open), beside the latest infractions.
fn violations<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    let list: Element<'a, Msg> = if state.laws.is_empty() {
        ui::faint("No laws yet.").into()
    } else {
        column![
            ui::faint("Select a law to see its recent violations."),
            divided(state.laws.iter().map(|l| law_row(state, l, now)).collect()),
        ]
        .spacing(14)
        .into()
    };
    row![
        container(list).width(Length::Fill),
        container(infractions(state, now)).width(Length::Fixed(340.0)),
    ]
    .spacing(28)
    .into()
}

fn law_row<'a>(state: &'a State, l: &'a Law, now: i64) -> Element<'a, Msg> {
    let selected = state.selected_law.as_deref() == Some(l.id.as_str());
    let head = button(
        column![
            text(l.title.clone()).font(SEMIBOLD_SERIF).size(15.0).color(INK),
            row![
                ui::mono(l.id.clone(), 12.0, INK_3),
                ui::faint(format!("Refused {} {}", l.refusals, times(l.refusals))),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        ]
        .spacing(6),
    )
    .on_press(Msg::SelectLaw(l.id.clone()))
    .width(Length::Fill)
    .padding([10, 12])
    .style(pick_style(selected));
    if selected {
        column![head, container(violation_list(state, now)).padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 16.0 })]
            .spacing(8)
            .into()
    } else {
        head.into()
    }
}

fn violation_list<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    match &state.violations {
        None => ui::loading(),
        Some(Err(e)) => ui::error_line(&format!("The violations could not be loaded: {e}")),
        Some(Ok(rows)) if rows.is_empty() => ui::faint("No violations recorded for this law.").into(),
        Some(Ok(rows)) => divided(rows.iter().map(|v| violation_row(v, now)).collect()),
    }
}

fn violation_row<'a>(v: &Violation, now: i64) -> Element<'a, Msg> {
    let agent = if v.agent_name.is_empty() { format!("agent {}", v.cid) } else { v.agent_name.clone() };
    let mut meta: Vec<Element<'a, Msg>> = Vec::new();
    if !v.tool.is_empty() {
        meta.push(ui::mono(v.tool.clone(), 12.0, INK_3).into());
    }
    if !v.detail.is_empty() {
        meta.push(ui::faint(v.detail.clone()).into());
    }
    meta.push(ui::faint(ago(v.at_ms, now)).into());
    column![
        button(text(agent).font(SERIF).size(14.0).color(INK))
            .padding([0, 0])
            .style(link_style)
            .on_press(Msg::Go(Id::Agent(v.cid))),
        row(meta).spacing(10).align_y(Alignment::Center),
    ]
    .spacing(4)
    .into()
}

fn infractions<'a>(state: &State, now: i64) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if !state.loaded {
        ui::loading()
    } else {
        let mut latest: Vec<&Infraction> = state.recent.iter().collect();
        latest.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
        latest.truncate(LATEST_SHOWN);
        if latest.is_empty() {
            ui::faint("No infractions yet.").into()
        } else {
            let mut rows = column![].width(Length::Fill);
            for (k, i) in latest.into_iter().enumerate() {
                if k > 0 {
                    rows = rows.push(container(Space::new().height(1.0)).width(Length::Fill).style(|_: &Theme| container::Style {
                        background: Some(RULE_FAINT.into()),
                        ..Default::default()
                    }));
                }
                rows = rows.push(infraction_row(i, now));
            }
            rows.into()
        }
    };
    container(column![
        text("Latest infractions").font(BOLD_MONO).size(16.0).color(INK),
        Space::new().height(12.0),
        body,
    ])
    .padding(Padding { top: 16.0, right: 18.0, bottom: 16.0, left: 18.0 })
    .width(Length::Fill)
    .style(sheet_box)
    .into()
}

fn infraction_row<'a>(i: &Infraction, now: i64) -> Element<'a, Msg> {
    let agent = if i.agent_name.is_empty() { format!("agent {}", i.cid) } else { i.agent_name.clone() };
    let law = if i.law_title.is_empty() { i.law.clone() } else { i.law_title.clone() };
    let mut meta: Vec<Element<'a, Msg>> = Vec::new();
    if !i.tool.is_empty() {
        meta.push(ui::mono(i.tool.clone(), 12.0, INK_3).into());
    }
    if !i.detail.is_empty() {
        meta.push(text(i.detail.clone()).font(SERIF).size(12.0).color(INK_3).into());
    }
    meta.push(text(ago(i.at_ms, now)).font(SERIF).size(12.0).color(INK_3).into());
    let line = row![
        button(text(agent).font(SERIF).size(14.0).color(INK))
            .padding(0.0)
            .style(link_style)
            .on_press(Msg::Go(Id::Agent(i.cid))),
        text("violated").font(SERIF).size(14.0).color(INK_2),
        text(law).font(SERIF).size(14.0).color(INK),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    column![line, row(meta).spacing(10).align_y(Alignment::Center)]
        .spacing(4)
        .padding(Padding { top: 12.0, right: 0.0, bottom: 12.0, left: 0.0 })
        .into()
}

fn strict_permits<'a>(state: &'a State) -> Element<'a, Msg> {
    column![
        ui::seg(&[(true, "On"), (false, "Off")], state.strict, Msg::Strict),
        ui::prose(STRICT_HELP),
    ]
    .spacing(12)
    .into()
}


