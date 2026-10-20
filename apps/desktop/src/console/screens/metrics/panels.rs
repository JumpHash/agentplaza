//! A Metrics panel: the card (title, sub, right text, freshness stamp, pinned-from line, the ⋯ button) and its
//! body. Charts are in [`super::charts`] and [`super::figures`]; the alerting, rules, traces and events bodies in
//! [`super::panels_ops`]; everything else is built from widgets here.

use apz_metrics_view::format;
use apz_metrics_view::{
    BarRow, Body, Cell, Chart, Column as TableColumn, Flag, HealthItem, LegendItem, Panel,
    Row as TableRow, SplitRow, StatLine, Table, Tile, XAxis,
};
use iced::alignment::Horizontal;
use iced::widget::{Space, button, column, container, responsive, row, text};
use iced::{Alignment, Border, Color, Element, Length, Size, Theme};

use super::grid_row::GridRow;
use super::measure::{self, ColumnWidth};
use super::{Msg, State, charts, figures, panels_ops, status, tone};
use crate::console::ui::{self, Kind};
use crate::theme::{
    MONO, SANS, SANS_SEMIBOLD, TYPE_BADGE, TYPE_META, TYPE_SECTION, TYPE_UI, palette,
};

/// Bar and split widths are counted in thousandths of the track.
const PORTIONS: u16 = 1000;

/// A panel's card with its body. The grid sets the card's width from `p.span`.
pub fn panel<'a>(p: &'a Panel, state: &'a State) -> Element<'a, Msg> {
    let pal = palette();
    // Tiles and health have no title: they sit bare on the grid, as in the design, unless pinned elsewhere.
    if p.title.is_empty() && p.pinned.is_none() {
        return container(body(p, state)).width(Length::Fill).into();
    }
    let mut head: Vec<Element<'a, Msg>> = vec![
        text(p.title.as_str())
            .font(SANS_SEMIBOLD)
            .size(TYPE_UI)
            .color(pal.ink)
            .width(Length::Fill)
            .into(),
    ];
    if !p.right.is_empty() {
        head.push(meta(p.right.as_str(), pal.ink_3).into());
    }
    if p.live
        && let Some(stamp) = freshness(state)
    {
        head.push(stamp);
    }
    for action in &p.actions {
        head.push(ui::btn_sm(
            action.label.as_str(),
            None,
            Kind::Secondary,
            Some(Msg::Action {
                post: action.post.clone(),
                done: action.done.clone(),
            }),
        ));
    }
    head.push(ui::btn_sm(
        "⋯",
        None,
        Kind::Ghost,
        Some(Msg::OpenMenu(p.id.clone())),
    ));

    let mut lines: Vec<Element<'a, Msg>> = vec![
        row(head)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
    ];
    if let Some(pin) = &p.pinned {
        lines.push(meta(format!("from {}", pin.from), pal.signal_ink).into());
    }
    if !p.sub.is_empty() {
        lines.push(meta(p.sub.as_str(), pal.ink_3).into());
    }
    lines.push(body(p, state));
    // The web's hint under a panel's time chart.
    if let Body::Chart(chart) = &p.body
        && matches!(chart.x, apz_metrics_view::XAxis::Time { .. })
        && !state.as_table.contains(&p.id)
    {
        // The web's `.mx-hint`, a size under the meta text.
        lines.push(
            text("drag to zoom · click a point for what changed")
                .font(SANS)
                .size(TYPE_BADGE)
                .color(pal.ink_3)
                .into(),
        );
    }

    container(column(lines).spacing(8).width(Length::Fill))
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(card_surface)
        .into()
}

