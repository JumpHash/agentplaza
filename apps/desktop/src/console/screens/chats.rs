//! The Chats screens (the web's `chats` view): the chats (`Id::Chats`) with the plaza's orchestrator, with each project's
//! planner and with each task's seats; and one chat (`Id::Chat(id)`): its last messages, the questions the plaza asks
//! (answered in place, with the inbox's buttons), its rename and archive actions, and a composer. The chat's messages
//! reload on its `chat` frames, its questions on `card` frames; the list reloads on any `chat` frame.

use std::collections::HashMap;

use iced::widget::{column, container, pick_list, row, text_input};
use iced::{Element, Length, Task};
use serde_json::Value;

use super::Id;
use super::chat_model::{self as chat, AmountUnit};
use super::inbox::{self, Answers};
use super::work_ui::{self, Col, EDGE};
use crate::console::events;
use crate::console::work_api::{self, Card, Chat, ChatMessage};
use crate::console::{Ctx, client, scope, ui};
use crate::theme;

/// The messages a chat screen reads: the last of them.
const MESSAGES: u32 = 200;

#[derive(Debug, Default)]
pub struct State {
    /// The chat on the page; None on the list.
    on: Option<String>,
    /// The chats the list shows, and the projects a new chat can open with.
    chats: Vec<Chat>,
    projects: Vec<Value>,
    /// The first read of the list has answered.
    list_loaded: bool,
    list_failed: bool,
    archived: bool,
    expert: bool,
    /// The New chat dialog, while it is open: its project and its title.
    new_chat: Option<NewChat>,
    /// The chat on the page, as last read; None until its first read, or when it cannot be read (`missing`).
    chat: Option<Chat>,
    missing: Option<String>,
    /// The page's questions as cards, by id, and the units their amounts are counted in.
    cards: HashMap<String, Card>,
    answers: Answers,
    /// The title typed while the chat is renamed; None when it is not.
    renaming: Option<String>,
    /// The message being typed, and whether it is on its way to the hub.
    draft: String,
    sending: bool,
    /// A line for the last action ("Chat archived") or why it failed.
    note: Option<String>,
    /// A list load is in flight; a chat frame that comes meanwhile only asks for one more.
    loading: bool,
    again: bool,
}

/// The New chat dialog's fields.
#[derive(Debug, Clone, Default)]
pub struct NewChat {
    project: Option<String>,
    title: String,
}

/// What the list read: the chats, and the projects the dialog offers.
#[derive(Debug, Clone, Default)]
pub struct Listed {
    chats: Vec<Chat>,
    projects: Vec<Value>,
}

/// The page's questions: their cards, and the units their amounts are counted in.
#[derive(Debug, Clone, Default)]
pub struct Questions {
    cards: Vec<Card>,
    units: HashMap<String, AmountUnit>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen (a chat's link, or the chat a new one opened).
    Go(Id),
    Listed(Result<Listed, String>),
    /// The page's chat, as read: or the hub's words for why it cannot be read.
    Read(Result<Chat, String>),
    Questions(Result<Questions, String>),
    Archived(bool),
    Expert(bool),
    OpenNewChat,
    CloseNewChat,
    NewChatProject(String),
    NewChatTitle(String),
    StartChat,
    Started(Result<Chat, String>),
    StartRename,
    RenameTo(String),
    SaveRename,
    CancelRename,
    SetArchived(bool),
    /// The chat as the hub settled a rename or an archive.
    Edited(Result<Chat, String>),
    Draft(String),
    Send,
    Sent(Result<ChatMessage, String>),
    /// A question's answer, as the inbox's buttons send it.
    Answers(inbox::Msg),
    /// A hub event: a chat frame, or a card frame.
    Event(events::Event),
}

/// A chat screen's state at open: the list (`None`), or the page of one chat.
pub fn opened(on: Option<String>) -> State {
    State {
        on,
        ..Default::default()
    }
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The hub's events while the screen is open: the chat and card frames.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// Reads the list (the chats in the scope, and the projects), or the page (its chat and its questions).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    match state.on.clone() {
        None => {
            state.loading = true;
            let project = ctx.project.clone();
            let archived = state.archived;
            client::task(
                move || {
                    let chats = work_api::chats(&client, project.as_deref(), archived)?;
                    let projects = client
                        .get::<Value>("/v1/projects")?
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    Ok(Listed { chats, projects })
                },
                Msg::Listed,
            )
        }
        Some(id) => {
            let (read_client, questions_client) = (client.clone(), client);
            let (chat_id, questions_id) = (id.clone(), id);
            let read = client::task(
                move || work_api::chat(&read_client, &chat_id, MESSAGES),
                Msg::Read,
            );
            let questions = client::task(
                move || {
                    let cards = work_api::cards(&questions_client, false, Some(&questions_id))?;
                    let units = inbox::amount_units(&questions_client, &cards);
                    Ok(Questions { cards, units })
                },
                Msg::Questions,
            );
            Task::batch([read, questions])
        }
    }
}

