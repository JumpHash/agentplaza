//! The Metrics dialogs: an alert rule (new or edited) and the metrics settings — the same fields as the web's
//! dialogs.ts.

use apz_metrics_view::{MetricInfo, ScopeNode};
use iced::widget::{
    Space, TextInput, checkbox, column, container, pick_list, radio, row, text, text_input,
};
use iced::{Alignment, Element, Length};
use serde_json::json;

use super::Msg;
use crate::console::ui;
use crate::theme::{MONO, SANS_SEMIBOLD, TYPE_META, TYPE_SECTION, TYPE_UI};

/// The alert rule form.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleForm {
    pub id: String,
    pub name: String,
    pub metric: String,
    /// p95, p50, max, avg, sum, last.
    pub agg: String,
    pub scope: String,
    /// "gt" or "lt".
    pub cmp: String,
    pub threshold: String,
    pub for_ms: u64,
    /// "warning" or "critical".
    pub severity: String,
    pub console: bool,
    pub desktop: bool,
    pub board: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RuleMsg {
    Name(String),
    Metric(String),
    Agg(String),
    Scope(String),
    Cmp(String),
    Threshold(String),
    For(u64),
    Severity(String),
    Console(bool),
    Desktop(bool),
    Board(bool),
}

/// The settings form: the hub's settings as JSON values the fields edit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsForm {
    pub level: String,
    /// (field, value text) for each retention field.
    pub retention: Vec<(String, String)>,
    pub monthly_budget: String,
    /// (scope, USD text) caps.
    pub budgets: Vec<(String, String)>,
    pub basis_days: u32,
    pub openmetrics: bool,
    pub otlp: bool,
    pub otlp_url: String,
    pub otlp_confirmed: bool,
    /// Shown once after OpenMetrics was switched on.
    pub token: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SettingsMsg {
    Level(String),
    Retention(String, String),
    MonthlyBudget(String),
    BudgetScope(usize, String),
    BudgetUsd(usize, String),
    AddBudget,
    RemoveBudget(usize),
    Basis(u32),
    OpenMetrics(bool),
    Otlp(bool),
    OtlpUrl(String),
    OtlpConfirmed(bool),
}

/// The scope kinds a rule or a budget can name, as the scope tree lists them.
const NAMEABLE_KINDS: [&str; 7] = [
    "global", "node", "run", "role", "agent", "provider", "model",
];

/// The "every" scopes a rule can name besides a tree node: the value and its label.
const EVERY_SCOPES: [(&str, &str); 4] = [
    ("every:node", "every node"),
    ("every:agent", "every agent"),
    ("every:run", "every run"),
    ("every:volume", "every volume"),
];

/// The aggregations a rule can take.
const AGGREGATIONS: [&str; 6] = ["p95", "p50", "max", "avg", "sum", "last"];

/// The "for" choices of a rule: the value in milliseconds and its label.
const FOR_CHOICES: [(u64, &str); 5] = [
    (0, "0 s"),
    (60_000, "1 min"),
    (300_000, "5 min"),
    (900_000, "15 min"),
    (3_600_000, "1 h"),
];

/// The collection levels: the settings value, its label and the one-line description the plan gives it.
const LEVELS: [(&str, &str, &str); 4] = [
    ("off", "Off", "only what billing needs"),
    (
        "basic",
        "Basic",
        "host and VM CPU and memory at 60 s, lifecycle, latency",
    ),
    (
        "standard",
        "Standard",
        "plus disk, IO, network at 10 s, context split, traces (default)",
    ),
    (
        "full",
        "Full",
        "plus 2 s everywhere and every trace; reverts to Standard after 24 h",
    ),
];

/// The retention fields in the order the form shows them: the settings key, its label and its unit.
const RETENTION_FIELDS: [(&str, &str, &str); 7] = [
    ("chunks_hours", "Chunks", "hours"),
    ("minute_days", "Minute rollups", "days"),
    ("hour_days", "Hour rollups", "days"),
    ("calls_days", "Model calls", "days"),
    ("traces_days", "Kept traces", "days"),
    ("traces_cap_mb", "Kept traces, size cap", "MB"),
    ("events_days", "Events", "days"),
];

/// One entry of a pick list: the value the form keeps and the label the owner sees.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    value: String,
    label: String,
}

