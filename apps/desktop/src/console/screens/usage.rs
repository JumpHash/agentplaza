//! The Usage screen (parity with the web console's usage view): what the model calls cost and
//! consumed over a range, grouped by model, agent, role or provider; the calls themselves; and the
//! prices the spend is priced with. Read-only, except for the price refresh.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::{button, column, container, pick_list, row, text, Space};
use iced::{alignment, Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme};
use serde::Deserialize;
use serde_json::json;

use super::Id;
use crate::console::{chart, client::{self, Client}, ui, Ctx};
use crate::theme::{self, MONO};

const VALUE: Font = Font { weight: font::Weight::Bold, ..theme::MONO };
const ITALIC: Font = Font { style: font::Style::Italic, ..theme::SERIF };
/// How long a figure or a chart takes to settle after its data changes.
const ANIM: Duration = Duration::from_millis(600);
/// Groups drawn as their own band; the rest sum into "other".
const TOP: usize = 6;
const OTHER: &str = "*other*";
fn palette() -> [Color; TOP] {
    [crate::theme::palette().signal, crate::theme::palette().ok, Color::from_rgb8(0x6f, 0xa8, 0xdc), crate::theme::palette().warn, Color::from_rgb8(0xb4, 0x8e, 0xd6), Color::from_rgb8(0x4f, 0xc3, 0xc3)]
}
const DAY_MS: u64 = 86_400_000;
const CALLS_PAGE: usize = 50;
const CALLS_LIMIT: usize = 200;

// ---- The screen's choices ----

/// The span the figures cover: the last 24 hours, 7, 30 or 90 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    H24,
    #[default]
    D7,
    D30,
    D90,
}

impl Range {
    const ALL: [(Range, &'static str); 4] = [(Range::H24, "24 h"), (Range::D7, "7 d"), (Range::D30, "30 d"), (Range::D90, "90 d")];

    fn ms(self) -> u64 {
        match self {
            Range::H24 => DAY_MS,
            Range::D7 => 7 * DAY_MS,
            Range::D30 => 30 * DAY_MS,
            Range::D90 => 90 * DAY_MS,
        }
    }

    /// Hourly points for the short spans, daily for the long ones.
    fn bucket(self) -> &'static str {
        match self {
            Range::H24 | Range::D7 => "hour",
            Range::D30 | Range::D90 => "day",
        }
    }
}

/// What the figures are grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    Model,
    Agent,
    Role,
    Provider,
}

impl GroupBy {
    const ALL: [(GroupBy, &'static str); 4] = [(GroupBy::Model, "Model"), (GroupBy::Agent, "Agent"), (GroupBy::Role, "Role"), (GroupBy::Provider, "Provider")];

    fn key(self) -> &'static str {
        match self {
            GroupBy::Model => "model",
            GroupBy::Agent => "agent",
            GroupBy::Role => "role",
            GroupBy::Provider => "provider",
        }
    }

    fn rows(self, s: &Summary) -> &[Group] {
        match self {
            GroupBy::Model => &s.by_model,
            GroupBy::Agent => &s.by_agent,
            GroupBy::Role => &s.by_role,
            GroupBy::Provider => &s.by_provider,
        }
    }

    /// A group key as a reader sees it: an agent's name, otherwise the key itself.
    fn display(self, key: &str, agents: &BTreeMap<u64, String>) -> String {
        if key == OTHER {
            return "Other".to_string();
        }
        if key.is_empty() {
            return "unknown".to_string();
        }
        match self {
            GroupBy::Agent => key.parse::<u64>().ok().and_then(|cid| agents.get(&cid)).cloned().unwrap_or_else(|| format!("agent {key}")),
            _ => key.to_string(),
        }
    }
}

/// The unit the chart and the shares read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unit {
    #[default]
    Usd,
    Tokens,
}