/// A list read landed: one more runs if chat frames came in while it was in flight.
fn landed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Listed(Ok(listed)) => {
            state.chats = listed.chats;
            state.projects = listed.projects;
            state.list_failed = false;
            state.list_loaded = true;
            landed(state, ctx)
        }
        Msg::Listed(Err(_)) => {
            state.list_failed = true;
            state.list_loaded = true;
            landed(state, ctx)
        }
        Msg::Read(Ok(read)) => {
            state.chat = Some(read);
            state.missing = None;
            Task::none()
        }
        Msg::Read(Err(message)) => {
            if state.chat.is_none() {
                state.missing = Some(message);
            }
            Task::none()
        }
        Msg::Questions(Ok(questions)) => {
            state.cards = questions
                .cards
                .into_iter()
                .map(|c| (c.id.clone(), c))
                .collect();
            state.answers.set_units(questions.units);
            Task::none()
        }
        Msg::Questions(Err(_)) => Task::none(),
        Msg::Archived(on) => {
            state.archived = on;
            load(state, ctx)
        }
        Msg::Expert(on) => {
            state.expert = on;
            Task::none()
        }
        Msg::OpenNewChat => {
            let live: Vec<String> = scope::picks(&state.projects)
                .into_iter()
                .map(|p| p.id)
                .collect();
            if live.is_empty() {
                state.note = Some("There is no live project to start a chat in.".into());
                return Task::none();
            }
            state.new_chat = Some(NewChat {
                project: chat::preselect_project(&live, ctx.project.as_deref()),
                title: String::new(),
            });
            Task::none()
        }
        Msg::CloseNewChat => {
            state.new_chat = None;
            Task::none()
        }
        Msg::NewChatProject(project) => {
            if let Some(new) = state.new_chat.as_mut() {
                new.project = Some(project);
            }
            Task::none()
        }
        Msg::NewChatTitle(title) => {
            if let Some(new) = state.new_chat.as_mut() {
                new.title = title;
            }
            Task::none()
        }
        Msg::StartChat => {
            let Some(new) = state.new_chat.clone() else {
                return Task::none();
            };
            let Some(project) = new.project else {
                return Task::none();
            };
            let client = ctx.client.clone();
            let title = chat::non_blank(&new.title);
            client::task(
                move || work_api::open_chat(&client, &project, title.as_deref()),
                Msg::Started,
            )
        }
        Msg::Started(Ok(started)) => {
            state.new_chat = None;
            Task::done(Msg::Go(Id::Chat(started.id)))
        }
        Msg::Started(Err(message)) => {
            state.note = Some(message);
            Task::none()
        }
        Msg::StartRename => {
            state.renaming = state.chat.as_ref().map(|c| c.title.clone());
            Task::none()
        }
        Msg::RenameTo(title) => {
            state.renaming = Some(title);
            Task::none()
        }
        Msg::CancelRename => {
            state.renaming = None;
            Task::none()
        }
        Msg::SaveRename => {
            let Some(title) = state.renaming.as_deref().and_then(chat::non_blank) else {
                state.note = Some("A chat needs a title.".into());
                return Task::none();
            };
            edit(state, ctx, Some(title), None)
        }
        Msg::SetArchived(archived) => edit(state, ctx, None, Some(archived)),
        Msg::Edited(Ok(edited)) => {
            state.renaming = None;
            let note = if edited.archived {
                "Chat archived"
            } else {
                "Chat saved"
            };
            state.note = Some(note.into());
            state.chat = Some(edited);
            Task::none()
        }
        Msg::Edited(Err(message)) => {
            state.note = Some(message);
            Task::none()
        }
        Msg::Draft(text) => {
            state.draft = text;
            Task::none()
        }
        Msg::Send => {
            let Some(text) = chat::non_blank(&state.draft) else {
                return Task::none();
            };
            if state.sending {
                return Task::none();
            }
            let Some(id) = state.on.clone() else {
                return Task::none();
            };
            state.sending = true;
            let client = ctx.client.clone();
            client::task(move || work_api::say(&client, &id, &text), Msg::Sent)
        }
        Msg::Sent(Ok(_)) => {
            state.sending = false;
            state.draft.clear();
            load(state, ctx)
        }
        Msg::Sent(Err(message)) => {
            state.sending = false;
            state.note = Some(message);
            Task::none()
        }
        Msg::Answers(inbox::Msg::Amount(id, text)) => {
            state.answers.set_amount(&id, text);
            Task::none()
        }
        Msg::Answers(inbox::Msg::Choose(id, choice)) => {
            let Some(card) = state.cards.get(&id).cloned() else {
                return Task::none();
            };
            match state.answers.send(ctx, &card, &choice) {
                Ok(task) => task.map(Msg::Answers),
                Err(message) => {
                    state.note = Some(message);
                    Task::none()
                }
            }
        }
        Msg::Answers(inbox::Msg::Answered(id, Ok(answer))) => {
            state.answers.settled(&id, true);
            state.cards.insert(answer.card.id.clone(), answer.card);
            state.note = Some(chat::done_text(&answer.done));
            Task::none()
        }
        Msg::Answers(inbox::Msg::Answered(id, Err(message))) => {
            state.answers.settled(&id, false);
            state.note = Some(message);
            load(state, ctx)
        }
        Msg::Answers(_) => Task::none(),
        Msg::Event(event) => match state.on.clone() {
            None => {
                if event.kind != "chat" {
                    return Task::none();
                }
                if state.loading {
                    state.again = true;
                    return Task::none();
                }
                load(state, ctx)
            }
            Some(id) => match event.kind.as_str() {
                "chat" if chat::frame_chat(&event.data).as_deref() == Some(id.as_str()) => {
                    load(state, ctx)
                }
                "card" => load(state, ctx),
                _ => Task::none(),
            },
        },
    }
}

