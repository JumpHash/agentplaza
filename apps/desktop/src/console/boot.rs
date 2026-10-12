//! The startup chooser: how this app is used on this machine.
//!
//! - **Start locally**: hub and one node here (loopback, no owner token needed).
//! - **Host a hub**: a hub other machines join — bound to an address, HTTPS, owner token, join codes.
//! - **Join a hub**: this machine becomes a node of another hub (paste a join token).
//! - **Connect to a hub**: open the console of a hub elsewhere (its URL and the owner token).
//! The choice is remembered (`~/.config/agentplaza/desktop.json`) and offered first next time.

use std::time::Duration;

use iced::widget::{button, column, container, row, text_input, Space};
use iced::{Alignment, Border, Color, Element, Length, Subscription, Task, Theme};

use super::client::{self, Client};
use super::services::{self, Mode};
use super::ui::{self, Kind, Tone};
use crate::theme::HOVER;

/// The hub the local services run; the chooser polls it until it answers.
const LOCAL_HUB: &str = "http://127.0.0.1:7700";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    Local,
    Host,
    Join,
    Connect,
}

impl Choice {
    fn title(self) -> &'static str {
        match self {
            Choice::Local => "Start locally",
            Choice::Host => "Host a hub",
            Choice::Join => "Join a hub",
            Choice::Connect => "Connect to a hub",
        }
    }

    fn sentence(self) -> &'static str {
        match self {
            Choice::Local => "The hub and one node on this machine. Nothing leaves it.",
            Choice::Host => "Other machines join this one as nodes with one-time codes.",
            Choice::Join => "This machine becomes a node of a hub elsewhere.",
            Choice::Connect => "Open the console of a hub elsewhere.",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Choice::Local => "play",
            Choice::Host => "machine",
            Choice::Join => "link",
            Choice::Connect => "external",
        }
    }
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
    /// The local services, listed on the "Start locally" form.
    pub services: Vec<services::Service>,
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

/// The remembered choice, as stored in `desktop.json`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Remembered {
    choice: Choice,
}

pub fn init() -> (State, Task<Msg>) {
    (
        State {
            bind: "0.0.0.0".into(),
            url: LOCAL_HUB.into(),
            remembered: load_remembered(),
            ..Default::default()
        },
        Task::none(),
    )
}

/// Returns a task and, once connected, the client the console uses.
pub fn update(state: &mut State, msg: Msg) -> (Task<Msg>, Option<Client>) {
    match msg {
        Msg::Pick(choice) => {
            if state.busy {
                return (Task::none(), None);
            }
            state.choice = Some(choice);
            state.error = None;
            state.services = if choice == Choice::Local { services::status(Mode::Local) } else { Vec::new() };
            (Task::none(), None)
        }
        Msg::Back => {
            if !state.busy {
                state.choice = None;
                state.error = None;
                state.log.clear();
            }
            (Task::none(), None)
        }
        Msg::Url(value) => {
            state.url = value;
            (Task::none(), None)
        }
        Msg::Token(value) => {
            state.token = value;
            (Task::none(), None)
        }
        Msg::JoinToken(value) => {
            state.join_token = value;
            (Task::none(), None)
        }
        Msg::Bind(value) => {
            state.bind = value;
            (Task::none(), None)
        }
        Msg::Go => go(state),
        Msg::Started(result) => started(state, result),
        Msg::Connected(result) => match result {
            Ok(client) => {
                state.busy = false;
                state.error = None;
                if let Some(choice) = state.choice {
                    // Remembering is a convenience: a failed write must not block the console.
                    let _ = save_remembered(choice);
                }
                (Task::none(), Some(client))
            }
            Err(error) => {
                state.busy = false;
                state.error = Some(error);
                (Task::none(), None)
            }
        },
    }
}

