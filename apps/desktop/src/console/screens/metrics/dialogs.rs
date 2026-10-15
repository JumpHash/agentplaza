//! The Metrics dialogs: an alert rule (new or edited) and the metrics settings — the same fields as the web's
//! dialogs.ts.

use apz_metrics_view::{MetricInfo, ScopeNode};
use iced::Element;

use super::Msg;

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

impl RuleForm {
    /// A new rule's form, scoped to the scope on screen.
    pub fn new(scope: &str) -> RuleForm {
        let _ = scope;
        RuleForm::default()
    }

    pub fn apply(&mut self, m: RuleMsg) {
        let _ = m;
    }

    /// The rule as `PUT /v1/metrics/alerts/rules` takes it, or the reason it is not valid.
    pub fn to_json(&self, catalog: &[MetricInfo]) -> Result<serde_json::Value, String> {
        let _ = catalog;
        Err("not written yet".into())
    }
}

impl SettingsForm {
    /// The form for the hub's settings JSON.
    pub fn from_json(v: &serde_json::Value) -> SettingsForm {
        let _ = v;
        SettingsForm::default()
    }

    pub fn apply(&mut self, m: SettingsMsg) {
        let _ = m;
    }

    pub fn to_json(&self) -> Result<serde_json::Value, String> {
        Err("not written yet".into())
    }
}

pub fn rule<'a>(
    form: &'a RuleForm,
    catalog: &'a [MetricInfo],
    scopes: &'a [ScopeNode],
) -> Element<'a, Msg> {
    let _ = (form, catalog, scopes);
    iced::widget::Space::new().into()
}

pub fn settings<'a>(form: &'a SettingsForm, scopes: &'a [ScopeNode]) -> Element<'a, Msg> {
    let _ = (form, scopes);
    iced::widget::Space::new().into()
}
