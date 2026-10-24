//! The Rules screen (parity with the web console's `rules` view): the plaza's laws (JHRL) by tier
//! with filters, each built-in statute with its override, the owner's own rules in a checked JHRL
//! editor, each statute's recent refusals, and the strict permits switch.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::{
    Space, button, column, container, pick_list, row, text, text_editor, text_input, toggler,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme};
use serde_json::{Value, json};

use super::Id;
use super::tools::encode;
use crate::console::ui::{self, Kind, Tone};
use crate::console::{Ctx, client, events, roles};
use crate::theme::SANS;

/// The tier of the constitution: switching one of its laws off needs its id typed in.
const CONSTITUTION: &str = "constitution";
/// The gap between the screen's cards and rows (the web's `--gap`).
const GAP: f32 = 16.0;

const SANS_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..SANS
};

const INTRO: &str = "The laws the plaza ships with, and the owner’s own JHRL rules.";
const OWNER_DESC: &str = "Laws you write in JHRL. A rule takes effect once its source compiles; its laws stop applying when the rule is disabled or deleted.";
const BUILTIN_DESC: &str = "The laws the plaza ships with. Disabling one asks for a reason, and the reason stays with the override until the statute is enabled again.";
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
    (
        "constitution",
        "Constitution",
        "The plaza's fundamental rules; they override everything else.",
    ),
    (
        "statutes",
        "Statutes",
        "Standing rules for roles and everyday work.",
    ),
    ("tools", "Tools", "Who may use which tool."),
    (
        "operations",
        "Operations",
        "The kinds of actions the laws talk about.",
    ),
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

