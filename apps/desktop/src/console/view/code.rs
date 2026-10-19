//! The code artifact body: a box with a head (the file name or the language, and a Copy press) over the source in the
//! mono voice, with line numbers in a gutter. The first [`SHOWN_LINES`] lines show until the panel is expanded.

use super::ViewAction;
use crate::theme::{MONO, TYPE_CODE, TYPE_META, palette, skin};
use apz_view::CodeBody;
use iced::widget::text::{LineHeight, Span};
use iced::widget::{button, column, container, rich_text, row, space, text};
use iced::{Background, Border, Color, Element, Length};

/// The lines a code panel shows before its "Show all" press.
pub const SHOWN_LINES: usize = 40;

/// A run of one highlight class in a code text: char indices, end exclusive.
struct Run {
    start: usize,
    end: usize,
    class: &'static str,
}

/// The highlight runs of a code text, from the highlighter both consoles share (`apz_view::highlight`, rules in
/// `apps/shared/highlight.json`).
fn runs(language: &str, text: &str) -> Vec<Run> {
    apz_view::highlight::spans(language, text)
        .into_iter()
        .map(|s| Run {
            start: s.start,
            end: s.end,
            class: apz_view::highlight::token(s.class),
        })
        .collect()
}

/// The palette colour of a highlight class (`keyword`, `syn-keyword` …); unknown classes read as ink.
fn token_color(class: &str, p: &crate::theme::Palette) -> Color {
    match class.trim_start_matches("syn-") {
        "keyword" => p.syn_keyword,
        "string" => p.syn_string,
        "type" => p.syn_type,
        "number" => p.syn_number,
        "comment" => p.syn_comment,
        "function" => p.syn_function,
        "variable" => p.syn_variable,
        _ => p.ink,
    }
}

/// One line of code as coloured spans: the runs that fall in `[from, to)` of the text's chars, the gaps in ink.
fn line_spans(
    chars: &[char],
    from: usize,
    to: usize,
    runs: &[Run],
    p: &crate::theme::Palette,
) -> Vec<Span<'static, ViewAction>> {
    let mut spans = Vec::new();
    let mut at = from;
    for run in runs.iter().filter(|r| r.end > from && r.start < to) {
        let start = run.start.max(from);
        let end = run.end.min(to);
        if start > at {
            spans.push(segment(chars, at, start, p.ink));
        }
        if end > start {
            spans.push(segment(chars, start, end, token_color(run.class, p)));
        }
        at = end.max(at);
    }
    if at < to {
        spans.push(segment(chars, at, to, p.ink));
    }
    if spans.is_empty() {
        spans.push(segment(chars, from, from, p.ink));
    }
    spans
}

/// A span of the chars in `[from, to)`, in the mono voice at the code size.
fn segment(chars: &[char], from: usize, to: usize, color: Color) -> Span<'static, ViewAction> {
    let s: String = chars[from..to].iter().collect();
    // An empty line still needs a span of one space so it keeps its height.
    let s = if s.is_empty() { " ".to_owned() } else { s };
    Span::new(s).font(MONO).size(TYPE_CODE).color(color)
}

/// One code panel, collapsed to [`SHOWN_LINES`] lines: the head (file name or language, Copy) over the source.
pub fn code_view<'a, M: Clone + 'a>(
    panel_id: &'a str,
    c: &'a CodeBody,
    expanded: bool,
    on: &'a dyn Fn(ViewAction) -> M,
) -> Element<'a, M> {
    let p = *palette();
    let radius = skin().radius;
    let label = if c.filename.is_empty() {
        c.language.as_str()
    } else {
        c.filename.as_str()
    };

    let copy = button(text("Copy").size(TYPE_META).color(p.ink_2))
        .style(button::text)
        .on_press(on(ViewAction::CopyCode(c.text.clone())));
    let head = row![
        text(label).font(MONO).size(TYPE_META).color(p.ink_2),
        space::horizontal(),
        copy,
    ]
    .spacing(8);

    let chars: Vec<char> = c.text.chars().collect();
    let runs = runs(&c.language, &c.text);
    let total = c.text.split('\n').count();
    let shown = if expanded {
        total
    } else {
        total.min(SHOWN_LINES)
    };
    let digits = total.to_string().len();
    let gutter_width = 12.0 + 8.0 * digits as f32;

    let mut rows: Vec<Element<'a, M>> = Vec::new();
    let mut offset = 0usize;
    for (index, line) in c.text.split('\n').enumerate().take(shown) {
        let len = line.chars().count();
        let number = text(format!("{}", index + 1))
            .font(MONO)
            .size(TYPE_CODE)
            .color(p.ink_3)
            .line_height(LineHeight::Relative(1.5))
            .width(Length::Fixed(gutter_width));
        // The spans never carry a link, so the Rich widget's `ViewAction` message is never sent; the map only
        // lifts its type to the screen's message.
        let source: Element<'a, M> = Element::from(
            rich_text(line_spans(&chars, offset, offset + len, &runs, &p))
                .line_height(LineHeight::Relative(1.5)),
        )
        .map(on);
        rows.push(row![number, source].spacing(12).into());
        offset += len + 1;
    }

    let mut body = column![head, column(rows)].spacing(10);
    if !expanded && total > SHOWN_LINES {
        body = body.push(
            button(
                text(format!("Show all {total} lines"))
                    .size(TYPE_META)
                    .color(p.signal_ink),
            )
            .style(button::text)
            .on_press(on(ViewAction::LoadMore(panel_id.to_owned()))),
        );
    }

    container(body)
        .padding(10)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(p.sheet_2)),
            border: Border {
                color: p.rule_faint,
                width: 1.0,
                radius: radius.into(),
            },
            ..container::Style::default()
        })
        .into()
}
