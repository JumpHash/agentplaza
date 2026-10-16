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
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_metrics_view::{
    Around, Body, LiveFrame, MetricInfo, Pin, Saved, ScopeNode, Status, Table, Tone, View,
};
use iced::{Color, Element, Subscription, Task};
use serde_json::{Value, json};

use super::Id;
use crate::console::client::{Client, task};
use crate::console::{Ctx, events};

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
            // `APZ_METRICS_TAB` opens another tab first (design review and snapshots, like the fixture flag).
            tab: std::env::var("APZ_METRICS_TAB").unwrap_or_else(|_| "overview".into()),
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

#[derive(Debug)]
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
    /// Panels whose older rows are on their way (one request at a time per panel).
    pub loading_more: BTreeSet<String>,
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
    /// Narrow to a scope and/or open a tab and/or a trace, in one step (rows, flags, alerts, movers, pins).
    Open(Option<String>, Option<String>, Option<String>),
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
    /// A button in a panel's head: posts `post` with an empty body, then toasts `done` and loads the view again.
    Action {
        post: String,
        done: String,
    },
    /// "Load older" under a table: fetches the page at `path` (a `Table`) for the panel.
    More {
        panel: String,
        path: String,
    },
    /// The older rows of a panel's table (or why they did not load).
    MoreLoaded(String, Result<Table, String>),
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
impl Default for State {
    fn default() -> Self {
        let prefs = Prefs::load();
        State {
            q: Query::default(),
            view: None,
            error: None,
            loading: false,
            scopes: Vec::new(),
            saved: Saved::default(),
            catalog: Vec::new(),
            tree_filter: String::new(),
            tree_open: false,
            overlay: None,
            pointer: Pointer::default(),
            as_table: BTreeSet::new(),
            full: None,
            span: None,
            event_kind: None,
            live: prefs.live,
            annotations: prefs.annotations,
            compact: prefs.compact,
            fresh_at: None,
            leased_at: None,
            toast: None,
            scopes_at: None,
            loading_more: BTreeSet::new(),
        }
    }
}

/// The console's own preferences (live, annotations, compact), kept in `desktop.json` under `metrics` as the web
/// keeps them in localStorage (`apz.metrics.prefs`). Live and annotations start on, compact off.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Prefs {
    live: bool,
    annotations: bool,
    compact: bool,
}

impl Prefs {
    fn load() -> Prefs {
        Prefs::from_json(crate::console::boot::saved_setting("metrics").as_ref())
    }

    /// The preferences in a saved value; a missing or wrong field takes its default.
    fn from_json(v: Option<&serde_json::Value>) -> Prefs {
        let flag = |key: &str, default: bool| {
            v.and_then(|v| v.get(key))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        Prefs {
            live: flag("live", true),
            annotations: flag("annotations", true),
            compact: flag("compact", false),
        }
    }

    fn save(state: &State) {
        let value = serde_json::json!({
            "live": state.live,
            "annotations": state.annotations,
            "compact": state.compact,
        });
        // A failed save leaves the preferences for this run only.
        let _ = crate::console::boot::save_setting("metrics", value);
    }
}

/// A tab another screen asked the next opening of Metrics to show (the Treasury's "Real model cost" asks for Tokens
/// & spend), as the web's `#/metrics?tab=tokens` link does.
static ASKED_TAB: Mutex<Option<String>> = Mutex::new(None);

/// Asks the next opening of the screen to show `tab`.
pub fn ask_tab(tab: &str) {
    *ASKED_TAB
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tab.to_string());
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if let Some(tab) = ASKED_TAB
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        state.q.tab = tab;
        state.q.trace = None;
        state.q.filter = None;
    }
    // Entering the screen takes a lease even when one was sent a moment ago: the hub drops it once the screen is left.
    state.leased_at = None;
    let mut tasks = vec![reload(state, ctx)];
    if state.saved == Saved::default() {
        tasks.push(fetch_saved(ctx));
    }
    if state.catalog.is_empty() {
        tasks.push(fetch_catalog(ctx));
    }
    Task::batch(tasks)
}

