//! The IDE's centre for the selected agent: its header (status dot, label, and the Chat | Terminal | Editor |
//! Diff tabs), then the view the tab picks. Chat is the transcript as a reading column: text as prose, owner
//! messages as right-aligned bubbles, board nudges as system bubbles, a run of tool calls as one "N steps · tools"
//! line that opens, a working line while the agent works, and the composer that steers the agent.

// The IDE's own helpers and state, for the messages this area takes (moved from `ide.rs`).
use super::ide::*;
use super::{ide_chat, ide_chrome};
use crate::console::{Ctx, client};
use apz_ide::state::Panel;
use std::collections::BTreeSet;
use std::sync::LazyLock;

use apz_ide::EntryKind;
use iced::widget::scrollable::Viewport;
use iced::widget::{Space, Stack, button, column, container, operation, row, text_editor};
use iced::{Alignment, Element, Length, Padding, Task, Theme};
use serde_json::{Value, json};

use super::ide::{self, Centre, Msg, State};
use super::ide_model::{self, Row};
use super::ide_panels;
use super::ide_sidebar;
use crate::console::chat_tools;
use crate::console::rich;
use crate::console::ui::{self, Kind};
use crate::theme;

/// The reading column's width, and the composer's height.
const COLUMN_MAX: f32 = 780.0;
const COMPOSER_H: f32 = 64.0;
/// How far from the end the chat still counts as at the bottom, in px (a rounding margin).
const BOTTOM_SLACK: f32 = 4.0;

/// The chat's scroll area, by id: the chat follows the newest entry through it.
pub static CHAT_ID: LazyLock<iced::widget::Id> =
    LazyLock::new(|| iced::widget::Id::new("ide-chat"));

/// The chat's own messages: a run of tool steps opens or folds (by its first entry), the agent's work stops, the
/// scroll area moved (its viewport), and the reader jumped to the newest entry from the "New messages" pill.
#[derive(Debug, Clone)]
pub enum ChatMsg {
    Steps(usize),
    Cancel,
    Cancelled(Result<(), String>),
    Scrolled(Viewport),
    JumpToNewest,
    /// Approve (true) or deny (false) an approval a coding engine waits for; the same call the approvals screen makes.
    Decide(String, bool),
    Decided(Result<(), String>),
}

/// What the chat does when its transcript changes (see `decide`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// Scroll to the newest entry.
    Snap,
    /// Keep the reader's place and count these entries for the "New messages" pill.
    Pill(usize),
    /// Nothing to do: the reader is above the newest entries and none came.
    Keep,
}

/// The chat's scroll state for the agent on screen: whether it has been shown yet, whether the reader is at the
/// newest entry, how many entries the transcript had when it was last looked at, and how many came since while the
/// reader was above them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scroll {
    pub shown: bool,
    pub at_bottom: bool,
    pub seen: usize,
    pub unread: usize,
}

/// The auto-scroll decision: the first view of an agent's chat, and any change while the reader is at the bottom,
/// follow the newest entry; while the reader is above it, new entries are counted for the pill instead.
pub fn decide(shown: bool, at_bottom: bool, seen: usize, now: usize) -> Follow {
    if !shown || at_bottom {
        Follow::Snap
    } else if now > seen {
        Follow::Pill(now - seen)
    } else {
        Follow::Keep
    }
}

/// Whether a viewport is scrolled to its end (within the rounding margin). A content that fits is at its end.
pub fn at_end(viewport: &Viewport) -> bool {
    let overflow = viewport.content_bounds().height - viewport.bounds().height;
    overflow <= 0.0 || viewport.absolute_offset().y >= overflow - BOTTOM_SLACK
}

/// Scrolls the chat to its newest entry.
fn snap() -> Task<Msg> {
    operation::snap_to_end(CHAT_ID.clone())
}

/// The transcript landed: follows it, or counts what came for the pill (see `decide`). The entries are the
/// agent's transcript as the panels hold it.
pub fn transcript_landed(st: &mut State) -> Task<Msg> {
    let now = st.panels.entries.len();
    let scroll = &mut st.chat_scroll;
    match decide(scroll.shown, scroll.at_bottom, scroll.seen, now) {
        Follow::Snap => {
            scroll.shown = true;
            scroll.at_bottom = true;
            scroll.unread = 0;
            scroll.seen = now;
            snap()
        }
        Follow::Pill(count) => {
            scroll.unread += count;
            scroll.seen = now;
            Task::none()
        }
        Follow::Keep => {
            scroll.seen = now;
            Task::none()
        }
    }
}