impl Unit {
    const ALL: [(Unit, &'static str); 2] = [(Unit::Usd, "USD"), (Unit::Tokens, "Tokens")];

    fn value(self, t: &Totals) -> f64 {
        match self {
            Unit::Usd => t.usd,
            Unit::Tokens => (t.input + t.output) as f64,
        }
    }

    fn format(self) -> fn(f64) -> String {
        match self {
            Unit::Usd => usd,
            Unit::Tokens => compact,
        }
    }
}

/// The main panel's two faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Chart,
    Table,
}

impl View {
    const ALL: [(View, &'static str); 2] = [(View::Chart, "Chart"), (View::Table, "Table")];
}

/// The table's sort columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    Name,
    Calls,
    Input,
    Output,
    CacheRead,
    CacheWrite,
    Hit,
    Reported,
    Estimated,
    #[default]
    Share,
}

/// A pick-list entry: a value and what it reads as.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice<T> {
    value: T,
    label: String,
}

impl<T> fmt::Display for Choice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

// ---- The hub's answers ----

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Totals {
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: f64,
    pub est_usd: f64,
    pub usd: f64,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Group {
    pub key: String,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Summary {
    pub totals: Totals,
    pub by_model: Vec<Group>,
    pub by_agent: Vec<Group>,
    pub by_role: Vec<Group>,
    pub by_provider: Vec<Group>,
    pub unpriced: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Point {
    pub at_ms: u64,
    pub groups: BTreeMap<String, Totals>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Timeline {
    pub bucket_ms: u64,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Call {
    pub at_ms: u64,
    pub cid: u64,
    pub role: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: Option<f64>,
    pub est_usd: f64,
    pub task: Option<String>,
    pub via: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CallsPage {
    pub calls: Vec<Call>,
    pub total: u64,
    pub next_before: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Source {
    pub name: String,
    pub ok: bool,
}

/// `GET /v1/prices`' head: when the lists were fetched and from where (its model list is not read).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PriceHead {
    pub fetched_at_ms: u64,
    pub sources: Vec<Source>,
}

/// USD per million tokens.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub source: String,
}

#[derive(Deserialize)]
struct Priced {
    price: Price,
}

#[derive(Deserialize)]
struct AgentRow {
    cid: u64,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SettingsView {
    pub default_model: String,
    pub orchestrator_model: String,
    pub role_models: BTreeMap<String, String>,
}

/// Everything the screen shows, from one load.
#[derive(Debug, Clone, Default)]
pub struct Data {
    summary: Summary,
    timeline: Timeline,
    calls: Vec<Call>,
    calls_total: u64,
    calls_next: Option<u64>,
    agents: BTreeMap<u64, String>,
    settings: SettingsView,
    /// Every price looked up so far; `None` where the hub has no price.
    prices: BTreeMap<String, Option<Price>>,
    head: Option<PriceHead>,
    /// The contributor models the presets flag (`provider/model`): they train on what they are sent.
    trains: BTreeSet<String>,
}

// ---- Messages ----

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Data, String>),
    Range(Range),
    Group(GroupBy),
    Unit(Unit),
    View(View),
    Sort(Sort),
    ModelFilter(Choice<Option<String>>),
    AgentFilter(Choice<Option<u64>>),
    /// The empty calls state's "Clear filters": both filters back to every call.
    ClearFilters,
    RefreshPrices,
    Refreshed(Result<PriceHead, String>),
    LoadOlder,
    Older(Result<CallsPage, String>),
    /// An animation frame while something is settling.
    Frame,
}

// ---- State ----

#[derive(Debug, Default)]
pub struct State {
    /// The last failed call's message.
    pub error: Option<String>,
    data: Option<Data>,
    range: Range,
    group: GroupBy,
    unit: Unit,
    view: View,
    sort: Sort,
    /// Ascending when set; the default is descending.
    asc: bool,
    model_filter: Option<String>,
    agent_filter: Option<u64>,
    busy: bool,
    calls_busy: bool,
    labels: Vec<String>,
    chart: Animated,
    counters: Counters,
}

/// A chart's series, easing from the frame on screen to a new target.
#[derive(Debug, Default)]
struct Animated {
    from: Vec<chart::Series>,
    to: Vec<chart::Series>,
    start: Option<Instant>,
}

impl Animated {
    fn current(&self, now: Instant) -> Vec<chart::Series> {
        match self.start {
            Some(s) => chart::blend(&self.from, &self.to, chart::ease(progress(s, now))),
            None => self.to.clone(),
        }
    }

    fn retarget(&mut self, next: Vec<chart::Series>, now: Instant) {
        if next == self.to {
            return;
        }
        self.from = self.current(now);
        self.to = next;
        self.start = Some(now);
    }

    fn running(&self, now: Instant) -> bool {
        self.start.is_some_and(|s| now.saturating_duration_since(s) < ANIM)
    }
}

/// A figure that counts from its old value to a new one.
#[derive(Debug, Default, Clone, Copy)]
struct Tween {
    from: f64,
    to: f64,
    start: Option<Instant>,
}

impl Tween {
    fn at(&self, now: Instant) -> f64 {
        match self.start {
            Some(s) => self.from + (self.to - self.from) * chart::ease(progress(s, now)),
            None => self.to,
        }
    }

    fn aim(&mut self, to: f64, now: Instant) {
        if to == self.to {
            return;
        }
        self.from = self.at(now);
        self.to = to;
        self.start = Some(now);
    }

    fn running(&self, now: Instant) -> bool {
        self.start.is_some_and(|s| now.saturating_duration_since(s) < ANIM)
    }
}

#[derive(Debug, Default)]
struct Counters {
    usd: Tween,
    input: Tween,
    output: Tween,
    hit: Tween,
    calls: Tween,
}

impl Counters {
    fn all(&self) -> [&Tween; 5] {
        [&self.usd, &self.input, &self.output, &self.hit, &self.calls]
    }
}

fn progress(start: Instant, now: Instant) -> f64 {
    (now.saturating_duration_since(start).as_secs_f64() / ANIM.as_secs_f64()).min(1.0)
}

impl State {
    /// Rebuilds the chart's target and the figures' targets from the loaded data.
    fn sync(&mut self) {
        let now = Instant::now();
        let Some(d) = self.data.as_ref() else { return };
        let (series, labels) = chart_series(&d.timeline, self.unit);
        self.chart.retarget(series, now);
        self.labels = labels;
        let t = &d.summary.totals;
        self.counters.usd.aim(t.usd, now);
        self.counters.input.aim(t.input as f64, now);
        self.counters.output.aim(t.output as f64, now);
        self.counters.hit.aim(t.cache_hit_rate * 100.0, now);
        self.counters.calls.aim(t.calls as f64, now);
    }

    fn animating(&self, now: Instant) -> bool {
        self.chart.running(now) || self.counters.all().iter().any(|c| c.running(now))
    }

    fn clear_calls(&mut self) {
        if let Some(d) = self.data.as_mut() {
            d.calls.clear();
            d.calls_next = None;
        }
    }
}

// ---- Loading ----

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The animation frames, while a figure or the chart is settling.
pub fn subscription(state: &State) -> Subscription<Msg> {
    if state.animating(Instant::now()) {
        iced::time::every(Duration::from_millis(33)).map(|_| Msg::Frame)
    } else {
        Subscription::none()
    }
}

/// What one load asks the hub for.
struct Query {
    from: u64,
    to: u64,
    bucket: &'static str,
    by: &'static str,
    model: Option<String>,
    agent: Option<u64>,
    limit: usize,
    /// Prices already known, so a poll does not look them up again.
    cache: BTreeMap<String, Option<Price>>,
    want_head: bool,
}

/// Loads the summary, the series, the calls page, the roster, the settings and the prices in use
/// (called when the screen opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let to = now_ms();
    let (cache, want_head, limit) = match &state.data {
        Some(d) => (d.prices.clone(), d.head.is_none(), d.calls.len().clamp(CALLS_PAGE, CALLS_LIMIT)),
        None => (BTreeMap::new(), true, CALLS_PAGE),
    };
    let q = Query {
        from: to.saturating_sub(state.range.ms()),
        to,
        bucket: state.range.bucket(),
        by: state.group.key(),
        model: state.model_filter.clone(),
        agent: state.agent_filter,
        limit,
        cache,
        want_head,
    };
    let c = ctx.client.clone();
    client::task(move || fetch(&c, q), Msg::Loaded)
}

fn fetch(c: &Client, q: Query) -> Result<Data, String> {
    let summary: Summary = c.get(&format!("/v1/usage/summary?from={}&to={}", q.from, q.to))?;
    let timeline: Timeline = c.get(&format!("/v1/usage/series?from={}&to={}&bucket={}&by={}", q.from, q.to, q.bucket, q.by))?;
    let page: CallsPage = c.get(&calls_path(q.limit, None, q.model.as_deref(), q.agent))?;
    let roster: Vec<AgentRow> = c.get("/v1/agents")?;
    let settings: SettingsView = c.get("/v1/settings")?;
    let mut prices = q.cache;
    for model in models_in_use(&summary, &settings) {
        if !prices.contains_key(&model) {
            let found = c.get::<Priced>(&format!("/v1/prices/{}", enc(&model))).ok().map(|p| p.price);
            prices.insert(model, found);
        }
    }
    let head = if q.want_head { c.get::<PriceHead>("/v1/prices").ok() } else { None };
    let kinds: Vec<serde_json::Value> = c.get("/v1/provider-kinds").unwrap_or_default();
    Ok(Data {
        summary,
        timeline,
        calls: page.calls,
        calls_total: page.total,
        calls_next: page.next_before,
        agents: roster.into_iter().map(|a| (a.cid, a.name)).collect(),
        settings,
        prices,
        head,
        trains: trains_models(&kinds),
    })
}

/// The contributor models the provider presets flag (`options.trains_on_data`), as `provider/model`.
fn trains_models(kinds: &[serde_json::Value]) -> BTreeSet<String> {
    kinds
        .iter()
        .flat_map(|kind| {
            let id = kind["id"].as_str().unwrap_or_default().to_string();
            kind["options"]["trains_on_data"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |name| name.as_str().map(|n| format!("{id}/{n}")))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `GET /v1/usage/calls` with its filters; `before` pages back from an id.
fn calls_path(limit: usize, before: Option<u64>, model: Option<&str>, agent: Option<u64>) -> String {
    let mut q = format!("limit={limit}");
    if let Some(b) = before {
        q.push_str(&format!("&before={b}"));
    }
    if let Some(m) = model {
        q.push_str(&format!("&model={}", enc(m)));
    }
    if let Some(a) = agent {
        q.push_str(&format!("&cid={a}"));
    }
    format!("/v1/usage/calls?{q}")
}

/// Every model the screen prices: the ones with calls, the unpriced ones and the settings' models.
fn models_in_use(s: &Summary, settings: &SettingsView) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = s.by_model.iter().map(|g| g.key.clone()).chain(s.unpriced.iter().cloned()).collect();
    out.insert(settings.default_model.clone());
    out.insert(settings.orchestrator_model.clone());
    out.extend(settings.role_models.values().cloned());
    out.retain(|m| !m.is_empty());
    out
}

/// Where each settings model is used, as the prices panel reads it: "default", "orchestrator", "role architect".
fn settings_uses(s: &SettingsView) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut add = |model: &str, label: String| {
        if !model.is_empty() {
            out.entry(model.to_string()).or_default().push(label);
        }
    };
    add(&s.default_model, "default".to_string());
    add(&s.orchestrator_model, "orchestrator".to_string());
    for (role, model) in &s.role_models {
        add(model, format!("role {role}"));
    }
    out
}

// ---- Update ----

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::Frame => Task::none(),
        Msg::Loaded(Ok(mut data)) => {
            if data.head.is_none() {
                data.head = state.data.as_mut().and_then(|d| d.head.take());
            }
            state.data = Some(data);
            state.error = None;
            state.sync();
            Task::none()
        }
        Msg::Loaded(Err(err)) => {
            state.error = Some(err);
            Task::none()
        }
        Msg::Range(range) => {
            state.range = range;
            load(state, ctx)
        }
        Msg::Group(group) => {
            state.group = group;
            load(state, ctx)
        }
        Msg::Unit(unit) => {
            state.unit = unit;
            state.chart = Animated::default();
            state.sync();
            Task::none()
        }
        Msg::View(view) => {
            state.view = view;
            Task::none()
        }
        Msg::Sort(sort) => {
            if state.sort == sort {
                state.asc = !state.asc;
            } else {
                state.sort = sort;
                state.asc = sort == Sort::Name;
            }
            Task::none()
        }
        Msg::ModelFilter(choice) => {
            state.model_filter = choice.value;
            state.clear_calls();
            load(state, ctx)
        }
        Msg::AgentFilter(choice) => {
            state.agent_filter = choice.value;
            state.clear_calls();
            load(state, ctx)
        }
        Msg::ClearFilters => {
            state.model_filter = None;
            state.agent_filter = None;
            state.clear_calls();
            load(state, ctx)
        }
        Msg::RefreshPrices => {
            state.busy = true;
            let c = ctx.client.clone();
            client::task(move || c.post::<PriceHead>("/v1/prices/refresh", &json!({})), Msg::Refreshed)
        }
        Msg::Refreshed(Ok(head)) => {
            state.busy = false;
            if let Some(d) = state.data.as_mut() {
                d.prices.clear();
                d.head = Some(head);
            }
            load(state, ctx)
        }
        Msg::Refreshed(Err(err)) => {
            state.busy = false;
            state.error = Some(err);
            Task::none()
        }
        Msg::LoadOlder => {
            let Some(before) = state.data.as_ref().and_then(|d| d.calls_next) else { return Task::none() };
            if state.calls_busy {
                return Task::none();
            }
            state.calls_busy = true;
            let path = calls_path(CALLS_PAGE, Some(before), state.model_filter.as_deref(), state.agent_filter);
            let c = ctx.client.clone();
            client::task(move || c.get::<CallsPage>(&path), Msg::Older)
        }
        Msg::Older(Ok(page)) => {
            state.calls_busy = false;
            if let Some(d) = state.data.as_mut() {
                d.calls.extend(page.calls);
                d.calls_next = page.next_before;
                d.calls_total = page.total;
            }
            Task::none()
        }
        Msg::Older(Err(err)) => {
            state.calls_busy = false;
            state.error = Some(err);
            Task::none()
        }
    }
}

// ---- The series and the sums (pure) ----

/// The chart's series for a timeline in `unit`: the top groups in rank order, then "other" for the
/// rest of each point, plus one label per point.
fn chart_series(tl: &Timeline, unit: Unit) -> (Vec<chart::Series>, Vec<String>) {
    let labels = tl.points.iter().map(|p| if tl.bucket_ms >= DAY_MS { day_label(p.at_ms) } else { stamp(p.at_ms) }).collect();
    let mut totals: BTreeMap<&str, f64> = BTreeMap::new();
    for p in &tl.points {
        for (key, t) in &p.groups {
            *totals.entry(key.as_str()).or_default() += unit.value(t);
        }
    }
    let mut ranked: Vec<(&str, f64)> = totals.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    let mut out: Vec<chart::Series> = ranked
        .iter()
        .take(TOP)
        .enumerate()
        .map(|(i, (key, _))| chart::Series {
            key: key.to_string(),
            color: palette()[i],
            values: tl.points.iter().map(|p| p.groups.get(*key).map_or(0.0, |t| unit.value(t))).collect(),
        })
        .collect();
    let rest: Vec<f64> = tl
        .points
        .iter()
        .enumerate()
        .map(|(j, p)| {
            let shown: f64 = out.iter().map(|s| s.values[j]).sum();
            (unit.value(&p.totals) - shown).max(0.0)
        })
        .collect();
    if rest.iter().any(|v| *v > 0.0) {
        out.push(chart::Series { key: OTHER.to_string(), color: crate::theme::palette().ink_3, values: rest });
    }
    (out, labels)
}

/// The value of a numeric table column.
fn column_value(t: &Totals, sort: Sort) -> f64 {
    match sort {
        Sort::Calls => t.calls as f64,
        Sort::Input => t.input as f64,
        Sort::Output => t.output as f64,
        Sort::CacheRead => t.cache_read as f64,
        Sort::CacheWrite => t.cache_write as f64,
        Sort::Hit => t.cache_hit_rate,
        Sort::Reported => t.cost_usd,
        Sort::Estimated => t.est_usd,
        Sort::Name | Sort::Share => 0.0,
    }
}

// ---- Formatting ----

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Digits with thousands separators.
fn commas(digits: &str) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Dollars to the cent with separators; a non-zero amount under a cent reads as "<$0.01".
pub fn usd(v: f64) -> String {
    if v != 0.0 && v.abs() < 0.005 {
        return "<$0.01".to_string();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let s = format!("{:.2}", v.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, "00"));
    format!("{sign}${}.{frac}", commas(int))
}

/// A count or a token total in k, M or B.
pub fn compact(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

fn pct(rate: f64) -> String {
    format!("{:.1}%", rate.clamp(0.0, 1.0) * 100.0)
}

/// UTC calendar fields of epoch milliseconds: (year, month 1-12, day, hour, minute).
fn civil(ms: u64) -> (i64, usize, i64, u64, u64) {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let (h, mi) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m as usize, d, h, mi)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "Oct 12 14:03" in UTC.
fn stamp(ms: u64) -> String {
    let (_, m, d, h, mi) = civil(ms);
    format!("{} {d} {h:02}:{mi:02}", MONTHS[m - 1])
}

/// "Oct 12" in UTC.
fn day_label(ms: u64) -> String {
    let (_, m, d, _, _) = civil(ms);
    format!("{} {d}", MONTHS[m - 1])
}

/// A relative time, as the web's `ago`; `ms` is epoch milliseconds.
fn ago(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let s = now_ms().saturating_sub(ms) / 1000;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// Percent-encodes a query or path value (model ids carry slashes).
pub(crate) fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn per_million(v: Option<f64>) -> String {
    v.map_or("—".to_string(), |p| format!("${p:.2}"))
}

// ---- View ----

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let now = Instant::now();
    let head = ui::view_head("Usage", vec![segmented(&Range::ALL, state.range, Msg::Range)]);
    let mut parts: Vec<Element<'a, Msg>> = vec![prices_line(state), group_tabs(state)];
    match &state.data {
        None => parts.push(match &state.error {
            Some(err) => ui::empty("alert", "Usage unreachable", err),
            None => ui::loading(),
        }),
        Some(d) => {
            if let Some(err) = &state.error {
                parts.push(ui::error_line(err));
            }
            parts.push(tiles(state, d, now));
            parts.push(ui::card("Over time", chart_panel(state, d, now)));
            parts.push(calls_card(state, d));
            parts.push(prices_card(d));
        }
    }
    ui::sheet(head, ui::scroll(column(parts).spacing(22).width(Length::Fill)).height(Length::Fill).into())
}

/// The prices' source and age, with the refresh button.
fn prices_line<'a>(state: &'a State) -> Element<'a, Msg> {
    let note = match state.data.as_ref().and_then(|d| d.head.as_ref()) {
        Some(h) => {
            let names: Vec<String> = h
                .sources
                .iter()
                .filter(|s| s.ok)
                .map(|s| match s.name.as_str() {
                    "openrouter" => "OpenRouter".to_string(),
                    "litellm" => "LiteLLM".to_string(),
                    "opencode" => "OpenCode Zen".to_string(),
                    other => other.to_string(),
                })
                .collect();
            if names.is_empty() {
                "No price list yet".to_string()
            } else {
                format!("Prices from {} · updated {}", names.join(" and "), ago(h.fetched_at_ms))
            }
        }
        None => "Prices not fetched yet".to_string(),
    };
    row![
        ui::faint(note),
        Space::new().width(Length::Fill),
        ui::btn("Refresh", Some("refresh"), ui::Kind::Secondary, (!state.busy).then_some(Msg::RefreshPrices)),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn group_tabs<'a>(state: &'a State) -> Element<'a, Msg> {
    row![ui::label("Group by"), segmented(&GroupBy::ALL, state.group, Msg::Group)].spacing(16).align_y(Alignment::Center).into()
}

/// A segmented control: the current option solid, the others plain.
fn segmented<'a, T: Copy + PartialEq + 'a>(options: &[(T, &str)], current: T, make: fn(T) -> Msg) -> Element<'a, Msg> {
    let mut line = row![].spacing(6);
    for (value, label) in options {
        let kind = if *value == current { ui::Kind::Primary } else { ui::Kind::Secondary };
        line = line.push(ui::btn(label, None, kind, Some(make(*value))));
    }
    line.into()
}

/// The four tallies: spend (with its sparkline), tokens in and out, cache hit rate, calls.
fn tiles<'a>(state: &'a State, d: &'a Data, now: Instant) -> Element<'a, Msg> {
    let c = &state.counters;
    let t = &d.summary.totals;
    let spark: Vec<f64> = d.timeline.points.iter().map(|p| p.totals.usd).collect();
    let spend_note = format!("reported {} · estimated {}", usd(t.cost_usd), usd(t.est_usd));
    let calls_note = if t.calls == 0 { "no calls in this range yet" } else { "in this range" };
    row![
        tile(usd(c.usd.at(now)), "Spend", spend_note, Some(chart::sparkline(spark, crate::theme::palette().signal))),
        tile(
            format!("{} / {}", compact(c.input.at(now)), compact(c.output.at(now))),
            "Tokens in / out",
            format!("cache read {} · write {}", compact(t.cache_read as f64), compact(t.cache_write as f64)),
            None,
        ),
        tile(pct(c.hit.at(now) / 100.0), "Cache hit rate", "of input served from cache".to_string(), None),
        tile(commas(&(c.calls.at(now).round() as u64).to_string()), "Calls", calls_note.to_string(), None),
    ]
    .spacing(12)
    .into()
}

/// A tally tile (the web's `.stat`): the figure, its label, an optional extra, then an italic note.
fn tile<'a>(figure: String, title: &str, note: String, extra: Option<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut stack = column![text(figure).font(VALUE).size(30.0).color(crate::theme::palette().ink), ui::label(title)].spacing(4);
    if let Some(e) = extra {
        stack = stack.push(e);
    }
    stack = stack.push(text(note).font(ITALIC).size(13.0).color(crate::theme::palette().ink_2));
    container(stack)
        .padding(Padding { top: 14.0, right: 16.0, bottom: 16.0, left: 16.0 })
        .width(Length::FillPortion(1))
        .height(Length::Fixed(156.0))
        .style(|_: &Theme| container::Style { border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() }, ..Default::default() })
        .into()
}