/// Every message of the screen: loads, navigation, charts, panels, alerts, overlays, the keyboard and the live stream.
pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(view)) => {
            state.loading = false;
            state.error = None;
            state.fresh_at = Some(Instant::now());
            // The hub may resolve the tab (a scope may not have the one asked for).
            if view.tab != state.q.tab {
                state.q.tab = view.tab.clone();
            }
            state.view = Some(view);
            sync_live(state, ctx)
        }
        Msg::Loaded(Err(error)) => {
            state.loading = false;
            state.error = Some(error);
            Task::none()
        }
        Msg::Scopes(Ok(scopes)) => {
            state.scopes = scopes;
            Task::none()
        }
        Msg::Scopes(Err(_)) => {
            // Asked again on the next load.
            state.scopes_at = None;
            Task::none()
        }
        Msg::SavedLoaded(Ok(saved)) => {
            state.saved = saved;
            Task::none()
        }
        // Without the pins and recent scopes the tree still works.
        Msg::SavedLoaded(Err(_)) => Task::none(),
        Msg::Catalog(Ok(catalog)) => {
            state.catalog = catalog;
            Task::none()
        }
        // "About this metric" then says there is no entry.
        Msg::Catalog(Err(_)) => Task::none(),
        Msg::AroundLoaded(Ok(around)) => {
            state.overlay = Some(Overlay::Drawer(around));
            Task::none()
        }
        Msg::AroundLoaded(Err(error)) => {
            show_toast(state, error);
            Task::none()
        }
        Msg::Frame(frame) => {
            apply_frame(state, frame);
            Task::none()
        }
        Msg::Alert(alert) => {
            let what = alert.get("what").and_then(Value::as_str).unwrap_or("Alert");
            let text = alert.get("text").and_then(Value::as_str).unwrap_or("");
            show_toast(state, format!("{what}: {text}"));
            if state.q.tab == "alerts" {
                reload(state, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Tick => {
            if state
                .toast
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() >= TOAST)
            {
                state.toast = None;
            }
            sync_live(state, ctx)
        }

        // Navigation: a new scope clears the zoom and the trace and is remembered as recent.
        Msg::Scope(scope) => {
            let changed = scope != state.q.scope;
            let mut next = state.q.clone();
            let mut tasks = Vec::new();
            if changed {
                visit(&mut state.saved, &scope);
                tasks.push(save_dashboards(state, ctx));
                next.zoom = None;
                next.trace = None;
            }
            next.scope = scope;
            tasks.push(navigate(state, next, ctx));
            Task::batch(tasks)
        }
        Msg::Open(scope, tab, trace) => {
            let mut next = state.q.clone();
            let mut tasks = Vec::new();
            if let Some(scope) = scope
                && scope != next.scope
            {
                visit(&mut state.saved, &scope);
                tasks.push(save_dashboards(state, ctx));
                next.zoom = None;
                next.trace = None;
                next.scope = scope;
            }
            if let Some(tab) = tab {
                next.tab = tab;
            }
            if let Some(trace) = trace {
                next.tab = "traces".into();
                next.trace = Some(trace);
            }
            tasks.push(navigate(state, next, ctx));
            Task::batch(tasks)
        }
        Msg::Tab(key) => {
            let listed = state
                .view
                .as_ref()
                .is_some_and(|view| view.tabs.iter().any(|t| t.key == key));
            if !listed {
                return Task::none();
            }
            let mut next = state.q.clone();
            if key != next.tab {
                next.trace = None;
            }
            next.tab = key;
            navigate(state, next, ctx)
        }
        Msg::Trace(id) => {
            state.span = None;
            with_query(state, ctx, |q| {
                q.trace = if id.is_empty() { None } else { Some(id) }
            })
        }
        Msg::Range(range) => {
            if !RANGES.contains(&range.as_str()) {
                return Task::none();
            }
            with_query(state, ctx, |q| {
                q.range = range;
                q.zoom = None;
            })
        }
        Msg::Shift(shift) => {
            let shift = shift.min(0);
            with_query(state, ctx, |q| {
                q.shift = shift;
                q.zoom = None;
            })
        }
        Msg::ResetZoom => with_query(state, ctx, |q| q.zoom = None),
        Msg::Zoom(fa, fb) => match zoom_window(state.q.zoom, fa, fb) {
            Some(zoom) => with_query(state, ctx, |q| q.zoom = Some(zoom)),
            None => Task::none(),
        },
        Msg::Group(group) => with_query(state, ctx, |q| q.group = group),
        Msg::Compare(compare) => with_query(state, ctx, |q| q.compare = compare),
        Msg::Filter(filter) => with_query(state, ctx, |q| {
            q.filter = if filter.is_empty() {
                None
            } else {
                Some(filter)
            }
        }),
        Msg::EventKind(kind) => {
            state.event_kind = kind;
            Task::none()
        }
        Msg::TreeFilter(text) => {
            state.tree_filter = text;
            Task::none()
        }
        Msg::ToggleTree => {
            state.tree_open = !state.tree_open;
            Task::none()
        }
        Msg::ToggleLive => {
            state.live = !state.live;
            Prefs::save(state);
            sync_live(state, ctx)
        }
        Msg::ToggleAnnotations => {
            state.annotations = !state.annotations;
            Prefs::save(state);
            Task::none()
        }
        Msg::ToggleCompact => {
            state.compact = !state.compact;
            Prefs::save(state);
            Task::none()
        }
        Msg::TogglePinScope => {
            let scope = state.q.scope.clone();
            let was_pinned = state.saved.scopes.contains(&scope);
            toggle_scope(&mut state.saved, &scope);
            show_toast(
                state,
                if was_pinned {
                    "Scope unpinned"
                } else {
                    "Scope pinned"
                },
            );
            save_dashboards(state, ctx)
        }

        // Charts: a hover, a drag (more than 0.01 of the width zooms), a click on a point.
        Msg::Hover(hover) => {
            match hover {
                Some((panel, fraction)) => {
                    state.pointer.hover = Some(fraction);
                    state.pointer.hover_panel = Some(panel);
                }
                None => {
                    state.pointer.hover = None;
                    state.pointer.hover_panel = None;
                }
            }
            Task::none()
        }
        Msg::DragStart(panel, fraction) => {
            state.pointer.drag = Some((panel, fraction, fraction));
            Task::none()
        }
        Msg::DragTo(fraction) => {
            if let Some((_, _, end)) = state.pointer.drag.as_mut() {
                *end = fraction;
            }
            Task::none()
        }
        Msg::DragEnd(fraction) => match state.pointer.drag.take() {
            Some((_, start, _)) if (fraction - start).abs() > 0.01 => {
                update(state, Msg::Zoom(start as f64, fraction as f64), ctx)
            }
            _ => Task::none(),
        },
        Msg::Point(_, _, Some(t)) => {
            let path = format!(
                "/v1/metrics/around?{}&t={t}",
                query_params(&state.q, WIDTH, local_offset())
            );
            let client = ctx.client.clone();
            task(move || client.get::<Around>(&path), Msg::AroundLoaded)
        }
        Msg::Point(_, _, None) => Task::none(),
        Msg::SelectSpan(index) => {
            state.span = Some(index);
            Task::none()
        }

        // Panels
        Msg::OpenMenu(panel) => {
            state.overlay = Some(Overlay::PanelMenu { panel });
            Task::none()
        }
        Msg::AsTable(panel) => {
            if !state.as_table.remove(&panel) {
                state.as_table.insert(panel);
            }
            Task::none()
        }
        Msg::Full(panel) => {
            state.full = panel;
            Task::none()
        }
        Msg::Pin(panel) => {
            let pin = Pin {
                tab: state.q.tab.clone(),
                scope: state.q.scope.clone(),
                panel,
            };
            let was_pinned = state.saved.pins.iter().any(|p| same_pin(p, &pin));
            show_toast(
                state,
                if was_pinned {
                    "Unpinned"
                } else {
                    "Pinned to My dashboard"
                },
            );
            toggle_pin(&mut state.saved, pin);
            save_dashboards(state, ctx)
        }
        Msg::About(panel) => {
            state.overlay = Some(Overlay::About { panel });
            Task::none()
        }
        Msg::CopyLink(_) => {
            let text = format!("{}/{}", ctx.client.base(), route_hash(&state.q));
            show_toast(state, "Link copied");
            iced::clipboard::write(text)
        }
        Msg::Action { post, done } => {
            let client = ctx.client.clone();
            let body = json!({});
            task(
                move || client.post::<Value>(&post, &body),
                move |result| action_done(result, (!done.is_empty()).then_some(done.as_str())),
            )
        }
        Msg::More { panel, path } => {
            if !state.loading_more.insert(panel.clone()) {
                return Task::none();
            }
            let client = ctx.client.clone();
            task(
                move || client.get::<Table>(&path),
                move |result| Msg::MoreLoaded(panel.clone(), result),
            )
        }
        Msg::MoreLoaded(panel, result) => {
            state.loading_more.remove(&panel);
            match result {
                Ok(page) => {
                    if let Some(table) = state.view.as_mut().and_then(|view| table_of(view, &panel))
                    {
                        append_page(table, page);
                    }
                }
                Err(error) => show_toast(state, error),
            }
            Task::none()
        }
        Msg::ExportCsv(_) => {
            let path = format!(
                "/v1/metrics/export?{}",
                query_params(&state.q, WIDTH, local_offset())
            );
            let tab = state.q.tab.clone();
            let client = ctx.client.clone();
            let secs = now_ms() / 1000;
            task(
                move || write_export(&client, &path, &tab, secs),
                export_done,
            )
        }

        // Alerts
        Msg::Mute(rule) => {
            let path = format!("/v1/metrics/alerts/{}/mute", percent_encode(&rule, false));
            let body = json!({ "until_ms": now_ms() + MUTE_MS });
            let client = ctx.client.clone();
            task(
                move || client.post::<Value>(&path, &body),
                |r| action_done(r, Some("Muted for 1 hour")),
            )
        }
        Msg::Ack(rule, subject) => {
            let path = format!("/v1/metrics/alerts/{}/ack", percent_encode(&rule, false));
            let body = json!({ "subject": subject });
            let client = ctx.client.clone();
            task(
                move || client.post::<Value>(&path, &body),
                |r| action_done(r, Some("Acknowledged")),
            )
        }
        Msg::RuleOn(rule, on) => {
            let path = format!("/v1/metrics/alerts/{}/on", percent_encode(&rule, false));
            let body = json!({ "on": on });
            let client = ctx.client.clone();
            task(
                move || client.post::<Value>(&path, &body),
                |r| action_done(r, None),
            )
        }
        Msg::NewRule => {
            let mut form = dialogs::RuleForm::new(&state.q.scope);
            if let Some(first) = state.catalog.first() {
                form.metric = first.id.clone();
            }
            state.overlay = Some(Overlay::Rule(form));
            Task::none()
        }

        // Overlays and dialogs
        Msg::OpenPalette(compare) => {
            state.overlay = Some(Overlay::Palette {
                query: String::new(),
                selected: 0,
                compare,
            });
            Task::none()
        }
        Msg::PaletteInput(text) => {
            if let Some(Overlay::Palette {
                query, selected, ..
            }) = state.overlay.as_mut()
            {
                *query = text;
                *selected = 0;
            }
            Task::none()
        }
        Msg::PaletteMove(step) => {
            let count = match &state.overlay {
                Some(Overlay::Palette { query, compare, .. }) => {
                    overlays::palette_items(state, query, *compare).len()
                }
                _ => return Task::none(),
            };
            if count > 0 {
                if let Some(Overlay::Palette { selected, .. }) = state.overlay.as_mut() {
                    let next = (*selected as i64 + i64::from(step)).rem_euclid(count as i64);
                    *selected = next as usize;
                }
            }
            Task::none()
        }
        Msg::PaletteRun(index) => {
            let picked = match &state.overlay {
                Some(Overlay::Palette { query, compare, .. }) => {
                    overlays::palette_items(state, query, *compare)
                        .into_iter()
                        .nth(index)
                        .map(|(_, _, msg)| msg)
                }
                _ => return Task::none(),
            };
            // The item may open an overlay of its own, so the palette closes first.
            state.overlay = None;
            match picked {
                Some(msg) => update(state, msg, ctx),
                None => Task::none(),
            }
        }
        Msg::Siblings => {
            state.overlay = Some(Overlay::Siblings);
            Task::none()
        }
        Msg::Help => {
            state.overlay = Some(Overlay::Help);
            Task::none()
        }
        Msg::CloseOverlay => {
            state.overlay = None;
            Task::none()
        }
        Msg::RuleForm(message) => {
            if let Some(Overlay::Rule(form)) = state.overlay.as_mut() {
                form.apply(message);
            }
            Task::none()
        }
        Msg::SaveRule => {
            let Some(Overlay::Rule(form)) = state.overlay.as_mut() else {
                return Task::none();
            };
            let body = match form.to_json(&state.catalog) {
                Ok(body) => body,
                Err(error) => {
                    form.error = Some(error);
                    return Task::none();
                }
            };
            let client = ctx.client.clone();
            task(
                move || client.call("PUT", "/v1/metrics/alerts/rules", Some(&body)),
                |r| {
                    Msg::Saved(Ok(match r {
                        Ok(_) => json!({ "done": "Rule saved", "close": true, "reload": true }),
                        Err(error) => json!({ "form_error": error }),
                    }))
                },
            )
        }
        Msg::OpenSettings => {
            let client = ctx.client.clone();
            task(
                move || client.get::<Value>("/v1/metrics/settings"),
                |r| {
                    Msg::Saved(Ok(match r {
                        Ok(settings) => json!({ "settings": settings }),
                        Err(error) => json!({ "failed": error }),
                    }))
                },
            )
        }
        Msg::SettingsForm(message) => {
            if let Some(Overlay::Settings(form)) = state.overlay.as_mut() {
                form.apply(message);
            }
            Task::none()
        }
        Msg::SaveSettings => {
            let Some(Overlay::Settings(form)) = state.overlay.as_mut() else {
                return Task::none();
            };
            let body = match form.to_json() {
                Ok(body) => body,
                Err(error) => {
                    form.error = Some(error);
                    return Task::none();
                }
            };
            let client = ctx.client.clone();
            task(
                move || client.call("PUT", "/v1/metrics/settings", Some(&body)),
                |r| {
                    Msg::Saved(Ok(match r {
                        // The OpenMetrics token is shown once, so the dialog stays open to show it.
                        Ok(reply) => match reply.get("openmetrics_token").and_then(Value::as_str) {
                            Some(token) => json!({ "done": "Settings saved", "token": token }),
                            None => json!({ "done": "Settings saved", "close": true }),
                        },
                        Err(error) => json!({ "form_error": error }),
                    }))
                },
            )
        }
        Msg::Saved(result) => saved_answer(state, result, ctx),
        Msg::Key(name) => key(state, &name, ctx),
    }
}

