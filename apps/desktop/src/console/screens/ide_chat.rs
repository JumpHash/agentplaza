//! The IDE's centre for the selected agent: its header (status dot, name, and the Chat | Terminal | Editor |
//! Diff tabs), then the view the tab picks. Chat is the transcript as a reading column: text as prose, owner
//! messages as right-aligned bubbles, board nudges as system bubbles, a run of tool calls as one "N steps · tools"
//! line that opens, a working line while the agent works, and the composer that steers the agent.

use std::collections::BTreeSet;

use apz_ide::EntryKind;
use iced::widget::{Space, column, container, row, text_editor};
use iced::{Alignment, Element, Length, Padding};
use serde_json::Value;

use super::ide::{self, Centre, Msg, State};
use super::ide_model::{self, Row};
use super::ide_panels;
use super::ide_sidebar;
use crate::console::rich;
use crate::console::ui::{self, Kind};
use crate::theme;

/// The reading column's width, and the composer's height.
const COLUMN_MAX: f32 = 760.0;
const COMPOSER_H: f32 = 88.0;

/// The chat's own messages: a run of tool steps opens or folds (by its first entry), and the agent's work stops.
#[derive(Debug, Clone)]
pub enum ChatMsg {
    Steps(usize),
    Cancel,
    Cancelled(Result<(), String>),
}

/// Agent text, drawn with its rich content (chips, cards, images: `console::rich`). Every place that shows agent prose
/// calls this one function.
pub fn prose<'a>(source: &str, rctx: &rich::RichCtx, cards: &rich::Cards) -> Element<'a, Msg> {
    rich::view(source, rctx, cards, &on_rich)
}

/// A rich-content click or load, as the IDE's message.
fn on_rich(action: rich::RichAction) -> Msg {
    Msg::Rich(rich::Ev::Action(action))
}

/// The rich context of the IDE's chat: the workspace's files and folders (a folder with its `/`), the project the
/// workspace belongs to, and its repository.
pub fn rich_ctx(st: &State) -> rich::RichCtx {
    let paths: BTreeSet<String> = st
        .tree
        .entries
        .iter()
        .map(|e| if e.kind == EntryKind::Dir { format!("{}/", e.path) } else { e.path.clone() })
        .collect();
    let project = ide::selected_project(st);
    rich::context(&st.cards, project.map(|p| p.id.as_str()), project.and_then(|p| p.repo.as_deref()), Some(&paths))
}

/// The texts of the transcript that rich content reads: the prompts and the text blocks of the replies.
pub fn transcript_texts(entries: &[Value]) -> Vec<&str> {
    let mut texts = Vec::new();
    for entry in entries {
        let data = entry.get("data").unwrap_or(&Value::Null);
        match entry.get("kind").and_then(Value::as_str).unwrap_or("") {
            "work_start" => texts.push(s(data, "prompt")),
            "model_response" => {
                for block in data.get("content").and_then(Value::as_array).into_iter().flatten() {
                    if s(block, "type") == "text" {
                        texts.push(s(block, "text"));
                    }
                }
            }
            _ => {}
        }
    }
    texts.retain(|t| !t.is_empty());
    texts
}

/// The header of the centre: the agent's dot and name, and the four views.
pub fn header(st: &State) -> Element<'_, Msg> {
    let p = theme::palette();
    let (dot, name) = match ide::selected_agent(st) {
        Some(agent) => (
            Some(ide_model::dot(&agent.status, None)),
            agent.name.clone(),
        ),
        None => (None, ide::crumbs(st).1),
    };
    let mut head = row![].spacing(10).align_y(Alignment::Center);
    if let Some(dot) = dot {
        head = head.push(ide_sidebar::dot_mark(dot));
    }
    head = head.push(ui::mono(name, 14.0, p.ink));
    head = head.push(Space::new().width(Length::Fill));
    head = head.push(ui::tabs(
        &[
            (Centre::Chat, "Chat"),
            (Centre::Terminal, "Terminal"),
            (Centre::Editor, "Editor"),
            (Centre::Diff, "Diff"),
        ],
        st.centre,
        Msg::Centre,
    ));
    head.into()
}

/// The owner's message: a bubble on the right.
fn owner_bubble<'a>(body: &str) -> Element<'a, Msg> {
    let p = theme::palette();
    let bubble = container(ui::prose(body.to_string()))
        .padding([8, 12])
        .max_width(520.0)
        .style(move |_| container::Style {
            background: Some(p.hover.into()),
            border: iced::Border {
                color: p.rule,
                width: 1.0,
                radius: 12.0.into(),
            },
            ..Default::default()
        });
    row![Space::new().width(Length::Fill), bubble].into()
}

