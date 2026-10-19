//! Shared helpers for the Lab screens, as the web's `lab-common.ts`: the formatters every section
//! uses (counts, sizes, durations, percents, GPU hours, times), the hub's live `training` frames on
//! the event stream, and the pieces the sections share in the web's look: a table as the web's
//! `.tbl`, a meter, a note line and a clickable row. The shown screen reloads on every poll tick
//! (the console's 4 s `Tick`), so a Lab screen polls by its `load`; one that wants the live lines
//! subscribes with [`live`] and reads them with [`training_frame`].

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, button, column, container, row, text};
use iced::{
    Alignment, Background, Border, Color, Element, Length, Padding, Subscription, Theme, alignment,
};
use serde_json::Value;

use crate::console::ui::{SANS_MEDIUM, SANS_SEMIBOLD};
use crate::console::{client::Client, events, ui};
use crate::theme::{SANS, TYPE_ITEM, TYPE_META, TYPE_UI};

/// A trainer line from the hub's `training` frame: the job's id and its stored event line
/// (`{seq, island, node, event}`, where `event.ev` names the trainer's event).
#[derive(Debug, Clone)]
pub struct TrainingFrame {
    pub job: String,
    pub line: Value,
}

/// The hub's event stream, for a screen that wants live `training` frames. Map its events through
/// [`training_frame`]; the stream is shared with every other subscriber of the same hub.
pub fn live(client: &Client) -> Subscription<events::Event> {
    events::subscription(client)
}

/// The `training` frame an event carries, or None for any other event.
pub fn training_frame(event: &events::Event) -> Option<TrainingFrame> {
    if event.kind != "training" {
        return None;
    }
    let job = event.data.get("job")?.as_str()?.to_string();
    let line = event.data.get("line")?.clone();
    Some(TrainingFrame { job, line })
}

/// A count or quantity with grouping and `digits` decimals; a dash for a value that is not finite.
pub fn number(n: f64, digits: usize) -> String {
    if !n.is_finite() {
        return "—".to_string();
    }
    let text = format!("{:.*}", digits, n.abs());
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text.as_str(), None),
    };
    let mut out = String::new();
    if n < 0.0 {
        out.push('-');
    }
    let len = whole.len();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

/// A whole count with grouping.
pub fn count(n: u64) -> String {
    number(n as f64, 0)
}

/// A byte count in binary units (B, KB, MB, GB, TB).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    format!("{:.*} {}", digits, value, UNITS[unit])
}

/// A duration from milliseconds: `840 ms`, `12.5 s`, `4m 07s`, `2h 13m`.
pub fn duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let seconds = ms as f64 / 1000.0;
    if seconds < 60.0 {
        return format!("{seconds:.1} s");
    }
    let total = seconds.round() as u64;
    if total < 3600 {
        format!("{}m {:02}s", total / 60, total % 60)
    } else {
        format!("{}h {:02}m", total / 3600, (total % 3600) / 60)
    }
}

/// A ratio (0 to 1) as a percent: `0.42` is `42%`. Pass `util_pct / 100` for the device figures.
pub fn percent(ratio: f64) -> String {
    if !ratio.is_finite() {
        return "—".to_string();
    }
    format!("{:.0}%", ratio * 100.0)
}

/// The GPU time a job or run has used.
pub fn gpu_hours(hours: f64) -> String {
    if !hours.is_finite() {
        return "—".to_string();
    }
    format!("{hours:.2} GPU-h")
}

/// An absolute local time from unix milliseconds, `2026-10-13 14:05` (the web's `fmtTime` prints the
/// same); a dash for 0.
pub fn time(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    match chrono::DateTime::from_timestamp_millis(ms as i64) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => "—".to_string(),
    }
}

/// A relative time, as the web's `ago`; `ms` is unix milliseconds.
pub fn ago(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let seconds = now_ms().saturating_sub(ms) / 1000;
    match seconds {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

/// The current time, unix milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---- The web's pieces ------------------------------------------------------

/// A note line, as the web's `.lab-note`: the meta size in ink-3.
pub fn note<'a, M: 'a>(content: impl Into<String>) -> Element<'a, M> {
    ui::faint(content.into()).into()
}

/// A figure in the console's bold value face, as the web's `.lab-counter`: the page title's size.
pub fn figure<'a, M: 'a>(content: impl Into<String>) -> Element<'a, M> {
    text(content.into())
        .font(SANS_SEMIBOLD)
        .size(ui::TYPE_H1)
        .color(crate::theme::palette().ink)
        .into()
}