/// The 1 s tick, the keyboard, and the hub's events (`metrics` frames, `metrics.alert`).
pub fn subscription(state: &State, ctx: &Ctx) -> Subscription<Msg> {
    let _ = state;
    Subscription::batch([
        iced::time::every(Duration::from_secs(1)).map(|_| Msg::Tick),
        events::subscription(&ctx.client).filter_map(live_event),
        iced::event::listen_with(keys),
    ])
}

/// The chart width the view is composed for (the web's `FALLBACK_WIDTH` on a wide window).
const WIDTH: u32 = 1100;
/// How long the scope tree is kept before it is fetched again.
const SCOPES_TTL: Duration = Duration::from_secs(60);
/// How often a live lease is renewed while the view keeps it.
const LEASE_RENEW: Duration = Duration::from_secs(10);
/// How long an alert is muted from the Mute action.
const MUTE_MS: u64 = 60 * 60 * 1000;
/// How many scopes `recent` keeps.
const RECENT_MAX: usize = 6;

/// Fetches the view for the query, and the scope tree when it is older than a minute.
fn reload(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let path = view_path(&state.q, WIDTH, local_offset());
    let client = ctx.client.clone();
    let mut tasks = vec![task(move || client.get::<View>(&path), Msg::Loaded)];
    if state.scopes_at.is_none_or(|at| at.elapsed() > SCOPES_TTL) {
        state.scopes_at = Some(Instant::now());
        tasks.push(fetch_scopes(ctx, state.q.fixture));
    }
    Task::batch(tasks)
}

