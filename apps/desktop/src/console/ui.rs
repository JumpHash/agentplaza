//! The console's design kit, in the web console's language (`apps/web/DESIGN.md`): paper and
//! sheets, ink rules, the orange signal for the owner's hand, square corners, mono for titles,
//! labels and figures, serif for prose. Screens build from these, never from raw colours.

use iced::widget::{button, column, container, rich_text, row, scrollable, span, svg, text, Space};
use iced::{Alignment, Border, Color, Element, Font, Length, Padding, Theme};

pub use crate::theme::{INK, INK_2, INK_3, PAPER, RULE, RULE_FAINT, SHEET, SIGNAL};
use crate::theme::{CRIT, HOVER, MONO, OK, SERIF, WARN};

/// A semantic tone for badges, notices and buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Info,
    Ok,
    Warn,
    Danger,
    Signal,
}

impl Tone {
    pub fn color(self) -> Color {
        match self {
            Tone::Muted => INK_3,
            Tone::Info => Color::from_rgb8(0x2d, 0x5f, 0x8a),
            Tone::Ok => OK,
            Tone::Warn => WARN,
            Tone::Danger => CRIT,
            Tone::Signal => SIGNAL,
        }
    }
}

/// Mono text at `size` in `color`.
pub fn mono<'a>(content: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(content).font(MONO).size(size).color(color)
}

/// Serif prose.
pub fn prose<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).font(SERIF).size(15.0).color(INK)
}

/// Faint serif, for secondary lines.
pub fn faint<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).font(SERIF).size(13.0).color(INK_3)
}

/// A small uppercase mono label.
pub fn label<'a>(content: &str) -> text::Text<'a> {
    text(content.to_uppercase()).font(MONO).size(11.0).color(INK_3)
}

/// A lucide icon at `size`, tinted `color`.
pub fn icon<'a, M: 'a>(name: &str, size: f32, color: Color) -> Element<'a, M> {
    match crate::console::icons::svg(name) {
        Some(doc) => svg(svg::Handle::from_memory(doc.as_bytes().to_vec()))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .style(move |_: &Theme, _| svg::Style { color: Some(color) })
            .into(),
        None => Space::new().width(Length::Fixed(size)).height(Length::Fixed(size)).into(),
    }
}

/// A screen's sheet: the page's one bordered surface, uniform padding, full width.
pub fn sheet<'a, M: 'a>(head: Element<'a, M>, body: Element<'a, M>) -> Element<'a, M> {
    container(column![head, body].spacing(22))
        .padding(Padding { top: 0.0, right: 28.0, bottom: 28.0, left: 28.0 })
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(SHEET.into()),
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// A sheet's head: the title (uppercase mono) left, actions right, a 2px ink rule under it.
pub fn view_head<'a, M: 'a>(title: &str, actions: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut line = row![text(title.to_uppercase()).font(MONO).size(28.0).color(INK)].align_y(Alignment::End).spacing(16);
    line = line.push(Space::new().width(Length::Fill));
    for action in actions {
        line = line.push(action);
    }
    column![
        container(line).padding(Padding { top: 24.0, right: 0.0, bottom: 14.0, left: 0.0 }),
        container(Space::new().height(2.0)).width(Length::Fill).style(|_: &Theme| container::Style { background: Some(INK.into()), ..Default::default() }),
    ]
    .into()
}