/// The panel body by its type. Charts become a table while the panel is in `as_table`.
fn body<'a>(p: &'a Panel, state: &'a State) -> Element<'a, Msg> {
    let tz = local_offset();
    let annotations = state
        .view
        .as_ref()
        .map(|v| v.annotations.as_slice())
        .unwrap_or(&[]);
    match &p.body {
        Body::Health { items } => health(items),
        Body::Tiles { tiles } => tiles_row(tiles, compare_label(state)),
        Body::Chart(chart) => {
            if state.as_table.contains(&p.id) {
                chart_table(chart, tz)
            } else {
                charts::line(
                    &p.id,
                    chart,
                    &state.pointer,
                    annotations,
                    state.annotations,
                    state.compact,
                    tz,
                )
            }
        }
        Body::Heatmap(h) => figures::heatmap(&p.id, h, state.compact, tz),
        Body::Treemap { unit, items } => figures::treemap(*unit, items),
        Body::Gauges { gauges } => figures::gauges(gauges),
        Body::Table(t) => table(&p.id, t),
        Body::Flags { flags: list } => flags(list),
        Body::Bars { rows, note } => bars(rows, note),
        Body::Split { rows, legend } => split(rows, legend),
        Body::Stats { stats: list } => stats(list),
        Body::Swimlanes(s) => figures::swimlanes(s, tz),
        Body::Alerts { rows } => panels_ops::alerts(rows),
        Body::Rules { rules } => panels_ops::rules(rules),
        Body::Traces(t) => panels_ops::traces(t, state),
        Body::Events { kinds, rows } => panels_ops::events(kinds, rows, state),
        Body::Empty { text: message } => empty_line(message.as_str()),
        Body::Image(_)
        | Body::Graph(_)
        | Body::Dataset(_)
        | Body::Document { .. }
        | Body::Code(_)
        | Body::Html(_) => crate::console::view::body(&p.body),
    }
}

/// The local UTC offset in whole minutes, for the time labels.
fn local_offset() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// The freshness stamp: "live · N s ago" while live updates run, else "as of HH:MM:SS" (local time).
fn freshness<'a>(state: &State) -> Option<Element<'a, Msg>> {
    let at = state.fresh_at?;
    let pal = palette();
    let (label, dot) = if super::live_applies(state) {
        (format!("live · {} s ago", at.elapsed().as_secs()), pal.ok)
    } else {
        let wall =
            chrono::DateTime::<chrono::Local>::from(std::time::SystemTime::now() - at.elapsed());
        (format!("as of {}", wall.format("%H:%M:%S")), pal.ink_3)
    };
    // The web's `.mx-fresh`: a dot, green while live and muted otherwise, before the muted stamp.
    Some(
        row![
            text("●").font(SANS).size(TYPE_META - 2.0).color(dot),
            meta(label, pal.ink_3)
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into(),
    )
}

/// The comparison period's label ("vs previous"), when the view compares.
fn compare_label(state: &State) -> Option<&str> {
    state
        .view
        .as_ref()
        .map(|v| v.compare_label.as_str())
        .filter(|label| !label.is_empty())
}

/// A line of small muted text in the META size.
fn meta<'a>(content: impl text::IntoFragment<'a>, color: Color) -> text::Text<'a> {
    text(content).font(SANS).size(TYPE_META).color(color)
}

/// A centred muted line for a body with nothing in it.
fn empty_line<'a>(content: &'a str) -> Element<'a, Msg> {
    container(meta(content, palette().ink_3))
        .width(Length::Fill)
        .padding(16.0)
        .align_x(Horizontal::Center)
        .into()
}

/// A solid background of one colour.
fn fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| container::Style {
        background: Some(color.into()),
        ..Default::default()
    }
}

fn hovered(st: button::Status) -> bool {
    matches!(st, button::Status::Hovered | button::Status::Pressed)
}

