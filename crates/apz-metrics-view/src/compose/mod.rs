//! Composing a tab: [`compose`] works out the window, the comparison and the scope, then hands a [`Ctx`] to the
//! tab's composer (one file per tab), which reads from a [`Source`] and returns panels.
//!
//! The helpers here are what composers share, so panels look and behave the same on every tab: [`Ctx::chart`]
//! (a read becomes a chart with its comparison overlay and annotations), [`Ctx::tile`], [`Ctx::spark`],
//! [`panel`], and the window maths.

use apz_metrics::Event;
use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use crate::compare::{Compare, change_pct};
use crate::format::{format, time_label};
use crate::model::*;
use crate::source::{Group, GroupBy, Read, Source};

pub mod agents;
pub mod alerts;
pub mod around;
pub mod capacity;
pub mod compute;
pub mod context;
pub mod dash;
pub mod events;
pub mod hub;
pub mod latency;
pub mod lifecycle;
pub mod overview;
pub mod storage;
pub mod tokens;
pub mod traces;

/// Points per series for each range (before zoom), and so the step.
pub const POINTS: &[(&str, u32)] = &[
    ("15m", 90),
    ("1h", 120),
    ("6h", 144),
    ("24h", 144),
    ("7d", 168),
    ("30d", 180),
    ("90d", 180),
];

/// Everything a composer needs besides the source.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub q: ViewQuery,
    pub scope: ScopeInfo,
    /// The scope's kind prefix (`global`, `node`, `agent` …).
    pub kind: String,
    pub now_ms: u64,
    pub from_ms: u64,
    pub to_ms: u64,
    pub step_ms: u64,
    /// Points per series.
    pub points: usize,
    pub compare: Compare,
    /// The comparison window, when comparing.
    pub cmp_from_ms: u64,
    pub cmp_to_ms: u64,
    /// The scope the comparison reads (the view's own, or the other scope).
    pub cmp_scope: String,
    /// "previous", "last week", "n2 · rack-b", or "" when not comparing.
    pub cmp_label: String,
    /// The group-by choice of the view.
    pub group: GroupBy,
    pub annotations: Vec<Annotation>,
}

/// The range in ms of a range name (24 h for an unknown one).
pub fn range_ms(range: &str) -> u64 {
    RANGES
        .iter()
        .find(|r| r.0 == range)
        .map(|r| r.1)
        .unwrap_or(86_400_000)
}

/// Points per series of a range name.
pub fn range_points(range: &str) -> u32 {
    POINTS
        .iter()
        .find(|r| r.0 == range)
        .map(|r| r.1)
        .unwrap_or(144)
}

impl Ctx {
    /// Works out the window: ending `now + shift·range`, zoomed to the given fractions, at the range's points.
    pub fn new(q: &ViewQuery, src: &dyn Source) -> Ctx {
        let now = src.now_ms();
        let range = range_ms(&q.range);
        let end = if q.shift < 0 {
            now.saturating_sub(range * q.shift.unsigned_abs() as u64)
        } else {
            now
        };
        let start = end.saturating_sub(range);
        let points = range_points(&q.range) as u64;
        let (from, to) = match q.zoom {
            Some((a, b)) if b > a && (0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b) => (
                start + (a * range as f64) as u64,
                start + (b * range as f64) as u64,
            ),
            _ => (start, end),
        };
        let step = ((to - from) / points.max(1)).max(10_000);
        let step = step - step % 1_000;
        let from = to - step * ((to - from) / step);
        let scope = scope_info(&q.scope, src);
        let compare = Compare::parse(&q.compare);
        let (cmp_scope, cmp_label) = match &compare {
            Compare::Scope(id) => (id.clone(), scope_info(id, src).label),
            c => (q.scope.clone(), c.label("")),
        };
        let (cmp_from, cmp_to) = compare.window(from, to).unwrap_or((0, 0));
        let annotations = src
            .events("global", from, to, apz_metrics::ANNOTATION_KINDS, 50)
            .into_iter()
            .map(|e| Annotation {
                t_ms: e.t_ms,
                kind: e.kind.clone(),
                label: annotation_label(&e),
            })
            .collect();
        Ctx {
            kind: scope.kind.clone(),
            scope,
            now_ms: now,
            from_ms: from,
            to_ms: to,
            step_ms: step,
            points: ((to - from) / step) as usize,
            cmp_from_ms: cmp_from,
            cmp_to_ms: cmp_to,
            cmp_scope,
            cmp_label,
            compare,
            group: GroupBy::of_choice(&q.group),
            annotations,
            q: q.clone(),
        }
    }