/// Renames the page's chat, or archives it (or restores it): the hub answers with the chat.
fn edit(state: &mut State, ctx: &Ctx, title: Option<String>, archived: Option<bool>) -> Task<Msg> {
    let Some(id) = state.on.clone() else {
        return Task::none();
    };
    let client = ctx.client.clone();
    client::task(
        move || work_api::edit_chat(&client, &id, title.as_deref(), archived),
        Msg::Edited,
    )
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    match &state.on {
        None => list_view(state, ctx),
        Some(_) => page_view(state),
    }
}

/// The chats: the list's table, with the archived and the expert switches and the New chat button.
fn list_view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head_with_intro(
        "Chats",
        &match ctx.project.as_deref() {
            Some(project) => format!("The chats of {}.", scope::title(project)),
            None => {
                "The orchestrator's chat, each project's planner and each task's seats.".to_string()
            }
        },
        vec![
            work_ui::switch("Show archived", state.archived, Msg::Archived),
            work_ui::switch("Expert", state.expert, Msg::Expert),
            ui::btn(
                "New chat",
                Some("plus"),
                ui::Kind::Primary,
                Some(Msg::OpenNewChat),
            ),
        ],
    );
    let body: Element<'a, Msg> = if !state.list_loaded {
        ui::loading()
    } else if state.list_failed {
        ui::error_line("Could not load the chats.")
    } else if state.chats.is_empty() {
        ui::empty(
            "conversations",
            if ctx.project.is_some() {
                "No chats in this project yet"
            } else {
                "No chats yet"
            },
            "Start one with New chat.",
        )
    } else {
        let cols = list_columns(state.expert);
        let rows: Vec<Element<'a, Msg>> = state
            .chats
            .iter()
            .map(|c| chat_row(c, state.expert, &cols))
            .collect();
        container(work_ui::table(
            &cols,
            &list_labels(state.expert),
            rows,
            EDGE,
        ))
        .style(ui::card_style)
        .width(Length::Fill)
        .clip(true)
        .into()
    };
    let mut lines: Vec<Element<'a, Msg>> = vec![head];
    if let Some(note) = &state.note {
        lines.push(ui::faint(note.clone()).into());
    }
    lines.push(body);
    let page: Element<'a, Msg> = column(lines).spacing(16).width(Length::Fill).into();
    match &state.new_chat {
        Some(new) => ui::with_modal(page, Some(new_chat_dialog(state, new))),
        None => page,
    }
}