/// A rule across, in the given colour.
pub fn rule<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// A meter, as the web's `.lab-meter`: a 6-pixel track with the share in signal, or in crit when
/// `over` (a job past its budget).
pub fn meter<'a, M: 'a>(ratio: f64, over: bool) -> Element<'a, M> {
    const SCALE: u16 = 1000;
    let on = (ratio.clamp(0.0, 1.0) * f64::from(SCALE)).round() as u16;
    let fill = if over {
        crate::theme::palette().crit
    } else {
        crate::theme::palette().signal
    };
    let mut parts: Vec<Element<'a, M>> = Vec::new();
    if on > 0 {
        parts.push(
            container(Space::new().height(Length::Fill))
                .width(Length::FillPortion(on))
                .height(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(Background::Color(fill)),
                    ..Default::default()
                })
                .into(),
        );
    }
    if on < SCALE {
        parts.push(
            Space::new()
                .width(Length::FillPortion(SCALE - on))
                .height(Length::Fill)
                .into(),
        );
    }
    container(row(parts).width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(6.0))
        .clip(true)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(crate::theme::palette().sheet_2)),
            border: Border::default().rounded(3.0),
            ..Default::default()
        })
        .into()
}

/// A link inside a Lab screen, as the web's `a`: the label in ink, signal-ink under the pointer.
pub fn link<'a, M: Clone + 'a>(label: &str, msg: M) -> Element<'a, M> {
    ui::text_link(label, msg)
}

/// A table column: its head, its share of the width, and whether its cells read right-aligned.
#[derive(Debug, Clone, Copy)]
pub struct Col {
    pub head: &'static str,
    pub portion: u16,
    pub right: bool,
}

/// A column, for [`table`].
pub fn col(head: &'static str, portion: u16, right: bool) -> Col {
    Col {
        head,
        portion,
        right,
    }
}

/// One table row: its cells in column order and the message a press on the row sends (the web's
/// `tr.clickable`); a row without one is static.
pub struct Row<'a, M> {
    pub cells: Vec<Element<'a, M>>,
    pub on_press: Option<M>,
}

/// A table as the web's `.tbl` in a card body: the heads in the label's ink over a strong rule, then
/// each row (9 pixels above and below, a hairline between rows, the hover ground under a clickable
/// one). The first and last cells sit flush with the card body's edges.
pub fn table<'a, M: Clone + 'a>(cols: &[Col], rows: Vec<Row<'a, M>>) -> Element<'a, M> {
    let heads: Vec<Element<'a, M>> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let label = text(c.head.to_string())
                .font(SANS_MEDIUM)
                .size(TYPE_META)
                .color(crate::theme::palette().ink_3);
            cell_box(label.into(), c, i, cols.len(), 8.0)
        })
        .collect();
    let mut lines: Vec<Element<'a, M>> = vec![
        row(heads).width(Length::Fill).into(),
        rule(crate::theme::palette().rule_strong),
    ];
    let n = rows.len();
    for (slot, r) in rows.into_iter().enumerate() {
        let cells: Vec<Element<'a, M>> = r
            .cells
            .into_iter()
            .zip(cols)
            .enumerate()
            .map(|(i, (cell, c))| cell_box(cell, c, i, cols.len(), 9.0))
            .collect();
        let line = container(row(cells).align_y(Alignment::Center).width(Length::Fill))
            .width(Length::Fill);
        let line: Element<'a, M> = match r.on_press {
            Some(msg) => button(line)
                .on_press(msg)
                .style(row_style)
                .padding(0)
                .width(Length::Fill)
                .into(),
            None => line.into(),
        };
        lines.push(line);
        if slot + 1 < n {
            lines.push(rule(crate::theme::palette().rule));
        }
    }
    column(lines).width(Length::Fill).into()
}

/// A cell in its column's share: 12 pixels between columns, none at the card's edges, right-aligned
/// for a numeric column.
fn cell_box<'a, M: 'a>(
    content: Element<'a, M>,
    c: &Col,
    i: usize,
    n: usize,
    vertical: f32,
) -> Element<'a, M> {
    let left = if i == 0 { 0.0 } else { 12.0 };
    let right = if i + 1 == n { 0.0 } else { 12.0 };
    let align = if c.right {
        alignment::Horizontal::Right
    } else {
        alignment::Horizontal::Left
    };
    container(content)
        .padding(Padding {
            top: vertical,
            right,
            bottom: vertical,
            left,
        })
        .width(Length::FillPortion(c.portion))
        .align_x(align)
        .into()
}

