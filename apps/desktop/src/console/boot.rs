//! The startup chooser: how this app is used on this machine.
//!
//! - **Start locally**: hub and one node here (loopback, no owner token needed).
//! - **Host a hub**: a hub other machines join — bound to an address, HTTPS, owner token, join codes.
//! - **Join a hub**: this machine becomes a node of another hub (paste a join token).
//! - **Connect to a hub**: open the console of a hub elsewhere (its URL and the owner token).
//! The choice is remembered (`~/.config/agentplaza/desktop.json`) and offered first next time.

use iced::widget::{column, container, row, text_input, Space};
use iced::{Element, Length, Subscription, Task};

use super::client::Client;
use super::ui::{self, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    Local,
    Host,
    Join,
    Connect,
}

#[derive(Debug, Default)]
pub struct State {
    /// The option being set up (None = the four cards).
    pub choice: Option<Choice>,
    pub url: String,
    pub token: String,
    pub join_token: String,
    pub bind: String,
    /// Progress lines from starting services.
    pub log: Vec<String>,
    pub error: Option<String>,
    pub busy: bool,
    /// The remembered choice, offered as "Continue".
    pub remembered: Option<Choice>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Pick(Choice),
    Back,
    Url(String),
    Token(String),
    JoinToken(String),
    Bind(String),
    /// Start / connect for the chosen option.
    Go,
    /// Services started (lines) or failed.
    Started(Result<Vec<String>, String>),
    /// The hub answered (or not) — Ok carries the client to use.
    Connected(Result<Client, String>),
}

pub fn init() -> (State, Task<Msg>) {
    (State { bind: "0.0.0.0".into(), url: "http://127.0.0.1:7700".into(), ..Default::default() }, Task::none())
}

/// Returns a task and, once connected, the client the console uses.
pub fn update(_state: &mut State, _msg: Msg) -> (Task<Msg>, Option<Client>) {
    (Task::none(), None) // contract: D1
}

pub fn view(_state: &State) -> Element<'_, Msg> {
    // contract: D1 — the four option cards (icon, title, one sentence), then the chosen option's
    // form, progress log and errors, in the console's design language.
    container(
        column![
            ui::mono("agentplaza", 22.0, ui::INK),
            ui::faint("How do you want to run AgentPlaza on this machine?"),
            Space::new().height(16),
            row![
                ui::btn("Start locally", Some("play"), Kind::Primary, Some(Msg::Pick(Choice::Local))),
                ui::btn("Host a hub", Some("machine"), Kind::Secondary, Some(Msg::Pick(Choice::Host))),
                ui::btn("Join a hub", Some("link"), Kind::Secondary, Some(Msg::Pick(Choice::Join))),
                ui::btn("Connect to a hub", Some("external"), Kind::Secondary, Some(Msg::Pick(Choice::Connect))),
            ]
            .spacing(10),
            text_input("", "").width(Length::Fixed(0.0)),
        ]
        .spacing(8),
    )
    .padding(48)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(ui::paper)
    .into()
}

pub fn subscription(_state: &State) -> Subscription<Msg> {
    Subscription::none()
}
