//! The Workspace tab's drawing (docs/design/workspace.md, section 2): the agents grouped by project on the left, the selected
//! agent's chat with its composer in the middle, and the project's board on the right. Below 120 columns one pane shows at a
//! time, with a switcher on top. The model is in `workspace`; this file turns it into styled lines.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use serde_json::Value;

use super::app::Palette;
use super::screens::{clip, local_time, show};
use super::workspace::{
    Composer, Glyph, Item, Pane, Row, Target, Transcript, Workspace, agent_of, data_of, first_seq,
    glyph, items, kind_of, shared_names, str_of, task_of, visible,
};
use crate::rich;

static NULL: Value = Value::Null;

const AGENTS_WIDTH: u16 = 30;
const CHAT_MIN: u16 = 40;
const BOARD_WIDTH: u16 = 42;
/// The gutter a chat block gets: a bar for the cursor's block, two spaces otherwise.
const GUTTER: usize = 2;

/// A character with its style: the unit that text is wrapped in.
type Cell = (char, Style);

/// Draws the tab into `area`. The focused pane is the one the keys move.
pub fn render(frame: &mut Frame, area: Rect, ws: &mut Workspace, palette: Palette) {
    if area.height < 4 || area.width < 20 {
        return;
    }
    if visible(area.width, ws.focus).len() == 1 {
        let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        switcher(frame, bar, ws.focus, palette);
        pane(frame, body, ws, ws.focus, palette);
        return;
    }
    let [agents, chat, board] = Layout::horizontal([
        Constraint::Length(AGENTS_WIDTH),
        Constraint::Min(CHAT_MIN),
        Constraint::Length(BOARD_WIDTH),
    ])
    .areas(area);
    pane(frame, agents, ws, Pane::Agents, palette);
    pane(frame, chat, ws, Pane::Chat, palette);
    pane(frame, board, ws, Pane::Board, palette);
}

fn pane(frame: &mut Frame, area: Rect, ws: &mut Workspace, which: Pane, palette: Palette) {
    match which {
        Pane::Agents => agents_pane(frame, area, ws, palette),
        Pane::Chat => chat_pane(frame, area, ws, palette),
        Pane::Board => board_pane(frame, area, ws, palette),
    }
}