fn list_columns(expert: bool) -> Vec<Col> {
    let mut cols = vec![Col::flex(5), Col::flex(2), Col::right(2)];
    if expert {
        cols.push(Col::flex(3));
    }
    cols
}

fn list_labels(expert: bool) -> Vec<&'static str> {
    let mut labels = vec!["Chat", "Kind", "Last activity"];
    if expert {
        labels.push("Id");
    }
    labels
}

/// One chat's row: its title (a link), its project beneath it, its kind, when it was last active.
fn chat_row<'a>(c: &Chat, expert: bool, cols: &[Col]) -> Element<'a, Msg> {
    let mut title_lines: Vec<Element<'a, Msg>> =
        vec![ui::text_link(&c.title, Msg::Go(Id::Chat(c.id.clone())))];
    if let Some(project) = c.project.as_ref().filter(|_| c.kind != "main") {
        title_lines.push(ui::faint(scope::title(project)).into());
    }
    if c.archived {
        title_lines.push(ui::badge("Archived", ui::Tone::Muted));
    }
    let mut cells: Vec<Element<'a, Msg>> = vec![
        column(title_lines).spacing(3).into(),
        ui::sans(
            chat::chat_kind_label(&c.kind).to_string(),
            14.0,
            theme::palette().ink,
        )
        .into(),
        ui::faint(ui::ago(c.updated_ms)).into(),
    ];
    if expert {
        cells.push(ui::mono(c.id.clone(), 12.0, theme::palette().ink_2).into());
    }
    let line = container(work_ui::grid_row(cols, cells, EDGE))
        .padding([9, 0])
        .width(Length::Fill);
    work_ui::clickable(line.into(), Msg::Go(Id::Chat(c.id.clone())))
}

/// The New chat dialog: a live project to talk with its planner, and an optional title. Opens the chat when started.
fn new_chat_dialog<'a>(state: &'a State, new: &'a NewChat) -> Element<'a, Msg> {
    let picks = scope::picks(&state.projects);
    let selected = picks
        .iter()
        .find(|p| Some(&p.id) == new.project.as_ref())
        .cloned();
    let project = pick_list(picks, selected, |pick: scope::Pick| {
        Msg::NewChatProject(pick.id)
    });
    let title = text_input("Leave it empty to use the project's title.", &new.title)
        .on_input(Msg::NewChatTitle);
    let body = column![ui::label("Project"), project, ui::label("Title"), title,]
        .spacing(8)
        .width(Length::Fill);
    ui::modal(
        "New chat",
        body.into(),
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseNewChat)),
            ui::btn("Start chat", None, ui::Kind::Primary, Some(Msg::StartChat)),
        ],
        Msg::CloseNewChat,
        440.0,
    )
}

/// One chat: its head (its title and kind, the rename and archive actions, the expert switch), its messages, and the
/// composer.
fn page_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if let Some(message) = &state.missing {
        return ui::empty("conversations", "Could not open this chat", message);
    }
    let Some(page) = &state.chat else {
        return ui::loading();
    };
    let mut lines: Vec<Element<'a, Msg>> = vec![head(state, page)];
    if let Some(note) = &state.note {
        lines.push(ui::faint(note.clone()).into());
    }
    let messages = chat::recent_messages(&page.messages, MESSAGES as usize);
    if messages.is_empty() {
        lines.push(ui::faint("No messages yet. Write below to start.").into());
    } else {
        lines.push(
            column(
                messages
                    .into_iter()
                    .map(|m| message_view(state, m))
                    .collect::<Vec<_>>(),
            )
            .spacing(14)
            .width(Length::Fill)
            .into(),
        );
    }
    lines.push(composer(state));
    column(lines).spacing(16).width(Length::Fill).into()
}