/// The main panel: the chart (with its legend and calls per bucket) or the table.
fn chart_panel<'a>(state: &'a State, d: &'a Data, now: Instant) -> Element<'a, Msg> {
    let toolbar: Element<'a, Msg> = match state.view {
        View::Chart => row![segmented(&View::ALL, state.view, Msg::View), segmented(&Unit::ALL, state.unit, Msg::Unit)].spacing(16).into(),
        View::Table => row![segmented(&View::ALL, state.view, Msg::View)].into(),
    };
    if state.view == View::Chart && d.summary.totals.calls == 0 {
        return column![
            toolbar,
            ui::empty("zap", "No model calls in this window", "Every call an agent makes is recorded here with its tokens and cost. Nothing has been called in this window yet."),
        ]
        .spacing(14)
        .into();
    }
    match state.view {
        View::Chart => {
            let series = state.chart.current(now);
            let format = state.unit.format();
            let legend = series.iter().fold(row![].spacing(18), |line, s| {
                let total: f64 = s.values.iter().sum();
                let name = if s.key == OTHER { "Other".to_string() } else { state.group.display(&s.key, &d.agents) };
                let color = s.color;
                let swatch = container(Space::new().width(10.0).height(10.0)).style(move |_: &Theme| container::Style {
                    background: Some(color.into()),
                    ..Default::default()
                });
                line.push(row![swatch, text(name).font(MONO).size(11.0).color(crate::theme::palette().ink_2), text(format(total)).font(MONO).size(11.0).color(crate::theme::palette().ink)].spacing(6).align_y(Alignment::Center))
            });
            let calls: Vec<f64> = d.timeline.points.iter().map(|p| p.totals.calls as f64).collect();
            column![
                toolbar,
                chart::stacked_area(series, state.labels.clone(), format, "No calls in this range"),
                legend,
                ui::faint("Calls per bucket"),
                chart::bars(calls, crate::theme::palette().ink_2),
            ]
            .spacing(14)
            .into()
        }
        View::Table => column![toolbar, table(state, d)].spacing(14).into(),
    }
}