/// A button surface: an optional rule edge, and the hover tint when lit.
fn surface(edged: bool, lit: bool) -> button::Style {
    let pal = palette();
    button::Style {
        background: if lit { Some(pal.hover.into()) } else { None },
        text_color: pal.ink,
        border: Border {
            color: pal.rule,
            width: if edged { 1.0 } else { 0.0 },
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// The web's `.mt-card`: the sheet fill, a 1 px rule edge, the large radius.
fn card_surface(_: &Theme) -> container::Style {
    let pal = palette();
    container::Style {
        background: Some(pal.sheet.into()),
        border: Border {
            color: pal.rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        ..Default::default()
    }
}

/// The web's `.mt-tile` and `.mt-hi`: a raised fill with the rule edge, the large radius, no hover tint.
fn raised_tile(_: &Theme, _st: button::Status) -> button::Style {
    let pal = palette();
    button::Style {
        background: Some(pal.raised.into()),
        text_color: pal.ink,
        border: Border {
            color: pal.rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        ..Default::default()
    }
}

/// A row button with no edge: the hover tint marks it, and a selected row stays tinted.
fn quiet_row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, st: button::Status| surface(false, selected || hovered(st))
}

/// The message a clicked row or flag sends: its scope, tab and trace together (as the web's `go`).
fn target(scope: Option<&str>, tab: Option<&str>, trace: Option<&str>) -> Option<Msg> {
    if scope.is_none() && tab.is_none() && trace.is_none() {
        return None;
    }
    Some(Msg::Open(
        scope.map(String::from),
        tab.map(String::from),
        trace.map(String::from),
    ))
}

/// A share in 0 to 1 as thousandths of a track.
fn portion(share: f64) -> u16 {
    (share.clamp(0.0, 1.0) * f64::from(PORTIONS)).round() as u16
}

/// A track of `height` px filled from the left by segments (thousandths of the track, colour). The rest of the
/// track is the faint rule colour.
fn segments<'a>(parts: Vec<(u16, Color)>, height: f32) -> Element<'a, Msg> {
    let mut used: u16 = 0;
    let mut cells: Vec<Element<'a, Msg>> = Vec::new();
    for (share, color) in parts {
        if share == 0 {
            continue;
        }
        used = used.saturating_add(share);
        cells.push(
            container(Space::new())
                .width(Length::FillPortion(share))
                .height(Length::Fill)
                .style(fill(color))
                .into(),
        );
    }
    if used < PORTIONS {
        cells.push(
            Space::new()
                .width(Length::FillPortion(PORTIONS - used))
                .height(Length::Fill)
                .into(),
        );
    }
    container(row(cells))
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .style(fill(palette().rule_faint))
        .into()
}

/// A change line for a tile: "▲ 12% vs <label>", coloured crit when the move is bad, or the note without a change.
fn change_line<'a>(t: &'a Tile, compare: Option<&'a str>) -> Element<'a, Msg> {
    let pal = palette();
    match t.change_pct {
        None => meta(t.note.as_str(), pal.ink_3).into(),
        Some(pct) => {
            let arrow = if pct > 0.0 {
                "▲ "
            } else if pct < 0.0 {
                "▼ "
            } else {
                ""
            };
            let vs = compare
                .map(|label| format!(" vs {label}"))
                .unwrap_or_default();
            let color = if pct == 0.0 {
                pal.ink_3
            } else if (pct > 0.0) == t.up_bad {
                pal.crit
            } else {
                pal.ok
            };
            meta(format!("{arrow}{:.0}%{vs}", pct.abs()), color).into()
        }
    }
}

/// The health row: one button per area, with its status dot, the area and its text. A press opens the tab.
fn health<'a>(items: &'a [HealthItem]) -> Element<'a, Msg> {
    // As the web's `.mt-health` (auto-fit, 200 px minimum, gap 12): as many columns as fit, every cell as tall as
    // the tallest in its row.
    responsive(move |size: Size| {
        let per_row = (((size.width + GAP) / (HEALTH_MIN_W + GAP)).floor() as usize)
            .clamp(1, items.len().max(1));
        let cells: Vec<Element<'a, Msg>> = items.iter().map(health_cell).collect();
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        let mut cells = cells;
        while !cells.is_empty() {
            let rest = cells.split_off(per_row.min(cells.len()));
            rows.push(GridRow::even(cells, per_row, GAP).into());
            cells = rest;
        }
        column(rows).spacing(GAP).width(Length::Fill).into()
    })
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

/// The narrowest a health cell may be before it wraps to the next row.
const HEALTH_MIN_W: f32 = 200.0;

/// The gap between tiles and health cells (the web's `--gap`).
const GAP: f32 = 12.0;

/// One health cell, as the web's `.mt-hi`: a status dot with the area, the status text under it; a press opens its tab.
fn health_cell<'a>(item: &'a HealthItem) -> Element<'a, Msg> {
    let pal = palette();
    let color = status(item.status);
    let dot = container(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let label = row![
        dot,
        text(item.area.as_str())
            .font(SANS_SEMIBOLD)
            .size(TYPE_UI)
            .color(pal.ink)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let content = column![label, meta(item.text.as_str(), pal.ink_3)]
        .spacing(4)
        .width(Length::Fill);
    button(content)
        .width(Length::Fill)
        .padding([14.0, 16.0])
        .style(raised_tile)
        .on_press(Msg::Tab(item.tab.clone()))
        .into()
}

/// The tiles, sharing each row equally. Under 1000 px the row holds three tiles, so the tiles wrap; the last row is
/// padded with empty space so the columns stay aligned.
fn tiles_row<'a>(tiles: &'a [Tile], compare: Option<&'a str>) -> Element<'a, Msg> {
    responsive(move |size: Size| {
        // One row while each tile gets 130 px; below that, even rows (the web's `data-n` container rules).
        let per_row = if size.width < TILE_MIN_W * tiles.len() as f32 {
            narrow_tile_columns(tiles.len())
        } else {
            tiles.len().max(1)
        };
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        // Every tile in a row as tall as the tallest, as the web's grid stretches them.
        for chunk in tiles.chunks(per_row) {
            let cells: Vec<Element<'a, Msg>> = chunk.iter().map(|t| tile(t, compare)).collect();
            rows.push(GridRow::even(cells, per_row, GAP).into());
        }
        column(rows).spacing(GAP).width(Length::Fill).into()
    })
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

/// The width a tile needs to share one row with the others.
const TILE_MIN_W: f32 = 130.0;

/// The tiles per row once a panel is narrow: rows of about three, as even as they can be (6 → 3 + 3, 7 → 4 + 3,
/// 8 → 4 + 4, 9 → 3 × 3); five or fewer keep one row. As the web's `narrowTileColumns`.
fn narrow_tile_columns(count: usize) -> usize {
    let rows = (count / 3).max(1);
    count.div_ceil(rows).max(1)
}

/// One tile: label, big value, change and a sparkline that takes the tile's width. A tile with a tab opens it on a
/// press.
fn tile<'a>(t: &'a Tile, compare: Option<&'a str>) -> Element<'a, Msg> {
    let pal = palette();
    let spark = responsive(move |size: Size| figures::spark(&t.spark, t.tone, size.width, 26.0))
        .width(Length::Fill)
        .height(Length::Fixed(26.0));
    let lines: Vec<Element<'a, Msg>> = vec![
        meta(t.label.as_str(), pal.ink_3).into(),
        text(t.value.as_str())
            .font(SANS_SEMIBOLD)
            .size(TYPE_SECTION)
            .color(pal.ink)
            .into(),
        change_line(t, compare),
        spark.into(),
    ];
    let body = button(column(lines).spacing(4).width(Length::Fill))
        .width(Length::Fill)
        .padding([14.0, 16.0])
        .style(raised_tile);
    let tile: Element<'a, Msg> = match &t.tab {
        Some(tab) => body.on_press(Msg::Tab(tab.clone())).into(),
        None => body.into(),
    };
    // The button itself is the cell, so it takes the row's height (the tiles of a row are equally tall).
    tile
}

/// One table cell's content, by its kind.
fn cell_content<'a>(cell: &'a Cell) -> Element<'a, Msg> {
    let pal = palette();
    match cell {
        Cell::Text { text: value } | Cell::Num { text: value } => {
            text(value.as_str()).size(TYPE_UI).color(pal.ink).into()
        }
        Cell::Mono { text: value } => ui::mono(value.as_str(), TYPE_UI, pal.ink).into(),
        Cell::Named { name, id } => row![
            text(name.as_str()).size(TYPE_UI).color(pal.ink),
            ui::mono(id.as_str(), TYPE_META, pal.ink_3)
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        Cell::Status {
            text: value,
            status: s,
        } => text(value.as_str()).size(TYPE_UI).color(status(*s)).into(),
        Cell::Pill {
            text: value,
            status: s,
        } => container(
            text(value.as_str())
                .font(SANS_SEMIBOLD)
                .size(TYPE_BADGE)
                .color(status(*s)),
        )
        .padding([2.0, 6.0])
        .style(ui::frame(None, status(*s)))
        .into(),
        Cell::Muted { text: value } => meta(value.as_str(), pal.ink_3).into(),
        Cell::Spark { values, tone: t } => figures::spark(values, *t, SPARK_W, 18.0),
        Cell::Bar { parts } => segments(
            parts
                .iter()
                .map(|(share, t, _)| (portion(*share / 100.0), tone(*t)))
                .collect(),
            8.0,
        ),
    }
}

/// The gap between a table's columns, and the padding at its ends (the web's 8 px cell padding on each side).
const CELL_GAP: f32 = 16.0;
const CELL_EDGE: f32 = 8.0;
/// A sparkline cell's width (the web's `spark` at 90 px).
const SPARK_W: f32 = 90.0;

/// A cell in a table line: its content at its column's width, aligned right when the column (or the cell) is
/// numeric. Without widths (a chart shown as a table) the columns share the line equally.
fn cell<'a>(content: Element<'a, Msg>, numeric: bool, width: Option<f32>) -> Element<'a, Msg> {
    container(content)
        .width(width.map_or(Length::FillPortion(1), Length::Fixed))
        .align_x(if numeric {
            Horizontal::Right
        } else {
            Horizontal::Left
        })
        .into()
}