/// The pane switcher of a narrow terminal: the panes by name, the shown one highlighted.
fn switcher(frame: &mut Frame, area: Rect, focus: Pane, palette: Palette) {
    let mut spans = Vec::new();
    for pane in Pane::ALL {
        let style = if pane == focus {
            Style::new()
                .fg(palette.paper)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(palette.muted)
        };
        spans.push(Span::styled(format!(" {} ", pane.title()), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        "Tab switches pane",
        Style::new().fg(palette.muted),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn pane_block(title: String, focused: bool, palette: Palette) -> Block<'static> {
    let edge = if focused {
        palette.accent
    } else {
        palette.muted
    };
    Block::bordered()
        .title(title)
        .border_style(Style::new().fg(edge))
}

fn glyph_color(mark: Glyph, palette: Palette) -> Color {
    match mark {
        Glyph::Working | Glyph::Done => palette.ok,
        Glyph::Failed => palette.accent,
        Glyph::Idle | Glyph::Sleeping => palette.muted,
    }
}

/// A line padded with spaces to `width`, so a highlighted row fills its pane.
fn fill(mut line: Line<'static>, width: usize, style: Style) -> Line<'static> {
    let used: usize = line
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum();
    if used < width {
        line.spans
            .push(Span::styled(" ".repeat(width - used), style));
    }
    line
}

fn agents_pane(frame: &mut Frame, area: Rect, ws: &mut Workspace, palette: Palette) {
    let focused = ws.focus == Pane::Agents;
    let rows = ws.rows();
    let count = rows
        .iter()
        .filter(|row| matches!(row, Row::Agent { .. }))
        .count();
    let block = pane_block(format!(" Agents · {count} "), focused, palette);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [filter_row, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let width = usize::from(inner.width);
    let muted = Style::new().fg(palette.muted);
    let filter_line = match (&ws.filtering, ws.filter.is_empty()) {
        (Some(text), _) => Line::from(vec![
            Span::styled(
                " / ",
                Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{text}_")),
        ]),
        (None, false) => Line::from(Span::styled(
            clip(&format!(" / {}  (Esc clears)", ws.filter), width),
            muted,
        )),
        (None, true) => Line::from(Span::styled(" / to filter", muted)),
    };
    frame.render_widget(Paragraph::new(filter_line), filter_row);

    let names = shared_names(&ws.agents);
    let selected = ws.selected.clone();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut chosen_lines = None;
    if rows.is_empty() {
        let empty = if ws.filtering.is_some() || !ws.filter.is_empty() {
            "No agent matches."
        } else {
            "No agents yet."
        };
        lines.push(Line::from(Span::styled(empty, muted)));
    }
    for row in &rows {
        match row {
            Row::Header { title, count } => {
                let style = Style::new().fg(palette.muted).add_modifier(Modifier::BOLD);
                lines.push(Line::from(Span::styled(
                    clip(&format!("▾ {title}  {count}"), width),
                    style,
                )));
            }
            Row::Agent { cid, project } => {
                let is_chosen = selected
                    .as_ref()
                    .is_some_and(|s| s.cid == *cid && s.project == *project);
                let agent = agent_of(&ws.agents, *cid).unwrap_or(&NULL);
                let status = show(agent.get("status"));
                let mark = glyph(&status, ws.outcomes.get(cid).map(String::as_str));
                let mut name = show(agent.get("name"));
                if names.contains(cid) {
                    name.push_str(&format!(" #{cid}"));
                }
                let row_style = if !is_chosen {
                    Style::new()
                } else if focused {
                    Style::new().fg(palette.paper).bg(palette.accent)
                } else {
                    Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)
                };
                let mark_style = if is_chosen {
                    row_style
                } else {
                    Style::new().fg(glyph_color(mark, palette))
                };
                let start = lines.len();
                let head = Line::from(vec![
                    Span::styled(format!(" {} ", mark.mark()), mark_style),
                    Span::styled(clip(&name, width.saturating_sub(3)), row_style),
                ]);
                lines.push(fill(head, width, row_style));
                if let Some(task) = task_of(agent) {
                    let style = if is_chosen {
                        row_style
                    } else {
                        muted.add_modifier(Modifier::DIM)
                    };
                    let text = format!("   {}", clip(task, width.saturating_sub(3)));
                    lines.push(fill(Line::from(Span::styled(text, style)), width, style));
                }
                if is_chosen {
                    chosen_lines = Some((start, lines.len()));
                }
            }
        }
    }
    let height = usize::from(list.height);
    let top = ws.agents_view.settle(lines.len(), height, chosen_lines);
    let end = (top + height).min(lines.len());
    frame.render_widget(Paragraph::new(lines[top..end].to_vec()), list);
}

fn chat_pane(frame: &mut Frame, area: Rect, ws: &mut Workspace, palette: Palette) {
    let focused = ws.focus == Pane::Chat;
    let steering = matches!(
        ws.composer,
        Some(Composer {
            target: Target::Steer(_),
            ..
        })
    );
    let [body, compose] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(if steering { 3 } else { 0 }),
    ])
    .areas(area);
    let Some(cid) = ws.selected_cid() else {
        let block = pane_block(" Chat ".to_string(), focused, palette);
        let inner = block.inner(body);
        frame.render_widget(block, body);
        let hint = Line::from(Span::styled(
            "No agent is selected.",
            Style::new().fg(palette.muted),
        ));
        frame.render_widget(Paragraph::new(hint), inner);
        return;
    };
    let agent = agent_of(&ws.agents, cid).unwrap_or(&NULL);
    let name = show(agent.get("name"));
    let status = show(agent.get("status"));
    let following = ws.transcripts.get(&cid).is_none_or(|t| t.view.follow);
    let title = if following {
        format!(" {name} · {status} · {} ", show(agent.get("model")))
    } else {
        format!(" {name} · {status} · End shows the newest ")
    };
    let working = status == "working";
    let rctx = rich::from_agents(&ws.agents, None);

    let block = pane_block(title, focused, palette);
    let inner = block.inner(body);
    let width = usize::from(inner.width);
    let transcript = ws.transcripts.entry(cid).or_default();
    let cursor = transcript.cursor_seq();
    let (mut lines, ranges) = chat_lines(transcript, &rctx, palette, width, cursor);
    let muted = Style::new().fg(palette.muted);
    if !transcript.loaded {
        lines.push(Line::from(Span::styled("Loading the conversation…", muted)));
    } else if ranges.is_empty() {
        lines.push(Line::from(Span::styled(
            "No messages yet. Press i to write to the agent.",
            muted,
        )));
    }
    if working {
        lines.push(Line::from(vec![
            Span::styled("● ", Style::new().fg(palette.ok)),
            Span::styled("Working…", muted),
        ]));
    }
    let keep = if transcript.reveal {
        cursor
            .and_then(|seq| ranges.iter().find(|(s, _, _)| *s == seq))
            .map(|(_, start, end)| (*start, *end))
    } else {
        None
    };
    transcript.reveal = false;
    let height = usize::from(inner.height);
    let top = transcript.view.settle(lines.len(), height, keep);
    let end = (top + height).min(lines.len());
    let shown = lines[top..end].to_vec();
    frame.render_widget(block, body);
    frame.render_widget(Paragraph::new(shown), inner);

    if let Some(composer) = ws
        .composer
        .as_ref()
        .filter(|c| matches!(c.target, Target::Steer(_)))
    {
        composer_box(
            frame,
            compose,
            composer,
            format!(" Message to {name} · Enter sends · Esc cancels · Ctrl+C stops its work "),
            palette,
        );
    }
}

fn board_pane(frame: &mut Frame, area: Rect, ws: &mut Workspace, palette: Palette) {
    let focused = ws.focus == Pane::Board;
    let posting = matches!(
        ws.composer,
        Some(Composer {
            target: Target::Board(_),
            ..
        })
    );
    let [body, compose] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(if posting { 3 } else { 0 }),
    ])
    .areas(area);
    let project = ws.want_project();
    let project_title = match &project {
        Some(id) => ws
            .projects
            .iter()
            .find(|p| str_of(p, "id") == Some(id.as_str()))
            .and_then(|p| str_of(p, "title"))
            .unwrap_or(id.as_str())
            .to_string(),
        None => "all".to_string(),
    };
    let paused = if ws.board_paused() { " · paused" } else { "" };
    let block = pane_block(
        format!(" Board · {project_title}{paused} "),
        focused,
        palette,
    );
    let inner = block.inner(body);
    let width = usize::from(inner.width);
    let rctx = rich::from_agents(&ws.agents, None);
    let mut lines: Vec<Line<'static>> = Vec::new();
    match ws.board.as_ref().filter(|b| b.project == project) {
        None => lines.push(Line::from(Span::styled(
            "Loading the board…",
            Style::new().fg(palette.muted),
        ))),
        Some(board) => {
            let posts = board
                .value
                .get("posts")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if posts.is_empty() {
                lines.push(Line::from(Span::styled(
                    "Nothing on this board yet.",
                    Style::new().fg(palette.muted),
                )));
            }
            // The hub sends the newest post first; the board reads oldest first, with the newest at the bottom.
            for post in posts.iter().rev() {
                lines.extend(post_lines(post, &rctx, palette, width));
                lines.push(Line::default());
            }
        }
    }
    let height = usize::from(inner.height);
    let top = ws.board_view.settle(lines.len(), height, None);
    let end = (top + height).min(lines.len());
    let shown = lines[top..end].to_vec();
    frame.render_widget(block, body);
    frame.render_widget(Paragraph::new(shown), inner);

    if let Some(composer) = ws
        .composer
        .as_ref()
        .filter(|c| matches!(c.target, Target::Board(_)))
    {
        let title =
            format!(" Post to {project_title} · @all or @name · Enter posts · Esc cancels ");
        composer_box(frame, compose, composer, title, palette);
    }
}

/// A composer's box: its title says where the text goes, and the text shows its last characters with a cursor.
fn composer_box(
    frame: &mut Frame,
    area: Rect,
    composer: &Composer,
    title: String,
    palette: Palette,
) {
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(palette.accent));
    let room = usize::from(area.width).saturating_sub(6);
    let line = Line::from(vec![
        Span::styled("› ", Style::new().fg(palette.accent)),
        Span::raw(format!("{}_", tail(&composer.text, room))),
    ]);
    frame.render_widget(Paragraph::new(line).block(block), area);
}