    /// Whether the view compares at all.
    pub fn comparing(&self) -> bool {
        self.compare != Compare::None
    }

    /// A read of `metric` in the view's scope and window.
    pub fn read(&self, metric: &str, agg: AggFn, group: GroupBy) -> Read {
        Read {
            metric: metric.into(),
            scope: self.q.scope.clone(),
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            step_ms: self.step_ms,
            agg,
            group,
            top: Some(6),
        }
    }

    /// The same read over the comparison window and scope.
    pub fn cmp_read(&self, r: &Read) -> Read {
        Read {
            scope: self.cmp_scope.clone(),
            from_ms: self.cmp_from_ms,
            to_ms: self.cmp_to_ms,
            ..r.clone()
        }
    }

    /// A chart of a read: one series per group in chart colours, a dashed comparison overlay (the stacked total,
    /// or the first two series), and the view's annotations.
    pub fn chart(
        &self,
        src: &dyn Source,
        read: &Read,
        unit: Unit,
        stacked: bool,
        height: u16,
    ) -> Chart {
        let groups = src.series(read);
        let series: Vec<Series> = groups
            .iter()
            .enumerate()
            .map(|(i, g)| series_of(g, Tone::chart(i)))
            .collect();
        let mut ghosts = Vec::new();
        if self.comparing() && !groups.is_empty() {
            let before = src.series(&self.cmp_read(read));
            if stacked {
                let n = self.points;
                let mut total = vec![None; n];
                for g in &before {
                    for (i, v) in g.values.iter().enumerate().take(n) {
                        if let Some(v) = v {
                            total[i] = Some(total[i].unwrap_or(0.0) + v);
                        }
                    }
                }
                ghosts.push(Series {
                    name: format!("total · {}", self.cmp_label),
                    tone: Tone::Ink3,
                    dash: true,
                    values: total,
                    gap_reason: None,
                    scope: None,
                });
            } else {
                for (i, g) in groups.iter().take(2).enumerate() {
                    if let Some(b) = before.iter().find(|b| b.name == g.name).or(before.get(i)) {
                        ghosts.push(Series {
                            name: format!("{} · {}", g.name, self.cmp_label),
                            tone: Tone::chart(i),
                            dash: true,
                            values: b.values.clone(),
                            gap_reason: None,
                            scope: None,
                        });
                    }
                }
            }
        }
        Chart {
            unit,
            x: self.x(),
            series,
            stacked,
            area: false,
            y_max: None,
            band: None,
            ghosts,
            markers: Vec::new(),
            limit: None,
            height,
            annotate: true,
            point_tab: None,
        }
    }

    /// The time axis of the view.
    pub fn x(&self) -> XAxis {
        XAxis::Time {
            start_ms: self.from_ms,
            step_ms: self.step_ms,
        }
    }

    /// A stat tile: the value of a metric over the window (formatted), its change against the comparison, and a
    /// sparkline of 40 points.
    #[allow(clippy::too_many_arguments)]
    pub fn tile(
        &self,
        src: &dyn Source,
        label: &str,
        metric: &str,
        agg: AggFn,
        unit: Unit,
        up_bad: bool,
        tone: Tone,
        tab: &str,
    ) -> Tile {
        let now = src.scalar(metric, &self.q.scope, self.from_ms, self.to_ms, agg);
        let before = if self.comparing() {
            src.scalar(
                metric,
                &self.cmp_scope,
                self.cmp_from_ms,
                self.cmp_to_ms,
                agg,
            )
        } else {
            None
        };
        Tile {
            label: label.into(),
            value: now.map(|v| format(unit, v)).unwrap_or_else(|| "—".into()),
            change_pct: match (now, before) {
                (Some(a), Some(b)) => change_pct(a, b),
                _ => None,
            },
            up_bad,
            note: String::new(),
            spark: self.spark(src, metric, agg),
            tone,
            tab: Some(tab.into()),
        }
    }

