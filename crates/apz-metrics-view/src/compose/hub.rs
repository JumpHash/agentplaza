//! The hub tab: the API, the database, the node links, the event bus and the services outside the hub.

use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, series_of};
use crate::format::format;
use crate::model::*;
use crate::source::{Entity, GroupBy, Read, Source};

/// A node whose clock is further than this from the hub's, in milliseconds, is flagged.
const CLOCK_WARN_MS: f64 = 500.0;

/// The height of the charts, in pixels.
const CHART_HEIGHT: u16 = 180;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    vec![
        tiles(ctx, src),
        routes(ctx, src),
        postgres(ctx, src),
        links(src),
        bus(ctx, src),
        outside(src),
    ]
}

/// The six headline tiles of the hub.
fn tiles(ctx: &Ctx, src: &dyn Source) -> Panel {
    let tab = "hub";
    let mut points = ctx.tile(
        src,
        "Metric points in / s",
        "self.points_in",
        AggFn::Rate,
        Unit::Count,
        false,
        Tone::C5,
        tab,
    );
    let series = src.scalar(
        "self.series",
        &ctx.q.scope,
        ctx.from_ms,
        ctx.to_ms,
        AggFn::Last,
    );
    points.note = series
        .map(|n| format!("≈ {} series", format(Unit::Count, n)))
        .unwrap_or_default();
    let items = vec![
        ctx.tile(
            src,
            "API p95",
            "hub.http_ms",
            AggFn::P95,
            Unit::Ms,
            true,
            Tone::C1,
            tab,
        ),
        ctx.tile(
            src,
            "Open consoles / sockets",
            "hub.ws_clients",
            AggFn::Last,
            Unit::Count,
            false,
            Tone::C2,
            tab,
        ),
        ctx.tile(
            src,
            "Events / s",
            "hub.events",
            AggFn::Rate,
            Unit::Count,
            false,
            Tone::C3,
            tab,
        ),
        ctx.tile(
            src,
            "Postgres p95",
            "hub.pg_ms",
            AggFn::P95,
            Unit::Ms,
            true,
            Tone::C4,
            tab,
        ),
        points,
        ctx.tile(
            src,
            "Query cache hit",
            "self.cache_hit_pct",
            AggFn::Last,
            Unit::Percent,
            false,
            Tone::C6,
            tab,
        ),
    ];
    panel("tiles", "Hub", "", 12, Body::Tiles { tiles: items })
}

/// The API routes: requests per minute, latency, error rate and a request sparkline, one row per route family.
fn routes(ctx: &Ctx, src: &dyn Source) -> Panel {
    let minutes = (ctx.to_ms - ctx.from_ms) as f64 / 60_000.0;
    let rows: Vec<Row> = src
        .entities("global", "route")
        .iter()
        .map(|route| route_row(ctx, src, route, minutes))
        .collect();
    let body = if rows.is_empty() {
        empty("No API requests in this window.")
    } else {
        Body::Table(Table {
            columns: vec![
                column("Route", false),
                column("Req / min", true),
                column("p50", true),
                column("p95", true),
                column("Errors", true),
                column("Spark", false),
            ],
            rows,
            empty: String::new(),
        })
    };
    panel(
        "routes",
        "Routes",
        "Per route family, over the window",
        8,
        body,
    )
}

/// One route's row. Its requests are the sum over the window, its errors the 5xx answers, and its error rate the
/// errors as a share of its requests.
fn route_row(ctx: &Ctx, src: &dyn Source, route: &Entity, minutes: f64) -> Row {
    let scope = scope_id("route", &route.id);
    let window = |metric: &str, agg: AggFn| src.scalar(metric, &scope, ctx.from_ms, ctx.to_ms, agg);
    let requests = window("hub.http_requests", AggFn::Sum);
    let errors = window("hub.http_errors", AggFn::Sum);
    let per_minute = requests.filter(|_| minutes > 0.0).map(|r| r / minutes);
    let error_pct = requests
        .filter(|r| *r > 0.0)
        .map(|r| errors.unwrap_or(0.0) / r * 100.0);
    let cells = vec![
        Cell::Mono {
            text: route.label.clone(),
        },
        number(per_minute, Unit::Count),
        number(window("hub.http_ms", AggFn::P50), Unit::Ms),
        number(window("hub.http_ms", AggFn::P95), Unit::Ms),
        number(error_pct, Unit::Percent),
        Cell::Spark {
            values: route_requests(ctx, src, &scope),
            tone: Tone::C1,
        },
    ];
    Row {
        cells,
        scope: Some(scope),
        tab: None,
        trace: None,
        selected: false,
    }
}