/// A table line, as the web's `tr`: the cells with the cell padding, over a rule (the full rule under the head, the
/// faint one between rows). `widths` are the columns' widths (equal shares when empty).
fn cells_row<'a>(
    cells: Vec<(Element<'a, Msg>, bool)>,
    widths: &[f32],
    head: bool,
) -> Element<'a, Msg> {
    let pal = palette();
    let line = row(cells
        .into_iter()
        .enumerate()
        .map(|(i, (content, numeric))| cell(content, numeric, widths.get(i).copied())))
    .spacing(CELL_GAP)
    .align_y(Alignment::Center);
    let rule = if head { pal.rule } else { pal.rule_faint };
    column![
        container(line)
            .padding([if head { 4.0 } else { 5.0 }, CELL_EDGE])
            .width(Length::Fill),
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(fill(rule)),
    ]
    .width(Length::Fill)
    .into()
}

/// A table: the head line, then the rows.
fn grid<'a>(
    head: Vec<(Element<'a, Msg>, bool)>,
    rows: Vec<Element<'a, Msg>>,
    widths: &[f32],
) -> Element<'a, Msg> {
    let mut lines = vec![cells_row(head, widths, true)];
    lines.extend(rows);
    column(lines).width(Length::Fill).into()
}

/// Each column's widths as the web's table sizes it: the head and every cell on one line, except free text, which
/// can wrap down to its longest word.
fn column_widths(t: &Table) -> Vec<ColumnWidth> {
    t.columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let head = measure::width(&c.head, SANS, TYPE_META);
            t.rows
                .iter()
                .filter_map(|r| r.cells.get(i))
                .map(cell_width)
                .fold(
                    ColumnWidth {
                        min: head,
                        max: head,
                    },
                    |w, (min, max)| ColumnWidth {
                        min: w.min.max(min),
                        max: w.max.max(max),
                    },
                )
        })
        .collect()
}