/// An agent in a law's usage, by its label.
#[derive(Debug, Clone)]
pub struct LawAgent {
    cid: u64,
    label: String,
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
    author: String,
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

/// What a refresh reads: the laws, the owner's rules and the strict permits switch.
#[derive(Debug, Clone)]
pub struct Snapshot {
    laws: Vec<Law>,
    rules: Vec<Rule>,
    strict: bool,
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
    /// The owner rule whose delete dialog is open.
    confirm_delete: Option<Named>,
    /// The statute whose disable dialog is open.
    disable: Option<Named>,
    /// Whether the rule editor is open (New rule, or Edit on a rule).
    editor_open: bool,
    /// The constitutional law whose switch-off dialog is open, and the id typed into it so far.
    switch_off: Option<SwitchOff>,
}

/// A constitutional law being switched off: its id, and the text typed to confirm it.
#[derive(Debug, Clone, Default)]
pub struct SwitchOff {
    id: String,
    typed: String,
    heading: String,
}

/// A dialog about one law or rule: its id, and the heading the dialog shows.
#[derive(Debug, Clone, Default)]
struct Named {
    id: String,
    heading: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Snapshot, String>),
    /// "Try again" after a failed load: fetches the screen's data once more.
    Retry,
    /// An action finished: the line confirming it, or its error.
    Done(Result<String, String>),
    /// A rule was deleted: the editor clears.
    Deleted(Result<String, String>),
    Saved(Result<Rule, SaveFailure>),
    Checked(Result<Checked, String>),
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
    /// Opens the disable dialog of a built-in statute, or closes it.
    AskDisable(String),
    CancelDisable,
    /// The id typed into the open switch-off dialog.
    TypeConfirm(String),
    /// A hub event: a law or agent change reloads the screen.
    Event(events::Event),
    NewRule,
    SelectRule(String),
    /// Closes the rule editor.
    CloseEditor,
    Title(String),
    Source(text_editor::Action),
    Check,
    Save,
    /// Enables or disables an owner rule.
    SetRuleEnabled(String, bool),
    /// Opens the delete dialog of an owner rule, or closes it.
    AskDelete(String),
    CancelDelete,
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

/// Loads the laws and the owner's rules (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let laws = client.get::<Value>("/v1/laws")?;
            let rules = client.get::<Value>("/v1/laws/rules")?;
            Ok(Snapshot {
                laws: parse_laws(&laws),
                rules: parse_rules(&rules),
                strict: rules
                    .get("strict_permits")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => {}
        Msg::Loaded(Ok(snapshot)) => {
            state.laws = snapshot.laws;
            state.rules = snapshot.rules;
            state.strict = snapshot.strict;
            state.loaded = true;
            state.error = None;
        }
        Msg::Loaded(Err(e)) => state.error = Some(e),
        Msg::Retry => return load(state, ctx),
        Msg::Done(result) => {
            state.busy = false;
            match result {
                Ok(line) => {
                    state.notice = Some(line);
                    state.action_error = None;
                    state.switch_off = None;
                    state.disable = None;
                    return load(state, ctx);
                }
                Err(e) => state.action_error = Some(e),
            }
        }
        Msg::Deleted(result) => {
            state.busy = false;
            state.confirm_delete = None;
            match result {
                Ok(id) => {
                    if state.editing.as_deref() == Some(id.as_str()) {
                        clear_editor(state);
                        state.editor_open = false;
                    }
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
        Msg::AskSwitchOff(id) => {
            let title = state
                .laws
                .iter()
                .find(|l| l.id == id)
                .map(|l| l.title.clone())
                .unwrap_or_default();
            state.switch_off = Some(SwitchOff {
                heading: format!("Switch off \"{title}\"?"),
                id,
                typed: String::new(),
            })
        }
        Msg::AskDisable(id) => {
            let title = state
                .laws
                .iter()
                .find(|l| l.id == id)
                .map(|l| l.title.clone())
                .unwrap_or_default();
            state.disable = Some(Named {
                heading: format!("Disable {title}"),
                id,
            });
        }
        Msg::CancelDisable => state.disable = None,
        Msg::CancelSwitchOff => state.switch_off = None,
        Msg::TypeConfirm(typed) => {
            if let Some(dialog) = state.switch_off.as_mut() {
                dialog.typed = typed;
            }
        }
        Msg::Event(event) => {
            if event.kind == "law" || event.kind == "agent" {
                return load(state, ctx);
            }
        }
        Msg::SetOverride(id, disabled) => {
            if state.busy {
                return Task::none();
            }
            let reason = state
                .reasons
                .get(&id)
                .map(|r| r.trim().to_string())
                .unwrap_or_default();
            if disabled && reason.is_empty() {
                state.action_error = Some(format!("Give a reason before switching off {id}."));
                return Task::none();
            }
            // A constitutional law goes off only with its id typed in the dialog (the hub checks it too).
            let constitutional = state
                .laws
                .iter()
                .any(|l| l.id == id && l.tier == CONSTITUTION);
            let typed = state
                .switch_off
                .as_ref()
                .filter(|d| d.id == id)
                .map(|d| d.typed.trim().to_string())
                .unwrap_or_default();
            if disabled && constitutional && typed != id {
                state.action_error = Some(format!("Type {id} to confirm before switching it off."));
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            let line = if disabled {
                format!("Switched off {id}.")
            } else {
                format!("Switched on {id}.")
            };
            let path = format!("/v1/laws/overrides/{}", encode(&id));
            let body = if constitutional {
                json!({"disabled": disabled, "reason": reason, "confirm": typed})
            } else {
                json!({"disabled": disabled, "reason": reason})
            };
            let client = ctx.client.clone();
            return client::task(
                move || client.call("PUT", &path, Some(&body)).map(|_| line),
                Msg::Done,
            );
        }
        Msg::NewRule => {
            clear_editor(state);
            state.editor_open = true;
        }
        Msg::CloseEditor => {
            clear_editor(state);
            state.editor_open = false;
        }
        Msg::SelectRule(id) => {
            if let Some(rule) = state.rules.iter().find(|r| r.id == id).cloned() {
                state.editing = Some(rule.id);
                state.title = rule.title;
                state.source = text_editor::Content::with_text(&rule.source);
                state.editor_open = true;
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
                state.save_failure = Some(SaveFailure {
                    message: "Give the rule a title.".into(),
                    diagnostics: Vec::new(),
                });
                return Task::none();
            }
            state.busy = true;
            state.notice = None;
            state.action_error = None;
            state.save_failure = None;
            let editing = state.editing.clone();
            let source = state.source.text();
            let client = ctx.client.clone();
            return Task::perform(
                async move { save_rule(&client, editing.as_deref(), &title, &source) },
                Msg::Saved,
            );
        }
        Msg::SetRuleEnabled(id, enable) => {
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
            return client::task(
                move || client.post::<Value>(&path, &json!({})).map(|_| line),
                Msg::Done,
            );
        }
        Msg::AskDelete(id) => {
            let title = state
                .rules
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.title.clone())
                .unwrap_or_else(|| id.clone());
            state.confirm_delete = Some(Named {
                heading: format!("Delete {title}"),
                id,
            });
        }
        Msg::CancelDelete => state.confirm_delete = None,
        Msg::DeleteRule => {
            let Some(id) = state.confirm_delete.as_ref().map(|d| d.id.clone()) else {
                return Task::none();
            };
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

/// An error line beside a "Try again" button that reloads the screen, as the web's failure rows do.
fn with_retry<'a>(line: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        line,
        ui::btn(
            "Try again",
            Some("refresh"),
            Kind::Secondary,
            Some(Msg::Retry)
        )
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let now = now_ms();
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if !state.loaded {
        body.push(match &state.error {
            Some(e) => with_retry(ui::error_line(&format!(
                "The rules could not be loaded: {e}"
            ))),
            None => ui::loading(),
        });
    } else {
        if let Some(banner) = constitution_banner(state) {
            body.push(banner);
        }
        if let Some(e) = &state.error {
            body.push(with_retry(ui::error_line(&format!(
                "The rules could not be refreshed: {e}"
            ))));
        }
        if let Some(e) = &state.action_error {
            body.push(ui::error_line(e));
        }
        if let Some(line) = &state.notice {
            body.push(ui::faint(line.clone()).into());
        }
        body.push(stats(state));
        let main = column![
            panel(
                panel_head("Strict permits", Some(strict_switch(state))),
                ui::faint(STRICT_HELP).into()
            ),
            panel(
                panel_head("Built-in statutes", None),
                column![ui::faint(BUILTIN_DESC), statutes(state, now)]
                    .spacing(12)
                    .into()
            ),
            panel(
                panel_head(
                    "Owner rules",
                    Some(ui::btn(
                        "New rule",
                        None,
                        Kind::Secondary,
                        Some(Msg::NewRule)
                    ))
                ),
                owner_rules(state, now)
            ),
        ]
        .spacing(GAP)
        .width(Length::Fill);
        body.push(main.into());
    }
    // The web's dialogs lay over the screen: the switch-off, the disable and the delete.
    let dialog = state
        .switch_off
        .as_ref()
        .and_then(|d| switch_off_dialog(state, d))
        .or_else(|| {
            state
                .disable
                .as_ref()
                .and_then(|d| disable_dialog(state, d))
        })
        .or_else(|| {
            state
                .confirm_delete
                .as_ref()
                .and_then(|d| delete_dialog(state, d))
        });
    ui::with_modal(
        ui::sheet(
            ui::view_head_with_intro("Rules", INTRO, vec![]),
            ui::scroll(column(body).spacing(GAP).width(Length::Fill))
                .height(Length::Fill)
                .into(),
        ),
        dialog,
    )
}

/// A card of the Rules screen, as the web's `.rl-card`: its head (title and actions), then its body.
fn panel<'a>(head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(column![head, body].spacing(12).width(Length::Fill))
        .padding(Padding::from(16.0))
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// A card head: its title, and its action on the right when it has one.
fn panel_head<'a>(title: &str, action: Option<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::card_title(title).into(),
        Space::new().width(Length::Fill).into(),
    ];
    if let Some(a) = action {
        parts.push(a);
    }
    row(parts).spacing(12).align_y(Alignment::Center).into()
}

/// The banner while any constitutional law is switched off, with a link to each one.
fn constitution_banner<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let off: Vec<&Law> = state
        .laws
        .iter()
        .filter(|l| l.tier == CONSTITUTION && !l.enabled)
        .collect();
    if off.is_empty() {
        return None;
    }
    let n = off.len();
    let headline = format!(
        "{n} constitutional law{} {} switched off",
        if n == 1 { "" } else { "s" },
        if n == 1 { "is" } else { "are" }
    );
    let links: Vec<Element<'a, Msg>> = off
        .iter()
        .map(|l| {
            ui::mono(
                l.id.clone(),
                crate::theme::TYPE_META,
                crate::theme::palette().warn,
            )
            .into()
        })
        .collect();
    Some(
        container(
            column![
                text(headline)
                    .size(14.0)
                    .color(crate::theme::palette().warn),
                row(links).spacing(14).align_y(Alignment::Center)
            ]
            .spacing(8),
        )
        .padding(Padding {
            top: 12.0,
            right: 16.0,
            bottom: 12.0,
            left: 16.0,
        })
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(
                Color {
                    a: 0.08,
                    ..crate::theme::palette().warn
                }
                .into(),
            ),
            border: Border {
                color: crate::theme::palette().warn,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into(),
    )
}

/// The switch-off dialog of a constitutional law: what it protects, a reason, and its id typed in.
/// The switch stays disabled until both are given.
/// The field of a dialog: its label, the input, and a hint under it when there is one.
fn dialog_field<'a>(title: &str, input: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::label(title).into(), input];
    if !hint.is_empty() {
        parts.push(ui::faint(hint.to_string()).into());
    }
    column(parts).spacing(6).into()
}

/// The web's dialog for switching off a constitutional law: what it protects, a reason, and its id typed in.
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
        ui::prose("While it is off, agents may do what it forbids. The change is recorded with the time, and this screen flags it until the law is switched back on.").into(),
    );
    let reason_id = law.id.clone();
    let reason_in: Element<'a, Msg> = text_input("Why this law is off", reason)
        .on_input(move |t| Msg::Reason(reason_id.clone(), t))
        .padding([7, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(crate::theme::input)
        .into();
    body.push(dialog_field("Reason (required)", reason_in, ""));
    let typed_in: Element<'a, Msg> = text_input(&law.id, &dialog.typed)
        .on_input(Msg::TypeConfirm)
        .padding([7, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(crate::theme::input)
        .into();
    body.push(dialog_field(
        "Type the law’s id to confirm",
        typed_in,
        &format!("Exactly {}, as written", law.id),
    ));
    Some(ui::modal(
        &dialog.heading,
        column(body).spacing(12).into(),
        vec![
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelSwitchOff)),
            ui::btn(
                "Switch off constitutional law",
                None,
                Kind::Danger,
                ready.then(|| Msg::SetOverride(law.id.clone(), true)),
            ),
        ],
        Msg::CancelSwitchOff,
        480.0,
    ))
}

/// The web's dialog for disabling a built-in statute: its reason, which the override keeps.
fn disable_dialog<'a>(state: &'a State, dialog: &'a Named) -> Option<Element<'a, Msg>> {
    let law = state.laws.iter().find(|l| l.id == dialog.id)?;
    let reason = state.reasons.get(&law.id).map(String::as_str).unwrap_or("");
    let ready = !state.busy && !reason.trim().is_empty();
    let id = law.id.clone();
    let reason_in: Element<'a, Msg> = text_input("Why this statute is off", reason)
        .on_input(move |t| Msg::Reason(id.clone(), t))
        .padding([7, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(crate::theme::input)
        .into();
    Some(ui::modal(
        &dialog.heading,
        column![
            ui::faint(format!(
                "{} stops applying until it is enabled again. The reason is kept with the override.",
                law.id
            )),
            dialog_field("Reason (required)", reason_in, ""),
        ]
        .spacing(12)
        .into(),
        vec![
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelDisable)),
            ui::btn(
                "Disable statute",
                None,
                Kind::Danger,
                ready.then(|| Msg::SetOverride(law.id.clone(), true)),
            ),
        ],
        Msg::CancelDisable,
        480.0,
    ))
}

/// The web's dialog for deleting an owner rule: its laws stop applying, and it cannot be undone.
fn delete_dialog<'a>(state: &'a State, dialog: &'a Named) -> Option<Element<'a, Msg>> {
    let rule = state.rules.iter().find(|r| r.id == dialog.id)?;
    Some(ui::modal(
        &dialog.heading,
        ui::prose(format!(
            "Delete {} ({})? Its laws stop applying. This cannot be undone.",
            rule.title, rule.id
        ))
        .into(),
        vec![
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelDelete)),
            ui::btn(
                "Delete rule",
                None,
                Kind::Danger,
                (!state.busy).then_some(Msg::DeleteRule),
            ),
        ],
        Msg::CancelDelete,
        480.0,
    ))
}

/// The hub's events for this screen: a law or agent change on the hub reloads it.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

// ---- requests ----

/// One call that keeps the hub's answer whatever its status: a refused save carries diagnostics.
fn send(
    client: &client::Client,
    method: &str,
    path: &str,
    body: &Value,
) -> Result<(u16, Value), String> {
    apz_http::client::request_json(&client.target, method, path, Some(body), client.timeout)
        .map_err(|e| e.to_string())
}

fn save_rule(
    client: &client::Client,
    editing: Option<&str>,
    title: &str,
    source: &str,
) -> Result<Rule, SaveFailure> {
    let body = json!({"title": title, "source": source});
    let (method, path) = match editing {
        Some(id) => ("PUT", format!("/v1/laws/rules/{}", encode(id))),
        None => ("POST", "/v1/laws/rules".to_string()),
    };
    let (status, value) = send(client, method, &path, &body).map_err(|message| SaveFailure {
        message,
        diagnostics: Vec::new(),
    })?;
    if (200..300).contains(&status) {
        return Ok(parse_rule(&value));
    }
    let message = value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("The hub answered {status}."));
    Err(SaveFailure {
        message,
        diagnostics: parse_diagnostics(&value),
    })
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
    let root = settings
        .as_object_mut()
        .ok_or("The settings did not load.")?;
    // The settings write takes the whole object, minus the key the hub only reports.
    root.remove("dev_models");
    let laws = root.entry("laws").or_insert_with(|| json!({}));
    laws.as_object_mut()
        .ok_or("The laws settings are not an object.")?
        .insert("strict_permits".into(), json!(on));
    client
        .call("PUT", "/v1/settings", Some(&settings))
        .map(|_| ())
}

