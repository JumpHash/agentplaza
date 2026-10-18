//! New project (the web's `ui/new-project.ts`): talk it through with the orchestrator (a conversation, opened in the
//! IDE, where it drafts the project beside the chat and the owner starts it), or open one directly with a title and
//! a goal. The shell lays it over the screen; the top bar's switcher, the Projects screen and the IDE open it.

use iced::widget::{column, container, row, text_input};
use iced::{Border, Color, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::{Ctx, client, ui};
use crate::theme;

/// The dialog's fields, and the call in flight.
#[derive(Debug, Clone)]
pub struct Form {
    /// What the owner wants done: the conversation's first message.
    pub ask: String,
    pub title: String,
    pub goal: String,
    /// A code project gets a repository; the other kind does not.
    pub code: bool,
    pub busy: bool,
    pub error: Option<String>,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            ask: String::new(),
            title: String::new(),
            goal: String::new(),
            code: true,
            busy: false,
            error: None,
        }
    }
}

impl Form {
    /// The dialog with `ask` typed in (a capture hook, `APZ_NEW_PROJECT`).
    pub fn with_goal(ask: String) -> Form {
        Form {
            ask,
            ..Form::default()
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Ask(String),
    Title(String),
    Goal(String),
    /// The kind: code (true) or other.
    Code(bool),
    /// Start a conversation with the orchestrator, the goal its first message.
    Talk,
    /// Open the project directly.
    Open,
    Cancel,
    Talked(Result<Value, String>),
    Opened(Result<Value, String>),
}

/// What the shell does after a message: nothing more, close the dialog, open the new conversation in the IDE, or
/// scope the console to the new project and open its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Close,
    Conversation(String),
    Project(String),
}

pub fn update(form: &mut Form, msg: Msg, ctx: &Ctx) -> (Task<Msg>, Effect) {
    match msg {
        Msg::Ask(s) => form.ask = s,
        Msg::Title(s) => form.title = s,
        Msg::Goal(s) => form.goal = s,
        Msg::Code(code) => form.code = code,
        Msg::Cancel => return (Task::none(), Effect::Close),
        Msg::Talk => {
            let text = form.ask.trim().to_string();
            if text.is_empty() {
                form.error = Some("Say what you want done first.".into());
                return (Task::none(), Effect::None);
            }
            form.busy = true;
            form.error = None;
            let c = ctx.client.clone();
            let body = json!({ "text": text });
            return (
                client::task(
                    move || c.post::<Value>("/v1/owner/conversations", &body),
                    Msg::Talked,
                ),
                Effect::None,
            );
        }
        Msg::Open => {
            let (title, goal) = (form.title.trim().to_string(), form.goal.trim().to_string());
            if title.is_empty() || goal.is_empty() {
                form.error = Some("A project needs a title and a goal.".into());
                return (Task::none(), Effect::None);
            }
            form.busy = true;
            form.error = None;
            let c = ctx.client.clone();
            let body = json!({ "title": title, "goal": goal, "kind": if form.code { "code" } else { "other" } });
            return (
                client::task(move || c.post::<Value>("/v1/projects", &body), Msg::Opened),
                Effect::None,
            );
        }
        Msg::Talked(Err(e)) | Msg::Opened(Err(e)) => {
            form.busy = false;
            form.error = Some(e);
        }
        Msg::Talked(Ok(conversation)) => {
            form.busy = false;
            return match conversation["id"].as_str() {
                Some(id) => (Task::none(), Effect::Conversation(id.to_string())),
                None => {
                    form.error = Some("The hub answered without a conversation.".into());
                    (Task::none(), Effect::None)
                }
            };
        }
        Msg::Opened(Ok(project)) => {
            form.busy = false;
            return match project["id"].as_str() {
                Some(id) => (Task::none(), Effect::Project(id.to_string())),
                None => {
                    form.error = Some("The hub answered without a project.".into());
                    (Task::none(), Effect::None)
                }
            };
        }
    }
    (Task::none(), Effect::None)
}

/// The dialog, over a dimmed window.
pub fn view(form: &Form) -> Element<'_, Msg> {
    let p = theme::palette();
    let enabled = |msg: Msg| (!form.busy).then_some(msg);
    let talk = column![
        ui::sans("Talk it through", 14.0, p.ink),
        ui::faint("The orchestrator asks what it needs and drafts the project beside the conversation: its tasks, its team and its budget. You start it when it looks right."),
        ui::label("Your goal"),
        text_input(
            "What do you want done? “The scheduler tests are flaky and I want 0.9 out this week.”",
            &form.ask
        )
        .on_input(Msg::Ask)
        .on_submit(Msg::Talk)
        .padding(8)
        .style(theme::input),
        row![ui::btn(
            "Start a conversation",
            Some("conversations"),
            ui::Kind::Primary,
            enabled(Msg::Talk)
        )],
    ]
    .spacing(10);
    let kind = |code: bool, label: &str| {
        ui::btn(
            label,
            None,
            if form.code == code {
                ui::Kind::Primary
            } else {
                ui::Kind::Secondary
            },
            Some(Msg::Code(code)),
        )
    };
    let direct = column![
        ui::sans("Or open it directly", 14.0, p.ink),
        ui::faint("An empty project of yours: you put agents on it and give it tasks."),
        ui::label("Title"),
        text_input("Release 0.9", &form.title)
            .on_input(Msg::Title)
            .padding(8)
            .style(theme::input),
        ui::label("Goal"),
        text_input("What the project delivers", &form.goal)
            .on_input(Msg::Goal)
            .on_submit(Msg::Open)
            .padding(8)
            .style(theme::input),
        ui::label("Kind"),
        row![
            kind(true, "Code (it gets a repository)"),
            kind(false, "Other")
        ]
        .spacing(6),
        row![ui::btn(
            "Open project",
            None,
            ui::Kind::Secondary,
            enabled(Msg::Open)
        )],
    ]
    .spacing(10);
    let mut body = column![
        ui::sans("New project", 16.0, p.ink),
        talk,
        ui::hairline(),
        direct
    ]
    .spacing(16);
    if let Some(e) = &form.error {
        body = body.push(ui::error_line(e));
    }
    body = body.push(row![ui::btn(
        "Cancel",
        None,
        ui::Kind::Ghost,
        Some(Msg::Cancel)
    )]);
    let panel = container(body)
        .padding(20)
        .width(Length::Fixed(560.0))
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().ink,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(
                Color {
                    a: 0.5,
                    ..Color::BLACK
                }
                .into(),
            ),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            client: client::Client::new("http://127.0.0.1:9", None, None),
            project: None,
        }
    }

    #[test]
    fn the_dialog_asks_for_a_goal_or_a_title_and_goal_before_it_calls_the_hub() {
        let mut form = Form::default();
        let (_, effect) = update(&mut form, Msg::Talk, &ctx());
        assert_eq!(effect, Effect::None);
        assert_eq!(form.error.as_deref(), Some("Say what you want done first."));
        assert!(!form.busy);
        let _ = update(&mut form, Msg::Title("Release 0.9".into()), &ctx());
        let _ = update(&mut form, Msg::Open, &ctx());
        assert_eq!(
            form.error.as_deref(),
            Some("A project needs a title and a goal.")
        );
        assert!(!form.busy);
    }

    #[test]
    fn an_answer_opens_the_conversation_or_the_project() {
        let mut form = Form::default();
        let (_, effect) = update(
            &mut form,
            Msg::Talked(Ok(json!({ "id": "conv-4" }))),
            &ctx(),
        );
        assert_eq!(effect, Effect::Conversation("conv-4".into()));
        let (_, effect) = update(&mut form, Msg::Opened(Ok(json!({ "id": "pr7" }))), &ctx());
        assert_eq!(effect, Effect::Project("pr7".into()));
        let (_, effect) = update(&mut form, Msg::Opened(Err("no".into())), &ctx());
        assert_eq!(effect, Effect::None);
        assert_eq!(form.error.as_deref(), Some("no"));
        assert_eq!(update(&mut form, Msg::Cancel, &ctx()).1, Effect::Close);
    }
}
