//! The Chat tab: your conversations with the orchestrator, where a project is talked into being (`agentplaza chat`).
//! The list (`GET /v1/owner/conversations`, newest first) is on the left; the selected conversation on the right: its
//! messages, newest at the bottom, with an answer streaming as the orchestrator writes it, and under them its project
//! draft as `agentplaza chat` prints it. `t` writes to the selected conversation and `n` opens a new one (the footer
//! takes the text; Enter sends it); `S` starts the draft at the revision on screen, after a confirm. The conversations'
//! views come with the hub's `owner_conversation` events; one not seen yet is read when it is selected. Declared from
//! `app.rs`, which holds the tab's state and runs its requests.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use serde_json::Value;

use super::context_bar::{self, Owner};
use super::{App, Palette, style};
use crate::cmd::chat::{delta_text, draft_lines, speaker, startable, unanswered};
use crate::tui::screens::{clip, local_time, show};
use crate::tui::workspace_view::{prose_lines, text_lines, thinking, with_cursor, with_gutter};

/// The hub's list of your conversations, which the tab polls while it shows.
pub const PATH: &str = "/v1/owner/conversations";
/// The most bytes of an answer's streamed text the tab keeps.
const LIVE_MAX: usize = 20_000;
/// How long a read of a conversation waits for its answer before it may be asked again.
const RETRY: Duration = Duration::from_secs(5);
/// The width of the conversations list beside the conversation.
const LIST_WIDTH: u16 = 42;
/// Below this many columns the list sits above the conversation.
const SIDE_BY_SIDE: u16 = 100;

/// A message being written in the footer: to a conversation (by its id), or the first message of a new one.
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
            Compose::New(_) => "a new conversation".to_string(),
        }
    }
}

/// The tab's state: the conversations' views as the hub last sent them, the reads under way, and the answers being
/// written.
#[derive(Debug, Default)]
pub struct ChatTab {
    /// Each conversation's view, by id.
    pub views: HashMap<String, Value>,
    /// The orchestrator each conversation talks to, and the work items it waits on (`unanswered`), by id.
    waiting: HashMap<String, (u64, Vec<u64>)>,
    /// The reads asked for and not answered yet, and when they were asked.
    reading: HashMap<String, Instant>,
    /// The text streamed so far of each answer being written, by work item: the model call it is from, and the text.
    pub live: HashMap<u64, (String, String)>,
}

impl ChatTab {
    /// Takes a conversation's view (an event's, or a read's). An older one than the view held is skipped; the streamed
    /// text of the answers it holds is dropped.
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
        for work in answered(&view) {
            self.live.remove(&work);
        }
        let cid = view.get("cid").and_then(Value::as_u64).unwrap_or(0);
        self.waiting.insert(id.clone(), (cid, unanswered(&view)));
        self.views.insert(id, view);
    }

    /// Streams an answer: the text of a `delta` of a work item a conversation waits on. A new model call of the same
    /// work starts a new paragraph.
    pub fn on_delta(&mut self, data: &Value) {
        let Some(text) = delta_text(data) else {
            return;
        };
        let (Some(cid), Some(work)) = (
            data.get("cid").and_then(Value::as_u64),
            data.get("work").and_then(Value::as_u64),
        ) else {
            return;
        };
        let awaited = self
            .waiting
            .values()
            .any(|(orchestrator, works)| *orchestrator == cid && works.contains(&work));
        if !awaited {
            return;
        }
        let req = data.get("req").and_then(Value::as_str).unwrap_or("");
        let (call, streamed) = self.live.entry(work).or_default();
        if call != req {
            if !streamed.is_empty() {
                streamed.push_str("\n\n");
            }
            *call = req.to_string();
        }
        if streamed.len() < LIVE_MAX {
            streamed.push_str(text);
        }
    }

    /// The conversation to read for a row of the list, when one is due: its view is not here, or the list says it
    /// changed since. A read under way (or one that failed, for a few seconds) is not asked again.
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

    /// Notes that conversation `id` is being read.
    pub fn reading(&mut self, id: &str) {
        self.reading.insert(id.to_string(), Instant::now());
    }
}

fn updated(v: &Value) -> u64 {
    v.get("updated_ms").and_then(Value::as_u64).unwrap_or(0)
}

/// The work items a conversation's answers end.
fn answered(view: &Value) -> Vec<u64> {
    view.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("from").and_then(Value::as_str) == Some("orchestrator"))
        .filter_map(|m| m.get("work").and_then(Value::as_u64))
        .collect()
}