fn clear_editor(state: &mut State) {
    state.editing = None;
    state.title.clear();
    state.source = text_editor::Content::new();
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
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_laws(v: &Value) -> Vec<Law> {
    v.as_array()
        .map(|a| a.iter().map(parse_law).collect())
        .unwrap_or_default()
}

fn parse_law(v: &Value) -> Law {
    let agents = v
        .get("agents")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    let cid = x.get("cid").and_then(Value::as_u64).unwrap_or(0);
                    LawAgent {
                        cid,
                        label: roles::agent_label(&text_of(x, "role"), cid),
                        count: int_of(x, "count"),
                    }
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
        overridden: v
            .get("overridden")
            .filter(|o| !o.is_null())
            .map(|o| Override {
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
    v.get("rules")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(parse_rule).collect())
        .unwrap_or_default()
}

fn parse_rule(v: &Value) -> Rule {
    Rule {
        id: text_of(v, "id"),
        title: text_of(v, "title"),
        source: text_of(v, "source"),
        enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        updated_ms: int_of(v, "updated_ms"),
        author: text_of(v, "author"),
    }
}

fn parse_diagnostics(v: &Value) -> Vec<Diagnostic> {
    v.get("diagnostics")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Diagnostic {
                    line: int_of(x, "line"),
                    column: int_of(x, "column"),
                    message: text_of(x, "message"),
                })
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
    if let Some(s) = state.severity
        && l.severity != s
    {
        return false;
    }
    if let Some(t) = state.tier
        && tier_of(l) != t
    {
        return false;
    }
    let q = state.query.trim().to_lowercase();
    if !q.is_empty() {
        let hay =
            format!("{} {} {} {}", l.title, l.statement, l.id, l.ops.join(" ")).to_lowercase();
        if !hay.contains(&q) {
            return false;
        }
    }
    true
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    if n == 1 { "time" } else { "times" }
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
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn sheet_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// A row that opens something: the selected one (and the one under the pointer) in the hover fill.
fn link_style(_: &Theme, _: button::Status) -> button::Style {
    button::Style {
        text_color: crate::theme::palette().ink,
        ..Default::default()
    }
}

/// A mono text link to an agent or a law's source toggle.
fn link<'a>(label: &str, msg: Msg) -> Element<'a, Msg> {
    button(ui::sans(
        label.to_string(),
        12.0,
        crate::theme::palette().ink,
    ))
    .padding([0, 0])
    .style(link_style)
    .on_press(msg)
    .into()
}

fn stat_cell<'a>(label: &'static str, value: String) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    container(
        column![
            ui::label(label),
            text(value).font(SANS_SEMIBOLD).size(24.0).color(p.ink),
        ]
        .spacing(4),
    )
    .padding(Padding {
        top: 14.0,
        right: 16.0,
        bottom: 14.0,
        left: 16.0,
    })
    .width(Length::FillPortion(1))
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().raised.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        ..Default::default()
    })
    .into()
}