/// A card: a mono header row over its body, 1px rule borders.
pub fn card<'a, M: 'a>(title: &str, body: Element<'a, M>) -> Element<'a, M> {
    column![
        container(label(title)).padding([8, 12]).width(Length::Fill).style(|_: &Theme| container::Style {
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }),
        container(body).padding(12).width(Length::Fill).style(|_: &Theme| container::Style {
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }),
    ]
    .into()
}

/// A square tag: mono uppercase text in a tone, with a 1px border in that tone.
pub fn badge<'a, M: 'a>(content: &str, tone: Tone) -> Element<'a, M> {
    let c = tone.color();
    container(text(content.to_uppercase()).font(MONO).size(10.5).color(c))
        .padding([2, 6])
        .style(move |_: &Theme| container::Style {
            border: Border { color: c, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// Button kinds, as on the web console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The owner's primary act: solid signal.
    Primary,
    /// A plain bordered button.
    Secondary,
    /// Text only.
    Ghost,
    /// Irreversible: solid crit.
    Danger,
}

/// A mono uppercase button with an optional leading icon.
pub fn btn<'a, M: Clone + 'a>(label_text: &str, icon_name: Option<&str>, kind: Kind, on_press: Option<M>) -> Element<'a, M> {
    let fg = match kind {
        Kind::Primary | Kind::Danger => Color::WHITE,
        _ => INK,
    };
    let mut inner = row![].spacing(6).align_y(Alignment::Center);
    if let Some(name) = icon_name {
        inner = inner.push(icon(name, 14.0, fg));
    }
    inner = inner.push(text(label_text.to_uppercase()).font(MONO).size(11.5).color(fg));
    let b = button(inner).padding([7, 12]).style(move |_: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, border) = match kind {
            Kind::Primary => (Some(SIGNAL), INK),
            Kind::Danger => (Some(CRIT), INK),
            Kind::Secondary => (if hovered { Some(HOVER) } else { None }, INK),
            Kind::Ghost => (if hovered { Some(HOVER) } else { None }, Color::TRANSPARENT),
        };
        button::Style {
            background: bg.map(Into::into),
            text_color: fg,
            border: Border { color: border, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }
    });
    match on_press {
        Some(m) => b.on_press(m).into(),
        None => b.into(),
    }
}

/// An empty state: an icon, a bold mono line, a faint sentence; centred.
pub fn empty<'a, M: 'a>(icon_name: &str, title: &str, sentence: &str) -> Element<'a, M> {
    container(
        column![icon(icon_name, 18.0, INK_3), mono(title.to_string(), 13.0, INK), faint(sentence.to_string())]
            .spacing(6)
            .align_x(Alignment::Center),
    )
    .padding([40, 16])
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

/// An inline error line (crit).
pub fn error_line<'a, M: 'a>(message: &str) -> Element<'a, M> {
    text(message.to_string()).font(SERIF).size(13.0).color(CRIT).into()
}

/// A faint "Loading…" line.
pub fn loading<'a, M: 'a>() -> Element<'a, M> {
    mono("Loading…", 12.0, INK_3).into()
}

/// Agent-written markdown in the design language: mono headings (a hairline under the first),
/// serif prose, inline spans, sheet code blocks, ruled quotes and grid tables. Unknown syntax
/// renders as plain text; nothing is dropped.
pub fn markdown<'a, M: 'a>(source: &str) -> Element<'a, M> {
    blocks_view(parse_blocks(source))
}

/// A parsed block, before layout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    Heading(u8, String),
    Para(String),
    List(Vec<Item>),
    Code(String),
    Quote(Vec<Block>),
    Table { head: Vec<String>, rows: Vec<Vec<String>> },
    Rule,
}

/// One list item. `depth` 1 nests under the item before it; `marker` numbers an ordered item.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    depth: usize,
    marker: Option<usize>,
    text: String,
}

/// An inline span of a paragraph, heading or cell.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Inline {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    /// A link's label, shown underlined in signal; the target is not followed.
    Link(String),
}

fn parse_blocks(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
        } else if line.starts_with("```") {
            // A fence runs to its closing line or to the end of the document; unterminated, its text stays.
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != "```" {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code(code.join("\n")));
        } else if let Some((level, title)) = heading(line) {
            blocks.push(Block::Heading(level, title.to_string()));
            i += 1;
        } else if is_rule(line) {
            blocks.push(Block::Rule);
            i += 1;
        } else if line.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let body = lines[i].trim_start().strip_prefix('>').unwrap_or_default();
                inner.push(body.strip_prefix(' ').unwrap_or(body));
                i += 1;
            }
            blocks.push(Block::Quote(parse_blocks(&inner.join("\n"))));
        } else if list_item(lines[i]).is_some() {
            let mut items = Vec::new();
            while i < lines.len() {
                if let Some(item) = list_item(lines[i]) {
                    items.push(item);
                } else if let (true, Some(last)) = (lines[i].starts_with(' ') && !lines[i].trim().is_empty(), items.last_mut()) {
                    // An indented line continues the item above.
                    last.text = format!("{} {}", last.text, lines[i].trim());
                } else {
                    break;
                }
                i += 1;
            }
            blocks.push(Block::List(items));
        } else if is_table_start(&lines, i) {
            let head = split_row(lines[i]);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(split_row(lines[i]));
                i += 1;
            }
            blocks.push(Block::Table { head, rows });
        } else {
            // A paragraph runs to a blank line or the start of another block; its lines join with spaces.
            let mut para = vec![line];
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(&lines, i) {
                para.push(lines[i].trim());
                i += 1;
            }
            blocks.push(Block::Para(para.join(" ")));
        }
    }
    blocks
}