/// The last `max` characters of a text.
pub(super) fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        text.to_string()
    } else {
        text.chars().skip(count - max).collect()
    }
}

/// Plain text as cells in one style.
fn plain(text: &str, style: Style) -> Vec<Cell> {
    text.chars().map(|c| (c, style)).collect()
}

/// Agent text as cells: its references drawn as the rich spans draw them (`rich::spans`), on the base style.
fn prose(text: &str, rctx: &apz_rich::Ctx, base: Style) -> Vec<Cell> {
    let spans = rich::spans(text, rctx);
    let mut cells = Vec::new();
    for span in &spans {
        for c in span.content.chars() {
            cells.push((c, base.patch(span.style)));
        }
    }
    cells
}

/// `@name` and `@all` read as mentions: bold, in the accent colour.
fn highlight_mentions(cells: &mut [Cell], accent: Color) {
    let mut i = 0;
    while i < cells.len() {
        if cells[i].0 == '@' && (i == 0 || cells[i - 1].0.is_whitespace()) {
            let mut j = i;
            while j < cells.len() && !cells[j].0.is_whitespace() {
                cells[j].1 = cells[j].1.fg(accent).add_modifier(Modifier::BOLD);
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

/// Wraps cells to `width` columns: a line breaks at a space where a word would not fit, a word longer than a line is cut,
/// and a newline always breaks. The spaces at a break are dropped.
fn wrap(cells: &[Cell], width: usize) -> Vec<Vec<Cell>> {
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in cells.split(|cell| cell.0 == '\n') {
        wrap_paragraph(paragraph, width, &mut out);
    }
    out
}

fn wrap_paragraph(cells: &[Cell], width: usize, out: &mut Vec<Vec<Cell>>) {
    let mut line: Vec<Cell> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let spaces = cells[i].0 == ' ';
        let mut j = i;
        while j < cells.len() && (cells[j].0 == ' ') == spaces {
            j += 1;
        }
        let run = &cells[i..j];
        i = j;
        if spaces {
            if !line.is_empty() {
                if line.len() + run.len() <= width {
                    line.extend_from_slice(run);
                } else {
                    end_line(&mut line, out);
                }
            }
        } else if line.len() + run.len() <= width {
            line.extend_from_slice(run);
        } else if run.len() <= width {
            end_line(&mut line, out);
            line.extend_from_slice(run);
        } else {
            for cell in run {
                if line.len() == width {
                    end_line(&mut line, out);
                }
                line.push(*cell);
            }
        }
    }
    end_line(&mut line, out);
}

fn end_line(line: &mut Vec<Cell>, out: &mut Vec<Vec<Cell>>) {
    while line.last().is_some_and(|cell| cell.0 == ' ') {
        line.pop();
    }
    out.push(std::mem::take(line));
}

/// Cells as one line: runs of the same style become spans.
fn to_line(cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut current: Option<Style> = None;
    for (ch, style) in cells {
        if current != Some(*style) {
            if let Some(previous) = current {
                spans.push(Span::styled(std::mem::take(&mut text), previous));
            }
            current = Some(*style);
        }
        text.push(*ch);
    }
    if let Some(previous) = current {
        spans.push(Span::styled(text, previous));
    }
    Line::from(spans)
}

/// Lines of a plain text in one style, wrapped to `width`.
pub(super) fn text_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    wrap(&plain(text, style), width)
        .into_iter()
        .map(|line| to_line(&line))
        .collect()
}

/// Agent prose as lines: wrapped, with the references and the mentions styled.
pub(super) fn prose_lines(
    text: &str,
    rctx: &apz_rich::Ctx,
    accent: Color,
    width: usize,
) -> Vec<Line<'static>> {
    let mut cells = prose(text, rctx, Style::new());
    highlight_mentions(&mut cells, accent);
    wrap(&cells, width)
        .into_iter()
        .map(|line| to_line(&line))
        .collect()
}

/// Owner lines sit on the right edge.
fn right(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines.into_iter().map(Line::right_aligned).collect()
}

/// A line with a gutter span before it; the line's alignment is kept.
fn with_gutter(line: Line<'static>, gutter: Span<'static>) -> Line<'static> {
    let alignment = line.alignment;
    let mut spans = vec![gutter];
    spans.extend(line.spans);
    let mut out = Line::from(spans);
    out.alignment = alignment;
    out
}