/// Moves the query to `next`: the view and the lease follow. Nothing moves when the query is unchanged.
fn navigate(state: &mut State, next: Query, ctx: &Ctx) -> Task<Msg> {
    if next == state.q {
        return Task::none();
    }
    state.q = next;
    state.leased_at = None;
    let view = reload(state, ctx);
    Task::batch([view, sync_live(state, ctx)])
}

/// Changes one field of the query and moves to it.
fn with_query(state: &mut State, ctx: &Ctx, change: impl FnOnce(&mut Query)) -> Task<Msg> {
    let mut next = state.q.clone();
    change(&mut next);
    navigate(state, next, ctx)
}

/// Fetches the scope tree; the fixture flag keeps the request on the fixture data like the view request.
fn fetch_scopes(ctx: &Ctx, fixture: bool) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = if fixture {
        "/v1/metrics/scopes?fixture=1"
    } else {
        "/v1/metrics/scopes"
    };
    task(move || client.get::<Vec<ScopeNode>>(path), Msg::Scopes)
}

fn fetch_saved(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    task(
        move || client.get::<Saved>("/v1/metrics/dashboards"),
        Msg::SavedLoaded,
    )
}

fn fetch_catalog(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    task(
        move || client.get::<Vec<MetricInfo>>("/v1/metrics/catalog"),
        Msg::Catalog,
    )
}

