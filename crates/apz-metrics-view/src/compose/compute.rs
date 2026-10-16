//! The compute tab: what the nodes and the VMs in scope are doing, and how hard they are working.

use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, scope_info, status_of};
use crate::format::format;
use crate::model::*;
use crate::source::{Entity, GroupBy, Read, Source};

/// The height of the time charts on this tab, in pixels.
const CHART_HEIGHT: u16 = 180;

/// The memory of one VM in the density bars, in MiB.
const VM_MIB: f64 = 2048.0;

/// The most processes the table lists.
const PROCESS_ROWS: usize = 12;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let nodes = nodes_of(ctx, src);
    vec![
        cpu(ctx, src),
        memory(ctx, src),
        pressure(ctx, src),
        gpu(ctx, src),
        density(ctx, src, &nodes),
        services(ctx, src),
        processes(src, &nodes),
        vms(ctx, src),
    ]
}

fn cpu(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read("host.cpu_pct", AggFn::Avg, GroupBy::Entity);
    let body = grouped_chart(
        ctx,
        src,
        &read,
        Unit::Percent,
        Some(100.0),
        false,
        "No nodes report CPU in this scope.",
    );
    let mut p = panel("cpu", "CPU", "host CPU share, one line per node", 6, body);
    p.live = true;
    p
}

fn memory(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read("host.mem_used_mib", AggFn::Avg, GroupBy::Entity);
    let body = grouped_chart(
        ctx,
        src,
        &read,
        Unit::Mib,
        None,
        true,
        "No nodes report memory in this scope.",
    );
    let mut p = panel(
        "memory",
        "Memory",
        "host memory in use, one area per node",
        6,
        body,
    );
    p.live = true;
    p
}

fn pressure(ctx: &Ctx, src: &dyn Source) -> Panel {
    let body = lines_chart(
        ctx,
        src,
        &[
            ("host.psi_cpu_pct", "CPU", Tone::C1),
            ("host.psi_mem_pct", "memory", Tone::C2),
            ("host.psi_io_pct", "I/O", Tone::C3),
        ],
        Unit::Percent,
        Some(100.0),
        "No pressure readings from the nodes in scope.",
    );
    panel(
        "pressure",
        "Pressure",
        "share of time work waited for CPU, memory or I/O",
        4,
        body,
    )
}

fn gpu(ctx: &Ctx, src: &dyn Source) -> Panel {
    let body = lines_chart(
        ctx,
        src,
        &[
            ("host.gpu_util_pct", "utilisation", Tone::C1),
            ("host.gpu_mem_pct", "memory", Tone::C2),
        ],
        Unit::Percent,
        Some(100.0),
        "No GPU on the nodes in scope.",
    );
    panel(
        "gpu",
        "GPU",
        "utilisation and memory, averaged over the GPUs",
        4,
        body,
    )
}

fn density(ctx: &Ctx, src: &dyn Source, nodes: &[Entity]) -> Panel {
    let rows: Vec<BarRow> = nodes
        .iter()
        .map(|node| {
            let vms = src.entities(&node.id, "vm");
            let running = vms.iter().filter(|vm| vm.state == "running").count();
            let asleep = vms.iter().filter(|vm| vm.state == "asleep").count();
            let memory = src.scalar(
                "host.mem_total_mib",
                &node.id,
                ctx.from_ms,
                ctx.to_ms,
                AggFn::Last,
            );
            let fit = memory.map(|m| (m / VM_MIB).floor()).unwrap_or(0.0);
            let frac = if fit > 0.0 { running as f64 / fit } else { 0.0 };
            BarRow {
                label: node.label.clone(),
                value: format!("{running} running · {asleep} asleep"),
                frac: frac.min(1.0),
                status: status_of(frac, 0.8, 1.0),
                note: if fit > 0.0 {
                    format!("fits {} VMs of 2 GiB", fit as u64)
                } else {
                    "memory not reported".into()
                },
            }
        })
        .collect();
    let body = if rows.is_empty() {
        empty("No nodes in this scope.")
    } else {
        Body::Bars {
            rows,
            note: "Bars are running VMs over the VMs that fit each node's memory at 2 GiB each."
                .into(),
        }
    };
    panel(
        "density",
        "VM density",
        "running and asleep VMs per node",
        4,
        body,
    )
}