/// The chat as lines: one block per item (an entry, or a run of steps). Returns the lines, and the line range of each block
/// by its first seq, so the view can keep the cursor's block on screen. The cursor's block has a bar in its gutter.
fn chat_lines(
    transcript: &Transcript,
    rctx: &apz_rich::Ctx,
    palette: Palette,
    width: usize,
    cursor: Option<u64>,
) -> (Vec<Line<'static>>, Vec<(u64, usize, usize)>) {
    let body_width = width.saturating_sub(GUTTER).max(1);
    let mut lines = Vec::new();
    let mut ranges = Vec::new();
    for item in items(&transcript.entries) {
        let seq = first_seq(&transcript.entries, &item);
        let block = match &item {
            Item::Entry(i) => entry_lines(&transcript.entries[*i], rctx, palette, body_width),
            Item::Steps { first, end, label } => {
                steps_lines(transcript, seq, *first, *end, label, palette, body_width)
            }
        };
        let start = lines.len();
        let gutter = if cursor == Some(seq) {
            Span::styled("▎ ", Style::new().fg(palette.accent))
        } else {
            Span::raw("  ")
        };
        for line in block {
            lines.push(with_gutter(line, gutter.clone()));
        }
        if lines.len() > start {
            lines.push(Line::default());
        }
        ranges.push((seq, start, lines.len()));
    }
    (lines, ranges)
}

