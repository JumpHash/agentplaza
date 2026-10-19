//! The panel model: what the hub composes for Metrics and what agents build as artifacts. JSON field names are snake_case; enums are tagged with `"type"`.
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
    /// Buttons in the panel's head ("Refresh prices").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<PanelAction>,
    pub body: Body,
}

/// A button in a panel's head: the console posts to `post` (no body), then loads the view again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelAction {
    pub label: String,
    /// The hub route, e.g. `/v1/prices/refresh`.
    pub post: String,
    /// The toast after it succeeds ("Prices refreshed").
    #[serde(default)]
    pub done: String,
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
    /// A picture an agent made or found: a blob in the artifact's store. Alt text is required.
    Image(ImageBody),
    /// Nodes and edges: layered (apz-graph's layout) or force-directed.
    Graph(GraphBody),
    /// A columnar table with paging, sorting and filtering; rows inline up to the cap, else in a blob.
    Dataset(DatasetBody),
    /// Markdown, drawn by the console's Markdown renderer.
    Document {
        markdown: String,
    },
    /// Source code, highlighted with the theme's `syn-*` tokens.
    Code(CodeBody),
    /// A small web page or one React component that runs live in a sandboxed frame.
    Html(HtmlBody),
}

/// A file in an artifact's store (bytes in the deliverables store: Garage or the hub's disk).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// The blob's id within its artifact (`b1`, `b2` …), from `artifact_attach_data`.
    pub id: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageBody {
    pub blob: BlobRef,
    pub alt: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphLayout {
    /// Columns of cards with routed links (directed, acyclic or nearly so).
    #[default]
    Layered,
    /// A force-directed layout, for undirected graphs.
    Force,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    /// Nodes with the same group are drawn together and share a colour when no tone is set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphBody {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    #[serde(default)]
    pub layout: GraphLayout,
    #[serde(default = "yes")]
    pub directed: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    #[default]
    Text,
    Number,
    /// Milliseconds since the epoch, or an ISO 8601 string.
    Time,
    Bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasetColumn {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: ColumnKind,
    /// How numbers are formatted (the Metrics units); plain when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasetBody {
    pub columns: Vec<DatasetColumn>,
    /// Rows inline, each a list of values in column order. Empty when the rows are in `blob`.
    #[serde(default)]
    pub rows: Vec<Vec<serde_json::Value>>,
    /// CSV (with a header row) or JSON Lines, for more rows than fit inline. Consoles page it through the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobRef>,
    /// The row count, inline or in the blob.
    #[serde(default)]
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeBody {
    /// A language name as Markdown fences give it: `rust`, `ts`, `python` …
    pub language: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filename: String,
}

/// One file of an HTML artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HtmlFile {
    /// A relative path: `index.html`, `app.js`, `App.tsx`, `data/points.csv`.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HtmlBody {
    /// The file that runs: an `.html` page, or one component (`.jsx`, `.tsx`) whose default export is rendered.
    pub entry: String,
    pub files: Vec<HtmlFile>,
    /// The frame's starting height in px, until the page reports its own.
    #[serde(default = "html_height")]
    pub height: u32,
    /// Set by the hub: the vendored libraries the artifact uses (`apz_view::libs`), in load order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libs: Vec<String>,
    /// Set by the hub: a still of the first render, shown until the frame runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still: Option<BlobRef>,
}

fn html_height() -> u32 {
    360
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
    /// When older rows may exist: the hub route (GET) that answers the next page as a `Table`, whose rows go under
    /// these and whose own `more` replaces this one. The consoles show it as "Load older".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<String>,
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