fn stats<'a>(state: &State) -> Element<'a, Msg> {
    let ready = state.loaded;
    let figure = |v: String| if ready { v } else { "—".to_string() };
    let forbids = state.laws.iter().filter(|l| l.effect == "forbid").count();
    // The web's `.rl-strip`: three stat cards in a row.
    row![
        stat_cell("Laws", figure(state.laws.len().to_string())),
        stat_cell("Allows", figure((state.laws.len() - forbids).to_string())),
        stat_cell("Forbids", figure(forbids.to_string())),
    ]
    .spacing(GAP)
    .width(Length::Fill)
    .into()
}

fn toolbar<'a>(state: &'a State) -> Element<'a, Msg> {
    let search = text_input("Search by title, statement, id or operation", &state.query)
        .on_input(Msg::Query)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(crate::theme::input);

    let mut sev_options = vec![SevChoice {
        value: None,
        label: "Any severity".to_string(),
    }];
    sev_options.extend(SEVERITIES.iter().map(|s| SevChoice {
        value: Some(*s),
        label: cap(s),
    }));
    let selected = sev_options
        .iter()
        .find(|o| o.value == state.severity)
        .cloned();
    let severity = pick_list(sev_options, selected, |c: SevChoice| Msg::Severity(c.value))
        .style(crate::theme::pick_list);
    let search_row = row![search, severity]
        .spacing(12)
        .align_y(Alignment::Center);

    // The web's chips (the effect) and the tier tabs, on one row.
    let effects = ui::seg(
        &[
            (Effect::All, "All"),
            (Effect::Permit, "Allows"),
            (Effect::Forbid, "Forbids"),
        ],
        state.effect,
        Msg::Effect,
    );
    let mut tier_options: Vec<(Option<&'static str>, &'static str)> = vec![(None, "All")];
    tier_options.extend(
        TIERS
            .iter()
            .take(4)
            .map(|&(key, label, _)| (Some(key), label)),
    );
    let tiers = ui::tabs(&tier_options, state.tier, Msg::Tier);

    column![
        search_row,
        row![effects, tiers].spacing(16).align_y(Alignment::Center),
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

/// The built-in statutes: the filters, then the statutes grouped by tier.
fn statutes<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    let all: Vec<&Law> = state
        .laws
        .iter()
        .filter(|l| l.origin == "built-in")
        .collect();
    let shown: Vec<&Law> = all.iter().copied().filter(|l| visible(state, l)).collect();
    let noun = if all.len() == 1 {
        "statute"
    } else {
        "statutes"
    };
    let mut col: Vec<Element<'a, Msg>> = vec![
        toolbar(state),
        ui::faint(format!("Showing {} of {} {}", shown.len(), all.len(), noun)).into(),
    ];
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
            let items: Vec<&Law> = shown
                .iter()
                .copied()
                .filter(|l| tier_of(l) == key)
                .collect();
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
        text(label.to_string())
            .font(SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink),
        text(n.to_string())
            .font(SANS)
            .size(13.0)
            .color(crate::theme::palette().ink_3),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(4);
    if !desc.is_empty() {
        col = col.push(
            text(desc)
                .font(SANS)
                .size(13.0)
                .color(crate::theme::palette().ink_2),
        );
    }
    col.into()
}