/// A board nudge or a bookkeeping line: a system bubble, centred, with its text faint.
fn system_bubble<'a>(body: &str) -> Element<'a, Msg> {
    let p = theme::palette();
    container(ui::faint(body.to_string()))
        .padding([6, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            border: iced::Border {
                color: p.rule,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One transcript entry as the chat draws it. Entries that are only bookkeeping draw nothing.
fn entry_view<'a>(entry: &'a Value, rctx: &rich::RichCtx, cards: &rich::Cards) -> Option<Element<'a, Msg>> {
    let data = entry.get("data").unwrap_or(&Value::Null);
    let p = theme::palette();
    Some(match entry.get("kind").and_then(Value::as_str)? {
        "work_start" => {
            let prompt = s(data, "prompt");
            if prompt.is_empty() {
                return None;
            }
            if s(data, "kind") == "chat" || s(data, "origin") == "owner" {
                owner_bubble(prompt)
            } else if s(data, "kind") == "message" || s(data, "origin") == "board" {
                system_bubble(&format!("Board: {}", ui::ellipsize(prompt, 240)))
            } else {
                system_bubble(&format!("Work started: {}", ui::ellipsize(prompt, 240)))
            }
        }
        "model_response" => {
            let mut col = column![].spacing(8);
            for block in data
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match s(block, "type") {
                    "text" => col = col.push(prose(s(block, "text"), rctx, cards)),
                    "tool_use" => {
                        col = col.push(ui::mono(format!("→ {}", s(block, "name")), 11.5, p.ink_2))
                    }
                    _ => {}
                }
            }
            col.into()
        }
        "control" => {
            let note = data
                .get("control")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str);
            owner_bubble(note.unwrap_or("Note sent to the agent."))
        }
        "error" => ui::error_line(s(data, "message")),
        "work_end" => {
            let status = s(data, "status");
            let summary = s(data, "summary");
            let line = if summary.is_empty() {
                format!("Work {status}")
            } else {
                format!("Work {status}: {}", ui::ellipsize(summary, 200))
            };
            system_bubble(&line)
        }
        _ => return None,
    })
}

/// A run of tool steps: one line, which opens to show each call and its decision.
fn steps_view(entries: &[Value], first: usize, summary: &str, open: bool) -> Element<'static, Msg> {
    let p = theme::palette();
    let mut col = column![ui::disclosure(
        summary.to_string(),
        p.ink_2,
        open,
        Msg::Chat(ChatMsg::Steps(first))
    )]
    .spacing(6);
    if open {
        let mut lines = column![].spacing(3);
        for entry in entries[first..]
            .iter()
            .take_while(|e| ide_model::is_step(e))
        {
            let data = entry.get("data").unwrap_or(&Value::Null);
            let line: Option<Element<'static, Msg>> =
                match entry.get("kind").and_then(Value::as_str).unwrap_or("") {
                    "tool_request" => {
                        Some(ui::mono(format!("→ {}", s(data, "name")), 11.5, p.ink).into())
                    }
                    "tool_decision" => Some(
                        ui::mono(
                            format!("  decision: {}", s(data, "decision")),
                            11.5,
                            p.ink_3,
                        )
                        .into(),
                    ),
                    "tool_result" => Some(
                        ui::mono(
                            format!("  result: {}", ui::ellipsize(s(data, "output"), 160)),
                            11.5,
                            p.ink_3,
                        )
                        .into(),
                    ),
                    _ => None,
                };
            if let Some(line) = line {
                lines = lines.push(line);
            }
        }
        col = col.push(container(lines).padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 14.0,
        }));
    }
    col.into()
}

