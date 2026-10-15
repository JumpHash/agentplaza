//! The types of a composed Metrics view. JSON field names are snake_case; enums are tagged with `"type"`.
//! Colours are [`Tone`] tokens, which each console maps to its own palette (the web's `--chart-1` …, the
//! desktop's `theme::palette()`), so both follow the theme.

use apz_metrics::catalog::Unit;
use serde::{Deserialize, Serialize};

/// A colour token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    #[serde(rename = "chart-1")]
    C1,
    #[serde(rename = "chart-2")]
    C2,
    #[serde(rename = "chart-3")]
    C3,
    #[serde(rename = "chart-4")]
    C4,
    #[serde(rename = "chart-5")]
    C5,
    #[serde(rename = "chart-6")]
    C6,
    #[serde(rename = "chart-7")]
    C7,
    #[serde(rename = "chart-8")]
    C8,
    #[serde(rename = "chart-other")]
    Other,
    #[serde(rename = "ink-3")]
    Ink3,
    #[serde(rename = "rule")]
    Rule,
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "warn")]
    Warn,
    #[serde(rename = "crit")]
    Crit,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "signal")]
    Signal,
}

impl Tone {
    /// The n-th chart colour (wrapping after 8).
    pub fn chart(n: usize) -> Tone {
        [
            Tone::C1,
            Tone::C2,
            Tone::C3,
            Tone::C4,
            Tone::C5,
            Tone::C6,
            Tone::C7,
            Tone::C8,
        ][n % 8]
    }
}

/// A status: drives dots, pills, flag borders and coloured numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Crit,
    Info,
    #[default]
    Mute,
}

/// What the client asks for: `GET /v1/metrics/view?tab=…&scope=…&range=…&shift=…&zoom=a,b&compare=…&group=…
/// &trace=…&filter=…&width=…&fixture=1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewQuery {
    pub tab: String,
    pub scope: String,
    /// `15m`, `1h`, `6h`, `24h`, `7d`, `30d`, `90d`.
    pub range: String,
    /// Whole ranges back from now (0 = ending now; −1 = the range before).
    #[serde(default)]
    pub shift: i32,
    /// Zoomed span as fractions of the range, when zoomed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<(f64, f64)>,
    /// `prev`, `week`, `none`, or `scope:<id>`.
    #[serde(default = "prev")]
    pub compare: String,
    /// `provider`, `model`, `role`, `run`, `agent`, `engine`.
    #[serde(default = "provider")]
    pub group: String,
    /// The selected trace on the Traces tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
    /// The Traces filter: `all`, `error`, `slow`, `sampled`, `open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// The chart width in pixels, for the tier choice and downsampling.
    #[serde(default = "width")]
    pub width: u32,
    /// The console's offset from UTC in minutes, for the times written into tables ("14:20").
    #[serde(default)]
    pub tz: i32,
    /// Compose from the built-in fixture instead of recorded data (design review, screenshots).
    #[serde(default)]
    pub fixture: bool,
}

impl ViewQuery {
    /// A query for a tab and scope with every other field at its default.
    pub fn new(tab: &str, scope: &str, range: &str) -> ViewQuery {
        ViewQuery {
            tab: tab.into(),
            scope: scope.into(),
            range: range.into(),
            shift: 0,
            zoom: None,
            compare: prev(),
            group: provider(),
            trace: None,
            filter: None,
            width: width(),
            tz: 0,
            fixture: false,
        }
    }
}

fn prev() -> String {
    "prev".into()
}
fn provider() -> String {
    "provider".into()
}
fn width() -> u32 {
    900
}

/// The ranges, with their point counts and steps.
pub const RANGES: &[(&str, u64)] = &[
    ("15m", 15 * 60_000),
    ("1h", 3_600_000),
    ("6h", 6 * 3_600_000),
    ("24h", 24 * 3_600_000),
    ("7d", 7 * 86_400_000),
    ("30d", 30 * 86_400_000),
    ("90d", 90 * 86_400_000),
];

