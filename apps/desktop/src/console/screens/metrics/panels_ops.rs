//! The operational Metrics bodies, as the web's bodies-ops.ts: firing alerts (open, mute, acknowledge), the rules
//! table (on/off, new rule), kept traces (filter, list, waterfall, span details) and the events feed (kind filter).

use apz_metrics_view::format::format as format_value;
use apz_metrics_view::{
    AlertRow, EventRow, LegendItem, RuleRow, SpanRow, Status, Tone, TraceDetail, TraceRow,
    TracesBody, Unit,
};
use iced::widget::{
    Space, button, checkbox, column, container, responsive, rich_text, row, span, text,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Size, Theme};

use super::figures::spark;
use super::measure;
use super::{Msg, State, status, tone};
use crate::console::ui;
use crate::theme::{
    MONO, SANS, SANS_MEDIUM, SANS_SEMIBOLD, TYPE_BADGE, TYPE_META, TYPE_SECTION, TYPE_UI,
};

/// The trace filters, in the web's order.
const TRACE_FILTERS: [&str; 5] = ["all", "error", "slow", "sampled", "open"];

/// The firing rules, each with its severity bar, its current value, a spark line and the Open, Mute and
/// Acknowledge actions. With none firing, a line says so.
pub fn alerts<'a>(rows: &'a [AlertRow]) -> Element<'a, Msg> {
    if rows.is_empty() {
        return empty_line("Nothing firing. Muted rules are listed below.");
    }
    // A narrow grid drops each row's value and trend, as the web's `@container mxg (max-width: 800px)` does (the
    // firing panel spans the grid, less its 28 px of padding).
    responsive(move |size: Size| {
        let narrow = size.width < NARROW_ALERTS;
        column(
            rows.iter()
                .map(|r| alert_row(r, narrow))
                .collect::<Vec<_>>(),
        )
        .width(Length::Fill)
        .into()
    })
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

/// The firing panel's body width under which the rows drop their value and trend.
const NARROW_ALERTS: f32 = 772.0;

/// The rules table, with a New rule button above it. Turning a rule on or off reports the new state.
pub fn rules<'a>(rows: &'a [RuleRow]) -> Element<'a, Msg> {
    let new_rule = ui::btn_sm("New rule", None, ui::Kind::Primary, Some(Msg::NewRule));
    let heads = [
        ("On", 1, false),
        ("Rule", 3, false),
        ("Scope", 2, false),
        ("Condition", 4, false),
        ("Severity", 2, false),
        ("State", 2, false),
        ("Last fired", 2, false),
    ];
    let lines: Vec<Element<'a, Msg>> = rows.iter().map(rule_row).collect();
    column![new_rule, table(&heads, lines)].spacing(12).into()
}

/// The kept traces: a filter, a list of turns (left) and the selected turn's waterfall and span details (right).
pub fn traces<'a>(t: &'a TracesBody, state: &'a State) -> Element<'a, Msg> {
    let options: Vec<(&'a str, &'a str)> = TRACE_FILTERS.iter().map(|key| (*key, *key)).collect();
    let filter = ui::seg(options.as_slice(), t.filter.as_str(), |key: &str| {
        Msg::Filter(key.to_string())
    });
    if t.list.is_empty() {
        return column![filter, empty_line("No kept traces match.")]
            .spacing(12)
            .into();
    }
    let selected_id = t.selected.as_ref().map(|d| d.id.as_str());
    let list_rows: Vec<Element<'a, Msg>> = t
        .list
        .iter()
        .map(|r| trace_row(r, selected_id == Some(r.id.as_str())))
        .collect();
    let list = table(
        &[
            ("Time", 2, false),
            ("Agent", 2, false),
            ("Total", 2, false),
            ("Kept because", 4, false),
        ],
        list_rows,
    );
    let detail: Element<'a, Msg> = match &t.selected {
        Some(d) => trace_detail(d, state),
        None => Space::new().into(),
    };
    column![
        filter,
        row![
            container(list).width(Length::FillPortion(5)),
            container(detail).width(Length::FillPortion(7)),
        ]
        .spacing(16),
    ]
    .spacing(12)
    .into()
}