/// The chat is shown again (its scroll area is new on screen, so it starts at the top): it goes to the newest entry.
pub fn shown_again(st: &mut State) -> Task<Msg> {
    st.chat_scroll.at_bottom = true;
    st.chat_scroll.unread = 0;
    st.chat_scroll.shown = true;
    snap()
}

/// The chat's own messages (see `ChatMsg`): the reader's scroll is kept; the pill's press goes to the newest entry.
pub fn update_scroll(st: &mut State, msg: ChatMsg) -> Option<Task<Msg>> {
    match msg {
        ChatMsg::Scrolled(viewport) => {
            st.chat_scroll.at_bottom = at_end(&viewport);
            if st.chat_scroll.at_bottom {
                st.chat_scroll.unread = 0;
            }
            Some(Task::none())
        }
        ChatMsg::JumpToNewest => {
            st.chat_scroll.at_bottom = true;
            st.chat_scroll.unread = 0;
            st.chat_scroll.seen = st.panels.entries.len();
            Some(snap())
        }
        _ => None,
    }
}

/// Agent text, drawn with its rich content (chips, cards, images: `console::rich`). Every place that shows agent prose
/// calls this one function.
pub fn prose<'a>(source: &str, rctx: &rich::RichCtx, cards: &'a rich::Cards) -> Element<'a, Msg> {
    rich::view(source, rctx, cards, &on_rich)
}

/// A rich-content click or load, as the IDE's message.
fn on_rich(action: rich::RichAction) -> Msg {
    Msg::Rich(rich::Ev::Action(action))
}

/// The rich context of the IDE's chat: the workspace's files and folders (a folder with its `/`), and the project the
/// workspace belongs to.
pub fn rich_ctx(st: &State) -> rich::RichCtx {
    let paths: BTreeSet<String> = st
        .tree
        .entries
        .iter()
        .map(|e| {
            if e.kind == EntryKind::Dir {
                format!("{}/", e.path)
            } else {
                e.path.clone()
            }
        })
        .collect();
    let project = ide::selected_project(st);
    rich::context(&st.cards, project.map(|p| p.id.as_str()), Some(&paths))
}

