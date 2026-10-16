//! The lifecycle tab: how long VMs take to boot, wake, fork and move, and what state each VM is in over the window.

use apz_metrics::Event;
use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, series_of};
use crate::compare::change_pct;
use crate::format::format;
use crate::model::{
    Body, Cell, Chart, Column, Lane, LegendItem, Panel, Row, Seg, Series, SplitRow, Swimlanes,
    Table, Tile, Tone,
};
use crate::source::{GroupBy, Source};

/// The events the timeline cuts lanes from.
const TIMELINE_KINDS: &[&str] = &["boot", "wake", "sleep", "pause", "resume", "exit"];

/// The most VMs one timeline shows.
const MAX_LANES: usize = 12;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    vec![
        panel(
            "tiles",
            "",
            "Median and p95 of each step over the window",
            12,
            tiles(ctx, src),
        ),
        panel(
            "wake-split",
            "Where wake time goes",
            "Restore and harness-ready time, median",
            6,
            wake_split(ctx, src),
        ),
        panel(
            "wake",
            "Wake latency over time",
            "Wake request until the agent is ready",
            6,
            wake(ctx, src),
        ),
        panel(
            "timeline",
            "VM timeline",
            "State of each VM over the window",
            12,
            timeline(ctx, src),
        ),
        panel(
            "snapshots",
            "Snapshots",
            "Memory snapshots, pauses and writes",
            6,
            snapshots(ctx, src),
        ),
        panel(
            "sleep",
            "Sleep policy effect",
            "Running and asleep VMs over the window",
            6,
            sleep(ctx, src),
        ),
    ]
}

/// The stat tiles: boot (median and p95), resume, wake, template fork, sandbox create, and agent moves.
fn tiles(ctx: &Ctx, src: &dyn Source) -> Body {
    let mut moves = ctx.tile(
        src,
        "Agent move",
        "life.move_ms",
        AggFn::P50,
        Unit::Ms,
        true,
        Tone::C6,
        "lifecycle",
    );
    let move_count = src
        .events(&ctx.q.scope, ctx.from_ms, ctx.to_ms, &["move"], 500)
        .len();
    moves.note = format!("{move_count} moves");
    Body::Tiles {
        tiles: vec![
            boot_tile(ctx, src),
            ctx.tile(
                src,
                "Resume snapshot",
                "life.resume_ms",
                AggFn::P50,
                Unit::Ms,
                true,
                Tone::C2,
                "lifecycle",
            ),
            ctx.tile(
                src,
                "Wake (sleep → ready)",
                "life.wake_ms",
                AggFn::P50,
                Unit::Ms,
                true,
                Tone::C5,
                "lifecycle",
            ),
            ctx.tile(
                src,
                "Template fork",
                "life.fork_ms",
                AggFn::P50,
                Unit::Ms,
                true,
                Tone::C4,
                "lifecycle",
            ),
            ctx.tile(
                src,
                "Sandbox create",
                "life.sandbox_create_ms",
                AggFn::P50,
                Unit::Ms,
                true,
                Tone::C6,
                "lifecycle",
            ),
            moves,
        ],
    }
}

/// The boot tile: the median and the p95 of the boot time, joined as "410 ms / 690 ms". The change is the median
/// against the comparison window.
fn boot_tile(ctx: &Ctx, src: &dyn Source) -> Tile {
    let scope = ctx.q.scope.as_str();
    let p50 = src.scalar("life.boot_ms", scope, ctx.from_ms, ctx.to_ms, AggFn::P50);
    let p95 = src.scalar("life.boot_ms", scope, ctx.from_ms, ctx.to_ms, AggFn::P95);
    let before = if ctx.comparing() {
        src.scalar(
            "life.boot_ms",
            &ctx.cmp_scope,
            ctx.cmp_from_ms,
            ctx.cmp_to_ms,
            AggFn::P50,
        )
    } else {
        None
    };
    let value = match (p50, p95) {
        (None, None) => "—".to_string(),
        _ => format!("{} / {}", text(p50, Unit::Ms), text(p95, Unit::Ms)),
    };
    Tile {
        label: "Boot p50 / p95".into(),
        value,
        change_pct: p50.zip(before).and_then(|(now, b)| change_pct(now, b)),
        up_bad: true,
        note: String::new(),
        spark: ctx.spark(src, "life.boot_ms", AggFn::P50),
        tone: Tone::C1,
        tab: Some("lifecycle".into()),
    }
}