/// A cell's narrowest and widest width, in the fonts `cell_content` draws it with.
fn cell_width(cell: &Cell) -> (f32, f32) {
    let one = |w: f32| (w, w);
    match cell {
        Cell::Text { text } => (
            measure::longest_word(text, SANS, TYPE_UI),
            measure::width(text, SANS, TYPE_UI),
        ),
        Cell::Muted { text } => (
            measure::longest_word(text, SANS, TYPE_META),
            measure::width(text, SANS, TYPE_META),
        ),
        Cell::Num { text } | Cell::Status { text, .. } => one(measure::width(text, SANS, TYPE_UI)),
        Cell::Mono { text } => one(measure::width(text, MONO, TYPE_UI)),
        Cell::Named { name, id } => {
            one(measure::width(name, SANS, TYPE_UI) + 6.0 + measure::width(id, MONO, TYPE_META))
        }
        Cell::Pill { text, .. } => one(measure::width(text, SANS_SEMIBOLD, TYPE_BADGE) + 14.0),
        Cell::Spark { .. } | Cell::Bar { .. } => one(SPARK_W),
    }
}

/// A table of rows. A row that names a scope, tab or trace is a button that opens it. When the table has older
/// rows (`more`), a "Load older" button sits under them.
fn table<'a>(panel: &'a str, t: &'a Table) -> Element<'a, Msg> {
    let content = if t.rows.is_empty() {
        empty_line(if t.empty.is_empty() {
            "Nothing here yet."
        } else {
            t.empty.as_str()
        })
    } else {
        responsive(move |size: Size| {
            let pal = palette();
            let widths = measure::layout(&column_widths(t), size.width - 2.0 * CELL_EDGE, CELL_GAP);
            let total = widths.iter().sum::<f32>()
                + CELL_GAP * widths.len().saturating_sub(1) as f32
                + 2.0 * CELL_EDGE;
            let head: Vec<(Element<'a, Msg>, bool)> = t
                .columns
                .iter()
                .map(|c| (meta(c.head.as_str(), pal.ink_3).into(), c.num))
                .collect();
            let rows: Vec<Element<'a, Msg>> = t
                .rows
                .iter()
                .map(|r| table_row(r, &t.columns, &widths))
                .collect();
            let body = grid(head, rows, &widths);
            // Wider than the card even at its narrowest: the table scrolls sideways, as the web's `.mx-tablewrap`.
            if total > size.width + 0.5 {
                ui::scroll_x(container(body).width(Length::Fixed(total)))
                    .width(Length::Fill)
                    .into()
            } else {
                body
            }
        })
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
    };
    let Some(path) = &t.more else {
        return content;
    };
    let older = ui::btn_sm(
        "Load older",
        None,
        Kind::Secondary,
        Some(Msg::More {
            panel: panel.to_string(),
            path: path.clone(),
        }),
    );
    column(vec![content, older])
        .spacing(8)
        .width(Length::Fill)
        .into()
}

fn table_row<'a>(r: &'a TableRow, columns: &'a [TableColumn], widths: &[f32]) -> Element<'a, Msg> {
    let cells: Vec<(Element<'a, Msg>, bool)> = r
        .cells
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let numeric =
                columns.get(i).is_some_and(|column| column.num) || matches!(c, Cell::Num { .. });
            (cell_content(c), numeric)
        })
        .collect();
    let line = cells_row(cells, widths, false);
    match target(r.scope.as_deref(), r.tab.as_deref(), r.trace.as_deref()) {
        Some(msg) => button(line)
            .width(Length::Fill)
            .padding(0.0)
            .style(quiet_row(r.selected))
            .on_press(msg)
            .into(),
        None => {
            let back = if r.selected {
                palette().hover
            } else {
                Color::TRANSPARENT
            };
            container(line).width(Length::Fill).style(fill(back)).into()
        }
    }
}

