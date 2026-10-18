//! The IDE's conversation workspace (the web's `views/ide-conversation.ts`): the owner's conversation with the
//! orchestrator, the project it drafts beside it, and Start. The IDE's sidebar lists the conversations; the centre is
//! the chat (answers stream in as they are written); the right panel is the draft: its goal, tasks, team, budget and
//! settings, what still stops Start, the owner's edits, and Start. Once started, the conversation stays the project's
//! line to the orchestrator.

use iced::widget::{Space, column, container, row, text_editor, text_input};
use iced::{Alignment, Element, Length, Padding, Task};
use serde_json::{Value, json};

use super::Id;
use crate::console::shell;
use crate::console::{Ctx, client, roles, scope, ui};
use crate::theme;

/// The longest message the hub takes from the owner.
const MESSAGE_MAX: usize = 8000;

// ---- The words (pure; the web's `ide/conversation-model.ts`) --------------------------------------------------------

/// A workspace id the IDE opens as a conversation: `conv-<n>`.
pub fn is_conversation_id(id: &str) -> bool {
    id.strip_prefix("conv-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The name of agent `cid` from `roster` ((cid, name) pairs), or `agent <cid>`.
fn name_in(roster: &[(u64, String)], cid: u64) -> String {
    roster
        .iter()
        .find(|(c, _)| *c == cid)
        .map(|(_, n)| n.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("agent {cid}"))
}

/// One role of the team as the draft shows it: `2 Builder (Ada from the bench, 1 new)`.
pub fn seat_line(seat: &Value, roster: &[(u64, String)]) -> String {
    let count = seat["count"].as_u64().unwrap_or(0);
    let named: Vec<String> = seat["bench"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|cid| name_in(roster, cid))
        .collect();
    let fresh = count.saturating_sub(named.len() as u64);
    let mut parts = Vec::new();
    if !named.is_empty() {
        parts.push(format!("{} from the bench", named.join(", ")));
    }
    if fresh > 0 {
        parts.push(format!("{fresh} new"));
    }
    let role = roles::label(seat["role"].as_str().unwrap_or(""));
    if parts.is_empty() {
        format!("{count} {role}")
    } else {
        format!("{count} {role} ({})", parts.join(", "))
    }
}

/// The team in one line, roles joined by a dot.
pub fn team_line(team: &[Value], roster: &[(u64, String)]) -> String {
    team.iter()
        .map(|seat| seat_line(seat, roster))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A task's `after` in words: `after 1, 2`, or '' when it runs at once.
pub fn after_text(task: &Value) -> String {
    let after: Vec<&str> = task["after"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if after.is_empty() {
        String::new()
    } else {
        format!("after {}", after.join(", "))
    }
}

/// Why Start cannot be pressed now, or None when it can.
pub fn start_block(c: &Value) -> Option<String> {
    if c["project"].as_str().is_some_and(|p| !p.is_empty()) {
        return Some("The project is started.".into());
    }
    if c["archived"].as_bool().unwrap_or(false) {
        return Some("The conversation is archived.".into());
    }
    let draft = &c["draft"];
    if !draft.is_object() {
        return Some(
            "There is no draft yet: the orchestrator drafts the project as you talk.".into(),
        );
    }
    let problems: Vec<&str> = draft["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    (!problems.is_empty()).then(|| format!("Before it can start: {}.", problems.join("; ")))
}

/// The draft's budget in words.
pub fn budget_text(draft: &Value) -> String {
    match draft["budget"].as_i64() {
        Some(n) => format!("{} credits", ui::thousands(n)),
        None => "From the plaza's budget".into(),
    }
}

/// A draft's settings in words: `Builder model: anthropic/claude-sonnet-5-5 · effort high · permission: ask`.
pub fn settings_line(settings: &Value) -> String {
    let Some(map) = settings.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (role, model) in settings["role_models"].as_object().into_iter().flatten() {
        if let Some(model) = model.as_str() {
            parts.push(format!("{} model: {model}", roles::label(role)));
        }
    }
    for (key, value) in map {
        if key == "role_models" || value.is_null() {
            continue;
        }
        let shown = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let named = match key.as_str() {
            "default_model" => "model:",
            "effort" => "effort",
            "permission_mode" => "permission:",
            "task_budget" => "task budget",
            "max_turns" => "turns per task",
            "review_timeout_minutes" => "review timeout (min):",
            other => {
                parts.push(format!("{} {shown}", other.replace('_', " ")));
                continue;
            }
        };
        parts.push(format!("{named} {shown}"));
    }
    parts.join(" · ")
}

/// The text the pending answers have streamed so far, in arrival order (`deltas` are (work, text)).
pub fn streamed(deltas: &[(u64, String)], pending: &[u64]) -> String {
    deltas
        .iter()
        .filter(|(work, _)| pending.contains(work))
        .map(|(_, text)| text.as_str())
        .collect()
}

/// The work items a conversation still waits on.
fn pending_of(c: &Value) -> Vec<u64> {
    c["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect()
}

// ---- State and messages ------------------------------------------------------------------------------------------

/// The owner's edit of the draft's words and budget, on the revision on screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftEdit {
    pub revision: u64,
    pub title: String,
    pub goal: String,
    pub repo: String,
    pub budget: String,
}

/// The fields of the draft edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Goal,
    Repo,
    Budget,
}

/// The conversation open in the IDE.
#[derive(Debug)]
pub struct Conversation {
    pub id: String,
    /// `GET /v1/owner/conversations/{id}`; None until it is read.
    pub data: Option<Value>,
    pub error: Option<String>,
    /// A line for the owner: Start's outcome, a saved edit.
    pub notice: Option<String>,
    pub composer: text_editor::Content,
    pub sending: bool,
    pub starting: bool,
    /// What each pending answer has streamed so far, as (work, text) in arrival order.
    pub deltas: Vec<(u64, String)>,
    pub edit: Option<DraftEdit>,
}

impl Conversation {
    pub fn new(id: String) -> Conversation {
        Conversation {
            id,
            data: None,
            error: None,
            notice: None,
            composer: text_editor::Content::new(),
            sending: false,
            starting: false,
            deltas: Vec::new(),
            edit: None,
        }
    }

    /// The orchestrator this conversation talks to.
    fn cid(&self) -> Option<u64> {
        self.data.as_ref()?["cid"].as_u64()
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The conversation as the hub answered, for the id it was asked for.
    Loaded(String, Result<Value, String>),
    Compose(text_editor::Action),
    Send,
    Sent(Result<Value, String>),
    /// Start the drafted project at the revision on screen.
    Start,
    Started(Result<Value, String>),
    /// Opens the draft edit.
    Edit,
    EditField(Field, String),
    SaveEdit,
    CancelEdit,
    Edited(Result<Value, String>),
}

/// Reads conversation `id`.
pub fn load(id: &str, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = format!("/v1/owner/conversations/{id}");
    let id = id.to_string();
    client::task(
        move || c.get::<Value>(&path),
        move |res| Msg::Loaded(id.clone(), res),
    )
}

/// Takes in a fresh view of the conversation: an answer that arrived ends its stream.
fn adopt(conv: &mut Conversation, data: Value) {
    let pending = pending_of(&data);
    conv.deltas.retain(|(work, _)| pending.contains(work));
    conv.data = Some(data);
}

pub fn update(conv: &mut Conversation, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Loaded(id, res) => {
            if id == conv.id {
                match res {
                    Ok(data) => {
                        adopt(conv, data);
                        conv.error = None;
                    }
                    Err(e) => conv.error = Some(e),
                }
            }
            Task::none()
        }
        Msg::Compose(action) => {
            conv.composer.perform(action);
            Task::none()
        }
        Msg::Send => {
            let text = conv.composer.text().trim().to_string();
            if text.is_empty() || conv.sending || conv.data.is_none() {
                return Task::none();
            }
            if text.chars().count() > MESSAGE_MAX {
                conv.error = Some(format!("A message is at most {MESSAGE_MAX} characters."));
                return Task::none();
            }
            conv.sending = true;
            conv.error = None;
            let c = ctx.client.clone();
            let path = format!("/v1/owner/conversations/{}/messages", conv.id);
            client::task(
                move || c.post::<Value>(&path, &json!({ "text": text })),
                Msg::Sent,
            )
        }
        Msg::Sent(res) => {
            conv.sending = false;
            match res {
                Ok(data) => {
                    adopt(conv, data);
                    conv.composer = text_editor::Content::new();
                    conv.error = None;
                }
                Err(e) => conv.error = Some(e),
            }
            Task::none()
        }
        Msg::Start => {
            let Some(revision) = conv
                .data
                .as_ref()
                .and_then(|d| d["draft"]["revision"].as_u64())
            else {
                return Task::none();
            };
            if conv.starting || conv.data.as_ref().and_then(start_block).is_some() {
                return Task::none();
            }
            conv.starting = true;
            conv.error = None;
            let c = ctx.client.clone();
            let path = format!("/v1/owner/conversations/{}/start", conv.id);
            client::task(
                move || c.post::<Value>(&path, &json!({ "revision": revision })),
                Msg::Started,
            )
        }
        Msg::Started(res) => {
            conv.starting = false;
            match res {
                Ok(mut data) => {
                    let started = data
                        .as_object_mut()
                        .and_then(|o| o.remove("started"))
                        .unwrap_or_default();
                    let fresh: u64 = started["spawned"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s["count"].as_u64())
                        .sum();
                    conv.notice = Some(format!(
                        "{} started: {} tasks, {} agents from the bench, {fresh} new",
                        scope::title(started["project"].as_str().unwrap_or("")),
                        started["tasks"].as_array().map_or(0, Vec::len),
                        started["joined"].as_array().map_or(0, Vec::len),
                    ));
                    adopt(conv, data);
                    conv.error = None;
                }
                Err(e) => conv.error = Some(e),
            }
            Task::none()
        }
        Msg::Edit => {
            if let Some(d) = conv
                .data
                .as_ref()
                .map(|c| &c["draft"])
                .filter(|d| d.is_object())
            {
                conv.edit = Some(DraftEdit {
                    revision: d["revision"].as_u64().unwrap_or(0),
                    title: d["title"].as_str().unwrap_or("").to_string(),
                    goal: d["goal"].as_str().unwrap_or("").to_string(),
                    repo: d["repo"].as_str().unwrap_or("").to_string(),
                    budget: d["budget"]
                        .as_u64()
                        .map(|b| b.to_string())
                        .unwrap_or_default(),
                });
            }
            Task::none()
        }
        Msg::EditField(field, value) => {
            if let Some(edit) = conv.edit.as_mut() {
                match field {
                    Field::Title => edit.title = value,
                    Field::Goal => edit.goal = value,
                    Field::Repo => edit.repo = value,
                    Field::Budget => edit.budget = value,
                }
            }
            Task::none()
        }
        Msg::CancelEdit => {
            conv.edit = None;
            Task::none()
        }
        Msg::SaveEdit => {
            let Some(edit) = conv.edit.clone() else {
                return Task::none();
            };
            let mut body = json!({
                "revision": edit.revision,
                "title": edit.title.trim(),
                "goal": edit.goal.trim(),
                "repo": edit.repo.trim(),
            });
            match edit.budget.trim() {
                "" => {}
                raw => match raw.parse::<u64>() {
                    Ok(n) => body["budget"] = json!(n),
                    Err(_) => {
                        conv.error = Some("The budget is a whole number of credits.".into());
                        return Task::none();
                    }
                },
            }
            let c = ctx.client.clone();
            let path = format!("/v1/owner/conversations/{}/draft", conv.id);
            client::task(move || c.patch::<Value>(&path, &body), Msg::Edited)
        }
        Msg::Edited(res) => {
            match res {
                Ok(data) => {
                    adopt(conv, data);
                    conv.edit = None;
                    conv.error = None;
                    conv.notice = Some(
                        "Draft saved: the orchestrator sees your change with your next message."
                            .into(),
                    );
                }
                Err(e) => conv.error = Some(e),
            }
            Task::none()
        }
    }
}

/// A hub event for the open conversation: an `owner_conversation` frame of it replaces it; a `delta` of one of its
/// pending answers adds to that answer's stream. Returns whether the conversation changed.
pub fn on_event(conv: &mut Conversation, kind: &str, data: &Value) -> bool {
    match kind {
        "owner_conversation" if data["id"].as_str() == Some(conv.id.as_str()) => {
            adopt(conv, data.clone());
            true
        }
        "delta" => {
            let (Some(cid), Some(work)) = (data["cid"].as_u64(), data["work"].as_u64()) else {
                return false;
            };
            let pending = conv.data.as_ref().map(pending_of).unwrap_or_default();
            if conv.cid() != Some(cid) || !pending.contains(&work) {
                return false;
            }
            let delta = &data["delta"];
            if delta["type"].as_str() != Some("text") {
                return false;
            }
            let text = delta["text"].as_str().unwrap_or("").to_string();
            match conv.deltas.last_mut() {
                Some((w, t)) if *w == work => t.push_str(&text),
                _ => conv.deltas.push((work, text)),
            }
            true
        }
        _ => false,
    }
}

// ---- Views -------------------------------------------------------------------------------------------------------

/// The owner's message: a bubble on the right.
fn owner_bubble<'a, M: 'a>(body: &str) -> Element<'a, M> {
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

/// A plaza notice (a project started, a task ended): a ruled line, its text faint.
fn notice_line<'a, M: 'a>(body: &str) -> Element<'a, M> {
    let p = theme::palette();
    container(ui::faint(body.to_string()))
        .padding([6, 12])
        .width(Length::Fill)
        .align_x(Alignment::Center)
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

/// The orchestrator's answer: who and when, then its text (and why it has none, if its turn failed).
fn answer<'a, M: 'a>(who: &str, when: String, body: &str, status: Option<&str>) -> Element<'a, M> {
    let p = theme::palette();
    let mut col = column![
        row![
            ui::sans(who.to_string(), 12.5, p.ink),
            ui::faint(when).size(11.5)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::markdown(body),
    ]
    .spacing(6);
    match status {
        Some("failed") => col = col.push(ui::error_line("This turn failed.")),
        Some("stopped") => col = col.push(ui::error_line("This turn was stopped.")),
        _ => {}
    }
    col.into()
}

/// The chat: the messages in a reading column, the answer streaming in, and the composer.
pub fn chat<'a, M: Clone + 'a>(
    conv: &'a Conversation,
    who: String,
    now_ms: u64,
    wrap: fn(Msg) -> M,
) -> Element<'a, M> {
    let p = theme::palette();
    let ago = |ms: u64| super::ide_model::ago(now_ms, ms);
    let mut log = column![].spacing(14);
    match &conv.data {
        None => {
            log = log.push(match &conv.error {
                Some(e) => ui::error_line(e),
                None => ui::loading(),
            })
        }
        Some(c) => {
            let title = c["title"].as_str().unwrap_or(&conv.id).to_string();
            let mut head = row![ui::sans(title, 16.0, p.ink)]
                .spacing(10)
                .align_y(Alignment::Center);
            if let Some(project) = c["project"].as_str() {
                head = head.push(ui::badge(
                    &format!("Project {}", scope::title(project)),
                    ui::Tone::Signal,
                ));
            }
            log = log.push(head);
            for m in c["messages"].as_array().into_iter().flatten() {
                let text = m["text"].as_str().unwrap_or("");
                let at = m["at_ms"].as_u64().unwrap_or(0);
                log = log.push(match m["from"].as_str() {
                    Some("owner") => owner_bubble(text),
                    Some("plaza") => notice_line(text),
                    _ => answer(&who, ago(at), text, m["status"].as_str()),
                });
            }
            let pending = pending_of(c);
            if !pending.is_empty() {
                let text = streamed(&conv.deltas, &pending);
                log = log.push(if text.is_empty() {
                    row![
                        ui::sans(who.clone(), 12.5, p.ink),
                        ui::faint("answering…").size(11.5)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .into()
                } else {
                    answer(&who, "answering…".into(), &text, None)
                });
            }
        }
    }
    let reading = container(log.max_width(760.0))
        .width(Length::Fill)
        .center_x(Length::Fill);
    let scroll = ui::scroll(container(reading).padding([12, 16]))
        .anchor_bottom()
        .height(Length::Fill);
    let archived = conv
        .data
        .as_ref()
        .is_some_and(|c| c["archived"].as_bool().unwrap_or(false));
    let editor = text_editor(&conv.composer)
        .placeholder("Write to the orchestrator. Enter sends, Shift+Enter starts a new line.")
        .height(Length::Fixed(88.0))
        .on_action(move |action| wrap(Msg::Compose(action)))
        .key_binding(move |press| {
            if press.key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
                && !press.modifiers.shift()
            {
                Some(text_editor::Binding::Custom(wrap(Msg::Send)))
            } else {
                text_editor::Binding::from_key_press(press)
            }
        });
    let can_send = !conv.sending
        && !archived
        && conv.data.is_some()
        && !conv.composer.text().trim().is_empty();
    let mut foot = row![
        ui::sans(format!("Sending to: {who}"), 11.5, p.ink_3),
        Space::new().width(Length::Fill),
        ui::btn(
            "Send",
            Some("send"),
            ui::Kind::Primary,
            can_send.then(|| wrap(Msg::Send))
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if archived {
        foot = foot.push(ui::faint("Archived: it takes no more messages."));
    }
    let mut composer = column![editor, foot].spacing(8);
    if let Some(e) = &conv.error {
        composer = composer.push(ui::error_line(e));
    }
    column![
        scroll,
        container(container(composer).max_width(760.0))
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding([8, 16]),
    ]
    .height(Length::Fill)
    .into()
}

/// The draft beside the chat: its title and revision, goal, repository, team, budget, settings and tasks, then
/// what stops Start (or Start), or once started the project and the way to it. `roster` names the bench agents.
pub fn draft<'a, M: Clone + 'a>(
    conv: &'a Conversation,
    roster: &[(u64, String)],
    now_ms: u64,
    wrap: fn(Msg) -> M,
    to_console: fn(shell::Msg) -> M,
) -> Element<'a, M> {
    let p = theme::palette();
    let Some(c) = &conv.data else {
        return ui::loading();
    };
    let d = &c["draft"];
    if !d.is_object() {
        return column![
            ui::sans("The project draft", 15.0, p.ink),
            ui::faint("As you talk, the orchestrator drafts the project here: its goal, its tasks, its team and its budget. You start it when it looks right."),
        ]
        .spacing(10)
        .into();
    }
    let mut col = column![].spacing(12);
    let by = if d["updated_by"].as_str() == Some("owner") {
        "your edit"
    } else {
        "the orchestrator"
    };
    col = col.push(
        column![
            ui::sans(d["title"].as_str().unwrap_or("").to_string(), 15.0, p.ink),
            ui::faint(format!(
                "revision {} · {by} · {}",
                d["revision"].as_u64().unwrap_or(0),
                super::ide_model::ago(now_ms, d["updated_ms"].as_u64().unwrap_or(0))
            ))
            .size(11.5),
        ]
        .spacing(4),
    );
    col = col.push(ui::prose(d["goal"].as_str().unwrap_or("").to_string()));
    let fact = |k: &str, v: String| -> Element<'a, M> {
        row![
            container(ui::label(k)).width(Length::Fixed(72.0)),
            ui::sans(v, 12.5, p.ink_2).width(Length::Fill)
        ]
        .spacing(8)
        .into()
    };
    if let Some(repo) = d["repo"].as_str().filter(|r| !r.is_empty()) {
        col = col.push(fact("Repo", repo.to_string()));
    }
    let team: Vec<Value> = d["team"].as_array().cloned().unwrap_or_default();
    col = col.push(fact(
        "Team",
        if team.is_empty() {
            "Not drafted yet".into()
        } else {
            team_line(&team, roster)
        },
    ));
    col = col.push(fact("Budget", budget_text(d)));
    let settings = settings_line(&d["settings"]);
    if !settings.is_empty() {
        col = col.push(fact("Settings", settings));
    }
    let tasks: Vec<Value> = d["tasks"].as_array().cloned().unwrap_or_default();
    col = col.push(ui::label(&format!("Tasks ({})", tasks.len())));
    if tasks.is_empty() {
        col = col.push(ui::faint("No tasks yet."));
    }
    for t in &tasks {
        let role = t["role"]
            .as_str()
            .map_or_else(|| "any role".to_string(), roles::label);
        let after = after_text(t);
        let meta = if after.is_empty() {
            role
        } else {
            format!("{role} · {after}")
        };
        col = col.push(
            row![
                container(ui::mono(
                    t["key"].as_str().unwrap_or("").to_string(),
                    11.5,
                    p.ink_3
                ))
                .width(Length::Fixed(22.0)),
                column![
                    ui::sans(t["goal"].as_str().unwrap_or("").to_string(), 12.5, p.ink),
                    ui::faint(meta).size(11.5)
                ]
                .spacing(2)
                .width(Length::Fill),
            ]
            .spacing(6),
        );
    }
    if let Some(n) = &conv.notice {
        col = col.push(ui::faint(n.clone()));
    }
    match c["project"].as_str().filter(|p| !p.is_empty()) {
        // Started: the project, and the way to it in the console.
        Some(project) => {
            let go = |to: Id| to_console(shell::Msg::Scope(Some(project.to_string()), Some(to)));
            col = col.push(
                column![
                    row![
                        ui::sans("Started as", 12.5, p.ink_2),
                        ui::text_link(&scope::title(project), go(Id::Project(project.to_string()))),
                    ]
                    .spacing(5)
                    .align_y(Alignment::Center),
                    ui::faint("This conversation is its line to the orchestrator."),
                    row![
                        ui::btn_sm(
                            "Open in Work",
                            Some("commit"),
                            ui::Kind::Secondary,
                            Some(go(Id::WorkOn(project.to_string())))
                        ),
                        ui::btn_sm(
                            "Its agents",
                            Some("agents"),
                            ui::Kind::Secondary,
                            Some(go(Id::Agents))
                        ),
                    ]
                    .spacing(8),
                ]
                .spacing(8),
            );
        }
        None => {
            if let Some(edit) = &conv.edit {
                col = col.push(edit_form(edit, wrap));
            }
            let block = start_block(c);
            col = col.push(match &block {
                Some(why) => ui::error_line(why),
                None => ui::faint("Nothing stops Start.").into(),
            });
            let start = (block.is_none() && !conv.starting).then(|| wrap(Msg::Start));
            col = col.push(
                row![
                    ui::btn_sm(
                        "Edit…",
                        None,
                        ui::Kind::Ghost,
                        (conv.edit.is_none()).then(|| wrap(Msg::Edit))
                    ),
                    Space::new().width(Length::Fill),
                    ui::btn(
                        if conv.starting {
                            "Starting…"
                        } else {
                            "Start project"
                        },
                        Some("play"),
                        ui::Kind::Primary,
                        start
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
    }
    ui::scroll(container(col).padding(Padding {
        top: 4.0,
        right: 10.0,
        bottom: 12.0,
        left: 4.0,
    }))
    .height(Length::Fill)
    .into()
}

/// The owner's edit of the draft: its words, where the work goes, and its budget.
fn edit_form<'a, M: Clone + 'a>(edit: &'a DraftEdit, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let field = |label: &str, value: &'a str, placeholder: &str, which: Field| -> Element<'a, M> {
        column![
            ui::label(label),
            text_input(placeholder, value)
                .on_input(move |v| wrap(Msg::EditField(which, v)))
                .padding(6)
                .style(theme::input),
        ]
        .spacing(4)
        .into()
    };
    ui::card(
        "Edit the draft",
        column![
            field("Title", &edit.title, "Release 0.9", Field::Title),
            field("Goal", &edit.goal, "What the project delivers", Field::Goal),
            field(
                "Where the work goes",
                &edit.repo,
                "agentplaza, a new branch release-0.9",
                Field::Repo
            ),
            field(
                "Budget (credits)",
                &edit.budget,
                "From the plaza's budget",
                Field::Budget
            ),
            row![
                ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(wrap(Msg::CancelEdit))),
                ui::btn_sm("Save", None, ui::Kind::Primary, Some(wrap(Msg::SaveEdit))),
            ]
            .spacing(8),
        ]
        .spacing(10)
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<(u64, String)> {
        vec![(300, "Ada".into()), (304, "Rhea".into())]
    }

    #[test]
    fn the_drafts_words() {
        assert!(is_conversation_id("conv-12"));
        assert!(
            !is_conversation_id("agent:7")
                && !is_conversation_id("conv-")
                && !is_conversation_id("conv-1x")
        );
        assert_eq!(
            seat_line(
                &json!({ "role": "builder", "count": 2, "bench": [300] }),
                &roster()
            ),
            "2 Builder (Ada from the bench, 1 new)"
        );
        assert_eq!(
            seat_line(
                &json!({ "role": "librarian", "count": 1, "bench": [304] }),
                &roster()
            ),
            "1 Librarian (Rhea from the bench)"
        );
        assert_eq!(
            seat_line(&json!({ "role": "qa", "count": 1 }), &roster()),
            "1 QA (1 new)"
        );
        assert_eq!(
            seat_line(
                &json!({ "role": "qa", "count": 1, "bench": [999] }),
                &roster()
            ),
            "1 QA (agent 999 from the bench)"
        );
        assert_eq!(
            team_line(
                &[
                    json!({ "role": "qa", "count": 1 }),
                    json!({ "role": "release", "count": 1 })
                ],
                &roster()
            ),
            "1 QA (1 new) · 1 Release (1 new)"
        );
        assert_eq!(after_text(&json!({ "after": ["1", "2"] })), "after 1, 2");
        assert_eq!(after_text(&json!({})), "");
        assert_eq!(budget_text(&json!({ "budget": 15000 })), "15,000 credits");
        assert_eq!(budget_text(&json!({})), "From the plaza's budget");
        assert_eq!(
            settings_line(
                &json!({ "role_models": { "builder": "anthropic/claude-sonnet-5-5" }, "effort": "high", "permission_mode": "ask" })
            ),
            "Builder model: anthropic/claude-sonnet-5-5 · effort high · permission: ask"
        );
        assert_eq!(settings_line(&Value::Null), "");
    }

    #[test]
    fn what_stops_start() {
        let ready = json!({ "draft": { "problems": [] } });
        assert_eq!(start_block(&ready), None);
        assert_eq!(
            start_block(&json!({ "draft": { "problems": ["the draft has no tasks yet"] } }))
                .as_deref(),
            Some("Before it can start: the draft has no tasks yet.")
        );
        assert_eq!(
            start_block(&json!({ "project": "pr2", "draft": { "problems": [] } })).as_deref(),
            Some("The project is started.")
        );
        assert!(start_block(&json!({})).is_some_and(|w| w.starts_with("There is no draft yet")));
    }

    #[test]
    fn an_answer_streams_until_it_arrives() {
        let mut conv = Conversation::new("conv-1".into());
        conv.data = Some(json!({ "id": "conv-1", "cid": 109, "pending": [4] }));
        let delta = |work: u64, kind: &str, text: &str| json!({ "cid": 109, "work": work, "delta": { "type": kind, "text": text } });
        assert!(on_event(&mut conv, "delta", &delta(4, "text", "Which ")));
        assert!(
            !on_event(&mut conv, "delta", &delta(4, "thinking", "hmm")),
            "thinking is not shown"
        );
        assert!(
            !on_event(&mut conv, "delta", &delta(5, "text", "x")),
            "another answer's"
        );
        assert!(on_event(&mut conv, "delta", &delta(4, "text", "repo?")));
        assert_eq!(streamed(&conv.deltas, &[4]), "Which repo?");
        // The answer lands: its stream ends.
        let answered = json!({ "id": "conv-1", "cid": 109, "pending": [] });
        assert!(on_event(&mut conv, "owner_conversation", &answered));
        assert!(conv.deltas.is_empty());
        assert!(!on_event(
            &mut conv,
            "owner_conversation",
            &json!({ "id": "conv-2" })
        ));
    }
}