impl Choice {
    fn new(value: &str, label: &str) -> Choice {
        Choice {
            value: value.to_string(),
            label: label.to_string(),
        }
    }
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl RuleForm {
    /// A new rule's form, scoped to the scope on screen.
    pub fn new(scope: &str) -> RuleForm {
        RuleForm {
            scope: scope.to_string(),
            agg: "p95".into(),
            cmp: "gt".into(),
            threshold: "90".into(),
            for_ms: 300_000,
            severity: "warning".into(),
            console: true,
            desktop: true,
            ..RuleForm::default()
        }
    }

    pub fn apply(&mut self, m: RuleMsg) {
        match m {
            RuleMsg::Name(name) => self.name = name,
            RuleMsg::Metric(metric) => self.metric = metric,
            RuleMsg::Agg(agg) => self.agg = agg,
            RuleMsg::Scope(scope) => self.scope = scope,
            RuleMsg::Cmp(cmp) => self.cmp = cmp,
            RuleMsg::Threshold(threshold) => self.threshold = threshold,
            RuleMsg::For(for_ms) => self.for_ms = for_ms,
            RuleMsg::Severity(severity) => self.severity = severity,
            RuleMsg::Console(on) => self.console = on,
            RuleMsg::Desktop(on) => self.desktop = on,
            RuleMsg::Board(on) => self.board = on,
        }
    }

    /// The rule as `PUT /v1/metrics/alerts/rules` takes it, or the reason it is not valid.
    pub fn to_json(&self, catalog: &[MetricInfo]) -> Result<serde_json::Value, String> {
        if self.metric.is_empty() {
            return Err("Pick a metric.".into());
        }
        let threshold = match self.threshold.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => n,
            _ => return Err("The threshold must be a number.".into()),
        };
        let name = match self.name.trim() {
            "" => default_name(self, catalog),
            typed => typed.to_string(),
        };
        Ok(json!({
            "id": self.id,
            "name": name,
            "metric": self.metric,
            "agg": self.agg,
            "scope": self.scope,
            "cmp": self.cmp,
            "threshold": threshold,
            "for_ms": self.for_ms,
            "severity": self.severity,
            "on": true,
            "notify": {
                "console": self.console,
                "desktop": self.desktop,
                "board": self.board,
            },
            "starter": false,
        }))
    }
}