/// A chart as a table: the time, then one column per series, newest first. A gap is a dash.
fn chart_table<'a>(c: &'a Chart, tz: i32) -> Element<'a, Msg> {
    let pal = palette();
    let points = c.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let mut head: Vec<(Element<'a, Msg>, bool)> = vec![(meta("time", pal.ink_3).into(), false)];
    for s in &c.series {
        head.push((meta(s.name.as_str(), pal.ink_3).into(), true));
    }
    let rows: Vec<Element<'a, Msg>> = (0..points)
        .rev()
        .map(|i| {
            let when = match &c.x {
                XAxis::Time { start_ms, step_ms } => {
                    format::time_label(*start_ms + i as u64 * *step_ms, *step_ms, tz)
                }
                XAxis::Labels { labels } => labels.get(i).cloned().unwrap_or_default(),
            };
            let mut cells: Vec<(Element<'a, Msg>, bool)> =
                vec![(meta(when, pal.ink_3).into(), false)];
            for s in &c.series {
                let value = match s.values.get(i).copied().flatten() {
                    Some(v) => format::format(c.unit, v),
                    None => String::from("—"),
                };
                cells.push((text(value).size(TYPE_UI).color(pal.ink).into(), true));
            }
            cells_row(cells, &[], false)
        })
        .collect();
    ui::scroll(grid(head, rows, &[]))
        .height(Length::Fixed(360.0))
        .into()
}

/// A column of flags: each is a button with a status-coloured edge, its text and its time. A press opens its tab
/// or scope.
fn flags<'a>(list: &'a [Flag]) -> Element<'a, Msg> {
    if list.is_empty() {
        return empty_line("Nothing here yet.");
    }
    let pal = palette();
    column(list.iter().map(|f| {
        let strip = container(Space::new())
            .width(Length::Fixed(3.0))
            .height(Length::Fill)
            .style(fill(status(f.status)));
        // The text and its time on one line, the time at the right, as the web's `.mx-flag`.
        let words = row![
            text(f.text.as_str())
                .size(TYPE_UI)
                .color(pal.ink)
                .width(Length::Fill),
            meta(f.when.as_str(), pal.ink_3)
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        // Shrink: the strip fills the flag's height, not the card's.
        let content = row![
            strip,
            container(words).padding([6.0, 8.0]).width(Length::Fill)
        ]
        .width(Length::Fill)
        .height(Length::Shrink);
        // The web's `.mx-flag`: the paper ground and the status strip, no box around it.
        let flag = button(content).width(Length::Fill).padding(0.0).style(
            |_: &Theme, st: button::Status| {
                let pal = palette();
                button::Style {
                    background: Some(if hovered(st) { pal.hover } else { pal.paper }.into()),
                    text_color: pal.ink,
                    ..Default::default()
                }
            },
        );
        match target(f.scope.as_deref(), f.tab.as_deref(), None) {
            Some(msg) => flag.on_press(msg).into(),
            None => flag.into(),
        }
    }))
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// Budget rows: the label and value, a bar filled by its fraction in its status colour, and a muted note under it.
/// The body's note closes the list.
fn bars<'a>(rows: &'a [BarRow], note: &'a str) -> Element<'a, Msg> {
    let pal = palette();
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    for r in rows {
        let mut body: Vec<Element<'a, Msg>> = vec![
            row![
                // The label wraps, the value never does (as the web's budget rows).
                text(r.label.as_str())
                    .size(TYPE_UI)
                    .color(pal.ink)
                    .width(Length::Fill),
                text(r.value.as_str())
                    .size(TYPE_UI)
                    .color(pal.ink)
                    .wrapping(text::Wrapping::None)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
            segments(vec![(portion(r.frac), status(r.status))], 6.0),
        ];
        if !r.note.is_empty() {
            body.push(meta(r.note.as_str(), pal.ink_3).into());
        }
        lines.push(column(body).spacing(4).into());
    }
    if !note.is_empty() {
        lines.push(meta(note, pal.ink_3).into());
    }
    column(lines).spacing(10).width(Length::Fill).into()
}

/// Stacked rows on one scale. The largest row total fills the track, and each part's width is its value over that
/// total, so the rows compare with each other. A legend closes the list.
fn split<'a>(rows: &'a [SplitRow], legend: &'a [LegendItem]) -> Element<'a, Msg> {
    let pal = palette();
    let largest = rows
        .iter()
        .map(|r| r.parts.iter().map(|(value, _, _)| *value).sum::<f64>())
        .fold(0.0, f64::max);
    let mut lines: Vec<Element<'a, Msg>> = rows
        .iter()
        .map(|r| {
            let parts: Vec<(u16, Color)> = r
                .parts
                .iter()
                .map(|(value, t, _)| {
                    let share = if largest > 0.0 { value / largest } else { 0.0 };
                    (portion(share), tone(*t))
                })
                .collect();
            row![
                container(meta(r.label.as_str(), pal.ink_3)).width(Length::Fixed(80.0)),
                segments(parts, 10.0),
                text(r.total.as_str()).size(TYPE_UI).color(pal.ink),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    if !legend.is_empty() {
        let items: Vec<Element<'a, Msg>> = legend
            .iter()
            .map(|item| {
                row![
                    container(Space::new())
                        .width(Length::Fixed(8.0))
                        .height(Length::Fixed(8.0))
                        .style(fill(tone(item.tone))),
                    meta(item.label.as_str(), pal.ink_3)
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into()
            })
            .collect();
        lines.push(row(items).spacing(12).into());
    }
    column(lines).spacing(10).width(Length::Fill).into()
}

/// A vertical list of figures: a small muted label, then the big value with its note beside it.
fn stats<'a>(list: &'a [StatLine]) -> Element<'a, Msg> {
    let pal = palette();
    column(list.iter().map(|s| {
        let mut line: Vec<Element<'a, Msg>> = vec![
            text(s.value.as_str())
                .font(SANS_SEMIBOLD)
                .size(22.0)
                .color(pal.ink)
                .into(),
        ];
        if !s.note.is_empty() {
            line.push(meta(s.note.as_str(), pal.ink_3).into());
        }
        column![
            text(s.label.as_str())
                .font(SANS)
                .size(TYPE_BADGE)
                .color(pal.ink_3),
            row(line).spacing(6).align_y(Alignment::End)
        ]
        .spacing(2)
        .into()
    }))
    .spacing(10)
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_tiles_wrap_into_even_rows() {
        let cols: Vec<usize> = [1, 3, 4, 5, 6, 7, 8, 9]
            .iter()
            .map(|&n| narrow_tile_columns(n))
            .collect();
        assert_eq!(cols, vec![1, 3, 4, 5, 3, 4, 4, 3]);
    }
}