/// A sortable header cell: the active column in ink.
fn sort_head<'a>(label: &str, sort: Sort, state: &State, portion: u16) -> Element<'a, Msg> {
    let active = state.sort == sort;
    let arrow = if active { if state.asc { " ^" } else { " v" } } else { "" };
    let content = button(text(format!("{}{arrow}", label.to_uppercase())).font(MONO).size(11.0).color(if active { crate::theme::palette().ink } else { crate::theme::palette().ink_3 }))
        .style(link)
        .padding(0)
        .on_press(Msg::Sort(sort));
    cell(portion, false, content)
}

fn link(_: &Theme, _: button::Status) -> button::Style {
    button::Style { background: None, text_color: crate::theme::palette().ink, ..Default::default() }
}

/// The table: one row per group, sorted, with a share bar, and a total row.
fn table<'a>(state: &'a State, d: &'a Data) -> Element<'a, Msg> {
    let groups = state.group.rows(&d.summary);
    let total_value: f64 = groups.iter().map(|g| state.unit.value(&g.totals)).sum();
    let mut rows: Vec<&Group> = groups.iter().collect();
    rows.sort_by(|a, b| {
        let ord = match state.sort {
            Sort::Name => a.key.cmp(&b.key),
            Sort::Share => state.unit.value(&a.totals).total_cmp(&state.unit.value(&b.totals)),
            other => column_value(&a.totals, other).total_cmp(&column_value(&b.totals, other)),
        };
        if state.asc {
            ord
        } else {
            ord.reverse()
        }
    });
    let head = row![
        sort_head("Name", Sort::Name, state, 5),
        sort_head("Calls", Sort::Calls, state, 2),
        sort_head("In", Sort::Input, state, 2),
        sort_head("Out", Sort::Output, state, 2),
        sort_head("Cache read", Sort::CacheRead, state, 2),
        sort_head("Cache write", Sort::CacheWrite, state, 2),
        sort_head("Hit", Sort::Hit, state, 2),
        sort_head("Reported", Sort::Reported, state, 2),
        sort_head("Estimated", Sort::Estimated, state, 2),
        sort_head("Share", Sort::Share, state, 4),
    ].spacing(10);
    let mut lines = column![head].spacing(8);
    if rows.is_empty() {
        lines = lines.push(ui::faint("Nothing to group yet."));
    }
    for g in rows {
        let share = if total_value > 0.0 { state.unit.value(&g.totals) / total_value } else { 0.0 };
        let trains = matches!(state.group, GroupBy::Model) && d.trains.contains(&g.key);
        lines = lines.push(table_row(state.group.display(&g.key, &d.agents), trains, &g.totals, Some(share)));
    }
    if !groups.is_empty() {
        lines = lines.push(table_row("Total".to_string(), false, &d.summary.totals, None));
    }
    lines.into()
}