impl SettingsForm {
    /// The form for the hub's settings JSON.
    pub fn from_json(v: &serde_json::Value) -> SettingsForm {
        let outside = &v["outside"];
        SettingsForm {
            level: v["level"].as_str().unwrap_or("standard").to_string(),
            retention: RETENTION_FIELDS
                .iter()
                .map(|(key, _, _)| (key.to_string(), number_text(&v["retention"][*key])))
                .collect(),
            monthly_budget: number_text(&v["monthly_budget_usd"]),
            budgets: v["budgets"]
                .as_array()
                .map(|caps| {
                    caps.iter()
                        .map(|cap| {
                            (
                                cap["scope"].as_str().unwrap_or_default().to_string(),
                                number_text(&cap["cap_usd"]),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            basis_days: if v["basis_days"].as_u64() == Some(14) {
                14
            } else {
                30
            },
            openmetrics: outside["openmetrics"].as_bool().unwrap_or(false),
            otlp: outside["otlp"].as_bool().unwrap_or(false),
            otlp_url: outside["otlp_url"].as_str().unwrap_or_default().to_string(),
            otlp_confirmed: outside["otlp_confirmed"].as_bool().unwrap_or(false),
            token: None,
            error: None,
        }
    }

    pub fn apply(&mut self, m: SettingsMsg) {
        match m {
            SettingsMsg::Level(level) => self.level = level,
            SettingsMsg::Retention(key, days) => {
                if let Some(field) = self.retention.iter_mut().find(|(name, _)| *name == key) {
                    field.1 = days;
                }
            }
            SettingsMsg::MonthlyBudget(usd) => self.monthly_budget = usd,
            SettingsMsg::BudgetScope(at, scope) => {
                if let Some(cap) = self.budgets.get_mut(at) {
                    cap.0 = scope;
                }
            }
            SettingsMsg::BudgetUsd(at, usd) => {
                if let Some(cap) = self.budgets.get_mut(at) {
                    cap.1 = usd;
                }
            }
            SettingsMsg::AddBudget => self.budgets.push(("global".into(), String::new())),
            SettingsMsg::RemoveBudget(at) => {
                if at < self.budgets.len() {
                    self.budgets.remove(at);
                }
            }
            SettingsMsg::Basis(days) => self.basis_days = days,
            SettingsMsg::OpenMetrics(on) => self.openmetrics = on,
            SettingsMsg::Otlp(on) => self.otlp = on,
            SettingsMsg::OtlpUrl(url) => self.otlp_url = url,
            SettingsMsg::OtlpConfirmed(on) => self.otlp_confirmed = on,
        }
    }

    /// The settings as the hub takes them, or the reason they are not valid.
    pub fn to_json(&self) -> Result<serde_json::Value, String> {
        let mut retention = serde_json::Map::new();
        for (key, text) in &self.retention {
            let (label, _) = retention_field(key);
            let days = text
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|days| *days >= 1)
                .ok_or_else(|| format!("{label}: enter a whole number of at least 1."))?;
            retention.insert(key.clone(), json!(days));
        }

        let monthly = self.monthly_budget.trim();
        let monthly_usd = if monthly.is_empty() {
            serde_json::Value::Null
        } else {
            match monthly.parse::<f64>() {
                Ok(usd) if usd.is_finite() && usd >= 0.0 => json!(usd),
                _ => {
                    return Err(
                        "Monthly budget: enter a USD amount of 0 or more, or leave it empty."
                            .into(),
                    );
                }
            }
        };

        let mut budgets = Vec::new();
        let mut scopes_seen: Vec<&str> = Vec::new();
        for (scope, usd) in &self.budgets {
            let cap = match usd.trim().parse::<f64>() {
                Ok(n) if n.is_finite() && n >= 0.0 => n,
                _ => return Err("Daily caps: each cap needs a USD amount of 0 or more.".into()),
            };
            if scopes_seen.contains(&scope.as_str()) {
                return Err("Daily caps: each scope can have one cap.".into());
            }
            scopes_seen.push(scope.as_str());
            budgets.push(json!({ "scope": scope, "cap_usd": cap }));
        }

        let otlp_url = self.otlp_url.trim();
        if self.otlp {
            if !is_http_url(otlp_url) {
                return Err(
                    "OTLP export: enter the collector URL, starting with http:// or https://."
                        .into(),
                );
            }
            if !self.otlp_confirmed {
                return Err(
                    "OTLP export: confirm that kept traces may be sent to this collector.".into(),
                );
            }
        }

        Ok(json!({
            "level": self.level,
            "retention": retention,
            "budgets": budgets,
            "monthly_budget_usd": monthly_usd,
            "basis_days": self.basis_days,
            "outside": {
                "openmetrics": self.openmetrics,
                "otlp": self.otlp,
                "otlp_url": otlp_url,
                "otlp_confirmed": self.otlp_confirmed,
            },
        }))
    }
}

/// The name a rule takes when the owner leaves it empty: the metric's title, the sign and the threshold.
fn default_name(form: &RuleForm, catalog: &[MetricInfo]) -> String {
    let title = catalog
        .iter()
        .find(|m| m.id == form.metric)
        .map_or(form.metric.as_str(), |m| m.title.as_str());
    let sign = if form.cmp == "lt" { "<" } else { ">" };
    format!("{title} {sign} {}", form.threshold.trim())
}

/// A number of the settings JSON as the text its field shows, or empty when it is not a number.
fn number_text(value: &serde_json::Value) -> String {
    value.as_f64().map(|n| n.to_string()).unwrap_or_default()
}

/// Whether `text` is an http or https address with a host after the scheme.
fn is_http_url(text: &str) -> bool {
    text.strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// The label and unit of a retention field, by its settings key.
fn retention_field(key: &str) -> (&'static str, &'static str) {
    RETENTION_FIELDS
        .iter()
        .find(|(name, _, _)| *name == key)
        .map(|(_, label, unit)| (*label, *unit))
        .unwrap_or(("", ""))
}

/// The scope choices of a rule: the "every" scopes, then the scope tree's nameable nodes.
fn rule_scope_choices(scopes: &[ScopeNode]) -> Vec<Choice> {
    let mut choices: Vec<Choice> = EVERY_SCOPES
        .iter()
        .map(|(value, label)| Choice::new(value, label))
        .collect();
    choices.extend(tree_choices(scopes));
    choices
}

/// The scope tree's nodes a rule or a budget can name.
fn tree_choices(scopes: &[ScopeNode]) -> Vec<Choice> {
    scopes
        .iter()
        .filter(|s| NAMEABLE_KINDS.contains(&s.kind.as_str()))
        .map(|s| Choice::new(&s.id, &s.label))
        .collect()
}

/// A pick list over `options` that shows `value`; a value the list lacks is added, so it is never silently dropped.
fn picker<'a>(
    mut options: Vec<Choice>,
    value: &str,
    on_select: impl Fn(String) -> Msg + 'a,
) -> Element<'a, Msg> {
    if !options.iter().any(|o| o.value == value) {
        options.push(Choice::new(value, value));
    }
    let selected = options.iter().find(|o| o.value == value).cloned();
    pick_list(options, selected, move |choice: Choice| {
        on_select(choice.value)
    })
    .padding(8)
    .width(Length::Fill)
    .style(crate::theme::pick_list)
    .into()
}

/// A text field of a dialog, in the console's input style.
fn field_input<'a>(placeholder: &str, value: &str) -> TextInput<'a, Msg> {
    text_input(placeholder, value)
        .padding(8)
        .style(crate::theme::input)
}

/// A labelled row of a dialog: the label in a fixed column, the control beside it.
fn labelled<'a>(label: &str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        text(label.to_string())
            .size(TYPE_UI)
            .color(crate::theme::palette().ink_2)
            .width(Length::Fixed(150.0)),
        control,
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// A titled group of a settings dialog.
fn section<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    column![
        text(title.to_string())
            .size(TYPE_META)
            .color(crate::theme::palette().ink_3),
        body,
    ]
    .spacing(8)
    .into()
}

/// The dialog's title line.
fn title<'a>(words: &str) -> Element<'a, Msg> {
    text(words.to_string())
        .size(TYPE_SECTION)
        .font(SANS_SEMIBOLD)
        .color(crate::theme::palette().ink)
        .into()
}

/// A quiet line of help text.
fn hint<'a>(words: &str) -> Element<'a, Msg> {
    text(words.to_string())
        .size(TYPE_META)
        .color(crate::theme::palette().ink_3)
        .into()
}

/// The dialog's buttons, Cancel and the primary act, at the foot of the dialog.
fn buttons<'a>(primary: &str, on_primary: Msg) -> Element<'a, Msg> {
    row![
        Space::new().width(Length::Fill),
        ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseOverlay)),
        ui::btn(primary, None, ui::Kind::Primary, Some(on_primary)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

pub fn rule<'a>(
    form: &'a RuleForm,
    catalog: &'a [MetricInfo],
    scopes: &'a [ScopeNode],
) -> Element<'a, Msg> {
    let editing = !form.id.is_empty();

    let metrics: Vec<Choice> = catalog
        .iter()
        .map(|m| Choice::new(&m.id, &m.title))
        .collect();
    let aggregations: Vec<Choice> = AGGREGATIONS.iter().map(|a| Choice::new(a, a)).collect();
    let fors: Vec<Choice> = FOR_CHOICES
        .iter()
        .map(|(ms, label)| Choice::new(&ms.to_string(), label))
        .collect();
    let comparisons = vec![Choice::new("gt", ">"), Choice::new("lt", "<")];
    let severities = vec![
        Choice::new("warning", "warning"),
        Choice::new("critical", "critical"),
    ];

    // The name's placeholder is the name the rule takes when left empty.
    let name = field_input(&default_name(form, catalog), &form.name)
        .on_input(|s| Msg::RuleForm(RuleMsg::Name(s)));
    let threshold = field_input("90", &form.threshold)
        .on_input(|s| Msg::RuleForm(RuleMsg::Threshold(s)))
        .width(Length::Fixed(120.0));
    let condition: Element<'a, Msg> = row![
        container(picker(comparisons, &form.cmp, |s| Msg::RuleForm(
            RuleMsg::Cmp(s)
        )))
        .width(Length::Fixed(80.0)),
        threshold,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into();
    let notify: Element<'a, Msg> = row![
        checkbox(form.console)
            .label("console")
            .on_toggle(|on| Msg::RuleForm(RuleMsg::Console(on))),
        checkbox(form.desktop)
            .label("desktop notification")
            .on_toggle(|on| Msg::RuleForm(RuleMsg::Desktop(on))),
        checkbox(form.board)
            .label("board message")
            .on_toggle(|on| Msg::RuleForm(RuleMsg::Board(on))),
    ]
    .spacing(16)
    .into();