/// Stores the pins and recent scopes on the hub. A failure is reported as a toast.
fn save_dashboards(state: &State, ctx: &Ctx) -> Task<Msg> {
    let body = match serde_json::to_value(&state.saved) {
        Ok(body) => body,
        Err(_) => return Task::none(),
    };
    let client = ctx.client.clone();
    task(
        move || client.call("PUT", "/v1/metrics/dashboards", Some(&body)),
        |r| {
            Msg::Saved(Ok(match r {
                Ok(_) => Value::Null,
                Err(error) => json!({ "failed": format!("Could not save your pins: {error}") }),
            }))
        },
    )
}

/// Holds the live lease while live applies: sent when none is held or the last one is 10 s old, dropped otherwise.
fn sync_live(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if !live_applies(state) {
        state.leased_at = None;
        return Task::none();
    }
    if state.leased_at.is_some_and(|at| at.elapsed() < LEASE_RENEW) {
        return Task::none();
    }
    state.leased_at = Some(Instant::now());
    let body = json!({
        "scope": state.q.scope,
        "tab": state.q.tab,
        "group": state.q.group,
        "range": state.q.range,
    });
    let client = ctx.client.clone();
    // A failed renewal is not reported; the next tick tries again.
    task(
        move || client.post::<Value>("/v1/metrics/live", &body),
        |_| Msg::Saved(Ok(Value::Null)),
    )
}

/// Live applies to the 15m and 1h ranges, ending now, when live is on.
pub(super) fn live_applies(state: &State) -> bool {
    state.live && state.q.shift == 0 && matches!(state.q.range.as_str(), "15m" | "1h")
}