/// `content`, with the contributor warning badge after it when `trains` is set.
fn with_badge<'a>(content: impl Into<Element<'a, Msg>>, trains: bool) -> Element<'a, Msg> {
    let content = content.into();
    if trains {
        row![content, ui::trains_badge()].spacing(8).align_y(Alignment::Center).into()
    } else {
        content
    }
}

fn table_row<'a>(name: String, trains: bool, t: &Totals, share: Option<f64>) -> Element<'a, Msg> {
    let bar: Element<'a, Msg> = match share {
        Some(s) => {
            let pct = (s.clamp(0.0, 1.0) * 100.0).round() as u16;
            row![
                container(Space::new().width(Length::Fill).height(6.0))
                    .width(Length::FillPortion(pct.max(1)))
                    .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().signal.into()), ..Default::default() }),
                container(Space::new().width(Length::Fill).height(6.0))
                    .width(Length::FillPortion((100 - pct).max(1)))
                    .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().rule_faint.into()), ..Default::default() }),
                text(format!("{:.0}%", s * 100.0)).font(MONO).size(11.0).color(crate::theme::palette().ink_2).width(Length::Fixed(40.0)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        }
        None => text("").into(),
    };
    let num = |v: String| cell(2, true, text(v).font(MONO).size(12.0).color(crate::theme::palette().ink_2));
    row![
        cell(5, false, with_badge(text(name).font(MONO).size(12.0).color(crate::theme::palette().ink), trains)),
        num(commas(&t.calls.to_string())),
        num(compact(t.input as f64)),
        num(compact(t.output as f64)),
        num(compact(t.cache_read as f64)),
        num(compact(t.cache_write as f64)),
        num(pct(t.cache_hit_rate)),
        num(usd(t.cost_usd)),
        num(usd(t.est_usd)),
        cell(4, false, bar),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn cell<'a>(portion: u16, right: bool, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    let c = container(content).width(Length::FillPortion(portion));
    if right {
        c.align_x(alignment::Horizontal::Right).into()
    } else {
        c.into()
    }
}