/// Runs the chosen option's action.
fn go(state: &mut State) -> (Task<Msg>, Option<Client>) {
    let Some(choice) = state.choice else {
        return (Task::none(), None);
    };
    if state.busy {
        return (Task::none(), None);
    }
    state.error = None;
    let task = match choice {
        Choice::Local => {
            state.busy = true;
            Task::perform(async { services::start(Mode::Local) }, Msg::Started)
        }
        // The bind address is kept on the form; services::start takes no address yet.
        Choice::Host => {
            state.busy = true;
            Task::perform(async { services::start(Mode::Hub) }, Msg::Started)
        }
        Choice::Join => {
            let token = state.join_token.trim().to_string();
            if token.is_empty() {
                state.error = Some("Paste the join token from the hub.".into());
                return (Task::none(), None);
            }
            state.busy = true;
            Task::perform(async move { join_node(&token) }, Msg::Started)
        }
        Choice::Connect => {
            let url = state.url.trim().to_string();
            let token = state.token.trim().to_string();
            if url.is_empty() || token.is_empty() {
                state.error = Some("Enter the hub's URL and its owner token.".into());
                return (Task::none(), None);
            }
            state.busy = true;
            client::task(move || check(Client::new(&url, None, Some(token))), Msg::Connected)
        }
    };
    (task, None)
}

/// Services came up (or not). Local and Host wait for their hub to answer; Join stops here.
fn started(state: &mut State, result: Result<Vec<String>, String>) -> (Task<Msg>, Option<Client>) {
    match result {
        Ok(lines) => {
            state.log.extend(lines);
            match state.choice {
                Some(Choice::Local) | Some(Choice::Host) => {
                    state.log.push(format!("waiting for the hub on {LOCAL_HUB}"));
                    (client::task(await_local_hub, Msg::Connected), None)
                }
                _ => {
                    state.busy = false;
                    (Task::none(), None)
                }
            }
        }
        Err(error) => {
            state.busy = false;
            state.error = Some(error);
            (Task::none(), None)
        }
    }
}

/// Starts the node as a member of another hub, returning the progress lines.
fn join_node(token: &str) -> Result<Vec<String>, String> {
    let node = services::join(token)?;
    let mut lines = vec![format!("joined as node {node}")];
    lines.extend(services::start(Mode::Node)?);
    Ok(lines)
}

/// Checks that the hub answers the owner's token; Ok carries the client back.
fn check(client: Client) -> Result<Client, String> {
    client.get::<serde_json::Value>("/v1/auth/check")?;
    Ok(client)
}

/// Polls the local hub up to 20 times, a second apart.
fn await_local_hub() -> Result<Client, String> {
    let client = Client::new(LOCAL_HUB, None, services::local_owner_token());
    let mut last = String::from("no answer");
    for attempt in 0..20 {
        match client.get::<serde_json::Value>("/v1/auth/check") {
            Ok(_) => return Ok(client),
            Err(error) => last = error,
        }
        if attempt < 19 {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Err(format!("the hub did not answer on {LOCAL_HUB}: {last}"))
}

pub fn view(state: &State) -> Element<'_, Msg> {
    let content: Element<'_, Msg> = match state.choice {
        None => chooser(state),
        Some(choice) => form(state, choice),
    };
    container(container(content).max_width(760.0).width(Length::Fill))
        .padding(48)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .style(ui::paper)
        .into()
}

/// The four option cards, with the remembered choice offered first.
fn chooser(state: &State) -> Element<'_, Msg> {
    let busy = state.busy;
    let mut page: Vec<Element<'_, Msg>> = vec![
        ui::mono("AgentPlaza", 13.0, ui::INK).into(),
        ui::mono("How do you want to run it?", 30.0, ui::INK).into(),
    ];
    if let Some(choice) = state.remembered {
        let label = format!("Continue: {}", choice.title());
        page.push(ui::btn(&label, Some(choice.icon()), Kind::Primary, (!busy).then_some(Msg::Pick(choice))));
    }
    page.push(
        column![
            row![option_card(Choice::Local, busy), option_card(Choice::Host, busy)].spacing(12),
            row![option_card(Choice::Join, busy), option_card(Choice::Connect, busy)].spacing(12),
        ]
        .spacing(12)
        .into(),
    );
    column(page).spacing(20).into()
}

/// One square option card: icon, title, one serif sentence. Clicking it picks the option.
fn option_card(choice: Choice, busy: bool) -> Element<'static, Msg> {
    let icon: Element<'static, Msg> = ui::icon(choice.icon(), 22.0, ui::INK);
    let body = column![icon, ui::mono(choice.title(), 14.0, ui::INK), ui::prose(choice.sentence())].spacing(12);
    let card = button(body)
        .width(Length::Fill)
        .height(Length::Fixed(180.0))
        .padding(18)
        .style(|_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let background: Color = if hovered { HOVER } else { ui::SHEET };
            button::Style {
                background: Some(background.into()),
                text_color: ui::INK,
                border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            }
        });
    match (!busy).then_some(Msg::Pick(choice)) {
        Some(msg) => card.on_press(msg).into(),
        None => card.into(),
    }
}