/// The events feed: a kind filter over the rows, then a table. Subjects with a scope open that scope.
pub fn events<'a>(kinds: &'a [String], rows: &'a [EventRow], state: &'a State) -> Element<'a, Msg> {
    let mut options: Vec<(Option<&'a str>, &'a str)> = vec![(None, "all")];
    options.extend(
        kinds
            .iter()
            .map(|kind| (Some(kind.as_str()), kind.as_str())),
    );
    let filter = ui::seg(
        options.as_slice(),
        state.event_kind.as_deref(),
        |kind: Option<&str>| Msg::EventKind(kind.map(str::to_string)),
    );
    let shown: Vec<&EventRow> = rows
        .iter()
        .filter(|r| {
            state
                .event_kind
                .as_deref()
                .is_none_or(|kind| r.kind == kind)
        })
        .collect();
    let body: Element<'a, Msg> = if shown.is_empty() {
        empty_line("No events of this kind in range.")
    } else {
        let trs: Vec<Element<'a, Msg>> = shown.into_iter().map(event_row).collect();
        table(
            &[
                ("Time", 2, false),
                ("Kind", 2, false),
                ("Subject", 4, false),
                ("Detail", 7, false),
            ],
            trs,
        )
    };
    column![filter, body].spacing(12).into()
}

/// One firing rule: the severity bar, the name over its condition line, the current value, the spark line and
/// the three actions. Acknowledge is disabled once the alert is acknowledged.
fn alert_row<'a>(r: &'a AlertRow, narrow: bool) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let bar = block(Length::Fixed(4.0), Length::Fill, status(r.severity));
    let name = text(r.name.clone())
        .font(SANS_SEMIBOLD)
        .size(TYPE_UI)
        .color(p.ink);
    let facts = ui::sans(
        format!(
            "{} · {} for {} · since {}",
            r.scope_label, r.cond, r.for_text, r.since
        ),
        TYPE_META,
        p.ink_3,
    );
    let info = column![name, facts].spacing(2).width(Length::Fill);
    let value = row![
        ui::mono(r.now.clone(), TYPE_UI, p.ink),
        ui::sans("now", TYPE_META, p.ink_3),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let trend = spark(&r.spark, Tone::Crit, 120.0, 24.0);
    let actions = row![
        ui::btn_sm(
            "Open",
            None,
            ui::Kind::Secondary,
            Some(Msg::Open(Some(r.scope.clone()), Some(r.tab.clone()), None))
        ),
        ui::btn_sm(
            "Mute 1 h",
            None,
            ui::Kind::Secondary,
            Some(Msg::Mute(r.rule.clone())),
        ),
        ui::btn_sm(
            if r.acked {
                "Acknowledged"
            } else {
                "Acknowledge"
            },
            None,
            ui::Kind::Secondary,
            (!r.acked).then(|| Msg::Ack(r.rule.clone(), r.scope.clone())),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let content = if narrow {
        row![info, actions]
    } else {
        row![info, value, trend, actions]
    }
    .spacing(16)
    .align_y(Alignment::Center);
    column![
        // Shrink: the bar fills the row's height, not the scroll's, which would collapse the card.
        row![
            bar,
            container(content)
                .padding(Padding::from([10.0, 12.0]))
                .width(Length::Fill),
        ]
        .height(Length::Shrink),
        ui::hairline::<Msg>(),
    ]
    .into()
}

/// One rule: its checkbox, name, scope, condition with its hold time, severity pill, state and last firing.
fn rule_row<'a>(r: &'a RuleRow) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let id = r.id.clone();
    let toggle = checkbox(r.on).on_toggle(move |on| Msg::RuleOn(id.clone(), on));
    // One flowing line, as the web's `<span.mono>cond</span> for …`: the window follows the expression and wraps
    // with it, never in a column of its own.
    let condition: Element<'a, Msg> = rich_text::<(), Msg, iced::Theme, iced::Renderer>([
        span(r.cond.clone()).font(MONO),
        span(format!(" for {}", r.for_text)).font(SANS),
    ])
    .size(TYPE_UI)
    .color(p.ink)
    .width(Length::Fill)
    .into();
    let state_color = match r.state.as_str() {
        "firing" => p.crit,
        "ok" => p.ok,
        _ => p.ink_3,
    };
    let last = if r.last.is_empty() {
        "—".to_string()
    } else {
        r.last.clone()
    };
    line(cells(vec![
        (1, false, toggle.into()),
        (3, false, ui::sans(r.name.clone(), TYPE_UI, p.ink).into()),
        (
            2,
            false,
            ui::sans(r.scope_label.clone(), TYPE_UI, p.ink_2).into(),
        ),
        (4, false, condition),
        (2, false, pill(status_name(r.severity), status(r.severity))),
        (
            2,
            false,
            ui::sans(r.state.clone(), TYPE_UI, state_color).into(),
        ),
        (2, false, ui::sans(last, TYPE_META, p.ink_2).into()),
    ]))
}