    /// 40 points of a metric over the window, gaps as 0 (sparklines have no gaps).
    pub fn spark(&self, src: &dyn Source, metric: &str, agg: AggFn) -> Vec<f64> {
        let step = ((self.to_ms - self.from_ms) / 40).max(1_000);
        let read = Read {
            metric: metric.into(),
            scope: self.q.scope.clone(),
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            step_ms: step,
            agg,
            group: GroupBy::All,
            top: None,
        };
        src.series(&read)
            .into_iter()
            .next()
            .map(|g| g.values.iter().map(|v| v.unwrap_or(0.0)).collect())
            .unwrap_or_default()
    }

    /// A time in the console's zone: "14:20" (or "Oct 16 14:20" when not today).
    pub fn time(&self, t_ms: u64) -> String {
        let shift = i64::from(self.q.tz) * 60_000;
        let day = |t: u64| (t as i64 + shift).div_euclid(86_400_000);
        let same_day = day(t_ms) == day(self.now_ms);
        time_label(t_ms, if same_day { 60_000 } else { 3_600_000 }, self.q.tz)
    }
}

/// A panel with the common fields.
pub fn panel(id: &str, title: &str, sub: &str, span: u8, body: Body) -> Panel {
    Panel {
        id: id.into(),
        title: title.into(),
        sub: sub.into(),
        right: String::new(),
        span,
        metric: None,
        live: false,
        pinned: None,
        body,
        actions: Vec::new(),
    }
}

/// A chart series of a group.
pub fn series_of(g: &Group, tone: Tone) -> Series {
    Series {
        name: g.name.clone(),
        tone,
        dash: false,
        values: g.values.clone(),
        gap_reason: None,
        scope: g.scope.clone(),
    }
}

/// The status of a fraction against warn and crit thresholds.
pub fn status_of(frac: f64, warn: f64, crit: f64) -> Status {
    if frac >= crit {
        Status::Crit
    } else if frac >= warn {
        Status::Warn
    } else {
        Status::Ok
    }
}

/// An empty-state panel body.
pub fn empty(text: &str) -> Body {
    Body::Empty { text: text.into() }
}

/// What an annotation reads as.
pub fn annotation_label(e: &Event) -> String {
    if e.detail.is_empty() {
        e.kind.replace('_', " ")
    } else {
        e.detail.clone()
    }
}

/// The scope info of an id: its kind, label, path from Global and its siblings, from the source's tree.
pub fn scope_info(id: &str, src: &dyn Source) -> ScopeInfo {
    let tree = src.scopes();
    let Some(pos) = tree.iter().position(|n| n.id == id) else {
        return ScopeInfo {
            id: "global".into(),
            kind: "global".into(),
            label: "Global".into(),
            path: vec![("Global".into(), "global".into())],
            siblings: vec![("Global".into(), "global".into())],
        };
    };
    let me = &tree[pos];
    if me.id == "global" {
        return ScopeInfo {
            id: "global".into(),
            kind: "global".into(),
            label: "Global".into(),
            path: vec![("Global".into(), "global".into())],
            siblings: vec![("Global".into(), "global".into())],
        };
    }
    // Ancestors: walk back for each shallower depth.
    let mut path = vec![(me.label.clone(), me.id.clone())];
    let mut depth = me.depth;
    for n in tree[..pos].iter().rev() {
        if depth == 0 {
            break;
        }
        if n.depth < depth && n.group == me.group && n.id != "global" {
            path.push((n.label.clone(), n.id.clone()));
            depth = n.depth;
        }
    }
    path.push(("Global".into(), "global".into()));
    path.reverse();
    // Siblings: same kind, same group and the same parent.
    let parent = parent_of(&tree, pos);
    let siblings = tree
        .iter()
        .enumerate()
        .filter(|(i, n)| n.kind == me.kind && n.group == me.group && parent_of(&tree, *i) == parent)
        .map(|(_, n)| (n.label.clone(), n.id.clone()))
        .collect();
    ScopeInfo {
        id: me.id.clone(),
        kind: me.kind.clone(),
        label: me.label.clone(),
        path,
        siblings,
    }
}

