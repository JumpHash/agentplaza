//! The Metrics view's own types: the query, the composed tab, scopes, dashboards and live frames. The panel model
//! they carry ([`Panel`], [`Body`], [`Tone`] …) is `apz-view`'s, re-exported here.

use apz_metrics::catalog::Unit;
use serde::{Deserialize, Serialize};

pub use apz_view::model::*;

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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "metric_dashboards")]
pub struct Saved {
    /// Pinned scope ids, in pin order.
    #[serde(default)]
    #[table(list)]
    pub scopes: Vec<String>,
    /// Pinned panels.
    #[serde(default)]
    #[table(child)]
    pub pins: Vec<Pin>,
    /// Recently visited scope ids, newest first (at most 6).
    #[serde(default)]
    #[table(list)]
    pub recent: Vec<String>,
    /// True until the owner first changes the pins (the starter pins are shown).
    #[serde(default = "yes")]
    pub starter: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
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