/// A composed tab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub tab: String,
    pub scope: ScopeInfo,
    /// The tabs that apply to this scope, with badges.
    pub tabs: Vec<TabInfo>,
    pub panels: Vec<Panel>,
    /// The time window shown.
    pub from_ms: u64,
    pub to_ms: u64,
    pub step_ms: u64,
    /// "1-minute aggregates".
    pub tier: String,
    /// Points per series after downsampling.
    pub points: u32,
    /// The collection level in force ("standard"), and whether the governor stepped it down.
    pub level: String,
    #[serde(default)]
    pub stepped_down: bool,
    /// Whether live updates apply (range 15m or 1h, not shifted).
    pub live: bool,
    /// Firing alerts, for the sidebar badge.
    pub firing: u32,
    /// The label of the compared thing ("previous", "last week", "n2 · rack-b"), empty when not comparing.
    #[serde(default)]
    pub compare_label: String,
    /// Annotations in the window, drawn on every time chart.
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    /// True when composed from the built-in fixture.
    #[serde(default)]
    pub fixture: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScopeInfo {
    pub id: String,
    pub kind: String,
    pub label: String,
    /// Global first, the scope itself last: `(label, id)`.
    pub path: Vec<(String, String)>,
    /// Same-kind scopes under the same parent, the scope included: `(label, id)`.
    pub siblings: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    pub key: String,
    pub label: String,
    /// "Health", "Models", "Fleet", "Plan" or "".
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<u32>,
}