/// A route's requests over the window in 40 buckets, for its sparkline.
fn route_requests(ctx: &Ctx, src: &dyn Source, scope: &str) -> Vec<f64> {
    let read = Read {
        metric: "hub.http_requests".into(),
        scope: scope.into(),
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: ((ctx.to_ms - ctx.from_ms) / 40).max(1_000),
        agg: AggFn::Sum,
        group: GroupBy::All,
        top: None,
    };
    src.series(&read)
        .into_iter()
        .next()
        .map(|g| g.values.iter().map(|v| v.unwrap_or(0.0)).collect())
        .unwrap_or_default()
}

/// Postgres: the p95 query time, and the connections in use on the same panel.
///
/// The chart has one unit, so the connections (a count) share the milliseconds axis.
fn postgres(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut chart = ctx.chart(
        src,
        &ctx.read("hub.pg_ms", AggFn::P95, GroupBy::All),
        Unit::Ms,
        false,
        CHART_HEIGHT,
    );
    let in_use = src.series(&ctx.read("hub.pg_in_use", AggFn::Last, GroupBy::All));
    chart.series.extend(
        in_use
            .iter()
            .enumerate()
            .map(|(i, g)| series_of(g, Tone::chart(i + 1))),
    );
    chart_panel(
        "postgres",
        "Postgres",
        "Query time and connections in use",
        4,
        chart,
        "No database queries in this window.",
    )
}

/// The node links: each node's last beat, beat lag, round trip, queued commands, clock offset and payload size.
fn links(src: &dyn Source) -> Panel {
    let rows: Vec<Row> = src.links().into_iter().map(link_row).collect();
    let body = if rows.is_empty() {
        empty("No node is linked to the hub.")
    } else {
        Body::Table(Table {
            columns: vec![
                column("Node", false),
                column("Last beat", false),
                column("Beat lag p95", true),
                column("Round trip", true),
                column("Commands queued", true),
                column("Clock offset", true),
                column("Payload avg", true),
            ],
            rows,
            empty: String::new(),
        })
    };
    panel(
        "links",
        "Node links",
        "Each node's beat and command path to the hub",
        8,
        body,
    )
}

/// One node link's row, from the source's (node id, label, beat age ms, beat lag p95 ms, round trip ms, commands
/// queued, clock offset ms, mean payload bytes).
fn link_row(link: (String, String, u64, f64, f64, u32, f64, f64)) -> Row {
    let (id, label, beat_ms, lag_ms, round_trip_ms, queued, offset_ms, payload) = link;
    let cells = vec![
        Cell::Named {
            name: label,
            id: id.clone(),
        },
        Cell::Mono {
            text: beat_age(beat_ms),
        },
        number(Some(lag_ms), Unit::Ms),
        number(Some(round_trip_ms), Unit::Ms),
        number(Some(f64::from(queued)), Unit::Count),
        clock_offset_cell(offset_ms),
        number(Some(payload), Unit::Bytes),
    ];
    Row {
        cells,
        scope: Some(scope_id("node", &id)),
        tab: None,
        trace: None,
        selected: false,
    }
}

/// The event bus: the p95 lag from publishing an event to a console writing it.
fn bus(ctx: &Ctx, src: &dyn Source) -> Panel {
    let chart = ctx.chart(
        src,
        &ctx.read("hub.bus_lag_ms", AggFn::P95, GroupBy::All),
        Unit::Ms,
        false,
        CHART_HEIGHT,
    );
    chart_panel(
        "bus",
        "Event bus",
        "Publish to console lag, p95",
        4,
        chart,
        "No events reached a console in this window.",
    )
}