/// Replaces the panels of a live frame that is for the query on screen.
fn apply_frame(state: &mut State, frame: LiveFrame) {
    let on_screen = frame.scope == state.q.scope
        && frame.tab == state.q.tab
        && frame.group == state.q.group
        && frame.range == state.q.range;
    if !live_applies(state) || !on_screen {
        return;
    }
    if let Some(view) = state.view.as_mut() {
        for panel in frame.panels {
            if let Some(slot) = view.panels.iter_mut().find(|p| p.id == panel.id) {
                *slot = panel;
            }
        }
    }
    state.fresh_at = Some(Instant::now());
}

/// The hub's event `metrics` (a live frame) or `metrics.alert` (an alert transition); other events are ignored.
fn live_event(event: events::Event) -> Option<Msg> {
    match event.kind.as_str() {
        "metrics" => serde_json::from_value::<LiveFrame>(event.data)
            .ok()
            .map(Msg::Frame),
        "metrics.alert" => Some(Msg::Alert(event.data)),
        _ => None,
    }
}

/// The answer of a hub write that changes alerts: the toast (when given) and a reload when it worked.
fn action_done(result: Result<Value, String>, done: Option<&str>) -> Msg {
    Msg::Saved(Ok(match result {
        Ok(_) => match done {
            Some(text) => json!({ "done": text, "reload": true }),
            None => json!({ "reload": true }),
        },
        Err(error) => json!({ "failed": error }),
    }))
}

/// The result of an export: the path written, or the error.
fn export_done(result: Result<PathBuf, String>) -> Msg {
    Msg::Saved(Ok(match result {
        Ok(path) => json!({ "done": format!("Exported to {}", path.display()) }),
        Err(error) => json!({ "failed": format!("Export failed: {error}") }),
    }))
}

/// Writes the export of the view to `exports/metrics-<tab>-<secs>.csv` in the plaza's data directory.
fn write_export(client: &Client, path: &str, tab: &str, secs: u64) -> Result<PathBuf, String> {
    let reply = apz_http::client::request(&client.target, "GET", path, None, client.timeout)
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("HTTP {}", reply.status));
    }
    let dir = apz_os::dirs::data().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tab: String = tab
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let file = dir.join(format!("metrics-{tab}-{secs}.csv"));
    std::fs::write(&file, &reply.body).map_err(|e| e.to_string())?;
    Ok(file)
}

/// The answers the screen's hub writes and reads send back, as one envelope (see the helpers that build them):
/// `done` (a toast), `failed` (an error toast), `form_error` (the error line of the open dialog), `token` (the
/// OpenMetrics token, shown in the settings dialog), `settings` (opens the settings dialog), `close` and `reload`.
fn saved_answer(state: &mut State, result: Result<Value, String>, ctx: &Ctx) -> Task<Msg> {
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            show_toast(state, error);
            return Task::none();
        }
    };
    if let Some(text) = answer.get("done").and_then(Value::as_str) {
        show_toast(state, text);
    }
    if let Some(text) = answer.get("failed").and_then(Value::as_str) {
        show_toast(state, text);
    }
    if let Some(error) = answer.get("form_error").and_then(Value::as_str) {
        let shown = match state.overlay.as_mut() {
            Some(Overlay::Rule(form)) => {
                form.error = Some(error.into());
                true
            }
            Some(Overlay::Settings(form)) => {
                form.error = Some(error.into());
                true
            }
            _ => false,
        };
        if !shown {
            show_toast(state, error);
        }
    }
    if let Some(token) = answer.get("token").and_then(Value::as_str) {
        if let Some(Overlay::Settings(form)) = state.overlay.as_mut() {
            form.token = Some(token.into());
        }
    }
    if let Some(settings) = answer.get("settings") {
        state.overlay = Some(Overlay::Settings(dialogs::SettingsForm::from_json(
            settings,
        )));
    }
    if answer.get("close").and_then(Value::as_bool) == Some(true) {
        state.overlay = None;
    }
    if answer.get("reload").and_then(Value::as_bool) == Some(true) {
        return reload(state, ctx);
    }
    Task::none()
}

/// The keyboard: Escape closes the overlay or leaves full screen; with the find palette open the arrows and Enter
/// drive it; otherwise the digits pick a tab and the letters are the web's shortcuts.
fn key(state: &mut State, name: &str, ctx: &Ctx) -> Task<Msg> {
    if name == "Escape" {
        if state.overlay.is_some() {
            state.overlay = None;
        } else {
            state.full = None;
        }
        return Task::none();
    }
    if state.overlay.is_some() {
        let selected = match &state.overlay {
            Some(Overlay::Palette { selected, .. }) => *selected,
            _ => return Task::none(),
        };
        return match name {
            "ArrowUp" => update(state, Msg::PaletteMove(-1), ctx),
            "ArrowDown" => update(state, Msg::PaletteMove(1), ctx),
            "Enter" => update(state, Msg::PaletteRun(selected), ctx),
            _ => Task::none(),
        };
    }
    if let Ok(number) = name.parse::<usize>() {
        if !(1..=9).contains(&number) {
            return Task::none();
        }
        let tab = state
            .view
            .as_ref()
            .and_then(|view| view.tabs.get(number - 1))
            .map(|t| t.key.clone());
        return match tab {
            Some(key) => update(state, Msg::Tab(key), ctx),
            None => Task::none(),
        };
    }
    let msg = match name {
        "/" => Msg::OpenPalette(false),
        "?" => Msg::Help,
        "l" => Msg::ToggleLive,
        "z" => Msg::ResetZoom,
        "f" => Msg::TogglePinScope,
        "a" => Msg::ToggleAnnotations,
        "c" => Msg::OpenPalette(true),
        "d" => Msg::ToggleCompact,
        "n" => Msg::NewRule,
        "[" => Msg::Shift(state.q.shift - 1),
        "]" => Msg::Shift(state.q.shift + 1),
        _ => return Task::none(),
    };
    update(state, msg, ctx)
}