/// `#`, `##` or `###` and a title: the line's level and its text.
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let title = line.get(hashes..)?.strip_prefix(' ')?;
    matches!(hashes, 1..=3).then_some((hashes as u8, title.trim()))
}

/// Three or more of one of `-`, `*`, `_` alone on a line.
fn is_rule(line: &str) -> bool {
    line.len() >= 3 && ['-', '*', '_'].iter().any(|&c| line.chars().all(|x| x == c))
}

/// A list item line: a bullet (`-`, `*`, `+`) or a number (`1.`, `1)`), at depth 0 or, indented, 1.
fn list_item(line: &str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim();
    let depth = usize::from(indent >= 2);
    if let Some(body) = t.strip_prefix(['-', '*', '+']).and_then(|r| r.strip_prefix(' ')) {
        return Some(Item { depth, marker: None, text: body.trim().to_string() });
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=9).contains(&digits) {
        return None;
    }
    let rest = t.get(digits..)?;
    let body = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    let marker = t[..digits].parse().ok()?;
    Some(Item { depth, marker: Some(marker), text: body.trim().to_string() })
}

fn starts_block(lines: &[&str], i: usize) -> bool {
    let Some(raw) = lines.get(i) else { return false };
    let line = raw.trim();
    line.starts_with("```") || heading(line).is_some() || is_rule(line) || line.starts_with('>') || list_item(raw).is_some() || is_table_start(lines, i)
}

fn is_table_start(lines: &[&str], i: usize) -> bool {
    match (lines.get(i), lines.get(i + 1)) {
        (Some(head), Some(sep)) => head.contains('|') && is_table_sep(sep) && split_row(head).len() == split_row(sep).len(),
        _ => false,
    }
}

fn is_table_sep(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// The cells of a table row, outer pipes dropped.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Inline spans: `code`, **bold**, *italic*, [label](target). Anything else is text.
fn inline(source: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = source;
    while !rest.is_empty() {
        if let Some((token, used)) = inline_token(rest) {
            if !plain.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut plain)));
            }
            out.push(token);
            rest = &rest[used..];
        } else {
            let width = rest.chars().next().map_or(1, char::len_utf8);
            plain.push_str(&rest[..width]);
            rest = &rest[width..];
        }
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

/// The span at the start of `s` and how many bytes it used, if `s` opens one that closes.
fn inline_token(s: &str) -> Option<(Inline, usize)> {
    if let Some(body) = s.strip_prefix('`') {
        let end = body.find('`')?;
        Some((Inline::Code(body[..end].to_string()), end + 2))
    } else if let Some(body) = s.strip_prefix("**") {
        let end = body.find("**")?;
        Some((Inline::Bold(body[..end].to_string()), end + 4))
    } else if let Some(body) = s.strip_prefix('*') {
        let end = body.find('*')?;
        if end == 0 || body.starts_with(char::is_whitespace) {
            return None;
        }
        Some((Inline::Italic(body[..end].to_string()), end + 2))
    } else if let Some(body) = s.strip_prefix('[') {
        let close = body.find(']')?;
        let end = body[close + 1..].strip_prefix('(')?.find(')')?;
        Some((Inline::Link(body[..close].to_string()), close + end + 4))
    } else {
        None
    }
}

fn blocks_view<'a, M: 'a>(blocks: Vec<Block>) -> Element<'a, M> {
    column(blocks.into_iter().map(block_view).collect::<Vec<_>>()).spacing(12).width(Length::Fill).into()
}

fn block_view<'a, M: 'a>(block: Block) -> Element<'a, M> {
    match block {
        Block::Heading(level, title) => {
            let size = match level {
                1 => 18.0,
                2 => 15.0,
                _ => 13.0,
            };
            let head = mono(title, size, INK);
            if level == 1 {
                column![head, hairline()].spacing(8).width(Length::Fill).into()
            } else {
                head.into()
            }
        }
        Block::Para(body) => rich_para(inline(&body), SERIF, 15.0, INK),
        Block::List(items) => column(items.into_iter().map(item_view).collect::<Vec<_>>()).spacing(4).width(Length::Fill).into(),
        Block::Code(code) => container(
            scrollable(text(code).font(MONO).size(12.5).color(INK)).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::default())),
        )
        .padding(10)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(SHEET.into()),
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into(),
        Block::Quote(inner) => {
            let bar = container(Space::new()).width(Length::Fixed(3.0)).height(Length::Fill).style(|_: &Theme| container::Style {
                background: Some(INK_3.into()),
                ..Default::default()
            });
            row![bar, blocks_view(inner)].spacing(14).width(Length::Fill).into()
        }
        Block::Table { head, rows } => {
            let cols = rows.iter().map(Vec::len).chain(std::iter::once(head.len())).max().unwrap_or(1).max(1);
            let mut grid = vec![table_row(&head, cols, true)];
            grid.extend(rows.iter().map(|r| table_row(r, cols, false)));
            column(grid).width(Length::Fill).into()
        }
        Block::Rule => hairline(),
    }
}