/// The chosen option's form, with its progress log and errors.
fn form(state: &State, choice: Choice) -> Element<'_, Msg> {
    let busy = state.busy;
    let head = row![
        column![ui::mono(choice.title(), 26.0, ui::INK), ui::faint(choice.sentence())].spacing(4),
        Space::new().width(Length::Fill),
        ui::btn("Back", None, Kind::Ghost, (!busy).then_some(Msg::Back)),
    ]
    .align_y(Alignment::Center);
    let mut page: Vec<Element<'_, Msg>> = vec![head.into()];
    match choice {
        Choice::Local => page.push(services_list(&state.services)),
        Choice::Host => {
            page.push(ui::label("Bind address").into());
            page.push(text_input("0.0.0.0", &state.bind).on_input(Msg::Bind).on_submit(Msg::Go).into());
            page.push(ui::faint("Other machines reach this hub at this machine's public address, on the port it shows once started.").into());
        }
        Choice::Join => {
            page.push(ui::label("Join token").into());
            page.push(text_input("paste the join token", &state.join_token).on_input(Msg::JoinToken).on_submit(Msg::Go).secure(true).into());
        }
        Choice::Connect => {
            page.push(ui::label("Hub URL").into());
            page.push(text_input("https://hub.example:7700", &state.url).on_input(Msg::Url).on_submit(Msg::Go).into());
            page.push(ui::label("Owner token").into());
            page.push(text_input("owner token", &state.token).on_input(Msg::Token).on_submit(Msg::Go).secure(true).into());
        }
    }
    let action = match choice {
        Choice::Local => "Start",
        Choice::Host => "Start hub",
        Choice::Join => "Join",
        Choice::Connect => "Connect",
    };
    page.push(ui::btn(action, Some(choice.icon()), Kind::Primary, (!busy).then_some(Msg::Go)));
    if busy {
        page.push(ui::loading());
    }
    if !state.log.is_empty() {
        page.push(log_lines(&state.log));
    }
    if let Some(error) = &state.error {
        page.push(ui::error_line(error));
    }
    column(page).spacing(14).into()
}

/// The local services and their state.
fn services_list(services: &[services::Service]) -> Element<'static, Msg> {
    if services.is_empty() {
        return ui::faint("No services are listed for this mode.").into();
    }
    let rows: Vec<Element<'static, Msg>> = services
        .iter()
        .map(|svc| {
            let tone = match svc.state.as_str() {
                "running" => Tone::Ok,
                "missing" => Tone::Warn,
                _ => Tone::Muted,
            };
            row![
                ui::mono(svc.name.clone(), 13.0, ui::INK),
                ui::faint(svc.detail.clone()),
                Space::new().width(Length::Fill),
                ui::badge(&svc.state, tone),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    column(rows).spacing(8).into()
}

/// The progress lines, mono and faint.
fn log_lines(lines: &[String]) -> Element<'static, Msg> {
    let rows: Vec<Element<'static, Msg>> = lines.iter().map(|line| ui::mono(line.clone(), 11.5, ui::INK_3).into()).collect();
    column(rows).spacing(2).into()
}

pub fn subscription(_state: &State) -> Subscription<Msg> {
    Subscription::none()
}

/// `desktop.json` in the plaza's config directory (`apz_os::dirs::config`).
fn config_path() -> Option<std::path::PathBuf> {
    Some(apz_os::dirs::config().join("desktop.json"))
}

fn encode_remembered(choice: Choice) -> String {
    serde_json::to_string(&Remembered { choice }).unwrap_or_default()
}

fn decode_remembered(text: &str) -> Option<Choice> {
    serde_json::from_str::<Remembered>(text).ok().map(|remembered| remembered.choice)
}

fn load_remembered() -> Option<Choice> {
    let text = std::fs::read_to_string(config_path()?).ok()?;
    decode_remembered(&text)
}

fn save_remembered(choice: Choice) -> std::io::Result<()> {
    let path = config_path().ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, encode_remembered(choice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_choice_round_trips() {
        for choice in [Choice::Local, Choice::Host, Choice::Join, Choice::Connect] {
            assert_eq!(decode_remembered(&encode_remembered(choice)), Some(choice));
        }
        assert_eq!(decode_remembered("not json"), None);
    }
}