/// One kept turn in the list: its time, agent, total and why it was kept (crit for an error).
fn trace_row<'a>(r: &'a TraceRow, selected: bool) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    // As the web's row: the time in mono, the agent, the total on one line, why it was kept muted (crit on error).
    let kept = ui::sans(
        r.kept_text.clone(),
        TYPE_UI,
        if r.error { p.crit } else { p.ink_3 },
    );
    let content = cells(vec![
        (2, false, ui::mono(r.time.clone(), TYPE_UI, p.ink).into()),
        (2, false, ui::sans(r.agent.clone(), TYPE_UI, p.ink).into()),
        (
            2,
            false,
            ui::sans(r.total.clone(), TYPE_UI, p.ink)
                .wrapping(text::Wrapping::None)
                .into(),
        ),
        (4, false, kept.into()),
    ]);
    ruled(
        row_button(
            content,
            selected,
            Msg::Trace(r.id.clone()),
            Padding::from([5.0, 8.0]),
        ),
        p.rule_faint,
    )
}

/// The selected trace: its title, the error note, the waterfall, the legend and the chosen span's details.
fn trace_detail<'a>(d: &'a TraceDetail, state: &'a State) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let total = d
        .spans
        .iter()
        .fold(d.total_ms.max(1.0), |acc, s| acc.max(s.start_ms + s.dur_ms));
    let selected = if d.spans.is_empty() {
        None
    } else {
        Some(
            state
                .span
                .filter(|i| *i < d.spans.len())
                .unwrap_or_else(|| default_span(&d.spans)),
        )
    };
    let waterfall: Vec<Element<'a, Msg>> = d
        .spans
        .iter()
        .enumerate()
        .map(|(i, s)| waterfall_row(s, total, selected == Some(i), i))
        .collect();
    let details: Element<'a, Msg> = match selected {
        Some(i) => span_details(&d.spans[i]),
        None => Space::new().into(),
    };
    let mut parts: Vec<Element<'a, Msg>> = vec![
        text(d.title.clone())
            .font(SANS_SEMIBOLD)
            .size(TYPE_SECTION)
            .color(p.ink)
            .into(),
        ui::sans(d.sub.clone(), TYPE_META, p.ink_3).into(),
    ];
    if d.error {
        parts.push(ui::sans("Ended with an error.", TYPE_UI, p.crit).into());
    }
    parts.push(column(waterfall).width(Length::Fill).into());
    parts.push(legend(&d.legend));
    parts.push(details);
    column(parts).spacing(12).width(Length::Fill).into()
}

/// One span of the waterfall: its name indented by depth, the track of its time and its duration. The first
/// token's wait is drawn faint, the rest in the span's tone (crit when it failed).
fn waterfall_row<'a>(s: &SpanRow, total: f64, selected: bool, index: usize) -> Element<'a, Msg> {
    let first_token = s.first_token_ms.unwrap_or(0.0);
    let mut bars: Vec<(f64, f64, Color)> = Vec::new();
    if first_token > 0.0 {
        bars.push((
            s.start_ms / total * 100.0,
            first_token / total * 100.0,
            Color {
                a: 0.35,
                ..tone(Tone::C1)
            },
        ));
    }
    let body = ((s.dur_ms - first_token) / total * 100.0).max(0.4);
    let body_tone = if s.error { Tone::Crit } else { s.tone };
    bars.push((
        (s.start_ms + first_token) / total * 100.0,
        body,
        tone(body_tone),
    ));
    let depth = f32::from(s.depth) * 12.0;
    let name = s.name.clone();
    let dur = format_value(Unit::Ms, s.dur_ms);
    // The web's `.mx-wf-row`: the name in 90 to 160 px, cut with an ellipsis rather than wrapped; the track; the
    // duration in 56 px.
    let content = responsive(move |size: Size| {
        let p = crate::theme::palette();
        let name_w = (size.width - WF_DUR - 2.0 * WF_GAP - 80.0).clamp(90.0, 160.0);
        let shown = measure::ellipsize(&name, SANS, TYPE_META, (name_w - depth).max(0.0));
        row![
            container(ui::sans(shown, TYPE_META, p.ink_2))
                .padding(Padding {
                    left: depth,
                    ..Padding::ZERO
                })
                .width(Length::Fixed(name_w)),
            container(track(&bars)).width(Length::Fill),
            container(ui::sans(dur.clone(), TYPE_META, p.ink_3))
                .width(Length::Fixed(WF_DUR))
                .align_x(Alignment::End),
        ]
        .spacing(WF_GAP)
        .align_y(Alignment::Center)
        .into()
    })
    .width(Length::Fill)
    .height(Length::Shrink);
    row_button(
        content.into(),
        selected,
        Msg::SelectSpan(index),
        Padding::from([3.0, 0.0]),
    )
}

