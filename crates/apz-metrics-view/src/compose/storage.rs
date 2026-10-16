//! The storage tab.

use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, status_of};
use crate::format::format;
use crate::model::{Body, Cell, Chart, Column, GaugeItem, Panel, Row, Series, Table, Tone};
use crate::source::{Entity, GroupBy, Read, Source};

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let mut volumes = panel(
        "volumes",
        "Volumes",
        "Space used on each volume",
        4,
        volumes(ctx, src),
    );
    volumes.metric = Some("volume.used_pct".into());

    let mut disk = panel(
        "disk-io",
        "Disk IO",
        "Reads and writes of all VMs",
        8,
        disk_io(ctx, src),
    );
    disk.metric = Some("vm.blk_read_bps".into());
    disk.live = true;

    let mut vsock = panel(
        "vsock",
        "vsock traffic",
        "Bytes between the hub and the guests",
        6,
        vsock(ctx, src),
    );
    vsock.metric = Some("vm.vsock_rx_bps".into());

    let mut workspaces = panel(
        "workspaces",
        "/workspace growth",
        "/workspace of each VM",
        6,
        workspaces(ctx, src),
    );
    workspaces.metric = Some("vm.workspace_used_mib".into());

    let mut network = panel(
        "network",
        "Host network",
        "Each node, in and out",
        6,
        network(ctx, src),
    );
    network.metric = Some("host.net_rx_bps".into());

    let mut stores = panel(
        "stores",
        "Plaza stores",
        "Size and growth per day",
        6,
        stores(ctx, src),
    );
    stores.metric = Some("store.bytes".into());

    vec![volumes, disk, vsock, workspaces, network, stores]
}

/// Gauges of up to four volumes: the used percentage at the end of the window, warn from 75 %, crit from 90 %.
fn volumes(ctx: &Ctx, src: &dyn Source) -> Body {
    let gauges: Vec<GaugeItem> = src
        .entities(&ctx.q.scope, "volume")
        .into_iter()
        .take(4)
        .filter_map(|volume| {
            let used = src.scalar(
                "volume.used_pct",
                &volume.id,
                ctx.from_ms,
                ctx.to_ms,
                AggFn::Last,
            )?;
            let frac = (used / 100.0).clamp(0.0, 1.0);
            Some(GaugeItem {
                frac,
                label: format(Unit::Percent, used),
                sub: volume.label,
                status: status_of(frac, 0.75, 0.9),
            })
        })
        .collect();
    if gauges.is_empty() {
        empty("No volumes reported in this window.")
    } else {
        Body::Gauges { gauges }
    }
}

/// The guest disk reads and writes of the scope, summed over its VMs.
fn disk_io(ctx: &Ctx, src: &dyn Source) -> Body {
    let lines = vec![
        line(
            ctx,
            src,
            "reads",
            Tone::C1,
            &ctx.q.scope,
            &["vm.blk_read_bps"],
        ),
        line(
            ctx,
            src,
            "writes",
            Tone::C2,
            &ctx.q.scope,
            &["vm.blk_write_bps"],
        ),
    ];
    chart_body(
        ctx,
        Unit::BytesPerSec,
        lines,
        true,
        200,
        "No disk traffic in this window.",
    )
}

/// The vsock traffic of the scope: towards the guests, and from them.
fn vsock(ctx: &Ctx, src: &dyn Source) -> Body {
    let lines = vec![
        line(
            ctx,
            src,
            "to guests",
            Tone::C1,
            &ctx.q.scope,
            &["vm.vsock_rx_bps"],
        ),
        line(
            ctx,
            src,
            "from guests",
            Tone::C2,
            &ctx.q.scope,
            &["vm.vsock_tx_bps"],
        ),
    ];
    chart_body(
        ctx,
        Unit::BytesPerSec,
        lines,
        false,
        180,
        "No vsock traffic in this window.",
    )
}

/// The host network of each node under the scope: its inbound and outbound bytes summed.
fn network(ctx: &Ctx, src: &dyn Source) -> Body {
    let nodes = if ctx.kind == "node" {
        vec![Entity {
            id: ctx.q.scope.clone(),
            label: ctx.scope.label.clone(),
            state: String::new(),
        }]
    } else {
        src.entities(&ctx.q.scope, "node")
    };
    let lines: Vec<(Series, Option<Series>)> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let (mut series, ghost) = line(
                ctx,
                src,
                &node.label,
                Tone::chart(i),
                &node.id,
                &["host.net_rx_bps", "host.net_tx_bps"],
            );
            series.scope = Some(node.id.clone());
            (series, ghost)
        })
        .collect();
    chart_body(
        ctx,
        Unit::BytesPerSec,
        lines,
        false,
        180,
        "No node traffic in this window.",
    )
}