/// The keyboard as messages. Ctrl+K opens the find palette; Ctrl, Alt and Logo chords are left alone; a character
/// the focused field captured (typing into a text input) is not a shortcut.
fn keys(event: iced::Event, status: iced::event::Status, _window: iced::window::Id) -> Option<Msg> {
    use iced::keyboard::{Event as KeyEvent, Key, key::Named};
    let iced::Event::Keyboard(KeyEvent::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    if modifiers.control() && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("k")) {
        return Some(Msg::Key("/".into()));
    }
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    let name = match key {
        Key::Named(Named::Escape) => "Escape".to_string(),
        Key::Named(Named::Enter) => "Enter".to_string(),
        Key::Named(Named::ArrowUp) => "ArrowUp".to_string(),
        Key::Named(Named::ArrowDown) => "ArrowDown".to_string(),
        Key::Character(c) if status == iced::event::Status::Ignored => c.to_string(),
        _ => return None,
    };
    Some(Msg::Key(name))
}

fn show_toast(state: &mut State, text: impl Into<String>) {
    state.toast = Some((text.into(), Instant::now()));
}

/// The table body of a panel in the view, when the panel is a table.
fn table_of<'a>(view: &'a mut View, panel: &str) -> Option<&'a mut Table> {
    let panel = view.panels.iter_mut().find(|p| p.id == panel)?;
    match &mut panel.body {
        Body::Table(table) => Some(table),
        _ => None,
    }
}