/// A waterfall row's duration column and the gap between its columns (the web's `.mx-wf-row` grid).
const WF_DUR: f32 = 56.0;
const WF_GAP: f32 = 8.0;

/// The track of a waterfall row: each bar at its share of the width, in order, with the gaps between them in
/// rule_faint. Shares are in tenths of a percent, so a full track adds up to 1000 portions.
fn track<'a>(bars: &[(f64, f64, Color)]) -> Element<'a, Msg> {
    let gap = crate::theme::palette().rule_faint;
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    let mut cursor = 0.0_f64;
    for &(left, width, color) in bars {
        push_share(&mut parts, (left - cursor).max(0.0), gap);
        push_share(&mut parts, width, color);
        cursor = cursor.max(left + width);
    }
    push_share(&mut parts, (100.0 - cursor).max(0.0), gap);
    row(parts)
        .width(Length::Fill)
        .height(Length::Fixed(10.0))
        .into()
}

/// Appends a block of `share` percent of the track, unless it rounds to nothing.
fn push_share<'a>(parts: &mut Vec<Element<'a, Msg>>, share: f64, color: Color) {
    let units = (share * 10.0).round().min(1000.0) as u16;
    if units > 0 {
        parts.push(block(Length::FillPortion(units), Length::Fill, color));
    }
}

/// The span's attributes as a two-column list, with its name, start and duration first.
fn span_details<'a>(s: &SpanRow) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let mut entries: Vec<(String, String)> = vec![
        ("span".to_string(), s.name.clone()),
        (
            "start".to_string(),
            format!("+{}", format_value(Unit::Ms, s.start_ms)),
        ),
        ("duration".to_string(), format_value(Unit::Ms, s.dur_ms)),
    ];
    entries.extend(s.attrs.iter().cloned());
    let lines: Vec<Element<'a, Msg>> = entries
        .into_iter()
        .map(|(key, value)| {
            let color = if key == "outcome" && value != "ok" {
                p.crit
            } else {
                p.ink
            };
            cells(vec![
                (2, false, ui::sans(key, TYPE_META, p.ink_3).into()),
                (5, false, ui::sans(value, TYPE_UI, color).into()),
            ])
        })
        .collect();
    column(lines).spacing(4).width(Length::Fill).into()
}

/// The first error span, or the fourth when nothing failed.
fn default_span(spans: &[SpanRow]) -> usize {
    spans
        .iter()
        .position(|s| s.error)
        .unwrap_or(3.min(spans.len() - 1))
}

/// The legend of a trace: a swatch and a label per tone.
fn legend<'a>(items: &[LegendItem]) -> Element<'a, Msg> {
    let ink_2 = crate::theme::palette().ink_2;
    let entries: Vec<Element<'a, Msg>> = items
        .iter()
        .map(|item| {
            row![
                block(Length::Fixed(10.0), Length::Fixed(10.0), tone(item.tone)),
                ui::sans(item.label.clone(), TYPE_META, ink_2),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    row(entries).spacing(14).into()
}

/// One event: its time, kind pill, subject (a link when it has a scope) and detail.
fn event_row<'a>(r: &'a EventRow) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let subject: Element<'a, Msg> = match &r.scope {
        // The web's ghost button: plain text that takes the hover tint.
        Some(scope) => button(ui::sans(r.subject.clone(), TYPE_UI, p.ink))
            .padding(Padding::from([2.0, 4.0]))
            .style(|_: &Theme, st: button::Status| button::Style {
                background: matches!(st, button::Status::Hovered | button::Status::Pressed)
                    .then(|| crate::theme::palette().hover.into()),
                ..Default::default()
            })
            .on_press(Msg::Scope(scope.clone()))
            .into(),
        None => ui::sans(r.subject.clone(), TYPE_UI, p.ink).into(),
    };
    line(cells(vec![
        (2, false, ui::mono(r.time.clone(), TYPE_UI, p.ink).into()),
        (2, false, pill(&r.kind, status(r.status))),
        (4, false, subject),
        (7, false, ui::sans(r.detail.clone(), TYPE_UI, p.ink).into()),
    ]))
}

