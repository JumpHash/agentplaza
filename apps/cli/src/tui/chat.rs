//! The Chat tab: your chats (`agentplaza chats`): the orchestrator's (`main`), a project's with its planner and a task's.
//! The list (`GET /v1/chats`, newest activity first) is on the left; the selected chat on the right: its messages, newest
//! at the bottom. A card waiting for you shows as a line that names it, and the Inbox tab answers it. `t` writes to the
//! selected chat and `n` opens a new one with the project the switcher shows (or the first open project) and writes the
//! first message to it (the footer takes the text; Enter sends it). A chat is read when it is selected and the list says
//! it changed. Declared from `app.rs`, which holds the tab's state and runs its requests.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use serde_json::Value;

use super::{App, Palette, style};
use crate::tui::screens::{clip, local_time, show};
use crate::tui::workspace_view::{prose_lines, text_lines, with_gutter};

/// The hub's list of your chats, which the tab polls while it shows.
pub const PATH: &str = "/v1/chats";
/// How long a read of a chat waits for its answer before it may be asked again.
const RETRY: Duration = Duration::from_secs(5);
/// The width of the chats list beside the chat.
const LIST_WIDTH: u16 = 46;
/// Below this many columns the list sits above the chat.
const SIDE_BY_SIDE: u16 = 100;

/// A message being written in the footer: to a chat (by its id), or the first message of a new chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compose {
    To(String, String),
    New(String),
}

impl Compose {
    /// The text so far.
    pub fn text(&self) -> &str {
        match self {
            Compose::To(_, text) | Compose::New(text) => text,
        }
    }

    pub fn text_mut(&mut self) -> &mut String {
        match self {
            Compose::To(_, text) | Compose::New(text) => text,
        }
    }

    /// Where the text goes, as the footer says it.
    pub fn label(&self) -> String {
        match self {
            Compose::To(id, _) => format!("message to {id}"),
            Compose::New(_) => "a new chat".to_string(),
        }
    }
}

/// The tab's state: the chats' views as the hub last sent them, and the reads under way.
#[derive(Debug, Default)]
pub struct ChatTab {
    /// Each chat's view (its last messages), by id.
    pub views: HashMap<String, Value>,
    /// The reads asked for and not answered yet, and when they were asked.
    reading: HashMap<String, Instant>,
}

impl ChatTab {
    /// Takes a chat's view (a read's). An older one than the view held is skipped.
    pub fn on_view(&mut self, view: Value) {
        let Some(id) = view.get("id").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        self.reading.remove(&id);
        if self
            .views
            .get(&id)
            .is_some_and(|held| updated(held) > updated(&view))
        {
            return;
        }
        self.views.insert(id, view);
    }

    /// The chat to read for a row of the list, when one is due: its view is not here, or the list says it changed since.
    /// A read under way (or one that failed, for a few seconds) is not asked again.
    pub fn wants(&self, summary: &Value) -> Option<String> {
        let id = summary.get("id")?.as_str()?;
        if self
            .reading
            .get(id)
            .is_some_and(|since| since.elapsed() < RETRY)
        {
            return None;
        }
        let due = self
            .views
            .get(id)
            .is_none_or(|view| updated(summary) > updated(view));
        due.then(|| id.to_string())
    }

    /// Notes that chat `id` is being read.
    pub fn reading(&mut self, id: &str) {
        self.reading.insert(id.to_string(), Instant::now());
    }
}

fn updated(v: &Value) -> u64 {
    v.get("updated_ms").and_then(Value::as_u64).unwrap_or(0)
}

/// A chat of the list as cells: its id, its kind, the project or task it is about (`-` for the orchestrator's), and
/// its title.
pub fn summary_cells(c: &Value) -> [String; 4] {
    let about = match (c.get("project"), c.get("task")) {
        (_, Some(task)) if !task.is_null() => show(Some(task)),
        (Some(project), _) if !project.is_null() => show(Some(project)),
        _ => "-".to_string(),
    };
    [
        show(c.get("id")),
        show(c.get("kind")),
        about,
        show(c.get("title")),
    ]
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let (list_area, talk_area) = if area.width >= SIDE_BY_SIDE {
        let [list, talk] =
            Layout::horizontal([Constraint::Length(LIST_WIDTH), Constraint::Min(0)]).areas(area);
        (list, talk)
    } else {
        let rows = (app.array(PATH).len() as u16).saturating_add(3).clamp(4, 9);
        let [list, talk] =
            Layout::vertical([Constraint::Length(rows), Constraint::Min(0)]).areas(area);
        (list, talk)
    };
    list(frame, list_area, app);
    chat(frame, talk_area, app);
}