/// The services outside the hub. A service that is up but has a last error is degraded; one that is down is
/// critical.
fn outside(src: &dyn Source) -> Panel {
    let services = src.outside();
    let body = if services.is_empty() {
        empty("No outside service is reported.")
    } else {
        let rows = services
            .into_iter()
            .map(|(name, up, p95, last_error)| {
                let (state, status) = match (up, last_error.is_empty()) {
                    (false, _) => ("down", Status::Crit),
                    (true, false) => ("degraded", Status::Warn),
                    (true, true) => ("up", Status::Ok),
                };
                let cells = vec![
                    Cell::Text { text: name },
                    Cell::Pill {
                        text: state.into(),
                        status,
                    },
                    number(p95, Unit::Ms),
                    Cell::Muted {
                        text: if last_error.is_empty() {
                            "—".into()
                        } else {
                            last_error
                        },
                    },
                ];
                Row {
                    cells,
                    scope: None,
                    tab: None,
                    trace: None,
                    selected: false,
                }
            })
            .collect();
        Body::Table(Table {
            columns: vec![
                column("Service", false),
                column("State", false),
                column("p95", true),
                column("Last error", false),
            ],
            rows,
            empty: String::new(),
        })
    };
    panel(
        "outside",
        "Outside services",
        "Services the hub depends on",
        12,
        body,
    )
}

/// A chart panel, or an empty panel when the chart has no series.
fn chart_panel(
    id: &str,
    title: &str,
    sub: &str,
    span: u8,
    chart: Chart,
    empty_text: &str,
) -> Panel {
    let body = if chart.series.is_empty() {
        empty(empty_text)
    } else {
        Body::Chart(chart)
    };
    panel(id, title, sub, span, body)
}

/// A column of a table.
fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// A number cell, or a dash when there is no value.
fn number(value: Option<f64>, unit: Unit) -> Cell {
    Cell::Num {
        text: value.map(|v| format(unit, v)).unwrap_or_else(|| "—".into()),
    }
}

/// The clock offset of a node. It is flagged when it is further than [`CLOCK_WARN_MS`] from the hub's.
fn clock_offset_cell(offset_ms: f64) -> Cell {
    let text = format(Unit::Ms, offset_ms);
    if offset_ms.abs() > CLOCK_WARN_MS {
        Cell::Status {
            text,
            status: Status::Warn,
        }
    } else {
        Cell::Num { text }
    }
}

/// How long ago a node last beat: "14 s ago" under a minute, else "3 min ago".
fn beat_age(age_ms: u64) -> String {
    if age_ms < 60_000 {
        format!("{} s ago", age_ms / 1_000)
    } else {
        format!("{} min ago", age_ms / 60_000)
    }
}

/// The scope id of an entity of `kind`: the id as given when it already carries the kind, `kind:id` otherwise.
fn scope_id(kind: &str, id: &str) -> String {
    if id.starts_with(&format!("{kind}:")) {
        id.into()
    } else {
        format!("{kind}:{id}")
    }
}

#[cfg(test)]
mod tests {
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::{Body, View, ViewQuery};

    fn hub_view() -> View {
        let q = ViewQuery::new("hub", "global", "24h");
        compose(&q, &Fixture::new(1_760_600_000_000))
    }

    fn body<'a>(view: &'a View, id: &str) -> &'a Body {
        &view.panels.iter().find(|p| p.id == id).unwrap().body
    }

    #[test]
    fn the_hub_tab_returns_its_six_panels_in_order() {
        let view = hub_view();
        assert_eq!(view.tab, "hub");
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            ["tiles", "routes", "postgres", "links", "bus", "outside"]
        );
    }

    #[test]
    fn each_hub_panel_is_its_body_kind_or_empty_when_the_source_has_nothing() {
        let view = hub_view();
        assert!(matches!(body(&view, "tiles"), Body::Tiles { tiles } if tiles.len() == 6));
        assert!(matches!(
            body(&view, "routes"),
            Body::Table(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body(&view, "postgres"),
            Body::Chart(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body(&view, "links"),
            Body::Table(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body(&view, "bus"),
            Body::Chart(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body(&view, "outside"),
            Body::Table(_) | Body::Empty { .. }
        ));
    }
}