/// The texts of the transcript that rich content reads: the prompts and the text blocks of the replies.
pub fn transcript_texts(entries: &[Value]) -> Vec<&str> {
    let mut texts = Vec::new();
    for entry in entries {
        let data = entry.get("data").unwrap_or(&Value::Null);
        match entry.get("kind").and_then(Value::as_str).unwrap_or("") {
            "work_start" => texts.push(s(data, "prompt")),
            "model_response" => {
                for block in data
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
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

/// The header of the centre: the agent's dot and label, and the four views.
pub fn header(st: &State) -> Element<'_, Msg> {
    let p = theme::palette();
    let (dot, name) = match ide::selected_agent(st) {
        Some(agent) => (
            Some(ide_model::dot(&agent.status, None)),
            agent.label.clone(),
        ),
        None => (None, ide::crumbs(st).1),
    };
    let mut head = row![].spacing(10).align_y(Alignment::Center);
    if let Some(dot) = dot {
        head = head.push(ide_sidebar::dot_mark(dot));
    }
    head = head.push(ui::sans(name, 14.0, p.ink));
    head = head.push(Space::new().width(Length::Fill));
    // Only the views the workspace has: a workstation has its terminal and its editor, no chat and no diff.
    let views: Vec<(Centre, &str)> = centre_views(st.ide.workspace.as_ref())
        .into_iter()
        .map(|view| {
            (
                view,
                match view {
                    Centre::Chat => "Chat",
                    Centre::Terminal => "Terminal",
                    Centre::Editor => "Editor",
                    Centre::Diff => "Diff",
                },
            )
        })
        .collect();
    // The web's segmented control: the views in one bordered group, the open one filled.
    head = head.push(ui::seg(&views, st.centre, Msg::Centre));
    head.into()
}

/// The owner's message: the web's `.msg-owner`, a bubble on the right at most 78% of the column, its last corner square.
fn owner_bubble<'a>(body: &str) -> Element<'a, Msg> {
    let p = theme::palette();
    let bubble = container(ui::prose(body.to_string()))
        .padding([11, 15])
        .max_width(600.0)
        .style(move |_| container::Style {
            background: Some(p.hover.into()),
            border: iced::Border {
                color: p.rule,
                width: 1.0,
                radius: iced::border::Radius {
                    top_left: 16.0,
                    top_right: 16.0,
                    bottom_right: 4.0,
                    bottom_left: 16.0,
                },
            },
            ..Default::default()
        });
    row![Space::new().width(Length::Fill), bubble].into()
}

/// A board nudge or a bookkeeping line: the web's `.notice`, a bar on the paper with an info mark.
fn system_bubble<'a>(body: &str) -> Element<'a, Msg> {
    let p = theme::palette();
    let line = row![
        ui::icon::<Msg>("info", 14.0, p.ink_3),
        ui::sans(body.to_string(), 13.0, p.ink_2),
    ]
    .spacing(9)
    .align_y(Alignment::Center);
    container(line)
        .padding([8, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(p.paper_side.into()),
            border: iced::Border {
                color: p.rule,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One transcript entry as the chat draws it. Entries that are only bookkeeping draw nothing.
fn entry_view<'a>(
    entry: &'a Value,
    rctx: &rich::RichCtx,
    cards: &'a rich::Cards,
) -> Option<Element<'a, Msg>> {
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

/// The verb and icon of a step, by its tool's name: the web's first rule that matches, else the name itself.
fn step_verb(name: &str) -> (&'static str, String) {
    let n = name.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if n.starts_with("read") || n.starts_with("view") || n.starts_with("open") {
        ("file-text", "Read".into())
    } else if any(&["edit", "patch", "replace"]) {
        ("pencil", "Edited".into())
    } else if any(&["write", "create"]) {
        ("file-plus", "Wrote".into())
    } else if any(&["bash", "shell", "command", "exec", "run"]) {
        ("terminal", "Ran".into())
    } else if any(&["grep", "search", "glob", "find"]) {
        ("search", "Searched".into())
    } else {
        ("circle", name.to_string())
    }
}

/// A run of tool steps (the entries `first..end`): the web's "N steps · the last one · outcome" toggle over a row
/// per step (icon, verb, the argument it acted on, and its time or status), open by default.
fn steps_view(entries: &[Value], first: usize, end: usize, open: bool) -> Element<'static, Msg> {
    let p = theme::palette();
    let steps = ide_model::steps_of(&entries[first..end]);
    let count = steps.len();
    let noun = if count == 1 { "step" } else { "steps" };
    let last = steps.last().map(ide_model::step_label).unwrap_or_default();
    let label = format!(
        "{count} {noun} · {last} · {}",
        ide_model::group_outcome(&steps)
    );
    let chevron = ui::icon::<Msg>(
        if open {
            "chevron-down"
        } else {
            "chevron-right"
        },
        14.0,
        p.ink_3,
    );
    let head = button(
        row![
            ui::sans(label, 13.5, p.ink_3),
            Space::new().width(Length::Fill),
            chevron
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6, 0])
    .style(|_: &Theme, _| button::Style {
        background: None,
        text_color: theme::palette().ink_3,
        ..Default::default()
    })
    .on_press(Msg::Chat(ChatMsg::Steps(first)));
    let mut col = column![head, rule_line()].spacing(0);
    if open {
        let mut rows = column![].spacing(2);
        for step in &steps {
            let (icon_name, verb) = step_verb(&step.name);
            let meta = match step.ms {
                Some(ms) => ide_model::format_ms(ms),
                None => step.outcome.to_string(),
            };
            rows = rows.push(
                row![
                    ui::icon::<Msg>(icon_name, 14.0, p.ink_2),
                    ui::sans(verb, 13.0, p.ink_3),
                    ui::mono(step.detail.clone(), 12.0, p.ink).width(Length::Fill),
                    ui::sans(meta, 12.0, p.ink_3),
                ]
                .spacing(9)
                .align_y(Alignment::Center),
            );
        }
        col = col.push(
            container(rows)
                .padding(Padding {
                    top: 10.0,
                    right: 0.0,
                    bottom: 14.0,
                    left: 0.0,
                })
                .width(Length::Fill),
        );
    }
    col.into()
}

/// The glyph of a tool's kind, the web's `KIND_GLYPH`; a kind not listed gets the bullet.
fn kind_glyph(kind: &str) -> &'static str {
    match kind {
        "read" => "◫",
        "edit" => "✎",
        "delete" => "✕",
        "move" => "↦",
        "search" => "⌕",
        "execute" => "▸",
        "think" => "…",
        "fetch" => "↓",
        "switch_mode" => "⇄",
        _ => "•",
    }
}

/// A tool call's status word: the web's `TOOL_WORD`, from the ACP status the engine reports.
fn tool_word(status: &str) -> &'static str {
    match status {
        "in_progress" => "running",
        "completed" => "done",
        "failed" => "failed",
        _ => "pending",
    }
}

/// One labelled part of an engine block: the web's `.ic-eng-l` label over its body.
fn section<'a>(label: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let p = theme::palette();
    column![ui::sans(label.to_string(), 11.0, p.ink_3), body]
        .spacing(4)
        .into()
}

/// A coding engine's call as one card: its engine badge, session and status, what it said, its tools, plan and the
/// checks it holds for the owner, each with Deny and Approve while it waits.
fn engine_block<'a>(
    view: &ide_model::EngineView,
    rctx: &rich::RichCtx,
    cards: &'a rich::Cards,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let session: String = view.session.chars().take(8).collect();
    let mut head = row![ui::badge::<Msg>(&view.engine, ui::Tone::Signal)]
        .spacing(8)
        .align_y(Alignment::Center);
    if !session.is_empty() {
        head = head.push(ui::mono(session, 11.5, p.ink_3));
    }
    head = head.push(Space::new().width(Length::Fill)).push(ui::sans(
        view.status.clone(),
        13.0,
        p.ink_2,
    ));
    let mut col = column![head].spacing(12);
    if !view.message.trim().is_empty() {
        col = col.push(prose(view.message.trim(), rctx, cards));
    }
    if !view.tools.is_empty() {
        let mut tools = column![].spacing(8);
        for tool in &view.tools {
            let glyph = kind_glyph(&tool.kind);
            let mut item = column![
                row![
                    ui::mono(glyph, 13.0, p.ink_3),
                    ui::mono(tool.title.clone(), 12.5, p.ink).width(Length::Fill),
                    ui::sans(tool_word(&tool.status), 12.5, p.ink_3),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(4);
            if !tool.locations.is_empty() {
                item = item.push(ui::mono(tool.locations.join("  "), 11.5, p.ink_3));
            }
            tools = tools.push(item);
        }
        col = col.push(section("Tools", tools.into()));
    }
    if !view.plan.is_empty() {
        let mut plan = column![].spacing(6);
        for item in &view.plan {
            let (mark, tone) = if item.done {
                ("✓", p.ink_3)
            } else {
                ("○", p.ink_2)
            };
            plan = plan.push(
                row![
                    ui::mono(mark, 12.5, tone),
                    ui::sans(item.content.clone(), 13.0, tone),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        col = col.push(section("Plan", plan.into()));
    }
    if !view.pending.is_empty() {
        let mut checks = column![].spacing(8);
        for check in &view.pending {
            // A check the plaza named no approval for has nothing to decide, so its buttons stay off.
            let decide = |approve: bool| {
                (!check.approval.is_empty())
                    .then(|| Msg::Chat(ChatMsg::Decide(check.approval.clone(), approve)))
            };
            checks = checks.push(
                row![
                    ui::sans(check.title.clone(), 13.0, p.ink_2).width(Length::Fill),
                    ui::sans("waiting", 12.5, p.ink_3),
                    ui::btn_sm("Deny", None, Kind::Danger, decide(false)),
                    ui::btn_sm("Approve", None, Kind::Primary, decide(true)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        col = col.push(section("Checks", checks.into()));
    }
    container(col)
        .padding(16)
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// The transcript as rows, then the working line and any error.
fn transcript(st: &State) -> Element<'_, Msg> {
    let entries = &st.panels.entries;
    let rctx = rich_ctx(st);
    let agent = ide::selected_agent(st);
    let who = agent.map_or_else(|| "the agent".to_string(), |a| a.label.clone());
    let mut col = column![].spacing(12);
    if let Some(error) = &st.panels.entries_error {
        col = col.push(ui::error_line(error));
    } else if !st.panels.entries_loaded {
        col = col.push(ui::faint("Loading the conversation…"));
    } else if entries.is_empty() {
        col = col.push(ui::faint(format!("No messages with {who} yet.")));
    }
    for item in ide_model::rows(entries) {
        match item {
            Row::Entry(i) => {
                if let Some(view) = entry_view(&entries[i], &rctx, &st.cards) {
                    col = col.push(view);
                }
            }
            Row::Steps { first, end, .. } => {
                col = col.push(steps_view(
                    entries,
                    first,
                    end,
                    st.steps_open.contains(&first),
                ));
            }
            Row::Engine { call, .. } => {
                let view = ide_model::engine_view(entries, &call);
                col = col.push(engine_block(&view, &rctx, &st.cards));
            }
        }
    }
    if agent.is_some_and(|a| a.status == "working") {
        col = col.push(working_line());
    }
    col.into()
}

/// The web's working line: "Working" and three dots.
fn working_line<'a>() -> Element<'a, Msg> {
    let p = theme::palette();
    let dots = iced::widget::Row::with_children(
        (0..3).map(|_| Element::<Msg>::from(ui::sans("•", 10.0, p.ink_3))),
    )
    .spacing(3)
    .align_y(Alignment::Center);
    row![ui::sans("Working", 13.0, p.ink_3), dots]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// The web's `.worked` rule under a toggle: a 1px line across the column.
fn rule_line<'a>() -> Element<'a, Msg> {
    let p = theme::palette();
    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
        .style(move |_| container::Style {
            background: Some(p.rule.into()),
            ..Default::default()
        })
        .into()
}

/// The round icon button of the composer's bar that is not a send: the web's `.pick`, a ghost with the hover ground.
fn pick_style(_: &Theme, status: button::Status) -> button::Style {
    let p = theme::palette();
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hot.then(|| p.hover.into()),
        text_color: if hot { p.ink } else { p.ink_2 },
        border: iced::Border {
            radius: 9.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The composer, the web's `.composer`: a bordered box on the raised ground holding the note, and under it a bar with
/// the target line ("Sending to: …"), the paperclip and the round Send. While the agent works, Stop takes Send's place.
fn composer(st: &State) -> Element<'_, Msg> {
    let p = theme::palette();
    let agent = ide::selected_agent(st);
    let who = agent.map_or_else(|| "the agent".to_string(), |a| a.label.clone());
    let on = ide::selected_project(st)
        .map(|p| format!(" on {}", p.title))
        .unwrap_or_else(|| " on no project".to_string());
    let editor = text_editor(&st.panels.steer)
        .height(Length::Fixed(COMPOSER_H))
        .padding(Padding {
            top: 14.0,
            right: 16.0,
            bottom: 6.0,
            left: 16.0,
        })
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
        })
        .style(|_: &Theme, _| text_editor::Style {
            background: iced::Background::Color(iced::Color::TRANSPARENT),
            border: iced::Border::default(),
            placeholder: theme::palette().ink_3,
            value: theme::palette().ink,
            selection: theme::palette().signal,
        });
    let len = st.panels.steer.text().trim().chars().count();
    let files = !st.panels.attachments.is_empty();
    let can_send = !st.panels.steer_busy
        && (len > 0 || files)
        && len <= ide_panels::STEER_MAX
        && st.panels.uploading == 0
        && agent.is_some();
    let working = agent.is_some_and(|a| a.status == "working");
    let stop = working && !st.cancelling;
    let (signal_fill, on_signal) = (p.signal_fill, p.on_signal);
    let pick = button(ui::icon::<Msg>("paperclip", 16.0, p.ink_2))
        .padding([0, 9])
        .height(Length::Fixed(30.0))
        .style(pick_style)
        .on_press_maybe(
            agent
                .is_some()
                .then_some(Msg::Panels(ide_panels::Msg::Pick)),
        );
    let action: Element<'_, Msg> = if working {
        ui::btn_sm(
            "Stop",
            None,
            Kind::DangerOutline,
            stop.then_some(Msg::Chat(ChatMsg::Cancel)),
        )
    } else {
        button(ui::icon::<Msg>("arrow-up", 16.0, on_signal))
            .padding(0)
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .style(move |_: &Theme, status: button::Status| {
                let alpha = if matches!(status, button::Status::Disabled) {
                    0.4
                } else {
                    1.0
                };
                button::Style {
                    background: Some(signal_fill.scale_alpha(alpha).into()),
                    text_color: on_signal,
                    border: iced::Border {
                        radius: 16.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press_maybe(can_send.then_some(Msg::Panels(ide_panels::Msg::SteerSend)))
            .into()
    };
    let chip = container(
        chat_tools::chip(&st.panels.chat_tools).map(|m| Msg::Panels(ide_panels::Msg::ChatTools(m))),
    )
    .width(Length::Fixed(360.0));
    let bar = row![
        ui::sans(format!("Sending to: {who}{on}"), 12.0, p.ink_3),
        Space::new().width(Length::Fill),
        chip,
        pick,
        action,
    ]
    .spacing(2)
    .align_y(Alignment::Center);
    let bar = container(bar)
        .padding(Padding {
            top: 6.0,
            right: 8.0,
            bottom: 8.0,
            left: 8.0,
        })
        .width(Length::Fill);
    let mut boxed = column![].spacing(0);
    if let Some(chips) =
        ide_panels::attachment_chips(&st.panels.attachments, st.panels.uploading, |id| {
            Msg::Panels(ide_panels::Msg::Detach(id))
        })
    {
        boxed = boxed.push(container(chips).padding(Padding {
            top: 8.0,
            right: 12.0,
            bottom: 0.0,
            left: 12.0,
        }));
    }
    let context = container(
        chat_tools::bar(&st.panels.chat_tools).map(|m| Msg::Panels(ide_panels::Msg::ChatTools(m))),
    )
    .padding(Padding {
        top: 8.0,
        right: 12.0,
        bottom: 0.0,
        left: 12.0,
    })
    .width(Length::Fill);
    boxed = boxed.push(context).push(editor).push(bar);
    let (raised, rule_strong) = (p.raised, p.rule_strong);
    let mut col = column![
        container(boxed)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(raised.into()),
                border: iced::Border {
                    color: rule_strong,
                    width: 1.0,
                    radius: 18.0.into(),
                },
                ..Default::default()
            })
    ]
    .spacing(8);
    if let Some(notice) = &st.panels.steer_notice {
        col = col.push(ui::faint(notice.clone()));
    }
    if let Some(e) = chat_tools::notice(&st.panels.chat_tools) {
        col = col.push(e.map(|m| Msg::Panels(ide_panels::Msg::ChatTools(m))));
    }
    col.into()
}

/// The chat: the transcript in a reading column that scrolls (and follows its newest entry, see `decide`), a
/// "New messages" pill while the reader is above new entries, and the composer under it.
pub fn chat(st: &State) -> Element<'_, Msg> {
    let reading = container(transcript(st))
        .max_width(COLUMN_MAX)
        .width(Length::Fill);
    let body = container(reading)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(Padding {
            top: 28.0,
            right: 16.0,
            bottom: 20.0,
            left: 16.0,
        });
    let scroll = ui::scroll(body)
        .id(CHAT_ID.clone())
        .on_scroll(|viewport| Msg::Chat(ChatMsg::Scrolled(viewport)))
        .height(Length::Fill);
    let transcript_area: Element<'_, Msg> =
        if st.chat_scroll.unread > 0 && !st.chat_scroll.at_bottom {
            Stack::new()
                .push(scroll)
                .push(
                    container(pill())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(Alignment::Center)
                        .align_y(Alignment::End)
                        .padding(Padding {
                            top: 0.0,
                            right: 0.0,
                            bottom: 12.0,
                            left: 0.0,
                        }),
                )
                .into()
        } else {
            scroll.into()
        };
    let base = container(composer(st))
        .max_width(COLUMN_MAX)
        .width(Length::Fill);
    column![
        transcript_area,
        container(base)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: 18.0,
                left: 16.0
            }),
    ]
    .spacing(0)
    .height(Length::Fill)
    .into()
}

/// The pill that jumps the reader to the newest entry.
fn pill<'a>() -> Element<'a, Msg> {
    let p = theme::palette();
    container(ui::btn(
        "New messages ↓",
        None,
        Kind::Secondary,
        Some(Msg::Chat(ChatMsg::JumpToNewest)),
    ))
    .style(move |_| container::Style {
        background: Some(p.sheet.into()),
        border: iced::Border {
            color: p.rule,
            width: 1.0,
            radius: 14.0.into(),
        },
        ..Default::default()
    })
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
        Centre::Terminal => {
            ide_panels::terminal_body(st.terminals.as_ref(), has_shell(st.ide.workspace.as_ref()))
        }
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

// ---- The messages the IDE hands to this area (moved from `ide.rs`'s `update`) ----------------------

/// Whether a message is this area's: `update` below takes it, and `ide.rs` hands it over.
pub(super) fn handles(msg: &Msg) -> bool {
    matches!(msg, Msg::Centre(..) | Msg::Chat(..))
}

/// The area's messages: the state they change, and the tasks they start. See `handles`.
pub(super) fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        // A view the workspace does not have (a workstation's chat or diff) is not shown.
        Msg::Centre(centre) if !centre_views(st.ide.workspace.as_ref()).contains(&centre) => {
            Task::none()
        }
        Msg::Centre(centre) => {
            st.centre = centre;
            st.diff_file = None;
            // The Terminal view opens the drawer too, so the terminal is docked under the chat when the owner goes back.
            if centre == Centre::Terminal && !st.layout.drawer {
                st.layout.drawer = true;
                ide_chrome::save(&st.layout);
            }
            ensure_terminals(st, ctx);
            match (centre, st.ide.workspace.clone()) {
                (Centre::Chat, Some(ws)) => {
                    // The chat's scroll area is new on screen: it opens at the newest entry.
                    let to_newest = ide_chat::shown_again(st);
                    Task::batch([
                        ide_panels::show(&mut st.panels, Panel::Agent, &ctx.client, &ws)
                            .map(Msg::Panels),
                        to_newest,
                    ])
                }
                (Centre::Diff, Some(_)) if st.changes.is_none() && !st.changes_busy => {
                    changes_task(st, ctx)
                }
                _ => Task::none(),
            }
        }
        Msg::Chat(msg) => chat_task(st, msg, ctx),
        _ => Task::none(),
    }
}

/// The path a decision is posted to: the approval id is one path segment, percent-encoded.
fn decision_path(id: &str) -> String {
    format!("/v1/approvals/{}", enc(id))
}

/// The decisions a coding engine's block sends: an approval is decided as the approvals screen decides it, and a
/// refused send is said in the notice (the transcript shows the decision once the plaza records it).
pub(super) fn decision_task(st: &mut State, msg: ChatMsg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        ChatMsg::Decide(id, approve) => {
            let c = ctx.client.clone();
            let path = decision_path(&id);
            let body = json!({ "approve": approve });
            client::task(
                move || c.post::<Value>(&path, &body).map(|_| ()),
                |res| Msg::Chat(ChatMsg::Decided(res)),
            )
        }
        ChatMsg::Decided(res) => {
            if let Err(e) = res {
                st.notice = Some(format!("The decision was not sent: {e}"));
            }
            Task::none()
        }
        _ => Task::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_decision_names_its_approval_as_one_encoded_segment() {
        assert_eq!(decision_path("a7"), "/v1/approvals/a7");
        assert_eq!(decision_path("a7/../x y"), "/v1/approvals/a7%2F..%2Fx%20y");
    }

    #[test]
    fn an_engine_block_draws_its_tools_plan_and_pending_checks_with_their_decisions() {
        let entries = vec![
            json!({"kind": "tool_request", "data": {"call": "c1", "name": "code_engine",
                "input": {"engine": "claude", "prompt": "fix it"}}}),
            json!({"kind": "engine_update", "data": {"call": "c1", "item": "tool_call",
                "update": {"toolCallId": "t1", "title": "Read main.rs", "kind": "read", "status": "in_progress"}}}),
            json!({"kind": "engine_update", "data": {"call": "c1", "item": "plan",
                "update": [{"content": "Read", "status": "completed"}]}}),
            json!({"kind": "tool_request", "data": {"call": "c1.e1", "name": "engine.edit",
                "input": {"engine": "claude", "title": "Edit lib.rs"}}}),
            json!({"kind": "tool_decision", "data": {"call": "c1.e1", "decision": "pending",
                "output": "waiting for the owner's approval (ap_9)"}}),
        ];
        let view = ide_model::engine_view(&entries, "c1");
        let (rctx, cards) = (
            rich::context(&rich::Cards::default(), None, None),
            rich::Cards::default(),
        );
        // The block draws with a check to decide, and with no check and no approval id it still draws.
        let _waiting = engine_block(&view, &rctx, &cards);
        let mut unnamed = view.clone();
        unnamed.pending[0].approval.clear();
        let _unnamed = engine_block(&unnamed, &rctx, &cards);
        assert_eq!(view.pending[0].approval, "ap_9");
    }
}