fn labelled<'a>(name: &str, value: &str) -> Element<'a, Msg> {
    row![
        text(format!("{name}: "))
            .font(SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink),
        text(value.to_string())
            .font(SANS)
            .size(15.0)
            .color(crate::theme::palette().ink_2)
            .width(Length::Fill),
    ]
    .into()
}

fn op_tag<'a>(op: &str) -> Element<'a, Msg> {
    container(ui::sans(
        op.to_string(),
        11.5,
        crate::theme::palette().ink_2,
    ))
    .padding([2, 6])
    .style(boxed)
    .into()
}

fn applies<'a>(l: &Law) -> Element<'a, Msg> {
    let scope = format!(
        "{} → {}",
        or(&l.principal, "any principal"),
        or(&l.resource, "any resource")
    );
    let mut col: Vec<Element<'a, Msg>> =
        vec![ui::sans(scope, 12.0, crate::theme::palette().ink).into()];
    if !l.ops.is_empty() {
        let tags: Vec<Element<'a, Msg>> = l.ops.iter().map(|o| op_tag(o)).collect();
        col.push(row(tags).spacing(6).into());
    }
    row![
        text("Applies to")
            .font(SANS)
            .size(12.0)
            .color(crate::theme::palette().ink_3)
            .width(Length::Fixed(96.0)),
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
        items.push(link(
            &format!("{} ({})", a.label, a.count),
            Msg::Go(Id::Agent(a.cid)),
        ));
    }
    row(items).spacing(6).align_y(Alignment::Center).into()
}