/// One panel. `span` is its width in twelfths (12, 8, 6, 4, 3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    /// Stable within its tab (`spend-by-group`, `ttft`); pins refer to it.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub sub: String,
    /// Small text at the right of the title ("$38.2 total", "level: standard").
    #[serde(default)]
    pub right: String,
    pub span: u8,
    /// The catalog metric behind it, for "About this metric".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    /// Updated by live points.
    #[serde(default)]
    pub live: bool,
    /// Set when the panel is a pin on My dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<PinRef>,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinRef {
    pub tab: String,
    pub scope: String,
    pub panel: String,
    /// "Overview · n1 · rack-a".
    pub from: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Body {
    Health {
        items: Vec<HealthItem>,
    },
    Tiles {
        tiles: Vec<Tile>,
    },
    Chart(Chart),
    Heatmap(Heatmap),
    Treemap {
        unit: Unit,
        items: Vec<TreeItem>,
    },
    Gauges {
        gauges: Vec<GaugeItem>,
    },
    Table(Table),
    Flags {
        flags: Vec<Flag>,
    },
    Bars {
        rows: Vec<BarRow>,
        #[serde(default)]
        note: String,
    },
    Split {
        rows: Vec<SplitRow>,
        legend: Vec<LegendItem>,
    },
    Stats {
        stats: Vec<StatLine>,
    },
    Swimlanes(Swimlanes),
    Alerts {
        rows: Vec<AlertRow>,
    },
    Rules {
        rules: Vec<RuleRow>,
    },
    Traces(TracesBody),
    Events {
        kinds: Vec<String>,
        rows: Vec<EventRow>,
    },
    Empty {
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthItem {
    pub area: String,
    pub status: Status,
    pub text: String,
    pub tab: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub label: String,
    /// Formatted.
    pub value: String,
    /// Change against the comparison, in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_pct: Option<f64>,
    /// A rise is bad (spend, latency) rather than good (cache hit).
    #[serde(default)]
    pub up_bad: bool,
    /// Shown instead of the change when there is none.
    #[serde(default)]
    pub note: String,
    pub spark: Vec<f64>,
    pub tone: Tone,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum XAxis {
    /// Points at `start_ms + i·step_ms`.
    Time { start_ms: u64, step_ms: u64 },
    /// One label per point ("call 12", "Oct 16").
    Labels { labels: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chart {
    pub unit: Unit,
    pub x: XAxis,
    pub series: Vec<Series>,
    #[serde(default)]
    pub stacked: bool,
    #[serde(default)]
    pub area: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<Band>,
    /// Comparison overlays, drawn dashed in the series' tone.
    #[serde(default)]
    pub ghosts: Vec<Series>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
    /// Pixel height at comfortable density (compact draws 0.8×).
    pub height: u16,
    /// Draw the view's annotations on this chart (time charts only).
    #[serde(default)]
    pub annotate: bool,
    /// Clicking a point opens this tab instead of "what changed" (charts whose x is not time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_tab: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub name: String,
    pub tone: Tone,
    #[serde(default)]
    pub dash: bool,
    /// `null` is a gap, drawn as a shaded band with `gap_reason`, never as zero.
    pub values: Vec<Option<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_reason: Option<String>,
    /// Clicking the series' legend entry narrows to this scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub name: String,
    pub tone: Tone,
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub index: usize,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub value: f64,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub t_ms: u64,
    pub kind: String,
    pub label: String,
}

/// Counts per (time column, duration bucket), log-scale buckets from `lo` to `hi` in `unit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heatmap {
    pub start_ms: u64,
    pub step_ms: u64,
    pub unit: Unit,
    pub lo: f64,
    pub hi: f64,
    /// `counts[col][row]`, row 0 the lowest bucket.
    pub counts: Vec<Vec<f64>>,
    pub height: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeItem {
    pub name: String,
    pub value: f64,
    pub tone: Tone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GaugeItem {
    /// 0..=1.
    pub frac: f64,
    /// Big text in the arc ("91%").
    pub label: String,
    pub sub: String,
    /// The arc's status (ok under 75 %, warn under 90 %, crit above, unless the composer says otherwise).
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// Shown when there are no rows.
    #[serde(default)]
    pub empty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub head: String,
    /// Right-aligned, tabular figures.
    #[serde(default)]
    pub num: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Clicking the row narrows to this scope …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// … and/or opens this tab …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// … with this trace selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
    #[serde(default)]
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Cell {
    /// Plain text.
    Text {
        text: String,
    },
    /// Monospace (ids, times, routes, tools).
    Mono {
        text: String,
    },
    /// A name with a muted monospace id after it ("ada 4417").
    Named {
        name: String,
        id: String,
    },
    /// A formatted number.
    Num {
        text: String,
    },
    /// Text coloured by status.
    Status {
        text: String,
        status: Status,
    },
    /// A bordered pill.
    Pill {
        text: String,
        status: Status,
    },
    /// Muted text.
    Muted {
        text: String,
    },
    Spark {
        values: Vec<f64>,
        tone: Tone,
    },
    /// A small stacked bar: (share in %, tone, label).
    Bar {
        parts: Vec<(f64, Tone, String)>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub when: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarRow {
    pub label: String,
    /// Formatted, right of the label.
    pub value: String,
    /// 0..=1.
    pub frac: f64,
    pub status: Status,
    /// A muted line under the bar.
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitRow {
    pub label: String,
    /// (value, tone, label) — widths are value over the largest row total.
    pub parts: Vec<(f64, Tone, String)>,
    pub total: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegendItem {
    pub label: String,
    pub tone: Tone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatLine {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Swimlanes {
    pub start_ms: u64,
    pub end_ms: u64,
    pub lanes: Vec<Lane>,
    pub legend: Vec<LegendItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub name: String,
    pub scope: Option<String>,
    pub segs: Vec<Seg>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seg {
    pub start_ms: u64,
    pub end_ms: u64,
    /// running, paused, asleep, booting, waking.
    pub state: String,
    pub tone: Tone,
    /// Hover text.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRow {
    pub rule: String,
    pub name: String,
    /// The subject (scope id) that fires.
    pub scope: String,
    pub scope_label: String,
    /// "p95(ttft) > 2 s".
    pub cond: String,
    /// "10 min".
    pub for_text: String,
    pub since: String,
    pub now: String,
    pub severity: Status,
    pub spark: Vec<f64>,
    pub tab: String,
    #[serde(default)]
    pub acked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleRow {
    pub id: String,
    pub on: bool,
    pub name: String,
    pub scope_label: String,
    pub cond: String,
    pub for_text: String,
    pub severity: Status,
    /// "ok", "firing", "muted", "off".
    pub state: String,
    pub last: String,
    #[serde(default)]
    pub starter: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TracesBody {
    pub filter: String,
    pub list: Vec<TraceRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<TraceDetail>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceRow {
    pub id: String,
    pub time: String,
    pub agent: String,
    pub total: String,
    /// error, slow, sampled, open.
    pub kept: String,
    pub kept_text: String,
    pub error: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceDetail {
    pub id: String,
    pub title: String,
    pub sub: String,
    #[serde(default)]
    pub error: bool,
    pub total_ms: f64,
    pub spans: Vec<SpanRow>,
    pub legend: Vec<LegendItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpanRow {
    pub name: String,
    pub depth: u8,
    pub tone: Tone,
    /// Offset from the turn start.
    pub start_ms: f64,
    pub dur_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<f64>,
    #[serde(default)]
    pub error: bool,
    /// Shown when the span is clicked.
    pub attrs: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRow {
    pub t_ms: u64,
    pub time: String,
    pub kind: String,
    pub status: Status,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub detail: String,
}

/// `GET /v1/metrics/around?scope=…&t=…`: what changed in ± 10 minutes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Around {
    /// "14:20".
    pub label: String,
    pub sub: String,
    pub events: Vec<EventRow>,
    pub movers: Vec<MoverRow>,
    pub traces: TraceCount,
    /// The ± 30 min zoom, as fractions of the current range.
    pub zoom: (f64, f64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoverRow {
    pub name: String,
    /// "+63%", "3 → 9", "2.1×".
    pub change: String,
    pub status: Status,
    pub spark: Vec<f64>,
    pub tab: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceCount {
    pub total: u32,
    pub errors: u32,
    pub slow: u32,
}

/// One node of `GET /v1/metrics/scopes`, in tree order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopeNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    /// The tree group it sits under: "", "Nodes", "Agents · by role", "Providers".
    pub group: String,
    pub depth: u8,
    /// A small tag at the right: node, run, vm, qemu, sandbox, group.
    #[serde(default)]
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

/// `GET`/`PUT /v1/metrics/dashboards`: what the owner keeps, shared by web and desktop.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// Pinned scope ids, in pin order.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Pinned panels.
    #[serde(default)]
    pub pins: Vec<Pin>,
    /// Recently visited scope ids, newest first (at most 6).
    #[serde(default)]
    pub recent: Vec<String>,
    /// True until the owner first changes the pins (the starter pins are shown).
    #[serde(default = "yes")]
    pub starter: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub tab: String,
    pub scope: String,
    pub panel: String,
}

/// The starter pins shown on My dashboard until the owner pins something.
pub fn starter_pins() -> Vec<Pin> {
    vec![
        Pin {
            tab: "overview".into(),
            scope: "global".into(),
            panel: "ttft".into(),
        },
        Pin {
            tab: "context".into(),
            scope: "global".into(),
            panel: "nearest".into(),
        },
        Pin {
            tab: "compute".into(),
            scope: "global".into(),
            panel: "cpu".into(),
        },
        Pin {
            tab: "capacity".into(),
            scope: "global".into(),
            panel: "forecast".into(),
        },
    ]
}

/// The live push (`metrics` event on `/v1/ws`): the live panels of a watched scope and tab, recomposed every 2 s
/// while a console holds a lease on it (`POST /v1/metrics/live`). Consoles replace those panels' bodies and redraw
/// them at most once a second.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveFrame {
    pub scope: String,
    pub tab: String,
    pub group: String,
    pub range: String,
    pub t_ms: u64,
    pub panels: Vec<Panel>,
}

/// A console's lease on live frames for a scope and tab, renewed every 10 s while the view is visible and live;
/// it lapses 15 s after the last renewal.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveLease {
    pub scope: String,
    pub tab: String,
    pub group: String,
    /// `15m` or `1h`.
    pub range: String,
}

/// A catalog entry as `GET /v1/metrics/catalog` serves it (the owned form of `apz_metrics::MetricDef`), for
/// "About this metric".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricInfo {
    pub id: String,
    pub title: String,
    pub unit: Unit,
    pub kind: String,
    pub level: String,
    pub entity: String,
    pub labels: Vec<String>,
    pub source: String,
    pub cost: String,
    pub description: String,
}