/// One row per VM under the scope: the agent placed on it, its /workspace used now, the growth over the window and
/// a sparkline.
fn workspaces(ctx: &Ctx, src: &dyn Source) -> Body {
    let rows: Vec<Row> = src
        .entities(&ctx.q.scope, "vm")
        .into_iter()
        .map(|vm| {
            let agent = match src.entities(&vm.id, "agent").into_iter().next() {
                Some(a) => Cell::Named {
                    id: a.id.rsplit(':').next().unwrap_or(&a.id).to_string(),
                    name: a.label,
                },
                None => Cell::Muted { text: "—".into() },
            };
            let read = Read {
                metric: "vm.workspace_used_mib".into(),
                scope: vm.id.clone(),
                from_ms: ctx.from_ms,
                to_ms: ctx.to_ms,
                step_ms: spark_step(ctx),
                agg: AggFn::Last,
                group: GroupBy::All,
                top: None,
            };
            let values = src
                .series(&read)
                .into_iter()
                .next()
                .map(|g| g.values)
                .unwrap_or_default();
            let first = values.iter().flatten().next().copied();
            let last = values.iter().rev().flatten().next().copied();
            let used = last
                .map(|v| format(Unit::Mib, v))
                .unwrap_or_else(|| "—".into());
            let growth = match (first, last) {
                (Some(a), Some(b)) => signed(Unit::Mib, b - a),
                _ => "—".into(),
            };
            Row {
                cells: vec![
                    Cell::Mono { text: vm.label },
                    agent,
                    Cell::Num { text: used },
                    Cell::Num { text: growth },
                    Cell::Spark {
                        values: values.iter().map(|v| v.unwrap_or(0.0)).collect(),
                        tone: Tone::C1,
                    },
                ],
                scope: Some(vm.id),
                tab: None,
                trace: None,
                selected: false,
            }
        })
        .collect();
    if rows.is_empty() {
        empty("No VM workspaces in this window.")
    } else {
        Body::Table(Table {
            columns: vec![
                column("VM", false),
                column("Agent", false),
                column("Used", true),
                column("Growth", true),
                column("Trend", false),
            ],
            rows,
            empty: String::new(),
        })
    }
}

/// One row per store (the service entities whose id starts with `service:store:`): its size now, its growth per day
/// over the window and how long its metrics table keeps data.
fn stores(ctx: &Ctx, src: &dyn Source) -> Body {
    let days = (ctx.to_ms - ctx.from_ms) as f64 / 86_400_000.0;
    let rows: Vec<Row> = src
        .entities(&ctx.q.scope, "service")
        .into_iter()
        .filter(|store| store.id.starts_with("service:store:"))
        .map(|store| {
            let read = metric_read(ctx, "store.bytes", &store.id, AggFn::Last);
            let values = src
                .series(&read)
                .into_iter()
                .next()
                .map(|g| g.values)
                .unwrap_or_default();
            let first = values.iter().flatten().next().copied();
            let last = values.iter().rev().flatten().next().copied();
            let size = last
                .map(|v| format(Unit::Bytes, v))
                .unwrap_or_else(|| "—".into());
            let growth = match (first, last) {
                (Some(a), Some(b)) if days > 0.0 => signed(Unit::Bytes, (b - a) / days),
                _ => "—".into(),
            };
            Row {
                cells: vec![
                    Cell::Text {
                        text: store.label.clone(),
                    },
                    Cell::Num { text: size },
                    Cell::Num { text: growth },
                    Cell::Muted {
                        text: retention_note(&store.label).into(),
                    },
                ],
                scope: Some(store.id),
                tab: None,
                trace: None,
                selected: false,
            }
        })
        .collect();
    if rows.is_empty() {
        empty("No stores reported in this window.")
    } else {
        Body::Table(Table {
            columns: vec![
                column("Store", false),
                column("Size", true),
                column("Growth / day", true),
                column("Note", false),
            ],
            rows,
            empty: String::new(),
        })
    }
}

/// The retention a store's table keeps, from its label: the metrics tables by their grain (10 s, 1 m, 1 h, 1 d),
/// the logs and traces by their cap; nothing for the other stores.
fn retention_note(label: &str) -> &'static str {
    let label = label.to_lowercase();
    if label.contains("metrics") {
        if label.contains("1m") {
            "14 d kept"
        } else if label.contains("1h") {
            "400 d kept"
        } else if label.contains("1d") {
            "90 d kept"
        } else {
            "48 h kept"
        }
    } else if label.contains("log") || label.contains("trace") {
        "7 d · 2 GB cap"
    } else {
        ""
    }
}

/// A read of one metric in one scope over the view's window and step, summed over the scope's entities.
fn metric_read(ctx: &Ctx, metric: &str, scope: &str, agg: AggFn) -> Read {
    Read {
        metric: metric.into(),
        scope: scope.into(),
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: ctx.step_ms,
        agg,
        group: GroupBy::All,
        top: None,
    }
}