/// A conversation of the list as cells: its id; where it stands (its project once it started one, else its draft's
/// revision); `…` while an answer is awaited; and its title.
pub fn summary_cells(c: &Value) -> [String; 4] {
    let project = c.get("project").and_then(Value::as_str);
    let revision = c.pointer("/draft/revision").and_then(Value::as_u64);
    let state = match (project, revision) {
        (Some(project), _) => project.to_string(),
        (None, Some(revision)) => format!("r{revision}"),
        (None, None) => "-".to_string(),
    };
    let waiting = c.get("pending").and_then(Value::as_u64).unwrap_or(0) > 0;
    [
        show(c.get("id")),
        state,
        if waiting { "…" } else { "" }.to_string(),
        show(c.get("title")),
    ]
}

/// The draft pane's title: the draft's revision, and what can be done with it.
pub fn draft_title(draft: &Value, project: Option<&str>) -> String {
    let revision = show(draft.get("revision"));
    if draft.get("started_ms").is_some_and(|ms| !ms.is_null()) {
        let project = project.map(|p| format!(" as {p}")).unwrap_or_default();
        format!(" draft r{revision} · started{project} ")
    } else if startable(draft) {
        format!(" draft r{revision} · S starts it ")
    } else {
        let problems = draft
            .get("problems")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let what = if problems == 1 { "thing" } else { "things" };
        format!(" draft r{revision} · {problems} {what} to settle before Start ")
    }
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
    conversation(frame, talk_area, app);
    if let Some(picker) = &app.ws.picker {
        context_bar::overlay(frame, area, picker, app.palette);
    }
}

/// The conversation the tab selects, as the context bar reads it: its owner, and its model as the list says it.
pub fn selected(app: &App) -> Option<(Owner, Option<String>)> {
    let summary = app.array(PATH).get(app.selected)?;
    let id = summary.get("id")?.as_str()?;
    Some((
        Owner::Conversation(id.to_string()),
        context_bar::model_of(summary.get("model")),
    ))
}

fn list(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let all = app.array(PATH);
    let block = style::panel(&format!("conversations · {}", all.len()), &palette, false);
    if all.is_empty() {
        let empty = Line::from(Span::styled(
            "none yet: n opens one",
            Style::new().fg(palette.muted),
        ));
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let head = Row::new(["ID", "STATE", "", "TITLE"].map(String::from))
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
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(1),
        Constraint::Fill(1),
    ];
    frame.render_widget(Table::new(body, widths).header(head).block(block), area);
}

fn conversation(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let muted = Style::new().fg(palette.muted);
    let Some(summary) = app.array(PATH).get(app.selected) else {
        let hint = Line::from(Span::styled(
            "n opens a conversation with the orchestrator",
            muted,
        ));
        frame.render_widget(
            Paragraph::new(hint).block(style::panel("conversation", &palette, false)),
            area,
        );
        return;
    };
    let id = show(summary.get("id"));
    let view = app.chat.views.get(&id);
    let draft = view.and_then(|v| v.get("draft")).filter(|d| d.is_object());
    let project = view.and_then(|v| v.get("project")).and_then(Value::as_str);
    let drafted = draft.map(|d| draft_lines(d, project)).unwrap_or_default();
    let (talk, below) = if drafted.is_empty() {
        (area, None)
    } else {
        let most = (area.height * 3 / 5).max(3);
        let height = (drafted.len() as u16 + 2).min(most);
        let [talk, below] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(height)]).areas(area);
        (talk, Some(below))
    };
    // The context bar sits under the transcript, above the draft and the footer's input.
    let [talk, bar] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(talk);
    let title = format!(
        " {id} · {} · t message · n new ",
        clip(&show(summary.get("title")), 40)
    );
    let width = usize::from(talk.width.saturating_sub(2));
    let height = usize::from(talk.height.saturating_sub(2));
    let lines = match view {
        Some(view) => talk_lines(
            view,
            &app.chat.live,
            width,
            height,
            palette,
            &apz_rich::Ctx::default(),
            app.clock,
        ),
        None => vec![Line::from(Span::styled("reading…", muted))],
    };
    frame.render_widget(
        Paragraph::new(lines).block(style::panel(&title, &palette, true)),
        talk,
    );
    let owner = Owner::Conversation(id);
    let context = app.ws.contexts.get(&owner);
    frame.render_widget(
        Paragraph::new(context_bar::bar_line(
            context, bar.width, palette, app.clock,
        )),
        bar,
    );
    if app.ws.breakdown {
        context_bar::panel(frame, talk, context, palette);
    }
    if let (Some(below), Some(draft)) = (below, draft) {
        let lines: Vec<Line> = drafted.into_iter().map(Line::from).collect();
        let block = style::panel(&draft_title(draft, project), &palette, false);
        frame.render_widget(Paragraph::new(lines).block(block), below);
    }
}