    let mut body = column![
        title(if editing { "Edit alert rule" } else { "New alert rule" }),
        labelled("Name", name.into()),
        labelled(
            "Metric",
            picker(metrics, &form.metric, |s| Msg::RuleForm(RuleMsg::Metric(s))),
        ),
        labelled(
            "Aggregation",
            picker(aggregations, &form.agg, |s| Msg::RuleForm(RuleMsg::Agg(s))),
        ),
        labelled(
            "Scope",
            picker(rule_scope_choices(scopes), &form.scope, |s| Msg::RuleForm(RuleMsg::Scope(s))),
        ),
        labelled("Condition", condition),
        labelled(
            "For",
            picker(fors, &form.for_ms.to_string(), move |s| {
                Msg::RuleForm(RuleMsg::For(s.parse().unwrap_or(form.for_ms)))
            }),
        ),
        labelled(
            "Severity",
            picker(severities, &form.severity, |s| Msg::RuleForm(RuleMsg::Severity(s))),
        ),
        labelled("Notify", notify),
        hint("Rules run on the hub's in-memory aggregates as points arrive, so they cost no database queries."),
    ]
    .spacing(12)
    .width(Length::Fixed(520.0));
    if let Some(error) = &form.error {
        body = body.push(ui::error_line(error));
    }
    body.push(buttons(
        if editing { "Save rule" } else { "Create rule" },
        Msg::SaveRule,
    ))
    .into()
}