/// One transcript entry as the chat shows it. Bookkeeping entries show nothing.
fn entry_lines(
    entry: &Value,
    rctx: &apz_rich::Ctx,
    palette: Palette,
    width: usize,
) -> Vec<Line<'static>> {
    let data = data_of(entry);
    let muted = Style::new().fg(palette.muted);
    let accent = Style::new().fg(palette.accent);
    match kind_of(entry) {
        "work_start" => {
            let prompt = str_of(data, "prompt").unwrap_or("").trim();
            if prompt.is_empty() {
                Vec::new()
            } else if str_of(data, "kind") == Some("chat")
                || str_of(data, "origin") == Some("owner")
            {
                right(text_lines(prompt, width, accent))
            } else if str_of(data, "kind") == Some("message")
                || str_of(data, "origin") == Some("board")
            {
                text_lines(
                    &format!("Board: {}", clip(prompt, 240)),
                    width,
                    muted.add_modifier(Modifier::ITALIC),
                )
            } else {
                text_lines(
                    &format!("Work started: {}", clip(prompt, 240)),
                    width,
                    muted,
                )
            }
        }
        "model_response" => {
            let mut out = Vec::new();
            for block in data
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let text = str_of(block, "text").unwrap_or("").trim_end();
                if str_of(block, "type") != Some("text") || text.trim().is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(Line::default());
                }
                out.extend(prose_lines(text, rctx, palette.accent, width));
            }
            out
        }
        "control" => {
            let note = data
                .get("control")
                .and_then(|c| str_of(c, "text"))
                .unwrap_or("Note sent to the agent.");
            right(text_lines(note, width, accent))
        }
        "error" => text_lines(str_of(data, "message").unwrap_or("error"), width, accent),
        "work_end" => {
            let status = str_of(data, "status").unwrap_or("finished");
            let summary = str_of(data, "summary").unwrap_or("").trim();
            let line = if summary.is_empty() {
                format!("Work {status}")
            } else {
                format!("Work {status}: {}", clip(summary, 200))
            };
            text_lines(&line, width, muted)
        }
        _ => Vec::new(),
    }
}