fn item_view<'a, M: 'a>(item: Item) -> Element<'a, M> {
    let marker = item.marker.map_or_else(|| "•".to_string(), |n| format!("{n}."));
    let indent = if item.depth > 0 { 24.0 } else { 0.0 };
    row![
        Space::new().width(Length::Fixed(indent)),
        mono(marker, 13.0, INK_2).width(Length::Fixed(22.0)),
        rich_para(inline(&item.text), SERIF, 15.0, INK),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// A table row over `cols` equal cells; the header's cells are mono.
fn table_row<'a, M: 'a>(cells: &[String], cols: usize, header: bool) -> Element<'a, M> {
    let row_cells = (0..cols)
        .map(|k| -> Element<'a, M> {
            let cell = cells.get(k).map_or("", String::as_str);
            let content = if header { rich_para(inline(cell), MONO, 12.0, INK) } else { rich_para(inline(cell), SERIF, 14.0, INK) };
            container(content)
                .padding([5, 8])
                .width(Length::FillPortion(1))
                .style(|_: &Theme| container::Style {
                    border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
                    ..Default::default()
                })
                .into()
        })
        .collect::<Vec<_>>();
    row(row_cells).width(Length::Fill).into()
}

/// A run of inline spans as one wrapping rich text.
fn rich_para<'a, M: 'a>(parts: Vec<Inline>, base: Font, size: f32, color: Color) -> Element<'a, M> {
    let spans: Vec<text::Span<'a>> = parts.into_iter().map(|part| part_span(part, base)).collect();
    rich_text(spans).font(base).size(size).color(color).width(Length::Fill).into()
}

fn part_span<'a>(part: Inline, base: Font) -> text::Span<'a> {
    match part {
        Inline::Text(s) => span(s),
        Inline::Bold(s) => span(s).font(Font { weight: iced::font::Weight::Bold, ..base }),
        Inline::Italic(s) => span(s).font(Font { style: iced::font::Style::Italic, ..base }),
        Inline::Code(s) => span(s).font(MONO).background(RULE_FAINT),
        Inline::Link(label) => span(label).color(SIGNAL).underline(true),
    }
}

/// The page background.
pub fn paper(_: &Theme) -> container::Style {
    container::Style { background: Some(PAPER.into()), ..Default::default() }
}

/// A hairline rule.
pub fn hairline<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().height(1.0)).width(Length::Fill).style(|_: &Theme| container::Style { background: Some(RULE_FAINT.into()), ..Default::default() }).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_document_into_blocks() {
        let doc = "# Title\n\nSome para\ncontinues here.\n\n- one\n  - nested\n\n1. first\n\n```\nlet x = 1;\n```\n\n> quoted\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n---";
        assert_eq!(
            parse_blocks(doc),
            vec![
                Block::Heading(1, "Title".into()),
                Block::Para("Some para continues here.".into()),
                Block::List(vec![
                    Item { depth: 0, marker: None, text: "one".into() },
                    Item { depth: 1, marker: None, text: "nested".into() },
                ]),
                Block::List(vec![Item { depth: 0, marker: Some(1), text: "first".into() }]),
                Block::Code("let x = 1;".into()),
                Block::Quote(vec![Block::Para("quoted".into())]),
                Block::Table { head: vec!["a".into(), "b".into()], rows: vec![vec!["1".into(), "2".into()]] },
                Block::Rule,
            ]
        );
    }

    #[test]
    fn inline_spans_keep_their_kinds() {
        assert_eq!(
            inline("a **b** `c` [d](e)"),
            vec![
                Inline::Text("a ".into()),
                Inline::Bold("b".into()),
                Inline::Text(" ".into()),
                Inline::Code("c".into()),
                Inline::Text(" ".into()),
                Inline::Link("d".into()),
            ]
        );
    }

    #[test]
    fn unterminated_fence_keeps_its_text() {
        assert_eq!(parse_blocks("```\nfoo\nbar"), vec![Block::Code("foo\nbar".into())]);
    }
}