pub fn settings<'a>(form: &'a SettingsForm, scopes: &'a [ScopeNode]) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let mut body = column![title("Metrics settings")]
        .spacing(16)
        .width(Length::Fixed(560.0));
    match &form.token {
        // Once OpenMetrics is switched on the token is shown, and the form is replaced by it, as on the web.
        Some(token) => {
            body = body.push(section(
                "OpenMetrics token",
                column![
                    hint("Copy it now; it is not shown again."),
                    container(text(token.clone()).font(MONO).size(TYPE_UI).color(p.ink))
                        .padding(8)
                        .width(Length::Fill)
                        .style(crate::theme::boxed),
                ]
                .spacing(8)
                .into(),
            ));
        }
        None => {
            body = body
                .push(level_section(form))
                .push(retention_section(form))
                .push(budget_section(form, scopes))
                .push(basis_section(form))
                .push(outside_section(form));
        }
    }
    if let Some(error) = &form.error {
        body = body.push(ui::error_line(error));
    }
    let (primary, on_primary) = if form.token.is_some() {
        ("Done", Msg::CloseOverlay)
    } else {
        ("Save settings", Msg::SaveSettings)
    };
    body.push(buttons(primary, on_primary)).into()
}

/// The collection level, one radio per level with its description.
fn level_section<'a>(form: &'a SettingsForm) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let selected: Option<&'static str> = LEVELS
        .iter()
        .map(|(value, _, _)| *value)
        .find(|value| *value == form.level);
    let levels: Vec<Element<'a, Msg>> = LEVELS
        .iter()
        .map(|(value, label, description)| {
            row![
                radio(*label, *value, selected, |level: &'static str| {
                    Msg::SettingsForm(SettingsMsg::Level(level.to_string()))
                }),
                text(*description).size(TYPE_META).color(p.ink_3),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    section("Collection level", column(levels).spacing(8).into())
}

/// The retention fields, each a whole number of days (or hours, or MB) with its unit.
fn retention_section<'a>(form: &'a SettingsForm) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let rows: Vec<Element<'a, Msg>> = form
        .retention
        .iter()
        .map(|(key, days)| {
            let (label, unit) = retention_field(key);
            let key = key.clone();
            labelled(
                label,
                row![
                    field_input("", days)
                        .on_input(move |s| Msg::SettingsForm(SettingsMsg::Retention(
                            key.clone(),
                            s
                        )))
                        .width(Length::Fixed(120.0)),
                    text(unit).size(TYPE_UI).color(p.ink_3),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
            )
        })
        .collect();
    section("Retention", column(rows).spacing(10).into())
}