/// The line over a message: its role's glyph (`›` for you, `●` for the orchestrator and the other agents), who wrote it,
/// when, and how an answer's work ended when it did not end done.
fn head_line(m: &Value, palette: Palette) -> Line<'static> {
    let from = m.get("from").and_then(Value::as_str).unwrap_or("");
    let who = speaker(from).to_string();
    let at = m
        .get("at_ms")
        .and_then(Value::as_i64)
        .map(local_time)
        .unwrap_or_default();
    let status = match m.get("status").and_then(Value::as_str) {
        None | Some("done") => String::new(),
        Some(status) => format!(" · {status}"),
    };
    let (glyph, name) = if from == "owner" {
        (
            Span::styled(
                "› ",
                Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
            ),
            Style::new().fg(palette.ink()).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Span::styled("● ", Style::new().fg(palette.ok)),
            Style::new().fg(palette.ink2()).add_modifier(Modifier::BOLD),
        )
    };
    Line::from(vec![
        glyph,
        Span::styled(who, name),
        Span::styled(format!(" · {at}{status}"), Style::new().fg(palette.muted)),
    ])
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

/// The newest `height` lines of the conversation, wrapped to `width`: each message's head, its text and a blank line,
/// oldest first; then the answers being written, as far as they have streamed (the cursor after their words), or a
/// `Thinking…` line while the orchestrator has written nothing yet. Only the messages that reach the screen are wrapped.
fn talk_lines(
    view: &Value,
    live: &HashMap<u64, (String, String)>,
    width: usize,
    height: usize,
    palette: Palette,
    rctx: &apz_rich::Ctx,
    clock: f32,
) -> Vec<Line<'static>> {
    let muted = Style::new().fg(palette.muted);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let inner = width.saturating_sub(2).max(1);
    // The blocks, newest first, until the screen is full.
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    let waiting = unanswered(view);
    let mut streamed = Vec::new();
    for work in &waiting {
        if let Some((_, text)) = live.get(work).filter(|(_, text)| !text.is_empty()) {
            streamed.push(Line::from(vec![
                Span::styled("● ", Style::new().fg(palette.ok)),
                Span::styled("orchestrator", bold.fg(palette.ink2())),
                Span::styled(" · writing…", muted),
            ]));
            let mut body = indent(prose_lines(text, rctx, palette.accent, inner));
            with_cursor(&mut body, clock, palette.accent);
            streamed.extend(body);
        }
    }
    if streamed.is_empty() && !waiting.is_empty() {
        streamed.push(thinking(clock, palette));
    }
    let mut count = streamed.len();
    blocks.push(streamed);
    let messages = view
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for m in messages.iter().rev() {
        if count >= height {
            break;
        }
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        let mut block = vec![head_line(m, palette)];
        let body = if m.get("from").and_then(Value::as_str) == Some("owner") {
            text_lines(text, inner, Style::new().fg(palette.ink()))
        } else {
            prose_lines(text, rctx, palette.accent, inner)
        };
        block.extend(indent(body));
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

    fn view(updated_ms: u64, answered: bool) -> Value {
        let mut messages =
            vec![json!({ "from": "owner", "text": "Fix the flaky tests", "at_ms": 0, "work": 41 })];
        if answered {
            messages.push(json!({ "from": "orchestrator", "text": "Which repo?", "at_ms": 60_000, "work": 41, "status": "done" }));
        }
        json!({ "id": "conv-3", "cid": 3, "title": "Release 0.9", "updated_ms": updated_ms, "messages": messages })
    }

    #[test]
    fn an_answer_streams_until_it_is_in_and_another_agents_text_is_left_out() {
        let mut tab = ChatTab::default();
        tab.on_view(view(10, false));
        let delta = |cid: u64, req: &str, text: &str| json!({ "cid": cid, "work": 41, "req": req, "delta": { "type": "text", "text": text } });
        tab.on_delta(&delta(3, "r1", "Which "));
        tab.on_delta(&delta(3, "r1", "repo?"));
        tab.on_delta(&delta(9, "r1", "not mine"));
        assert_eq!(tab.live[&41].1, "Which repo?");
        tab.on_delta(&delta(3, "r2", "Thanks."));
        assert_eq!(tab.live[&41].1, "Which repo?\n\nThanks.");
        let lines = talk_lines(
            &tab.views["conv-3"],
            &tab.live,
            40,
            20,
            palette(),
            &apz_rich::Ctx::default(),
            0.75,
        );
        assert_eq!(
            text_of(&lines),
            [
                "› you · 1970-01-01 00:00",
                "  Fix the flaky tests",
                "",
                "● orchestrator · writing…",
                "  Which repo?",
                "",
                "  Thanks.",
            ]
        );
        // The answer is in: its streamed text goes, and an older view does not bring it back.
        tab.on_view(view(20, true));
        assert!(tab.live.is_empty());
        tab.on_view(view(10, false));
        assert_eq!(tab.views["conv-3"]["updated_ms"], 20);
        let lines = talk_lines(
            &tab.views["conv-3"],
            &tab.live,
            40,
            3,
            palette(),
            &apz_rich::Ctx::default(),
            0.75,
        );
        assert_eq!(
            text_of(&lines),
            ["● orchestrator · 1970-01-01 00:01", "  Which repo?", ""],
            "only the newest lines that fit"
        );
    }

    #[test]
    fn a_conversation_is_read_when_it_is_new_or_the_list_says_it_changed() {
        let mut tab = ChatTab::default();
        let summary = json!({ "id": "conv-3", "updated_ms": 10 });
        assert_eq!(tab.wants(&summary).as_deref(), Some("conv-3"));
        tab.reading("conv-3");
        assert_eq!(tab.wants(&summary), None, "a read is under way");
        tab.on_view(view(10, false));
        assert_eq!(tab.wants(&summary), None, "the view is current");
        let newer = json!({ "id": "conv-3", "updated_ms": 20 });
        assert_eq!(tab.wants(&newer).as_deref(), Some("conv-3"));
    }

    #[test]
    fn the_list_shows_where_each_conversation_stands() {
        let started = json!({ "id": "conv-3", "title": "Release 0.9", "project": "pr3", "draft": { "revision": 4 }, "pending": 0 });
        assert_eq!(
            summary_cells(&started),
            ["conv-3", "pr3", "", "Release 0.9"]
        );
        let drafting = json!({ "id": "conv-4", "title": "Docs", "project": null, "draft": { "revision": 2 }, "pending": 1 });
        assert_eq!(summary_cells(&drafting), ["conv-4", "r2", "…", "Docs"]);
        let new = json!({ "id": "conv-5", "title": "Hello", "draft": null, "pending": 1 });
        assert_eq!(summary_cells(&new), ["conv-5", "-", "…", "Hello"]);
    }

    #[test]
    fn the_draft_title_says_what_can_be_done_with_it() {
        let ready = json!({ "revision": 3, "problems": [] });
        assert_eq!(draft_title(&ready, None), " draft r3 · S starts it ");
        let blocked = json!({ "revision": 3, "problems": ["the draft has no tasks yet"] });
        assert_eq!(
            draft_title(&blocked, None),
            " draft r3 · 1 thing to settle before Start "
        );
        let started = json!({ "revision": 3, "problems": [], "started_ms": 9 });
        assert_eq!(
            draft_title(&started, Some("pr3")),
            " draft r3 · started as pr3 "
        );
    }

    #[test]
    fn the_composer_says_where_its_text_goes() {
        let mut to = Compose::To("conv-3".into(), String::new());
        to.text_mut().push_str("hi");
        assert_eq!(
            (to.text(), to.label().as_str()),
            ("hi", "message to conv-3")
        );
        assert_eq!(Compose::New(String::new()).label(), "a new conversation");
    }

    #[test]
    fn the_streamed_answer_ends_in_a_blinking_cursor_and_a_silent_turn_says_thinking() {
        let mut tab = ChatTab::default();
        tab.on_view(view(10, false));
        let text = |clock: f32, tab: &ChatTab| {
            text_of(&talk_lines(
                &tab.views["conv-3"],
                &tab.live,
                40,
                20,
                palette(),
                &apz_rich::Ctx::default(),
                clock,
            ))
        };
        let silent = text(0.0, &tab);
        assert!(silent.last().unwrap().ends_with("Thinking…"), "{silent:?}");
        let delta = json!({ "cid": 3, "work": 41, "req": "r1", "delta": { "type": "text", "text": "Which repo?" } });
        tab.on_delta(&delta);
        assert_eq!(
            text(0.0, &tab).last().unwrap(),
            "  Which repo?▌",
            "on in the first half second"
        );
        assert_eq!(
            text(0.75, &tab).last().unwrap(),
            "  Which repo?",
            "off in the second half"
        );
    }
}