/// Appends an older page under the rows already shown. The page's own `more` (the next page, if any) replaces
/// the table's, so "Load older" keeps going until the hub has no older rows.
pub fn append_page(table: &mut Table, page: Table) {
    table.rows.extend(page.rows);
    table.more = page.more;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The console's UTC offset in minutes: the hub places the time axis at it.
fn local_offset() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// The query string of a view request (no leading `?`): the web's `metricsQuery`, the same parameters in the same
/// order.
fn query_params(q: &Query, width: u32, tz: i32) -> String {
    let mut pairs = vec![
        ("tab", q.tab.clone()),
        ("scope", q.scope.clone()),
        ("range", q.range.clone()),
        ("compare", q.compare.clone()),
        ("group", q.group.clone()),
        ("width", width.to_string()),
        ("tz", tz.to_string()),
    ];
    if q.shift != 0 {
        pairs.push(("shift", q.shift.to_string()));
    }
    if let Some((a, b)) = q.zoom {
        pairs.push(("zoom", format!("{a},{b}")));
    }
    if let Some(trace) = &q.trace {
        pairs.push(("trace", trace.clone()));
    }
    if let Some(filter) = &q.filter {
        pairs.push(("filter", filter.clone()));
    }
    if q.fixture {
        pairs.push(("fixture", "1".into()));
    }
    encode_pairs(&pairs)
}

/// The view request path for a query.
fn view_path(q: &Query, width: u32, tz: i32) -> String {
    format!("/v1/metrics/view?{}", query_params(q, width, tz))
}

/// The route hash of a query (`#/metrics?…`, defaults left out), as the web writes it.
fn route_hash(q: &Query) -> String {
    let d = Query::default();
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if q.tab != "overview" {
        pairs.push(("tab", q.tab.clone()));
    }
    if q.scope != d.scope {
        pairs.push(("scope", q.scope.clone()));
    }
    if q.range != d.range {
        pairs.push(("range", q.range.clone()));
    }
    if q.shift != 0 {
        pairs.push(("shift", q.shift.to_string()));
    }
    if let Some((a, b)) = q.zoom {
        pairs.push(("zoom", format!("{a:.3},{b:.3}")));
    }
    if q.compare != d.compare {
        pairs.push(("compare", q.compare.clone()));
    }
    if q.group != d.group {
        pairs.push(("group", q.group.clone()));
    }
    if let Some(trace) = &q.trace {
        pairs.push(("trace", trace.clone()));
    }
    if let Some(filter) = &q.filter {
        pairs.push(("filter", filter.clone()));
    }
    if q.fixture {
        pairs.push(("fixture", "1".into()));
    }
    let query = encode_pairs(&pairs);
    if query.is_empty() {
        "#/metrics".into()
    } else {
        format!("#/metrics?{query}")
    }
}

fn encode_pairs(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                percent_encode(key, true),
                percent_encode(value, true)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes text as the web does: form encoding (a space is `+`) for query values, or a path segment
/// (`encodeURIComponent`) when `form` is false.
fn percent_encode(text: &str, form: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let plain = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'*')
            || (!form && matches!(byte, b'!' | b'~' | b'\'' | b'(' | b')'));
        if plain {
            out.push(byte as char);
        } else if byte == b' ' && form {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The window a zoom asks for: the fractions `fa` and `fb` (in either order) of the window on screen, which is the
/// current zoom or the whole range, mapped onto the range. None when the window would be empty.
fn zoom_window(current: Option<(f64, f64)>, fa: f64, fb: f64) -> Option<(f64, f64)> {
    let (z0, z1) = current.unwrap_or((0.0, 1.0));
    let a = z0 + fa.min(fb).clamp(0.0, 1.0) * (z1 - z0);
    let b = z0 + fa.max(fb).clamp(0.0, 1.0) * (z1 - z0);
    (b > a).then_some((a, b))
}

/// Remembers a visited scope first in `recent`: no duplicates, at most six.
fn visit(saved: &mut Saved, scope: &str) {
    saved.recent.retain(|r| r.as_str() != scope);
    saved.recent.insert(0, scope.to_string());
    saved.recent.truncate(RECENT_MAX);
}

fn toggle_scope(saved: &mut Saved, scope: &str) {
    if saved.scopes.iter().any(|s| s.as_str() == scope) {
        saved.scopes.retain(|s| s.as_str() != scope);
    } else {
        saved.scopes.push(scope.to_string());
    }
}

fn same_pin(a: &Pin, b: &Pin) -> bool {
    a.tab == b.tab && a.scope == b.scope && a.panel == b.panel
}

/// Pins or unpins a panel (pins are tab, scope and panel id); the saved pins then stop being the starter set.
fn toggle_pin(saved: &mut Saved, pin: Pin) {
    if saved.pins.iter().any(|p| same_pin(p, &pin)) {
        saved.pins.retain(|p| !same_pin(p, &pin));
    } else {
        saved.pins.push(pin);
    }
    saved.starter = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_start_live_with_annotations_and_keep_what_was_saved() {
        let fresh = Prefs::from_json(None);
        assert_eq!(
            fresh,
            Prefs {
                live: true,
                annotations: true,
                compact: false
            }
        );
        let saved = serde_json::json!({"live": false, "compact": true, "annotations": "x"});
        let kept = Prefs::from_json(Some(&saved));
        assert_eq!(
            kept,
            Prefs {
                live: false,
                annotations: true,
                compact: true
            }
        );
    }

    #[test]
    fn the_view_path_carries_the_query_the_hub_width_and_the_offset() {
        let query = Query {
            tab: "latency".into(),
            scope: "node:n1".into(),
            range: "1h".into(),
            shift: -2,
            zoom: Some((0.25, 0.5)),
            compare: "prev".into(),
            group: "provider".into(),
            trace: Some("t 1".into()),
            filter: None,
            fixture: true,
        };
        assert_eq!(
            view_path(&query, 1100, 120),
            "/v1/metrics/view?tab=latency&scope=node%3An1&range=1h&compare=prev&group=provider&width=1100&tz=120&shift=-2&zoom=0.25%2C0.5&trace=t+1&fixture=1"
        );
    }

    #[test]
    fn a_zoom_maps_the_fractions_of_the_window_on_screen_onto_the_range() {
        assert_eq!(zoom_window(None, 0.2, 0.6), Some((0.2, 0.6)));
        // The second half of the range, then its first half of that window (fractions in either order).
        assert_eq!(zoom_window(Some((0.5, 1.0)), 0.5, 0.0), Some((0.5, 0.75)));
        assert_eq!(zoom_window(None, 0.4, 0.4), None);
    }

    #[test]
    fn an_older_page_goes_under_the_rows_shown_and_brings_its_own_more() {
        let mut table: Table = serde_json::from_value(json!({
            "columns": [{ "head": "When" }],
            "rows": [{ "cells": [] }, { "cells": [] }],
            "more": "/v1/metrics/calls?before=1"
        }))
        .unwrap();
        let page: Table = serde_json::from_value(json!({
            "columns": [{ "head": "When" }],
            "rows": [{ "cells": [] }],
            "more": null
        }))
        .unwrap();
        append_page(&mut table, page);
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.more, None);
    }
}