/// The id of the chat the tab selects.
pub fn selected(app: &App) -> Option<String> {
    let summary = app.array(PATH).get(app.selected)?;
    Some(summary.get("id")?.as_str()?.to_string())
}

fn list(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let all = app.array(PATH);
    let block = style::panel(&format!("chats · {}", all.len()), &palette, false);
    if all.is_empty() {
        let empty = Line::from(Span::styled(
            "none yet: n opens one",
            Style::new().fg(palette.muted),
        ));
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let head = Row::new(["ID", "KIND", "ABOUT", "TITLE"].map(String::from))
        .style(Style::new().fg(palette.muted).add_modifier(Modifier::BOLD));
    let body = all.iter().enumerate().map(|(i, c)| {
        let row = Row::new(summary_cells(c));
        if i == app.selected {
            row.style(Style::new().fg(palette.paper).bg(palette.accent))
        } else {
            row
        }
    });
    let widths = [
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Fill(1),
    ];
    frame.render_widget(Table::new(body, widths).header(head).block(block), area);
}

fn chat(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let muted = Style::new().fg(palette.muted);
    let Some(summary) = app.array(PATH).get(app.selected) else {
        let hint = Line::from(Span::styled(
            "n opens a chat with a project's planner",
            muted,
        ));
        frame.render_widget(
            Paragraph::new(hint).block(style::panel("chat", &palette, false)),
            area,
        );
        return;
    };
    let id = show(summary.get("id"));
    let title = format!(
        " {id} · {} · t message · n new ",
        clip(&show(summary.get("title")), 40)
    );
    let width = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let lines = match app.chat.views.get(&id) {
        Some(view) => talk_lines(view, width, height, palette, &apz_rich::Ctx::default()),
        None => vec![Line::from(Span::styled("reading…", muted))],
    };
    frame.render_widget(
        Paragraph::new(lines).block(style::panel(&title, &palette, true)),
        area,
    );
}

/// The line over a message: `›` for you, `●` for a session, `·` for the hub; who wrote it, when, and the kind of a
/// message that is a question or a notice.
fn head_line(m: &Value, palette: Palette) -> Line<'static> {
    let at = m
        .get("at_ms")
        .and_then(Value::as_i64)
        .map(local_time)
        .unwrap_or_default();
    let kind = match show(m.get("kind")).as_str() {
        "message" | "-" => String::new(),
        kind => format!(" · {kind}"),
    };
    match show(m.get("author")).as_str() {
        "owner" => Line::from(vec![
            Span::styled(
                "› ",
                Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "you",
                Style::new().fg(palette.ink()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {at}{kind}"), Style::new().fg(palette.muted)),
        ]),
        "session" => {
            let seat = match show(m.get("seat")).as_str() {
                "-" => String::new(),
                seat => format!(" ({seat})"),
            };
            Line::from(vec![
                Span::styled("● ", Style::new().fg(palette.ok)),
                Span::styled(
                    format!("session {}{seat}", show(m.get("session"))),
                    Style::new().fg(palette.ink2()).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" · {at}{kind}"), Style::new().fg(palette.muted)),
            ])
        }
        _ => Line::from(Span::styled(
            format!("· hub · {at}{kind}"),
            Style::new().fg(palette.muted),
        )),
    }
}

/// Lines indented under a message's head, so the text lines up after the glyph.
fn indent(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            if line.spans.is_empty() {
                line
            } else {
                with_gutter(line, Span::raw("  "))
            }
        })
        .collect()
}