/// A table cell's text: Martian Mono for an id, a recipe or a path, Inter for the rest, in ink.
pub fn cell<'a, M: 'a>(value: impl Into<String>, mono: bool) -> Element<'a, M> {
    let value = value.into();
    let ink = crate::theme::palette().ink;
    if mono {
        ui::mono(value, 12.0, ink).into()
    } else {
        ui::sans(value, 13.0, ink).into()
    }
}

/// The row's hover ground, as the web's `tr.clickable:hover td`.
fn row_style(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(crate::theme::palette().hover)),
        text_color: crate::theme::palette().ink,
        border: Border::default(),
        ..Default::default()
    }
}

/// A failed load, in the web's `.lab-err`: the crit ink at the small size.
pub fn err<'a, M: 'a>(message: &str) -> Element<'a, M> {
    text(message.to_string())
        .font(SANS)
        .size(TYPE_SM)
        .color(crate::theme::palette().crit)
        .into()
}
/// The small size of the web's `--fs-sm`, for errors and dense notes.
const TYPE_SM: f32 = 12.5;

/// A relative time as the web's `agoText`: `just now`, `30 min ago`, `2 h ago`, `3 d ago`.
pub fn ago_text(ms: u64, now: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let s = now.saturating_sub(ms) / 1000;
    if s < 45 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{} min ago", ((s as f64) / 60.0).round().max(1.0) as u64)
    } else if s < 86_400 {
        format!("{} h ago", ((s as f64) / 3600.0).round() as u64)
    } else {
        format!("{} d ago", ((s as f64) / 86_400.0).round() as u64)
    }
}

/// A job's run time as the web's `runTimeText`: `40 min`, `2.0 h`.
pub fn run_time(ms: u64) -> String {
    if ms < 3_600_000 {
        format!("{} min", ((ms as f64) / 60_000.0).round() as u64)
    } else {
        format!("{:.1} h", ms as f64 / 3_600_000.0)
    }
}

/// A filter chip as the web's `.lab-chip`: a pill with its label and count; the chosen one is filled
/// in signal-soft.
pub fn chip<'a, M: Clone + 'a>(label: &str, count: usize, on: bool, msg: M) -> Element<'a, M> {
    let ink = if on {
        crate::theme::palette().ink
    } else {
        crate::theme::palette().ink_2
    };
    let body = row![
        text(label.to_string()).font(SANS).size(13.0).color(ink),
        text(count.to_string()).font(SANS).size(13.0).color(ink),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    button(body)
        .padding(Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 10.0,
        })
        .height(Length::Fixed(28.0))
        .on_press(msg)
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let palette = crate::theme::palette();
            button::Style {
                background: Some(Background::Color(if on {
                    palette.signal_soft
                } else if hovered {
                    palette.hover
                } else {
                    palette.panel
                })),
                text_color: ink,
                border: Border {
                    color: if on { Color::TRANSPARENT } else { palette.rule },
                    width: 1.0,
                    radius: 999.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

/// An empty state as the web's `labEmpty`: a circled icon, a title, an optional sentence and an
/// optional action under them.
pub fn empty<'a, M: 'a>(
    icon: &str,
    title: &str,
    sentence: Option<&str>,
    action: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut parts: Vec<Element<'a, M>> = vec![
        container(ui::icon(icon, 22.0, crate::theme::palette().ink_2))
            .padding(12)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(crate::theme::palette().sheet_2)),
                border: Border::default().rounded(999.0),
                ..Default::default()
            })
            .into(),
        text(title.to_string())
            .font(SANS_MEDIUM)
            .size(TYPE_ITEM)
            .color(crate::theme::palette().ink)
            .into(),
    ];
    if let Some(sentence) = sentence {
        parts.push(
            text(sentence.to_string())
                .font(SANS)
                .size(TYPE_UI)
                .color(crate::theme::palette().ink_3)
                .into(),
        );
    }
    if let Some(action) = action {
        parts.push(action);
    }
    container(column(parts).spacing(6).align_x(Alignment::Center))
        .padding(Padding {
            top: 44.0,
            right: 16.0,
            bottom: 44.0,
            left: 16.0,
        })
        .width(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .into()
}