/// The transcript as rows, then the working line and any error.
fn transcript(st: &State) -> Element<'_, Msg> {
    let entries = &st.panels.entries;
    let rctx = rich_ctx(st);
    let mut col = column![].spacing(14);
    if let Some(error) = &st.panels.entries_error {
        col = col.push(ui::error_line(error));
    } else if !st.panels.entries_loaded {
        col = col.push(ui::faint("Loading the conversation…"));
    } else if entries.is_empty() {
        col = col.push(ui::faint("No messages yet. Say something below."));
    }
    for item in ide_model::rows(entries) {
        match item {
            Row::Entry(i) => {
                if let Some(view) = entry_view(&entries[i], &rctx, &st.cards) {
                    col = col.push(view);
                }
            }
            Row::Steps { first, summary, .. } => {
                col = col.push(steps_view(
                    entries,
                    first,
                    &summary,
                    st.steps_open.contains(&first),
                ));
            }
        }
    }
    if ide::selected_agent(st).is_some_and(|a| a.status == "working") {
        col = col.push(
            row![
                ide_sidebar::dot_mark(ide_model::Dot::Working),
                ui::faint("Working…")
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    col.into()
}

/// The composer: who the note goes to, the note, and Send (or Stop while the agent works).
fn composer(st: &State) -> Element<'_, Msg> {
    let agent = ide::selected_agent(st);
    let who = agent.map_or_else(|| "the agent".to_string(), |a| a.name.clone());
    let on = ide::selected_project(st)
        .map(|p| format!(" on {}", p.title))
        .unwrap_or_default();
    let editor = text_editor(&st.panels.steer)
        .placeholder("Message the agent. Enter sends, Shift+Enter starts a new line.")
        .height(Length::Fixed(COMPOSER_H))
        .on_action(|action| Msg::Panels(ide_panels::Msg::Steer(action)))
        .key_binding(|press| {
            if press.key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
                && !press.modifiers.shift()
            {
                Some(text_editor::Binding::Custom(Msg::Panels(
                    ide_panels::Msg::SteerSend,
                )))
            } else {
                text_editor::Binding::from_key_press(press)
            }
        });
    let len = st.panels.steer.text().trim().chars().count();
    let can_send =
        !st.panels.steer_busy && len > 0 && len <= ide_panels::STEER_MAX && agent.is_some();
    let working = agent.is_some_and(|a| a.status == "working");
    let stop = working && !st.cancelling;
    let foot = row![
        ui::mono(
            format!("Sending to: {who}{on}"),
            11.5,
            theme::palette().ink_3
        ),
        Space::new().width(Length::Fill),
        ui::btn(
            "Stop",
            Some("stop"),
            Kind::Danger,
            stop.then_some(Msg::Chat(ChatMsg::Cancel))
        ),
        ui::btn(
            "Send",
            Some("send"),
            Kind::Primary,
            can_send.then_some(Msg::Panels(ide_panels::Msg::SteerSend))
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut col = column![editor, foot].spacing(8);
    if let Some(notice) = &st.panels.steer_notice {
        col = col.push(ui::faint(notice.clone()));
    }
    col.into()
}

/// The chat: the transcript in a reading column that scrolls, and the composer under it.
pub fn chat(st: &State) -> Element<'_, Msg> {
    let reading = container(transcript(st))
        .max_width(COLUMN_MAX)
        .width(Length::Fill);
    let body = container(reading)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([12, 16]);
    let base = container(composer(st))
        .max_width(COLUMN_MAX)
        .width(Length::Fill);
    column![
        ui::scroll(body).height(Length::Fill),
        container(base)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding([8, 16]),
    ]
    .spacing(0)
    .height(Length::Fill)
    .into()
}

/// The centre's body for the tab on screen.
pub fn body(st: &State) -> Element<'_, Msg> {
    let agent = matches!(st.ide.workspace, Some(apz_ide::WorkspaceId::Agent(_)));
    match st.centre {
        Centre::Chat if agent => chat(st),
        Centre::Chat => ui::empty(
            "agents",
            "Repo workspaces have no chat",
            "Pick an agent in the sidebar to talk to it.",
        ),
        Centre::Terminal => ide_panels::terminal_body(st.terminals.as_ref(), agent),
        Centre::Editor => super::ide_files::editor_centre(st),
        Centre::Diff => super::ide_files::diff_centre(st),
    }
}

/// The whole centre column: the header, then the body.
pub fn centre(st: &State) -> Element<'_, Msg> {
    let head = container(header(st)).padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 8.0,
        left: 0.0,
    });
    column![
        head,
        container(body(st)).width(Length::Fill).height(Length::Fill)
    ]
    .spacing(0)
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tool_step_run_opens_to_its_calls_only_while_its_first_entry_is_open() {
        let entries = vec![
            json!({"kind": "model_response", "data": {"content": [{"type": "tool_use", "name": "bash"}]}}),
            json!({"kind": "tool_request", "data": {"call": "c", "name": "bash"}}),
            json!({"kind": "tool_decision", "data": {"call": "c", "decision": "allow"}}),
        ];
        // Closed: the one line is all. Open: the call and its decision are drawn after it.
        let _closed = steps_view(&entries, 0, "1 step · bash", false);
        let _open = steps_view(&entries, 0, "1 step · bash", true);
        assert!(ide_model::is_step(&entries[2]));
    }

    #[test]
    fn an_owner_note_and_a_board_nudge_are_bubbles_and_bookkeeping_draws_nothing() {
        let note =
            json!({"kind": "work_start", "data": {"kind": "chat", "prompt": "Check the tests"}});
        let nudge = json!({"kind": "work_start", "data": {"kind": "message", "origin": "board", "prompt": "New post"}});
        let book = json!({"kind": "work_start", "data": {}});
        let (rctx, cards) = (rich::context(&rich::Cards::default(), None, None, None), rich::Cards::default());
        assert!(entry_view(&note, &rctx, &cards).is_some());
        assert!(entry_view(&nudge, &rctx, &cards).is_some());
        assert!(
            entry_view(&book, &rctx, &cards).is_none(),
            "a start with no prompt says nothing"
        );
        assert!(entry_view(&json!({"kind": "model_request", "data": {}}), &rctx, &cards).is_none());
    }
}