fn source_block<'a>(l: &Law, open: bool) -> Element<'a, Msg> {
    let toggle = ui::btn(
        "How it is written (JHRL)",
        None,
        Kind::Ghost,
        Some(Msg::ToggleSource(l.id.clone())),
    );
    if !open {
        return toggle;
    }
    let conds: Element<'a, Msg> = if l.conditions.is_empty() {
        ui::faint("No extra conditions.").into()
    } else {
        let lines: Vec<Element<'a, Msg>> = l
            .conditions
            .iter()
            .map(|c| ui::faint(format!("· {c}")).into())
            .collect();
        column(lines).spacing(4).into()
    };
    column![
        toggle,
        ui::faint(format!("File: {}", or(&l.file, "—"))),
        conds,
        container(ui::sans(
            or(&l.source, "—"),
            12.0,
            crate::theme::palette().ink
        ))
        .padding(10)
        .width(Length::Fill)
        .style(sheet_box),
    ]
    .spacing(10)
    .into()
}

/// One built-in statute: its card (as the statutes always showed), then its override control.
fn statute<'a>(state: &'a State, l: &'a Law, open: bool, now: i64) -> Element<'a, Msg> {
    let forbid = l.effect == "forbid";
    let mut tags: Vec<Element<'a, Msg>> = vec![
        ui::badge(
            if forbid { "Forbids" } else { "Allows" },
            if forbid { Tone::Danger } else { Tone::Ok },
        ),
        ui::badge(
            &format!("Severity: {}", cap(&l.severity)),
            severity_tone(&l.severity),
        ),
        ui::badge(
            &enforcement_label(&l.enforcement),
            if l.enforcement == "preventive" {
                Tone::Info
            } else {
                Tone::Muted
            },
        ),
    ];
    if !l.enabled {
        tags.push(ui::badge("Switched off", Tone::Warn));
    }
    let head = row![
        text(l.title.clone())
            .font(SANS_SEMIBOLD)
            .size(16.0)
            .color(crate::theme::palette().ink)
            .width(Length::Fill),
        ui::mono(l.id.clone(), 12.0, crate::theme::palette().ink_3),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut body: Vec<Element<'a, Msg>> = vec![
        head.into(),
        row(tags).spacing(6).align_y(Alignment::Center).into(),
        ui::prose(l.statement.clone()).into(),
        labelled("In short", &l.explanation),
    ];
    if !l.rationale.is_empty() {
        body.push(labelled("Why", &l.rationale));
    }
    body.push(applies(l));
    if l.refusals > 0 {
        body.push(usage(l, now));
    }
    body.push(override_control(state, l, now));
    body.push(source_block(l, open));

    // The web's `.rl-law` card: a raised card, ruled, with its parts a row apart.
    container(column(body).spacing(8).width(Length::Fill))
        .padding(Padding::from(16.0))
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// The switch for a statute: a reason field and "Switch off" while it is on; who switched it off,
/// why, and "Switch on" while it is off.
fn override_control<'a>(state: &'a State, l: &'a Law, now: i64) -> Element<'a, Msg> {
    match &l.overridden {
        // The web's override line: who turned the statute off, when and why, and Enable.
        Some(o) => column![
            ui::faint(format!(
                "Disabled by {}, {}: {}",
                or(&o.disabled_by, "the owner"),
                ago(o.at_ms, now),
                or(&o.reason, "no reason given")
            )),
            ui::btn_sm(
                "Enable",
                None,
                Kind::Secondary,
                (!state.busy).then(|| Msg::SetOverride(l.id.clone(), false))
            ),
        ]
        .spacing(8)
        .into(),
        // Both kinds go off from a dialog: the reason, and for a constitutional law its id typed in.
        None => {
            let msg = if l.tier == CONSTITUTION {
                Msg::AskSwitchOff(l.id.clone())
            } else {
                Msg::AskDisable(l.id.clone())
            };
            ui::btn_sm(
                "Disable",
                None,
                Kind::Secondary,
                (!state.busy).then_some(msg),
            )
        }
    }
}