/// The newest `height` lines of the chat, wrapped to `width`: each message's head, its text and a blank line, oldest
/// first. A card in a message names it on a line of its own, for the Inbox to answer.
fn talk_lines(
    view: &Value,
    width: usize,
    height: usize,
    palette: Palette,
    rctx: &apz_rich::Ctx,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2).max(1);
    let messages = view
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    let mut count = 0;
    for m in messages.iter().rev() {
        if count >= height {
            break;
        }
        let text = show(m.get("text"));
        let mut block = vec![head_line(m, palette)];
        let body = if show(m.get("author")) == "owner" {
            text_lines(&text, inner, Style::new().fg(palette.ink()))
        } else {
            prose_lines(&text, rctx, palette.accent, inner)
        };
        block.extend(indent(body));
        if let Some(card) = m.get("card").and_then(Value::as_str) {
            block.push(Line::from(Span::styled(
                format!("  ▸ card {card} waits for you: answer it in the Inbox (i)"),
                Style::new().fg(palette.warn()),
            )));
        }
        block.push(Line::raw(""));
        count += block.len();
        blocks.push(block);
    }
    let lines: Vec<Line<'static>> = blocks.into_iter().rev().flatten().collect();
    let skip = lines.len().saturating_sub(height);
    lines.into_iter().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;
    use serde_json::json;

    use super::*;

    fn palette() -> Palette {
        Palette {
            accent: Color::Red,
            paper: Color::White,
            ok: Color::Green,
            muted: Color::DarkGray,
        }
    }

    fn text_of(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn view(updated_ms: u64) -> Value {
        json!({ "id": "chat-pr3", "kind": "project", "project": "pr3", "title": "Release 0.9", "updated_ms": updated_ms,
            "messages": [
                { "author": "owner", "kind": "message", "text": "Fix the flaky tests", "at_ms": 0 },
                { "author": "session", "session": "s12", "seat": "planner", "kind": "question", "text": "Which repo?", "at_ms": 60_000, "card": "c4" }
            ] })
    }

    #[test]
    fn a_chat_reads_oldest_first_and_names_a_card_waiting_for_the_owner() {
        let lines = talk_lines(&view(10), 60, 20, palette(), &apz_rich::Ctx::default());
        assert_eq!(
            text_of(&lines),
            [
                "› you · 1970-01-01 00:00",
                "  Fix the flaky tests",
                "",
                "● session s12 (planner) · 1970-01-01 00:01 · question",
                "  Which repo?",
                "  ▸ card c4 waits for you: answer it in the Inbox (i)",
                "",
            ]
        );
    }

    #[test]
    fn only_the_newest_lines_that_fit_are_drawn() {
        let lines = talk_lines(&view(10), 60, 3, palette(), &apz_rich::Ctx::default());
        assert_eq!(
            text_of(&lines),
            [
                "  Which repo?",
                "  ▸ card c4 waits for you: answer it in the Inbox (i)",
                "",
            ]
        );
    }

    #[test]
    fn an_older_view_does_not_replace_a_newer_one() {
        let mut tab = ChatTab::default();
        tab.on_view(view(20));
        tab.on_view(view(10));
        assert_eq!(tab.views["chat-pr3"]["updated_ms"], 20);
        tab.on_view(view(30));
        assert_eq!(tab.views["chat-pr3"]["updated_ms"], 30);
    }

    #[test]
    fn a_chat_is_read_when_it_is_new_or_the_list_says_it_changed() {
        let mut tab = ChatTab::default();
        let summary = json!({ "id": "chat-pr3", "updated_ms": 10 });
        assert_eq!(tab.wants(&summary).as_deref(), Some("chat-pr3"));
        tab.reading("chat-pr3");
        assert_eq!(tab.wants(&summary), None, "a read is under way");
        tab.on_view(view(10));
        assert_eq!(tab.wants(&summary), None, "the view is current");
        let newer = json!({ "id": "chat-pr3", "updated_ms": 20 });
        assert_eq!(tab.wants(&newer).as_deref(), Some("chat-pr3"));
    }

    #[test]
    fn the_list_shows_each_chats_kind_and_what_it_is_about() {
        let main = json!({ "id": "main", "kind": "main", "project": null, "task": null, "title": "Orchestrator" });
        assert_eq!(summary_cells(&main), ["main", "main", "-", "Orchestrator"]);
        let task = json!({ "id": "chat-t4", "kind": "task", "project": "pr3", "task": "t4", "title": "Fix it" });
        assert_eq!(summary_cells(&task), ["chat-t4", "task", "t4", "Fix it"]);
    }

    #[test]
    fn the_composer_says_where_its_text_goes() {
        let mut to = Compose::To("chat-pr3".into(), String::new());
        to.text_mut().push_str("hi");
        assert_eq!(
            (to.text(), to.label().as_str()),
            ("hi", "message to chat-pr3")
        );
        assert_eq!(Compose::New(String::new()).label(), "a new chat");
    }
}