/// The id of the nearest node before `i` in the same group with a smaller depth.
fn parent_of(tree: &[ScopeNode], i: usize) -> Option<String> {
    let me = &tree[i];
    tree[..i]
        .iter()
        .rev()
        .find(|n| n.group == me.group && n.depth < me.depth)
        .map(|n| n.id.clone())
}

/// Composes a tab.
/// Marks the panels a live frame carries: every chart, tile row, health strip and gauge set reads the newest points, so
/// it moves while the scope is watched live (the forecasts of the Capacity tab do not). The hub recomposes the whole
/// tab for each frame anyway; this only decides what the frame sends.
fn mark_live(tab: &str, mut panels: Vec<Panel>) -> Vec<Panel> {
    if tab == "capacity" {
        return panels;
    }
    for p in &mut panels {
        if matches!(
            p.body,
            Body::Chart(_) | Body::Tiles { .. } | Body::Health { .. } | Body::Gauges { .. }
        ) {
            p.live = true;
        }
    }
    panels
}

pub fn compose(q: &ViewQuery, src: &dyn Source) -> View {
    let ctx = Ctx::new(q, src);
    let tab = if crate::tabs::applies(&q.tab, &ctx.kind, &ctx.scope.id) {
        q.tab.as_str()
    } else {
        "overview"
    };
    let panels = match tab {
        "alerts" => alerts::panels(&ctx, src),
        "tokens" => tokens::panels(&ctx, src),
        "context" => context::panels(&ctx, src),
        "latency" => latency::panels(&ctx, src),
        "traces" => traces::panels(&ctx, src),
        "agents" => agents::panels(&ctx, src),
        "compute" => compute::panels(&ctx, src),
        "lifecycle" => lifecycle::panels(&ctx, src),
        "storage" => storage::panels(&ctx, src),
        "hub" => hub::panels(&ctx, src),
        "capacity" => capacity::panels(&ctx, src),
        "events" => events::panels(&ctx, src),
        "dash" => dash::panels(&ctx, src),
        _ => overview::panels(&ctx, src),
    };
    let panels = mark_live(tab, panels);
    let firing = src
        .alerts()
        .subjects
        .iter()
        .filter(|s| {
            matches!(
                s.2,
                apz_metrics::alerts::State::Firing { .. }
                    | apz_metrics::alerts::State::Clearing { .. }
            )
        })
        .count() as u32;
    let (level, stepped) = src.level();
    let tabs = crate::tabs::TABS
        .iter()
        .filter(|t| crate::tabs::applies(t.0, &ctx.kind, &ctx.scope.id))
        .map(|t| TabInfo {
            key: t.0.into(),
            label: t.1.into(),
            group: t.2.into(),
            badge: (t.0 == "alerts" && firing > 0).then_some(firing),
        })
        .collect();
    let tier = apz_metrics::query::pick_tier(ctx.to_ms - ctx.from_ms, q.width);
    let live = matches!(q.range.as_str(), "15m" | "1h") && q.shift == 0;
    View {
        tab: tab.into(),
        scope: ctx.scope.clone(),
        tabs,
        panels,
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: ctx.step_ms,
        tier: if live && q.range == "15m" {
            format!("live 2 s stream + {}", tier.words())
        } else {
            tier.words().into()
        },
        points: ctx.points as u32,
        level: serde_json::to_value(level)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
        stepped_down: stepped,
        live,
        firing,
        compare_label: ctx.cmp_label.clone(),
        annotations: ctx.annotations.clone(),
        fixture: q.fixture,
    }
}