/// A run of tool steps: its line (`▸ 3 steps · bash`, or `▾` when open), and when open, each call and its decision.
fn steps_lines(
    transcript: &Transcript,
    seq: u64,
    first: usize,
    end: usize,
    label: &str,
    palette: Palette,
    width: usize,
) -> Vec<Line<'static>> {
    let open = transcript.open.contains(&seq);
    let marker = if open { "▾" } else { "▸" };
    let muted = Style::new().fg(palette.muted);
    let mut out = text_lines(&format!("{marker} {label}"), width, muted);
    if open {
        for entry in &transcript.entries[first..end] {
            if let Some(detail) = step_detail(entry) {
                out.extend(text_lines(&format!("    {detail}"), width, muted));
            }
        }
    }
    out
}

fn step_detail(entry: &Value) -> Option<String> {
    let data = data_of(entry);
    match kind_of(entry) {
        "tool_request" => Some(format!("→ {}", str_of(data, "name").unwrap_or("tool"))),
        "tool_decision" => Some(format!(
            "decision: {}",
            str_of(data, "decision").unwrap_or("-")
        )),
        "tool_result" => {
            let output = str_of(data, "output").unwrap_or("").replace('\n', " ");
            Some(format!("result: {}", clip(&output, 160)))
        }
        _ => None,
    }
}

/// A board post as lines: its author and time, its body with the mentions styled, and its comments.
fn post_lines(
    post: &Value,
    rctx: &apz_rich::Ctx,
    palette: Palette,
    width: usize,
) -> Vec<Line<'static>> {
    let owner = post.get("owner").and_then(Value::as_bool).unwrap_or(false);
    let author = if owner {
        "owner".to_string()
    } else {
        format!(
            "{} #{}",
            show(post.get("role")),
            show(post.get("agent_cid"))
        )
    };
    let created =
        i64::try_from(post.get("created_at").and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);
    let author_style = if owner {
        Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let mut out = vec![Line::from(vec![
        Span::styled(author, author_style),
        Span::styled(
            format!("  {}", local_time(created)),
            Style::new().fg(palette.muted),
        ),
    ])];
    out.extend(prose_lines(
        str_of(post, "body").unwrap_or("").trim_end(),
        rctx,
        palette.accent,
        width,
    ));
    for comment in post
        .get("comments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let who = str_of(comment, "author").unwrap_or("someone");
        let mut cells = plain(
            &format!("  {who}: "),
            Style::new().fg(palette.muted).add_modifier(Modifier::BOLD),
        );
        let mut body = prose(
            str_of(comment, "body").unwrap_or("").trim_end(),
            rctx,
            Style::new(),
        );
        highlight_mentions(&mut body, palette.accent);
        cells.extend(body);
        out.extend(wrap(&cells, width).into_iter().map(|line| to_line(&line)));
    }
    out
}