fn services(ctx: &Ctx, src: &dyn Source) -> Panel {
    let services = src.entities(&ctx.q.scope, "service");
    let body = if services.is_empty() {
        empty("No services report in this scope.")
    } else {
        let rows = services
            .iter()
            .map(|s| {
                row(
                    vec![
                        Cell::Text {
                            text: s.label.clone(),
                        },
                        Cell::Text {
                            text: node_of(src, &s.id),
                        },
                        num(
                            src.scalar("svc.cpu_pct", &s.id, ctx.from_ms, ctx.to_ms, AggFn::Avg),
                            Unit::Percent,
                        ),
                        num(
                            src.scalar("svc.rss_mib", &s.id, ctx.from_ms, ctx.to_ms, AggFn::Last),
                            Unit::Mib,
                        ),
                        Cell::Spark {
                            values: trend(ctx, src, &s.id, "svc.cpu_pct", AggFn::Avg),
                            tone: Tone::C1,
                        },
                    ],
                    None,
                )
            })
            .collect();
        table(
            vec![
                column("Service", false),
                column("Node", false),
                column("CPU", true),
                column("RSS", true),
                column("CPU trend", false),
            ],
            rows,
        )
    };
    panel(
        "services",
        "Plaza services",
        "services in scope, with their CPU and memory",
        8,
        body,
    )
}