/// The wake split: per run (and sandbox), the restore part and the harness-ready part of a wake, medians.
fn wake_split(ctx: &Ctx, src: &dyn Source) -> Body {
    let scope = ctx.q.scope.as_str();
    let runs = src
        .entities(scope, "run")
        .into_iter()
        .chain(src.entities(scope, "sandboxes"));
    let mut rows = Vec::new();
    for run in runs {
        let restore = src.scalar(
            "life.wake_restore_ms",
            &run.id,
            ctx.from_ms,
            ctx.to_ms,
            AggFn::P50,
        );
        let ready = src.scalar(
            "life.wake_ready_ms",
            &run.id,
            ctx.from_ms,
            ctx.to_ms,
            AggFn::P50,
        );
        if restore.is_none() && ready.is_none() {
            continue;
        }
        let restore = restore.unwrap_or(0.0);
        let ready = ready.unwrap_or(0.0);
        rows.push(SplitRow {
            label: run.label,
            parts: vec![
                (restore, Tone::C5, "restore".into()),
                (ready, Tone::C3, "harness ready".into()),
            ],
            total: format(Unit::Ms, restore + ready),
        });
    }
    if rows.is_empty() {
        return empty("No wakes in this window.");
    }
    Body::Split {
        rows,
        legend: vec![
            LegendItem {
                label: "Restore".into(),
                tone: Tone::C5,
            },
            LegendItem {
                label: "Harness ready".into(),
                tone: Tone::C3,
            },
        ],
    }
}

/// The wake time over the window: the p95 and the p50 of the wake, with the comparison overlay of the p95.
fn wake(ctx: &Ctx, src: &dyn Source) -> Body {
    let mut chart = ctx.chart(
        src,
        &ctx.read("life.wake_ms", AggFn::P95, GroupBy::All),
        Unit::Ms,
        false,
        200,
    );
    chart.series = [(AggFn::P95, "P95", Tone::C3), (AggFn::P50, "P50", Tone::C5)]
        .into_iter()
        .filter_map(|(agg, name, tone)| {
            src.series(&ctx.read("life.wake_ms", agg, GroupBy::All))
                .into_iter()
                .next()
                .map(|g| Series {
                    name: name.into(),
                    ..series_of(&g, tone)
                })
        })
        .collect();
    chart_or_empty(chart, "No wakes in this window.")
}

/// The timeline: one lane per VM (at most 12), cut into the states the VM was in over the window.
fn timeline(ctx: &Ctx, src: &dyn Source) -> Body {
    let lanes: Vec<Lane> = src
        .entities(&ctx.q.scope, "vm")
        .into_iter()
        .take(MAX_LANES)
        .map(|vm| {
            // The source keeps events newest first; the cut reads them oldest first.
            let mut events = src.events(&vm.id, ctx.from_ms, ctx.to_ms, TIMELINE_KINDS, 500);
            events.reverse();
            let state = if vm.state.is_empty() {
                "running"
            } else {
                vm.state.as_str()
            };
            let mut segs = cut_segments(&events, ctx.from_ms, ctx.to_ms, state);
            for seg in &mut segs {
                seg.detail = format!(
                    "{} · {} · {}",
                    vm.label,
                    seg.state,
                    duration_text(seg.end_ms - seg.start_ms)
                );
            }
            Lane {
                name: vm.label.clone(),
                scope: Some(vm.id.clone()),
                segs,
            }
        })
        .collect();
    if lanes.is_empty() {
        return empty("No VMs in this scope.");
    }
    Body::Swimlanes(Swimlanes {
        start_ms: ctx.from_ms,
        end_ms: ctx.to_ms,
        lanes,
        legend: legend(),
    })
}