/// The page's head: the title (or the rename field), the kind and project, and the actions.
fn head<'a>(state: &'a State, chat: &'a Chat) -> Element<'a, Msg> {
    let subtitle = if chat.kind != "main" {
        match &chat.project {
            Some(project) => format!(
                "{} · {}",
                chat::chat_kind_label(&chat.kind),
                scope::title(project)
            ),
            None => chat::chat_kind_label(&chat.kind).to_string(),
        }
    } else {
        chat::chat_kind_label(&chat.kind).to_string()
    };
    let (title, actions): (Element<'a, Msg>, Vec<Element<'a, Msg>>) = match &state.renaming {
        Some(text) => (
            text_input("Chat title", text)
                .on_input(Msg::RenameTo)
                .on_submit(Msg::SaveRename)
                .into(),
            vec![
                ui::btn("Save", None, ui::Kind::Primary, Some(Msg::SaveRename)),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelRename)),
            ],
        ),
        None => {
            let mut actions = vec![ui::btn(
                "Rename",
                None,
                ui::Kind::Secondary,
                Some(Msg::StartRename),
            )];
            if chat.kind != "main" {
                let (label, archive) = if chat.archived {
                    ("Restore", false)
                } else {
                    ("Archive", true)
                };
                actions.push(ui::btn(
                    label,
                    None,
                    ui::Kind::Secondary,
                    Some(Msg::SetArchived(archive)),
                ));
            }
            (
                ui::sans(chat.title.clone(), 23.0, theme::palette().ink).into(),
                actions,
            )
        }
    };
    let mut actions = actions;
    actions.push(work_ui::switch("Expert", state.expert, Msg::Expert));
    let mut lines: Vec<Element<'a, Msg>> = vec![title, ui::faint(subtitle).into()];
    if state.expert {
        lines.push(ui::mono(chat.id.clone(), 12.0, theme::palette().ink_3).into());
    }
    row![
        column(lines).spacing(4).width(Length::Fill),
        iced::widget::Row::with_children(actions)
            .spacing(8)
            .align_y(iced::Alignment::Center),
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

/// One message: a question as its card (its buttons while it is open), a notice muted, talk with its author and time.
fn message_view<'a>(state: &'a State, m: &'a ChatMessage) -> Element<'a, Msg> {
    if m.kind == "question" {
        let card = m.card.as_ref().and_then(|id| state.cards.get(id));
        let badge = match card {
            Some(c) => ui::badge(&chat::card_kind_label(&c.kind), chat::kind_tone(&c.kind)),
            None => ui::badge("Question", ui::Tone::Muted),
        };
        let mut lines: Vec<Element<'a, Msg>> = vec![badge, ui::prose(m.text.clone()).into()];
        match card {
            Some(c) if c.state == "open" => lines.push(state.answers.choices(c).map(Msg::Answers)),
            Some(c) => lines.push(
                ui::faint(chat::settled_text(
                    &c.state,
                    c.answer.as_deref(),
                    &c.choices,
                ))
                .into(),
            ),
            None => {}
        }
        if state.expert {
            lines.push(ui::mono(chat::message_expert_line(m), 12.0, theme::palette().ink_3).into());
        }
        return container(column(lines).spacing(10).width(Length::Fill))
            .padding(16)
            .width(Length::Fill)
            .style(ui::card_style)
            .into();
    }
    let author = chat::author_label(&m.author, m.seat.as_deref());
    let words: Element<'a, Msg> = if m.kind == "notice" {
        ui::faint(m.text.clone()).into()
    } else {
        ui::prose(m.text.clone()).into()
    };
    let mut lines: Vec<Element<'a, Msg>> = vec![
        row![
            ui::sans(author, 14.0, theme::palette().ink),
            ui::faint(chat::clock_text(m.at_ms)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center)
        .into(),
        words,
    ];
    if state.expert {
        lines.push(ui::mono(chat::message_expert_line(m), 12.0, theme::palette().ink_3).into());
    }
    column(lines).spacing(4).width(Length::Fill).into()
}

/// The composer: a line of text and its Send button; Enter sends.
fn composer<'a>(state: &'a State) -> Element<'a, Msg> {
    let field = text_input("Write a message. Enter sends.", &state.draft)
        .on_input(Msg::Draft)
        .on_submit(Msg::Send)
        .width(Length::Fill);
    let send = ui::btn(
        "Send",
        Some("send"),
        ui::Kind::Primary,
        (!state.sending).then_some(Msg::Send),
    );
    row![field, send]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
}

#[cfg(test)]
#[path = "chats_tests.rs"]
mod tests;