fn processes(src: &dyn Source, nodes: &[Entity]) -> Panel {
    let mut found: Vec<(String, String, f64, f64)> = Vec::new();
    for node in nodes {
        let report = src.report(&node.id);
        let Some(list) = report.get("top_processes").and_then(|v| v.as_array()) else {
            continue;
        };
        for item in list {
            let Some(fields) = item.as_array() else {
                continue;
            };
            let text = |i: usize| {
                fields
                    .get(i)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let number = |i: usize| fields.get(i).and_then(|v| v.as_f64());
            if let (Some(cpu), Some(rss)) = (number(2), number(3)) {
                found.push((text(0), text(1), cpu, rss));
            }
        }
    }
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    found.truncate(PROCESS_ROWS);
    let body = if found.is_empty() {
        empty("No process reports from the nodes in scope.")
    } else {
        let rows = found
            .into_iter()
            .map(|(process, vm, cpu, rss)| {
                row(
                    vec![
                        Cell::Mono { text: process },
                        Cell::Mono { text: vm },
                        num(Some(cpu), Unit::Percent),
                        num(Some(rss), Unit::Mib),
                    ],
                    None,
                )
            })
            .collect();
        table(
            vec![
                column("Process", false),
                column("VM", false),
                column("CPU", true),
                column("RSS", true),
            ],
            rows,
        )
    };
    panel(
        "processes",
        "Top guest processes",
        "by CPU, from the latest node reports",
        4,
        body,
    )
}

fn vms(ctx: &Ctx, src: &dyn Source) -> Panel {
    let vms = src.entities(&ctx.q.scope, "vm");
    let body = if vms.is_empty() {
        empty("No VMs in this scope.")
    } else {
        let rows = vms
            .iter()
            .map(|vm| {
                let asleep = vm.state == "asleep";
                let cpu = if asleep {
                    None
                } else {
                    src.scalar("vm.cpu_pct", &vm.id, ctx.from_ms, ctx.to_ms, AggFn::Avg)
                };
                let rss = src.scalar("vm.rss_mib", &vm.id, ctx.from_ms, ctx.to_ms, AggFn::Last);
                let used = src.scalar(
                    "vm.guest_used_mib",
                    &vm.id,
                    ctx.from_ms,
                    ctx.to_ms,
                    AggFn::Last,
                );
                let total = src.scalar(
                    "vm.guest_total_mib",
                    &vm.id,
                    ctx.from_ms,
                    ctx.to_ms,
                    AggFn::Last,
                );
                let guest = match (used, total) {
                    (Some(u), Some(t)) => {
                        format!("{} / {}", format(Unit::Mib, u), format(Unit::Mib, t))
                    }
                    _ => "—".into(),
                };
                let trend_cell = if asleep {
                    Cell::Muted {
                        text: "not sampled".into(),
                    }
                } else {
                    Cell::Spark {
                        values: trend(ctx, src, &vm.id, "vm.cpu_pct", AggFn::Avg),
                        tone: Tone::C1,
                    }
                };
                row(
                    vec![
                        Cell::Mono {
                            text: vm.label.clone(),
                        },
                        Cell::Text {
                            text: node_of(src, &vm.id),
                        },
                        Cell::Pill {
                            text: vm.state.clone(),
                            status: vm_status(&vm.state),
                        },
                        num(cpu, Unit::Percent),
                        trend_cell,
                        num(rss, Unit::Mib),
                        Cell::Num { text: guest },
                    ],
                    Some(vm.id.clone()),
                )
            })
            .collect();
        table(
            vec![
                column("VM", false),
                column("Node", false),
                column("State", false),
                column("CPU", true),
                column("CPU trend", false),
                column("VMM RSS", true),
                column("Guest used", true),
            ],
            rows,
        )
    };
    let mut p = panel("vms", "VMs", "one row per VM in scope", 12, body);
    p.right = "asleep VMs are not sampled".into();
    p
}

/// A chart of one grouped read (CPU, memory per node), or the empty state when the read has no series.
fn grouped_chart(
    ctx: &Ctx,
    src: &dyn Source,
    read: &Read,
    unit: Unit,
    y_max: Option<f64>,
    area: bool,
    text: &str,
) -> Body {
    let mut chart = ctx.chart(src, read, unit, false, CHART_HEIGHT);
    if chart.series.is_empty() {
        return empty(text);
    }
    mark_gaps(&mut chart.series);
    chart.y_max = y_max;
    chart.area = area;
    Body::Chart(chart)
}

/// A chart with one series per metric over the whole scope, or the empty state when no metric has data. Each
/// series is dashed over the comparison window when the view compares.
fn lines_chart(
    ctx: &Ctx,
    src: &dyn Source,
    lines: &[(&str, &str, Tone)],
    unit: Unit,
    y_max: Option<f64>,
    text: &str,
) -> Body {
    let mut series = Vec::new();
    let mut ghosts = Vec::new();
    for &(metric, name, tone) in lines {
        let read = ctx.read(metric, AggFn::Avg, GroupBy::All);
        let Some(group) = src.series(&read).into_iter().next() else {
            continue;
        };
        if ctx.comparing() {
            if let Some(before) = src.series(&ctx.cmp_read(&read)).into_iter().next() {
                ghosts.push(Series {
                    name: format!("{name} · {}", ctx.cmp_label),
                    tone,
                    dash: true,
                    values: before.values,
                    gap_reason: None,
                    scope: None,
                });
            }
        }
        series.push(Series {
            name: name.into(),
            tone,
            dash: false,
            values: group.values,
            gap_reason: None,
            scope: None,
        });
    }
    if series.is_empty() {
        return empty(text);
    }
    mark_gaps(&mut series);
    Body::Chart(Chart {
        unit,
        x: ctx.x(),
        series,
        stacked: false,
        area: false,
        y_max,
        band: None,
        ghosts,
        markers: Vec::new(),
        limit: None,
        height: CHART_HEIGHT,
        annotate: true,
        point_tab: None,
    })
}

/// Names the gap of each series that has one: the series is offline there.
fn mark_gaps(series: &mut [Series]) {
    for s in series.iter_mut() {
        if s.values.iter().any(Option::is_none) {
            s.gap_reason = Some(format!("{} offline", s.name));
        }
    }
}

/// The nodes in scope: the nodes under it, or the scope itself when it is a node with none beneath it.
fn nodes_of(ctx: &Ctx, src: &dyn Source) -> Vec<Entity> {
    let nodes = src.entities(&ctx.q.scope, "node");
    if nodes.is_empty() && ctx.kind == "node" {
        return vec![Entity {
            id: ctx.q.scope.clone(),
            label: ctx.scope.label.clone(),
            state: String::new(),
        }];
    }
    nodes
}

/// The label of the node a scope sits under, or a dash when the tree has none.
fn node_of(src: &dyn Source, id: &str) -> String {
    scope_info(id, src)
        .path
        .into_iter()
        .find(|(_, scope)| scope.starts_with("node:"))
        .map(|(label, _)| label)
        .unwrap_or_else(|| "—".into())
}

/// The sparkline of a metric in another scope than the view's.
fn trend(ctx: &Ctx, src: &dyn Source, scope: &str, metric: &str, agg: AggFn) -> Vec<f64> {
    let mut at = ctx.clone();
    at.q.scope = scope.into();
    at.spark(src, metric, agg)
}

/// The status of a VM's state: running is ok, paused warns, and asleep (or anything else) is muted.
fn vm_status(state: &str) -> Status {
    match state {
        "running" => Status::Ok,
        "paused" => Status::Warn,
        _ => Status::Mute,
    }
}

/// A formatted number cell, or a dash when there is no reading.
fn num(value: Option<f64>, unit: Unit) -> Cell {
    Cell::Num {
        text: value.map(|v| format(unit, v)).unwrap_or_else(|| "—".into()),
    }
}

fn row(cells: Vec<Cell>, scope: Option<String>) -> Row {
    Row {
        cells,
        scope,
        tab: None,
        trace: None,
        selected: false,
    }
}

fn column(head: &str, numeric: bool) -> Column {
    Column {
        head: head.into(),
        num: numeric,
    }
}

fn table(columns: Vec<Column>, rows: Vec<Row>) -> Body {
    Body::Table(Table {
        columns,
        rows,
        empty: String::new(),
        more: None,
    })
}

#[cfg(test)]
mod tests {
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::{Body, View, ViewQuery};

    const NOW: u64 = 1_760_600_000_000;

    fn query(scope: &str) -> ViewQuery {
        ViewQuery {
            tab: "compute".into(),
            scope: scope.into(),
            range: "24h".into(),
            shift: 0,
            zoom: None,
            compare: "none".into(),
            group: "provider".into(),
            trace: None,
            filter: None,
            width: 800,
            tz: 0,
            fixture: true,
        }
    }

    /// Every panel of the tab is there, in order, with a body of the variant it can show.
    fn assert_every_panel(view: &View) {
        assert_eq!(view.tab, "compute");
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "cpu",
                "memory",
                "pressure",
                "gpu",
                "density",
                "services",
                "processes",
                "vms"
            ]
        );
        for panel in &view.panels {
            let shown = match panel.id.as_str() {
                "cpu" | "memory" | "pressure" | "gpu" => {
                    matches!(panel.body, Body::Chart(_) | Body::Empty { .. })
                }
                "density" => matches!(panel.body, Body::Bars { .. } | Body::Empty { .. }),
                _ => matches!(panel.body, Body::Table(_) | Body::Empty { .. }),
            };
            assert!(shown, "panel {} has the wrong body variant", panel.id);
        }
    }

    #[test]
    fn the_compute_tab_has_every_panel_at_global_scope() {
        let view = compose(&query("global"), &Fixture::new(NOW));
        assert_every_panel(&view);
    }

    #[test]
    fn the_compute_tab_has_every_panel_at_a_node() {
        let view = compose(&query("node:n1"), &Fixture::new(NOW));
        assert_every_panel(&view);
    }
}