/// The step of a sparkline: 40 points over the window.
fn spark_step(ctx: &Ctx) -> u64 {
    ((ctx.to_ms - ctx.from_ms) / 40).max(1_000)
}

/// One chart line: the per-step sum of the metrics in `scope` (averaged over the step), and the same over the
/// comparison window as a dashed ghost when the view compares.
fn line(
    ctx: &Ctx,
    src: &dyn Source,
    name: &str,
    tone: Tone,
    scope: &str,
    metrics: &[&str],
) -> (Series, Option<Series>) {
    let reads: Vec<Read> = metrics
        .iter()
        .map(|metric| metric_read(ctx, metric, scope, AggFn::Avg))
        .collect();
    let values = summed(src, &reads, ctx.points);
    let ghost = ctx.comparing().then(|| {
        let before: Vec<Read> = reads
            .iter()
            .map(|read| Read {
                from_ms: ctx.cmp_from_ms,
                to_ms: ctx.cmp_to_ms,
                ..read.clone()
            })
            .collect();
        Series {
            name: format!("{} · {}", name, ctx.cmp_label),
            tone,
            dash: true,
            values: summed(src, &before, ctx.points),
            gap_reason: None,
            scope: None,
        }
    });
    (
        Series {
            name: name.into(),
            tone,
            dash: false,
            values,
            gap_reason: None,
            scope: None,
        },
        ghost,
    )
}

/// The per-step sum of several reads. A step is a gap where no read has a value.
fn summed(src: &dyn Source, reads: &[Read], points: usize) -> Vec<Option<f64>> {
    let mut total: Vec<Option<f64>> = vec![None; points];
    for read in reads {
        let Some(group) = src.series(read).into_iter().next() else {
            continue;
        };
        for (sum, value) in total.iter_mut().zip(&group.values) {
            if let Some(v) = value {
                *sum = Some(sum.unwrap_or(0.0) + v);
            }
        }
    }
    total
}

/// A time chart of lines and their ghosts, or the empty state when no line has a value.
fn chart_body(
    ctx: &Ctx,
    unit: Unit,
    lines: Vec<(Series, Option<Series>)>,
    area: bool,
    height: u16,
    empty_text: &str,
) -> Body {
    let mut series = Vec::new();
    let mut ghosts = Vec::new();
    for (line, ghost) in lines {
        series.push(line);
        ghosts.extend(ghost);
    }
    if series.iter().all(|s| s.values.iter().all(|v| v.is_none())) {
        return empty(empty_text);
    }
    Body::Chart(Chart {
        unit,
        x: ctx.x(),
        series,
        stacked: false,
        area,
        y_max: None,
        band: None,
        ghosts,
        markers: Vec::new(),
        limit: None,
        height,
        annotate: true,
        point_tab: None,
    })
}

/// A column of a table.
fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// A value with a sign: "+4 GiB" for a rise, "-4 GiB" for a fall.
fn signed(unit: Unit, v: f64) -> String {
    if v >= 0.0 {
        format!("+{}", format(unit, v))
    } else {
        format(unit, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::ViewQuery;

    fn storage_of_global() -> Vec<Panel> {
        let src = Fixture::new(1_760_600_000_000);
        let q = ViewQuery::new("storage", "global", "24h");
        compose(&q, &src).panels
    }

    #[test]
    fn the_storage_tab_has_its_six_panels_in_spec_order_and_spans() {
        let panels = storage_of_global();
        let ids: Vec<&str> = panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "volumes",
                "disk-io",
                "vsock",
                "workspaces",
                "network",
                "stores"
            ]
        );
        let spans: Vec<u8> = panels.iter().map(|p| p.span).collect();
        assert_eq!(spans, [4, 8, 6, 6, 6, 6]);
    }

    #[test]
    fn each_storage_panel_holds_its_own_body_or_an_empty_state() {
        let panels = storage_of_global();
        for panel in &panels {
            let ok = match panel.id.as_str() {
                "volumes" => matches!(panel.body, Body::Gauges { .. } | Body::Empty { .. }),
                "disk-io" | "vsock" | "network" => {
                    matches!(panel.body, Body::Chart(_) | Body::Empty { .. })
                }
                "workspaces" | "stores" => {
                    matches!(panel.body, Body::Table(_) | Body::Empty { .. })
                }
                other => panic!("unexpected panel {other}"),
            };
            assert!(ok, "panel {} has the wrong body", panel.id);
        }
    }

    #[test]
    fn a_store_label_names_the_retention_of_its_metrics_table() {
        assert_eq!(retention_note("postgres · metrics 1m"), "14 d kept");
        assert_eq!(retention_note("garage (objects)"), "");
    }
}