/// The calls log: filters, a header, the rows, and "Load older" while the hub has more.
fn calls_card<'a>(state: &'a State, d: &'a Data) -> Element<'a, Msg> {
    let mut models = vec![Choice { value: None, label: "All models".to_string() }];
    models.extend(d.summary.by_model.iter().map(|g| Choice { value: Some(g.key.clone()), label: g.key.clone() }));
    let model_sel = models.iter().find(|c| c.value == state.model_filter).cloned();
    let mut agents = vec![Choice { value: None, label: "All agents".to_string() }];
    agents.extend(d.agents.iter().map(|(cid, name)| Choice { value: Some(*cid), label: name.clone() }));
    let agent_sel = agents.iter().find(|c| c.value == state.agent_filter).cloned();
    let filters = row![
        pick_list(models, model_sel, Msg::ModelFilter).width(Length::Fixed(240.0)),
        pick_list(agents, agent_sel, Msg::AgentFilter).width(Length::Fixed(220.0)),
    ]
    .spacing(12);

    let mut lines = column![row![
        cell(3, false, ui::faint("When (UTC)")),
        cell(3, false, ui::faint("Agent")),
        cell(2, false, ui::faint("Role")),
        cell(4, false, ui::faint("Model")),
        cell(2, true, ui::faint("In")),
        cell(2, true, ui::faint("Out")),
        cell(2, true, ui::faint("Cache read")),
        cell(2, true, ui::faint("Cost")),
        cell(3, false, ui::faint("Via")),
    ].spacing(10)]
    .spacing(6);
    let txt = |s: String| text(s).font(MONO).size(12.0).color(crate::theme::palette().ink_2);
    for c in &d.calls {
        let agent = d.agents.get(&c.cid).cloned().unwrap_or_else(|| format!("#{}", c.cid));
        let cost = match c.cost_usd {
            Some(v) => usd(v),
            None => format!("{} est", usd(c.est_usd)),
        };
        let via = match &c.task {
            Some(task) => format!("{} · {task}", c.via),
            None => c.via.clone(),
        };
        lines = lines.push(row![
            cell(3, false, txt(stamp(c.at_ms))),
            cell(3, false, txt(agent)),
            cell(2, false, txt(c.role.clone())),
            cell(4, false, with_badge(txt(c.model.clone()), d.trains.contains(&c.model))),
            cell(2, true, txt(compact(c.input as f64))),
            cell(2, true, txt(compact(c.output as f64))),
            cell(2, true, txt(compact(c.cache_read as f64))),
            cell(2, true, txt(cost)),
            cell(3, false, txt(via)),
        ].spacing(10));
    }
    if d.calls.is_empty() {
        let filtered = state.model_filter.is_some() || state.agent_filter.is_some();
        let sentence = if filtered { "Clear the filters to see every call." } else { "Calls appear here as agents work." };
        lines = lines.push(ui::empty("zap", "No calls match", sentence));
        if filtered {
            lines = lines.push(ui::btn("Clear filters", None, ui::Kind::Ghost, Some(Msg::ClearFilters)));
        }
    }
    let mut footer = row![ui::faint(format!("{} of {} calls", d.calls.len(), commas(&d.calls_total.to_string())))].align_y(Alignment::Center);
    footer = footer.push(Space::new().width(Length::Fill));
    if d.calls_next.is_some() {
        footer = footer.push(ui::btn("Load older", Some("chevronDown"), ui::Kind::Secondary, (!state.calls_busy).then_some(Msg::LoadOlder)));
    }
    let body = column![filters, lines, footer].spacing(14);
    ui::card("Calls", body.into())
}