/// The memory snapshots of each run: size, pause, write time and how many were taken.
fn snapshots(ctx: &Ctx, src: &dyn Source) -> Body {
    let mut rows = Vec::new();
    for run in src.entities(&ctx.q.scope, "run") {
        let size = src.scalar(
            "life.snapshot_mib",
            &run.id,
            ctx.from_ms,
            ctx.to_ms,
            AggFn::Last,
        );
        let paused = src.scalar("life.pause_ms", &run.id, ctx.from_ms, ctx.to_ms, AggFn::P50);
        let written = src.scalar(
            "life.snapshot_ms",
            &run.id,
            ctx.from_ms,
            ctx.to_ms,
            AggFn::P50,
        );
        let taken = src
            .events(&run.id, ctx.from_ms, ctx.to_ms, &["snapshot"], 500)
            .len();
        if size.is_none() && paused.is_none() && written.is_none() && taken == 0 {
            continue;
        }
        rows.push(Row {
            cells: vec![
                Cell::Named {
                    name: run.label.clone(),
                    id: run.id.clone(),
                },
                Cell::Num {
                    text: text(size, Unit::Mib),
                },
                Cell::Num {
                    text: text(paused, Unit::Ms),
                },
                Cell::Num {
                    text: text(written, Unit::Ms),
                },
                Cell::Num {
                    text: taken.to_string(),
                },
            ],
            scope: Some(run.id.clone()),
            tab: None,
            trace: None,
            selected: false,
        });
    }
    if rows.is_empty() {
        return empty("No snapshots in this window.");
    }
    Body::Table(Table {
        columns: vec![
            column("Run", false),
            column("Memory snapshot", true),
            column("Paused for", true),
            column("Written in", true),
            column("Taken", true),
        ],
        rows,
        empty: "No snapshots in this window.".into(),
    })
}

/// The running and asleep VMs over the window, by state.
fn sleep(ctx: &Ctx, src: &dyn Source) -> Body {
    let series: Vec<Series> = src
        .series(&ctx.read("vm.count", AggFn::Last, GroupBy::Label("state".into())))
        .iter()
        .filter_map(|g| {
            let tone = match g.name.as_str() {
                "running" => Tone::C2,
                "asleep" => Tone::Ink3,
                _ => return None,
            };
            Some(series_of(g, tone))
        })
        .collect();
    let chart = Chart {
        unit: Unit::Count,
        x: ctx.x(),
        series,
        stacked: false,
        area: false,
        y_max: None,
        band: None,
        ghosts: Vec::new(),
        markers: Vec::new(),
        limit: None,
        height: 200,
        annotate: true,
        point_tab: None,
    };
    chart_or_empty(chart, "No VMs in this scope.")
}

/// The chart when any of its series has a value, else the empty state.
fn chart_or_empty(chart: Chart, text: &str) -> Body {
    if chart
        .series
        .iter()
        .any(|s| s.values.iter().any(|v| v.is_some()))
    {
        Body::Chart(chart)
    } else {
        empty(text)
    }
}

/// A value as text, or a dash when there is none.
fn text(v: Option<f64>, unit: Unit) -> String {
    v.map(|v| format(unit, v)).unwrap_or_else(|| "—".into())
}

/// A table column with its heading.
fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// The legend of the timeline's states.
fn legend() -> Vec<LegendItem> {
    [
        ("Running", Tone::C2),
        ("Paused", Tone::C3),
        ("Asleep", Tone::Rule),
        ("Booting", Tone::C1),
        ("Waking", Tone::C5),
        ("Exited", Tone::Ink3),
    ]
    .into_iter()
    .map(|(label, tone)| LegendItem {
        label: label.into(),
        tone,
    })
    .collect()
}

/// The colour of a timeline state.
fn state_tone(state: &str) -> Tone {
    match state {
        "running" => Tone::C2,
        "paused" => Tone::C3,
        "asleep" => Tone::Rule,
        "booting" => Tone::C1,
        "waking" => Tone::C5,
        _ => Tone::Ink3,
    }
}

/// The state an event moves a VM into, when it changes the state.
fn next_state(e: &Event) -> Option<&'static str> {
    match e.kind.as_str() {
        "boot" => Some("booting"),
        "wake" => Some("waking"),
        "sleep" => Some("asleep"),
        "pause" => Some("paused"),
        "resume" => Some("running"),
        "exit" => Some("exited"),
        _ => None,
    }
}

/// The state a VM was in before its first event, as far as that event tells. A boot has no earlier state.
fn state_before(kind: &str) -> Option<&'static str> {
    match kind {
        "wake" => Some("asleep"),
        "resume" => Some("paused"),
        "sleep" | "pause" | "exit" => Some("running"),
        _ => None,
    }
}