fn owner_rules<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    let mut col: Vec<Element<'a, Msg>> = vec![ui::faint(OWNER_DESC).into()];
    // The editor opens above the list (New rule, or Edit on a rule), as the web's editor slot does.
    if state.editor_open {
        col.push(editor(state, now));
    }
    if state.rules.is_empty() {
        col.push(ui::faint("No owner rules yet. Write one with New rule.").into());
    }
    col.extend(state.rules.iter().map(|r| rule_card(state, r, now)));
    column(col).spacing(12).width(Length::Fill).into()
}

/// One owner rule, as the web's rule card: its title and id, its state and law count, when it was updated and
/// by whom, its actions, and the laws it declares.
fn rule_card<'a>(state: &'a State, r: &'a Rule, now: i64) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let declared: Vec<&'a Law> = state.laws.iter().filter(|l| l.rule_id == r.id).collect();
    let count = if declared.len() == 1 {
        "1 law".to_string()
    } else {
        format!("{} laws", declared.len())
    };
    let state_tag = if r.enabled {
        ui::badge("Enabled", Tone::Ok)
    } else {
        ui::badge("Disabled", Tone::Muted)
    };
    let head = row![
        text(r.title.clone())
            .font(SANS_SEMIBOLD)
            .size(16.0)
            .color(p.ink)
            .width(Length::Fill),
        ui::mono(r.id.clone(), 12.0, p.ink_3),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let by = if r.author.is_empty() {
        String::new()
    } else {
        format!(" by {}", r.author)
    };
    let acts = row![
        ui::btn_sm(
            "Edit",
            None,
            Kind::Secondary,
            Some(Msg::SelectRule(r.id.clone()))
        ),
        ui::btn_sm(
            if r.enabled { "Disable" } else { "Enable" },
            None,
            Kind::Secondary,
            (!state.busy).then(|| Msg::SetRuleEnabled(r.id.clone(), !r.enabled)),
        ),
        ui::btn_sm(
            "Delete",
            None,
            Kind::Danger,
            Some(Msg::AskDelete(r.id.clone()))
        ),
    ]
    .spacing(8);
    let mut body: Vec<Element<'a, Msg>> = vec![
        head.into(),
        row![state_tag, ui::badge(&count, Tone::Muted)]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        ui::faint(format!("Updated {}{}", ago(r.updated_ms, now), by)).into(),
        acts.into(),
    ];
    if declared.is_empty() {
        body.push(ui::faint("Declares no laws.").into());
    } else {
        body.extend(declared.into_iter().map(declared_law));
    }
    container(column(body).spacing(8).width(Length::Fill))
        .padding(Padding::from(16.0))
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// A law a rule declares (the web's law row inside a rule): its title and id, its tags, and what it says.
fn declared_law<'a>(l: &'a Law) -> Element<'a, Msg> {
    let forbid = l.effect == "forbid";
    let mut tags: Vec<Element<'a, Msg>> = vec![
        ui::badge(
            if forbid { "Forbids" } else { "Allows" },
            if forbid { Tone::Danger } else { Tone::Ok },
        ),
        ui::badge(
            &format!("Severity: {}", cap(&l.severity)),
            severity_tone(&l.severity),
        ),
        ui::badge(
            &enforcement_label(&l.enforcement),
            if l.enforcement == "preventive" {
                Tone::Info
            } else {
                Tone::Muted
            },
        ),
    ];
    if !l.enabled {
        tags.push(ui::badge("Rule disabled", Tone::Warn));
    }
    let head = row![
        text(l.title.clone())
            .font(SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink)
            .width(Length::Fill),
        ui::mono(l.id.clone(), 12.0, crate::theme::palette().ink_3),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let mut body: Vec<Element<'a, Msg>> = vec![
        head.into(),
        row(tags).spacing(6).align_y(Alignment::Center).into(),
        ui::prose(l.statement.clone()).into(),
    ];
    if !l.explanation.is_empty() {
        body.push(labelled("In short", &l.explanation));
    }
    if !l.rationale.is_empty() {
        body.push(labelled("Why", &l.rationale));
    }
    body.push(applies(l));
    column(body).spacing(6).width(Length::Fill).into()
}