/// The prices in use: each model with where it is used, its calls, its prices per million tokens and its source.
fn prices_card<'a>(d: &'a Data) -> Element<'a, Msg> {
    let uses = settings_uses(&d.settings);
    let models = models_in_use(&d.summary, &d.settings);
    let head = row![
        cell(5, false, ui::faint("Model")),
        cell(3, false, ui::faint("Used by")),
        cell(2, true, ui::faint("Calls")),
        cell(2, true, ui::faint("Input")),
        cell(2, true, ui::faint("Output")),
        cell(2, true, ui::faint("Cache read")),
        cell(2, true, ui::faint("Cache write")),
        cell(3, false, ui::faint("Source")),
    ].spacing(10);
    let mut lines = column![head].spacing(6);
    if models.is_empty() {
        lines = lines.push(ui::empty("models", "No models in use", "Models appear here once a model is called or set as a default."));
    }
    let txt = |s: String| text(s).font(MONO).size(12.0).color(crate::theme::palette().ink_2);
    for model in &models {
        let calls = d.summary.by_model.iter().find(|g| &g.key == model).map_or(0, |g| g.totals.calls);
        let used = uses.get(model).map(|v| v.join(", ")).unwrap_or_default();
        let (input, output, cache_read, cache_write, source, flag) = match d.prices.get(model) {
            Some(Some(p)) => (per_million(Some(p.input)), per_million(Some(p.output)), per_million(p.cache_read), per_million(p.cache_write), p.source.clone(), None),
            Some(None) => ("—".into(), "—".into(), "—".into(), "—".into(), String::new(), Some(Element::from(row![ui::badge::<Msg>("Unpriced", ui::Tone::Warn), ui::link("set a price under Models", Msg::Go(Id::Models), false)].spacing(6).align_y(Alignment::Center)))),
            None =>("…".into(), "…".into(), "…".into(), "…".into(), String::new(), None),
        };
        let flagged = flag.or_else(|| d.summary.unpriced.contains(model).then(|| ui::badge::<Msg>("Unpriced", ui::Tone::Warn)));
        let source_cell: Element<'_, Msg> = match flagged {
            Some(badge) => row![txt(source), badge].spacing(8).align_y(Alignment::Center).into(),
            None => txt(source).into(),
        };
        lines = lines.push(
            row![
                cell(5, false, text(model.clone()).font(MONO).size(12.0).color(crate::theme::palette().ink)),
                cell(3, false, txt(used)),
                cell(2, true, txt(commas(&calls.to_string()))),
                cell(2, true, txt(input)),
                cell(2, true, txt(output)),
                cell(2, true, txt(cache_read)),
                cell(2, true, txt(cache_write)),
                cell(3, false, source_cell),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    ui::card("Prices in use · USD per million tokens", lines.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(usd: f64, calls: u64) -> Totals {
        Totals { usd, calls, input: 10, output: 5, ..Default::default() }
    }

    fn point(at_ms: u64, groups: &[(&str, f64)]) -> Point {
        let mut map = BTreeMap::new();
        let mut total = Totals::default();
        for (k, v) in groups {
            total.usd += v;
            total.input += 10;
            map.insert(k.to_string(), t(*v, 1));
        }
        total.calls = groups.len() as u64;
        Point { at_ms, groups: map, totals: total }
    }

    #[test]
    fn stamp_and_civil_are_utc() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(stamp(1_000_000_000_000), "Sep 9 01:46");
        assert_eq!(day_label(1_000_000_000_000), "Sep 9");
    }

    #[test]
    fn money_and_counts_read_as_the_console_does() {
        assert_eq!(usd(1234.5), "$1,234.50");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(0.004), "<$0.01");
        assert_eq!(usd(-12.0), "-$12.00");
        assert_eq!(compact(950.0), "950");
        assert_eq!(compact(1_250.0), "1.2k");
        assert_eq!(compact(2_500_000.0), "2.5M");
        assert_eq!(commas("1234567"), "1,234,567");
        assert_eq!(pct(0.4567), "45.7%");
    }

    #[test]
    fn enc_escapes_model_ids() {
        assert_eq!(enc("anthropic/claude-haiku-5-5"), "anthropic%2Fclaude-haiku-5-5");
        assert_eq!(calls_path(50, Some(9), Some("a/b"), Some(3)), "/v1/usage/calls?limit=50&before=9&model=a%2Fb&cid=3");
    }

    #[test]
    fn chart_series_ranks_the_top_groups_and_sums_the_rest_into_other() {
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut points = Vec::new();
        for i in 0..3u64 {
            let groups: Vec<(&str, f64)> = names.iter().enumerate().map(|(k, n)| (*n, (k as f64 + 1.0) * (i as f64 + 1.0))).collect();
            points.push(point(i * 3_600_000, &groups));
        }
        let tl = Timeline { bucket_ms: 3_600_000, points };
        let (series, labels) = chart_series(&tl, Unit::Usd);
        assert_eq!(labels.len(), 3);
        assert_eq!(series.len(), TOP + 1);
        assert_eq!(series[0].key, "h");
        assert_eq!(series[0].values, vec![8.0, 16.0, 24.0]);
        assert_eq!(series[TOP].key, OTHER);
        // The other band is the two lowest groups (a and b) at each point.
        assert_eq!(series[TOP].values, vec![3.0, 6.0, 9.0]);
    }

    #[test]
    fn chart_series_is_empty_without_groups_and_other_is_only_kept_when_non_zero() {
        let tl = Timeline { bucket_ms: DAY_MS, points: vec![point(0, &[]), point(DAY_MS, &[])] };
        let (series, labels) = chart_series(&tl, Unit::Tokens);
        assert!(series.is_empty());
        assert_eq!(labels, vec!["Jan 1".to_string(), "Jan 2".to_string()]);
    }

    #[test]
    fn tween_counts_up_over_the_animation_and_settles() {
        let start = Instant::now();
        let mut tw = Tween::default();
        tw.aim(100.0, start);
        assert!(tw.running(start));
        let mid = tw.at(start + Duration::from_millis(300));
        assert!(mid > 0.0 && mid < 100.0);
        assert_eq!(tw.at(start + ANIM), 100.0);
        assert!(!tw.running(start + ANIM));
    }

    #[test]
    fn summary_parses_the_hub_shape_and_ignores_extra_fields() {
        let json = r#"{"from_ms":1,"to_ms":2,"totals":{"calls":2,"input":30,"output":4,"cache_read":10,"cache_write":0,"cost_usd":0.5,"est_usd":0.25,"usd":0.75,"cache_hit_rate":0.25},
            "by_model":[{"key":"m/x","totals":{"calls":2,"usd":0.75}}],"by_agent":[],"by_role":[],"by_provider":[],"unpriced":["m/y"]}"#;
        let s: Summary = serde_json::from_str(json).unwrap();
        assert_eq!(s.totals.calls, 2);
        assert_eq!(s.by_model[0].key, "m/x");
        assert_eq!(s.unpriced, vec!["m/y".to_string()]);
    }

    #[test]
    fn settings_uses_lists_where_each_model_is_used() {
        let mut roles = BTreeMap::new();
        roles.insert("architect".to_string(), "m/a".to_string());
        let s = SettingsView { default_model: "m/a".into(), orchestrator_model: "m/o".into(), role_models: roles };
        let uses = settings_uses(&s);
        assert_eq!(uses["m/a"], vec!["default".to_string(), "role architect".to_string()]);
        assert_eq!(uses["m/o"], vec!["orchestrator".to_string()]);
    }

    #[test]
    fn the_presets_flag_contributor_models_by_provider_and_model() {
        let kinds = vec![
            json!({"id": "meta", "options": {"trains_on_data": ["muse-spark-1.3-contributor"]}}),
            json!({"id": "deepseek", "options": {"reasoning_effort": true}}),
        ];
        let flagged = trains_models(&kinds);
        assert!(flagged.contains("meta/muse-spark-1.3-contributor"));
        assert!(!flagged.contains("meta/muse-spark-1.3"));
        assert_eq!(flagged.len(), 1);
    }
}