/// The duration an event carries, in whole milliseconds (zero when it has none).
fn duration_ms(e: &Event) -> u64 {
    e.duration_ms
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| d as u64)
        .unwrap_or(0)
}

/// Appends a segment when it has a length.
fn push_seg(segs: &mut Vec<Seg>, start_ms: u64, end_ms: u64, state: &str) {
    if end_ms > start_ms {
        segs.push(Seg {
            start_ms,
            end_ms,
            state: state.into(),
            tone: state_tone(state),
            detail: String::new(),
        });
    }
}

/// Cuts the window of one VM into segments by its lifecycle events, oldest first. A boot or wake shows as booting
/// or waking for its duration, then running. A lane with no events is one segment in the entity's state.
fn cut_segments(events: &[Event], from_ms: u64, to_ms: u64, entity_state: &str) -> Vec<Seg> {
    let mut segs = Vec::new();
    if events.is_empty() {
        push_seg(&mut segs, from_ms, to_ms, entity_state);
        return segs;
    }
    // The segment open now: its state and when it started.
    let mut open = events
        .first()
        .and_then(|e| state_before(&e.kind))
        .map(|state| (state, from_ms));
    for e in events {
        let t = e.t_ms.clamp(from_ms, to_ms);
        let Some(state) = next_state(e) else {
            continue;
        };
        if let Some((open_state, start)) = open.take() {
            push_seg(&mut segs, start, t, open_state);
        }
        match e.kind.as_str() {
            "boot" | "wake" => {
                let ready = t.saturating_add(duration_ms(e));
                push_seg(&mut segs, t, ready.min(to_ms), state);
                open = Some(("running", ready));
            }
            _ => open = Some((state, t)),
        }
    }
    if let Some((state, start)) = open {
        push_seg(&mut segs, start, to_ms, state);
    }
    segs
}

/// A duration as a short phrase: "40 min", "2 h", or the millisecond format under a minute.
fn duration_text(ms: u64) -> String {
    if ms < 60_000 {
        format(Unit::Ms, ms as f64)
    } else if ms < 3_600_000 {
        format!("{} min", ms / 60_000)
    } else {
        format!("{} h", ms / 3_600_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::ViewQuery;
    use apz_metrics::EntityKind;

    fn event(t_ms: u64, kind: &str, duration_ms: Option<f64>) -> Event {
        Event {
            t_ms,
            kind: kind.into(),
            entity_kind: EntityKind::Vm,
            entity: "vm".into(),
            duration_ms,
            detail: String::new(),
            parts: Vec::new(),
        }
    }

    #[test]
    fn three_lifecycle_events_cut_a_vm_lane_into_its_segments() {
        let events = vec![
            event(1_000, "wake", Some(400.0)),
            event(2_000, "pause", None),
            event(3_000, "resume", None),
        ];
        let segs = cut_segments(&events, 0, 4_000, "running");
        let got: Vec<(u64, u64, &str)> = segs
            .iter()
            .map(|s| (s.start_ms, s.end_ms, s.state.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, 1_000, "asleep"),
                (1_000, 1_400, "waking"),
                (1_400, 2_000, "running"),
                (2_000, 3_000, "paused"),
                (3_000, 4_000, "running"),
            ]
        );
    }

    #[test]
    fn compose_of_global_lists_the_lifecycle_panels_with_their_variants() {
        let src = Fixture::new(1_760_600_000_000);
        let view = compose(&ViewQuery::new("lifecycle", "global", "24h"), &src);
        assert_eq!(view.tab, "lifecycle");
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "tiles",
                "wake-split",
                "wake",
                "timeline",
                "snapshots",
                "sleep"
            ]
        );
        for p in &view.panels {
            let right_kind = match (p.id.as_str(), &p.body) {
                (_, Body::Empty { .. }) => true,
                ("tiles", Body::Tiles { .. })
                | ("wake-split", Body::Split { .. })
                | ("wake", Body::Chart(_))
                | ("timeline", Body::Swimlanes(_))
                | ("snapshots", Body::Table(_))
                | ("sleep", Body::Chart(_)) => true,
                _ => false,
            };
            assert!(right_kind, "panel {} has the wrong body", p.id);
        }
    }
}