/// The monthly budget, and one daily cap per scope.
fn budget_section<'a>(form: &'a SettingsForm, scopes: &'a [ScopeNode]) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let choices = tree_choices(scopes);
    let caps: Vec<Element<'a, Msg>> = form
        .budgets
        .iter()
        .enumerate()
        .map(|(at, (scope, usd))| {
            row![
                picker(choices.clone(), scope, move |s| {
                    Msg::SettingsForm(SettingsMsg::BudgetScope(at, s))
                }),
                field_input("0", usd)
                    .on_input(move |s| Msg::SettingsForm(SettingsMsg::BudgetUsd(at, s)))
                    .width(Length::Fixed(120.0)),
                text("USD").size(TYPE_UI).color(p.ink_3),
                ui::btn_sm(
                    "Remove",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::SettingsForm(SettingsMsg::RemoveBudget(at))),
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    let monthly: Element<'a, Msg> = row![
        field_input("none", &form.monthly_budget)
            .on_input(|s| Msg::SettingsForm(SettingsMsg::MonthlyBudget(s)))
            .width(Length::Fixed(160.0)),
        text("USD").size(TYPE_UI).color(p.ink_3),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into();
    let caps_block: Element<'a, Msg> = column![
        column(caps).spacing(8),
        ui::btn_sm(
            "Add daily cap",
            None,
            ui::Kind::Secondary,
            Some(Msg::SettingsForm(SettingsMsg::AddBudget)),
        ),
        hint("Laws enforce caps; Metrics only reports them."),
    ]
    .spacing(8)
    .into();
    section(
        "Budgets",
        column![
            labelled("Monthly budget", monthly),
            labelled("Daily caps", caps_block)
        ]
        .spacing(10)
        .into(),
    )
}

/// The forecast basis: the last 14 or 30 days.
fn basis_section<'a>(form: &'a SettingsForm) -> Element<'a, Msg> {
    let options = vec![Choice::new("14", "14 days"), Choice::new("30", "30 days")];
    let basis = picker(options, &form.basis_days.to_string(), |s| {
        Msg::SettingsForm(SettingsMsg::Basis(if s == "14" { 14 } else { 30 }))
    });
    section("Forecast basis", labelled("Basis", basis))
}

/// The opt-in outside endpoints: the OpenMetrics scrape and the OTLP collector, both off until switched on.
fn outside_section<'a>(form: &'a SettingsForm) -> Element<'a, Msg> {
    let openmetrics = checkbox(form.openmetrics)
        .label("/metrics for an outside scraper, with its own token")
        .on_toggle(|on| Msg::SettingsForm(SettingsMsg::OpenMetrics(on)));
    let otlp = checkbox(form.otlp)
        .label("OTLP export")
        .on_toggle(|on| Msg::SettingsForm(SettingsMsg::Otlp(on)));
    // The collector URL and the confirmation are editable only while OTLP is on.
    let url = field_input("https://collector.example.com:4318", &form.otlp_url);
    let url = if form.otlp {
        url.on_input(|s| Msg::SettingsForm(SettingsMsg::OtlpUrl(s)))
    } else {
        url
    };
    let confirmed = checkbox(form.otlp_confirmed).label("send kept traces to this collector");
    let confirmed = if form.otlp {
        confirmed.on_toggle(|on| Msg::SettingsForm(SettingsMsg::OtlpConfirmed(on)))
    } else {
        confirmed
    };
    section(
        "Outside endpoints",
        column![
            openmetrics,
            otlp,
            labelled("Collector URL", url.into()),
            confirmed,
        ]
        .spacing(10)
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_with_a_bad_threshold_is_refused_and_a_nameless_one_is_named_after_its_metric() {
        let bad = RuleForm {
            metric: "spend".into(),
            threshold: "ninety".into(),
            ..RuleForm::new("global")
        };
        assert_eq!(
            bad.to_json(&[]),
            Err("The threshold must be a number.".to_string())
        );

        let nameless = RuleForm {
            metric: "spend".into(),
            cmp: "lt".into(),
            threshold: " 25 ".into(),
            ..RuleForm::new("global")
        };
        let rule = nameless.to_json(&[]).expect("a number threshold is valid");
        assert_eq!(rule["name"], "spend < 25");
        assert_eq!(rule["threshold"], 25.0);
    }

    #[test]
    fn the_settings_form_gives_back_the_settings_it_was_read_from() {
        let settings = json!({
            "level": "full",
            "retention": {
                "chunks_hours": 48,
                "minute_days": 14,
                "hour_days": 400,
                "calls_days": 90,
                "traces_days": 7,
                "traces_cap_mb": 2048,
                "events_days": 90,
            },
            "budgets": [
                {"scope": "global", "cap_usd": 25.0},
                {"scope": "role:builder", "cap_usd": 4.5},
            ],
            "monthly_budget_usd": 300.0,
            "basis_days": 30,
            "outside": {
                "openmetrics": true,
                "otlp": true,
                "otlp_url": "https://collector.example.com:4318",
                "otlp_confirmed": true,
            },
        });
        let form = SettingsForm::from_json(&settings);
        assert_eq!(form.to_json(), Ok(settings));
    }
}
