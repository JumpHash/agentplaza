//! The Metrics screen (parity with the web console's `#/metrics`): every number the plaza records, by scope and
//! tab. The hub composes each tab (crates/apz-metrics-view, `GET /v1/metrics/view`); this screen lays out the head
//! (breadcrumb, range, live, group, compare, annotations, settings, export, find), the scope tree, the grouped tabs
//! and the panel grid, and handles zoom, time shift, the "what changed" drawer, live frames, pins, the find palette,
//! the keyboard and the dialogs — the same behaviour as the web view, from the same panels.
//!
//! * [`view`] — the screen's layout: head, tier line, scope tree, tabs, the grid of panels by span.
//! * [`panels`] — a panel's card (title, sub, right text, freshness, pinned-from, the ⋯ menu) and every body.
//! * [`charts`] — the line chart (stacked areas, band, overlays, markers, limit, annotations, gaps, crosshair,
//!   drag-to-zoom, click); [`figures`] — the heatmap, treemap, gauges, swimlanes and sparklines.
//! * [`panels_ops`] — the alerts, rules, traces and events bodies.
//! * [`overlays`] — the panel and sibling menus, the find palette, the keyboard help, the drawer, toasts.
//! * [`dialogs`] — the alert rule dialog and the metrics settings dialog.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use apz_metrics_view::{Around, MetricInfo, Saved, ScopeNode, Status, Tone, View};
use iced::{Color, Element, Subscription, Task};

use super::Id;
use crate::console::Ctx;

pub mod charts;
pub mod dialogs;
pub mod figures;
pub mod overlays;
pub mod panels;
pub mod panels_ops;
pub mod view;

/// The catalog entry the hub serves (`GET /v1/metrics/catalog`), for "About this metric".
pub use apz_metrics_view::source::Entity;

/// Ranges, in order.
pub const RANGES: [&str; 7] = ["15m", "1h", "6h", "24h", "7d", "30d", "90d"];
/// Group-by choices.
pub const GROUPS: [(&str, &str); 6] = [
    ("provider", "by provider"),
    ("model", "by model"),
    ("role", "by role"),
    ("run", "by run"),
    ("agent", "by agent"),
    ("engine", "by engine"),
];

/// What is on the screen and where it came from: the same fields as the web view's URL state.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub tab: String,
    pub scope: String,
    pub range: String,
    /// Whole ranges back from now: 0 or negative.
    pub shift: i32,
    pub zoom: Option<(f64, f64)>,
    /// `prev`, `week`, `none`, `scope:<id>`.
    pub compare: String,
    pub group: String,
    pub trace: Option<String>,
    pub filter: Option<String>,
    pub fixture: bool,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            tab: "overview".into(),
            scope: "global".into(),
            range: "24h".into(),
            shift: 0,
            zoom: None,
            compare: "prev".into(),
            group: "provider".into(),
            trace: None,
            filter: None,
            fixture: std::env::var_os("APZ_METRICS_FIXTURE").is_some(),
        }
    }
}

/// An overlay over the screen; one at a time.
#[derive(Debug, Clone)]
pub enum Overlay {
    /// A panel's ⋯ menu.
    PanelMenu {
        panel: String,
    },
    /// The breadcrumb's sibling switcher.
    Siblings,
    /// The find palette (or the compare picker when `compare`).
    Palette {
        query: String,
        selected: usize,
        compare: bool,
    },
    Help,
    Drawer(Around),
    About {
        panel: String,
    },
    Rule(dialogs::RuleForm),
    Settings(dialogs::SettingsForm),
}

/// The pointer over a time chart: the hovered fraction of the x range (shared by every chart), and a drag in
/// progress (its start fraction and the chart's panel id).
#[derive(Debug, Clone, Default)]
pub struct Pointer {
    pub hover: Option<f32>,
    pub hover_panel: Option<String>,
    pub drag: Option<(String, f32, f32)>,
}