/// A table: a head of mono-caps labels over a rule, then the rows. Each head entry is its label, its share of
/// the row and whether it sits on the right.
fn table<'a>(heads: &[(&str, u16, bool)], rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let ink_3 = crate::theme::palette().ink_3;
    let head_cells: Vec<(u16, bool, Element<'a, Msg>)> = heads
        .iter()
        .map(|(label, portion, right)| {
            (
                *portion,
                *right,
                ui::sans(label.to_string(), TYPE_BADGE, ink_3).into(),
            )
        })
        .collect();
    let head = container(cells(head_cells))
        .padding(Padding::from([4.0, 8.0]))
        .width(Length::Fill);
    let mut lines: Vec<Element<'a, Msg>> = vec![ruled(head.into(), crate::theme::palette().rule)];
    lines.extend(rows);
    column(lines).width(Length::Fill).into()
}

/// The cells of a row, each by its share of the width; a right cell aligns its content to the end.
fn cells<'a>(parts: Vec<(u16, bool, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let items: Vec<Element<'a, Msg>> = parts
        .into_iter()
        .map(|(portion, right, content)| {
            let cell = container(content).width(Length::FillPortion(portion));
            let cell = if right {
                cell.align_x(Alignment::End)
            } else {
                cell
            };
            cell.into()
        })
        .collect();
    // The web's 8 px cell padding on each side: 16 px between columns.
    row(items)
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// A table line that is not a button: its cells with the web's cell padding, over the faint rule between rows.
fn line<'a>(content: Element<'a, Msg>) -> Element<'a, Msg> {
    ruled(
        container(content)
            .padding(Padding::from([5.0, 8.0]))
            .width(Length::Fill)
            .into(),
        crate::theme::palette().rule_faint,
    )
}

/// `content` over a one-pixel rule in `color` (the web's `border-bottom` on a table cell).
fn ruled<'a>(content: Element<'a, Msg>, color: Color) -> Element<'a, Msg> {
    column![
        content,
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(color.into()),
                ..Default::default()
            }),
    ]
    .width(Length::Fill)
    .into()
}

/// A table row that opens something on a press. The selected row, and a hovered one, take the hover tint.
fn row_button<'a>(
    content: Element<'a, Msg>,
    selected: bool,
    on_press: Msg,
    padding: Padding,
) -> Element<'a, Msg> {
    let ink = crate::theme::palette().ink;
    button(content)
        .width(Length::Fill)
        .padding(padding)
        .style(move |_: &Theme, button_status: button::Status| {
            let hovered = matches!(
                button_status,
                button::Status::Hovered | button::Status::Pressed
            );
            button::Style {
                background: (selected || hovered).then(|| crate::theme::palette().hover.into()),
                text_color: ink,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press(on_press)
        .into()
}

/// A severity or kind pill: its label in the colour, boxed in the same colour.
fn pill<'a>(label: &str, color: Color) -> Element<'a, Msg> {
    container(
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(TYPE_BADGE)
            .color(color),
    )
    .padding(Padding::from([2.0, 8.0]))
    .style(move |_: &Theme| container::Style {
        border: Border {
            color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// A plain coloured block of the given size.
fn block<'a>(width: Length, height: Length, color: Color) -> Element<'a, Msg> {
    container(Space::new())
        .width(width)
        .height(height)
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            ..Default::default()
        })
        .into()
}

/// A line of muted text where a list has nothing to show.
fn empty_line<'a>(sentence: &'static str) -> Element<'a, Msg> {
    container(ui::faint(sentence))
        .padding(Padding::from([14.0, 12.0]))
        .into()
}

/// The word a severity is shown as.
fn status_name(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Crit => "crit",
        Status::Info => "info",
        Status::Mute => "mute",
    }
}