fn editor<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
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
            .style(crate::theme::editor)
            .placeholder("law \"R-100\" \"Title\" { ... }")
            .font(SANS)
            .size(13.0)
            .height(Length::Fixed(240.0))
            .on_action(Msg::Source),
    ]
    .spacing(6);
    let reference = container(
        column![
            ui::label("JHRL reference"),
            ui::sans(JHRL_SHAPE, 12.0, crate::theme::palette().ink_2),
            ui::label("Example"),
            ui::sans(JHRL_EXAMPLE, 12.0, crate::theme::palette().ink_2),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fill)
    .style(sheet_box);

    let actions: Vec<Element<'a, Msg>> = vec![
        ui::btn(
            "Check",
            None,
            Kind::Secondary,
            (!state.busy).then_some(Msg::Check),
        ),
        ui::btn(
            "Save",
            None,
            Kind::Primary,
            (!state.busy).then_some(Msg::Save),
        ),
        ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CloseEditor)),
    ];

    let mut items: Vec<Element<'a, Msg>> = vec![
        text(heading)
            .font(SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink)
            .into(),
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
        .map(|d| {
            ui::mono(
                format!("Line {}, column {}: {}", d.line, d.column, d.message),
                12.0,
                crate::theme::palette().crit,
            )
            .into()
        })
        .collect()
}

fn check_result<'a>(c: &Checked, now: i64) -> Element<'a, Msg> {
    if !c.ok {
        let mut items: Vec<Element<'a, Msg>> = vec![ui::badge("Not valid yet", Tone::Danger)];
        items.extend(diagnostics(&c.diagnostics));
        return column(items).spacing(6).into();
    }
    let declared = if c.laws.is_empty() {
        "No laws declared.".to_string()
    } else {
        format!("Declares {}.", c.laws.join(", "))
    };
    let mut items: Vec<Element<'a, Msg>> =
        vec![ui::badge("Valid", Tone::Ok), ui::faint(declared).into()];
    if c.would_refuse.is_empty() {
        items.push(ui::faint("None of the recent actions would have been refused.").into());
    } else {
        items.push(ui::label("Would have refused, recently").into());
        items.extend(c.would_refuse.iter().map(|w| refusal_row(w, now)));
    }
    column(items).spacing(6).into()
}

fn refusal_row<'a>(w: &Refusal, now: i64) -> Element<'a, Msg> {
    column![
        ui::sans(
            roles::agent_label(&w.role, w.cid),
            12.0,
            crate::theme::palette().ink_2
        ),
        ui::faint(format!(
            "{} · {} · {}",
            or(&w.tool, "a tool"),
            ago(w.at_ms, now),
            w.laws.join(", ")
        )),
    ]
    .spacing(2)
    .into()
}

/// The strict permits switch, in the card's head as the web's: its state as the label, off while an action runs.
fn strict_switch<'a>(state: &'a State) -> Element<'a, Msg> {
    let t = toggler(state.strict).label(if state.strict { "On" } else { "Off" });
    if state.busy {
        t.into()
    } else {
        t.on_toggle(Msg::Strict).into()
    }
}