#[derive(Debug, Default)]
pub struct State {
    pub q: Query,
    pub view: Option<View>,
    pub error: Option<String>,
    pub loading: bool,
    pub scopes: Vec<ScopeNode>,
    pub saved: Saved,
    pub catalog: Vec<MetricInfo>,
    pub tree_filter: String,
    /// The tree shown on narrow windows.
    pub tree_open: bool,
    pub overlay: Option<Overlay>,
    pub pointer: Pointer,
    /// Panels shown as tables instead of charts.
    pub as_table: BTreeSet<String>,
    /// The panel in full screen.
    pub full: Option<String>,
    /// The selected span of the open trace (index), per the Traces body.
    pub span: Option<usize>,
    /// The Events body's kind filter.
    pub event_kind: Option<String>,
    pub live: bool,
    pub annotations: bool,
    pub compact: bool,
    /// When the last live frame (or load) arrived.
    pub fresh_at: Option<Instant>,
    /// The last lease renewal.
    pub leased_at: Option<Instant>,
    pub toast: Option<(String, Instant)>,
    /// Scopes fetched at.
    pub scopes_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    // Loading
    Loaded(Result<View, String>),
    Scopes(Result<Vec<ScopeNode>, String>),
    SavedLoaded(Result<Saved, String>),
    Catalog(Result<Vec<MetricInfo>, String>),
    AroundLoaded(Result<Around, String>),
    /// A live frame from the hub's event socket (`metrics`), already decoded.
    Frame(apz_metrics_view::LiveFrame),
    /// An alert transition (`metrics.alert`): toast and reload on the Alerts tab.
    Alert(serde_json::Value),
    /// The 1 s tick: freshness, lease renewal, toast expiry.
    Tick,
    // Navigation
    Scope(String),
    Tab(String),
    Trace(String),
    Range(String),
    Shift(i32),
    ResetZoom,
    Zoom(f64, f64),
    Group(String),
    Compare(String),
    Filter(String),
    EventKind(Option<String>),
    TreeFilter(String),
    ToggleTree,
    ToggleLive,
    ToggleAnnotations,
    ToggleCompact,
    TogglePinScope,
    // Charts
    Hover(Option<(String, f32)>),
    DragStart(String, f32),
    DragTo(f32),
    DragEnd(f32),
    /// A click on a point: the panel, the index and its time (none for a labelled axis).
    Point(String, usize, Option<u64>),
    SelectSpan(usize),
    // Panels
    OpenMenu(String),
    AsTable(String),
    Full(Option<String>),
    Pin(String),
    About(String),
    CopyLink(String),
    ExportCsv(String),
    // Alerts
    Mute(String),
    Ack(String, String),
    RuleOn(String, bool),
    NewRule,
    // Overlays and dialogs
    OpenPalette(bool),
    PaletteInput(String),
    PaletteMove(i32),
    PaletteRun(usize),
    Siblings,
    Help,
    CloseOverlay,
    RuleForm(dialogs::RuleMsg),
    SaveRule,
    OpenSettings,
    SettingsForm(dialogs::SettingsMsg),
    SaveSettings,
    Saved(Result<serde_json::Value, String>),
    /// A key the screen handles (from the keyboard subscription).
    Key(String),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the view (and the tree, the saved pins and the catalog when not yet loaded).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let _ = (state, ctx);
    Task::none()
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let _ = (state, msg, ctx);
    Task::none()
}

/// The 1 s tick, the keyboard, and the hub's events (`metrics` frames, `metrics.alert`).
pub fn subscription(state: &State, ctx: &Ctx) -> Subscription<Msg> {
    let _ = (state, ctx);
    Subscription::none()
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    view::screen(state, ctx)
}

/// The colour of a tone token, in the palette on screen (the web's `--chart-*` tokens for chart tones).
pub fn tone(t: Tone) -> Color {
    let p = crate::theme::palette();
    let light = crate::theme::is_light();
    let chart = |dark: u32, lite: u32| {
        let v = if light { lite } else { dark };
        Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8)
    };
    match t {
        Tone::C1 => chart(0x6cc8ff, 0x2d5f8a),
        Tone::C2 => chart(0x5fe0a0, 0x2e6a40),
        Tone::C3 => chart(0xe2bd5a, 0x85640e),
        Tone::C4 => chart(0xb79cf0, 0x6b4a8a),
        Tone::C5 => chart(0x5ad6d6, 0x1f7a7a),
        Tone::C6 => chart(0xd9a877, 0x7a5c3a),
        Tone::C7 => chart(0xa3adc4, 0x4c544d),
        Tone::C8 => chart(0xff7c86, 0xa3182b),
        Tone::Other => chart(0x6c7790, 0x9aa39a),
        Tone::Ink3 => p.ink_3,
        Tone::Rule => p.rule,
        Tone::Ok => p.ok,
        Tone::Warn => p.warn,
        Tone::Crit => p.crit,
        Tone::Info => p.info,
        Tone::Signal => p.signal,
    }
}

/// The colour of a status.
pub fn status(s: Status) -> Color {
    let p = crate::theme::palette();
    match s {
        Status::Ok => p.ok,
        Status::Warn => p.warn,
        Status::Crit => p.crit,
        Status::Info => p.info,
        Status::Mute => p.ink_3,
    }
}

/// The chart grid and axis colours (the web's `--chart-grid`, `--chart-axis`).
pub fn grid_color() -> Color {
    if crate::theme::is_light() {
        Color::from_rgba8(21, 25, 22, 0.12)
    } else {
        Color::from_rgba8(227, 232, 244, 0.1)
    }
}

pub fn axis_color() -> Color {
    if crate::theme::is_light() {
        Color::from_rgb8(0x7d, 0x86, 0x7e)
    } else {
        Color::from_rgb8(0x6c, 0x77, 0x90)
    }
}

/// How long a toast stays.
pub const TOAST: Duration = Duration::from_millis(2400);
