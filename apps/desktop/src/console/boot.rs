//! The startup chooser: how this app is used on this machine.
//!
//! - **Start locally**: hub and one node here (loopback, no owner token needed).
//! - **Host a hub**: a hub other machines join — bound to an address, HTTPS, owner token, join codes.
//! - **Join a hub**: this machine becomes a node of another hub (paste a join code).
//! - **Connect to a hub**: open the console of a hub elsewhere (its URL and the owner token).
//!
//! The choice is remembered (`~/.config/agentplaza/desktop.json`) and offered first next time.
//!
//! Start locally and Host a hub first check every requirement (`setup`), and install or set up
//! what is missing only after the user agrees to each step.

use std::time::Duration;

use iced::widget::{Space, button, column, container, row, text_input};
use iced::{Alignment, Border, Color, Element, Length, Subscription, Task, Theme};

use apz_trust::token::JoinToken;

use super::cli_link::{self, CliLink};
use super::client::{self, Client};
use super::hubs;
use super::services::{self, Mode};
use super::setup;
use super::ui::{self, Kind, Tone};
use super::workstation_setup;

/// The hub the local services run; the chooser polls it until it answers.
pub const LOCAL_HUB: &str = "http://127.0.0.1:7700";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    Local,
    Host,
    Join,
    Workstation,
    Connect,
}

impl Choice {
    fn title(self) -> &'static str {
        match self {
            Choice::Local => "Start locally",
            Choice::Host => "Host a hub",
            Choice::Join => "Join a hub",
            Choice::Workstation => "Add as a workstation",
            Choice::Connect => "Connect to a hub",
        }
    }

    fn sentence(self) -> &'static str {
        match self {
            Choice::Local => "The hub and one node on this machine. Nothing leaves it.",
            Choice::Host => "Other machines join this one as nodes with one-time codes.",
            Choice::Join => {
                "This machine becomes a node of a hub elsewhere, with a one-time code from that hub."
            }
            Choice::Workstation => {
                "The agents work this machine's screen, shell, files and programs through a hub, with a join code from it."
            }
            Choice::Connect => "Open the console of a hub elsewhere.",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Choice::Local => "play",
            Choice::Host => "machine",
            Choice::Join => "link",
            Choice::Workstation => "terminal",
            Choice::Connect => "external",
        }
    }
}

#[derive(Debug, Default)]
pub struct State {
    /// The option being set up (None = the four cards).
    pub choice: Option<Choice>,
    /// The Connect screen: the saved hubs, the hub URL and the owner token.
    pub hubs: hubs::State,
    pub join_token: String,
    pub bind: String,
    /// Progress lines from starting services.
    pub log: Vec<String>,
    pub error: Option<String>,
    pub busy: bool,
    /// The remembered choice, offered as "Continue".
    pub remembered: Option<Choice>,
    /// The requirements of Start locally and Host a hub, as last checked.
    pub checks: Vec<setup::Check>,
    /// The requirements are being checked.
    pub checking: bool,
    /// The requirement being installed or set up.
    pub fixing: Option<setup::Need>,
    /// The requirement whose fix waits for the user's go-ahead.
    pub confirm: Option<setup::Need>,
    /// Progress lines from installing and setting up requirements.
    pub setup_log: Vec<String>,
    /// The `agentplaza` command: offered on the chooser when it can be linked onto PATH here.
    pub cli: CliLink,
    /// The outcome of the last "Add" press, shown under the command row.
    pub cli_note: Option<String>,
    /// Add as a workstation: its form, its plan, its join and its result.
    pub ws: workstation_setup::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Pick(Choice),
    Back,
    Hubs(hubs::Msg),
    JoinToken(String),
    Bind(String),
    /// Start / connect for the chosen option.
    Go,
    /// Services started (lines) or failed.
    Started(Result<Vec<String>, String>),
    /// The hub answered (or not) — Ok carries the client to use.
    Connected(Result<Client, String>),
    /// Check the requirements again.
    Check,
    /// The requirements were checked.
    Checked(Vec<setup::Check>),
    /// Ask before installing or setting up this requirement.
    Ask(setup::Need),
    /// Drop the pending question.
    Cancel,
    /// Install or set up this requirement (the user agreed).
    Fix(setup::Need),
    /// The fix finished: its progress lines and the outcome.
    Fixed(setup::Need, Result<(), String>, Vec<String>),
    /// Link the `agentplaza` command onto PATH.
    LinkCli,
    /// The command was linked (or not).
    CliLinked(Result<(), String>),
    /// Add as a workstation: its form, plan, join and result.
    Workstation(workstation_setup::Msg),
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
            hubs: hubs::State::new(),
            remembered: load_remembered(),
            cli: cli_link::status(),
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
            state.confirm = None;
            if choice == Choice::Workstation {
                state.ws = workstation_setup::State::default();
            }
            match setup_mode(Some(choice)) {
                Some(mode) => {
                    state.checks.clear();
                    state.checking = true;
                    (check_task(mode), None)
                }
                None => (Task::none(), None),
            }
        }
        Msg::Back => {
            if !state.busy && state.fixing.is_none() && !state.ws.busy {
                state.choice = None;
                state.error = None;
                state.log.clear();
                state.confirm = None;
            }
            (Task::none(), None)
        }
        Msg::Hubs(msg) => {
            let (task, client) = hubs::update(&mut state.hubs, msg);
            (task.map(Msg::Hubs), client)
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
        Msg::Check => match setup_mode(state.choice) {
            Some(mode) if !state.checking && state.fixing.is_none() => {
                state.checking = true;
                (check_task(mode), None)
            }
            _ => (Task::none(), None),
        },
        Msg::Checked(checks) => {
            state.checks = checks;
            state.checking = false;
            (Task::none(), None)
        }
        Msg::Ask(need) => {
            if state.fixing.is_none() {
                state.confirm = Some(need);
            }
            (Task::none(), None)
        }
        Msg::Cancel => {
            state.confirm = None;
            (Task::none(), None)
        }
        Msg::Fix(need) => fix_requirement(state, need),
        Msg::Fixed(need, result, lines) => fix_done(state, need, result, lines),
        Msg::LinkCli => link_cli(state),
        Msg::CliLinked(result) => {
            state.cli_note = Some(match result {
                Ok(()) => "The agentplaza command is ready: open a new terminal.".into(),
                Err(error) => format!("Could not add the agentplaza command: {error}"),
            });
            state.cli = cli_link::status();
            (Task::none(), None)
        }
        Msg::Workstation(msg) => (
            workstation_setup::update(&mut state.ws, msg).map(Msg::Workstation),
            None,
        ),
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

/// The setup mode of an option that needs the requirement checks (Start locally, Host a hub).
fn setup_mode(choice: Option<Choice>) -> Option<setup::Mode> {
    match choice {
        Some(Choice::Local) => Some(setup::Mode::Local),
        Some(Choice::Host) => Some(setup::Mode::Hub),
        _ => None,
    }
}

/// Start may run: the requirements are checked, every required one passes, nothing is in progress.
fn can_start(state: &State) -> bool {
    !state.checking && state.fixing.is_none() && setup::ready(&state.checks)
}

/// Checks every requirement off the UI thread.
fn check_task(mode: setup::Mode) -> Task<Msg> {
    Task::perform(async move { setup::check_all(mode) }, Msg::Checked)
}

/// Runs the chosen option's action.
fn go(state: &mut State) -> (Task<Msg>, Option<Client>) {
    let Some(choice) = state.choice else {
        return (Task::none(), None);
    };
    if state.busy {
        return (Task::none(), None);
    }
    if setup_mode(Some(choice)).is_some() && !can_start(state) {
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
            let code = state.join_token.trim().to_string();
            if code.is_empty() {
                state.error = Some("Paste the join code from the hub's Nodes screen.".into());
                return (Task::none(), None);
            }
            if let Err(error) = join_code(&code) {
                state.error = Some(error);
                return (Task::none(), None);
            }
            state.busy = true;
            Task::perform(async move { join_node(&code) }, Msg::Started)
        }
        // The Connect screen (`hubs`) verifies its own sign-in; Add as a workstation has its own Join button.
        Choice::Connect | Choice::Workstation => return (Task::none(), None),
    };
    (task, None)
}

/// Installs or sets up one requirement off the UI thread, its progress lines kept for the log.
fn fix_requirement(state: &mut State, need: setup::Need) -> (Task<Msg>, Option<Client>) {
    if state.fixing.is_some() {
        return (Task::none(), None);
    }
    state.confirm = None;
    state.fixing = Some(need);
    let task = Task::perform(
        async move {
            let mut lines = Vec::new();
            let result = setup::fix(need, &mut |line: String| lines.push(line));
            (need, result, lines)
        },
        |(need, result, lines)| Msg::Fixed(need, result, lines),
    );
    (task, None)
}

/// A fix finished: its lines and the outcome go to the log, then the requirements are checked again.
fn fix_done(
    state: &mut State,
    need: setup::Need,
    result: Result<(), String>,
    lines: Vec<String>,
) -> (Task<Msg>, Option<Client>) {
    state.fixing = None;
    let label = state
        .checks
        .iter()
        .find(|check| check.need == need)
        .map_or("requirement", |check| check.label);
    state.setup_log.extend(lines);
    state.setup_log.push(match result {
        Ok(()) => format!("{label}: ✓ done"),
        Err(error) => format!("{label}: {error}"),
    });
    match setup_mode(state.choice) {
        Some(mode) => {
            state.checking = true;
            (check_task(mode), None)
        }
        None => (Task::none(), None),
    }
}

/// Links the `agentplaza` command in the background (macOS asks for a password).
fn link_cli(state: &mut State) -> (Task<Msg>, Option<Client>) {
    let CliLink::Linkable { target, link } = state.cli.clone() else {
        return (Task::none(), None);
    };
    state.cli_note = None;
    (
        Task::perform(
            async move { cli_link::link(&target, &link) },
            Msg::CliLinked,
        ),
        None,
    )
}

/// Services came up (or not). Local and Host wait for their hub to answer; Join stops here.
fn started(state: &mut State, result: Result<Vec<String>, String>) -> (Task<Msg>, Option<Client>) {
    match result {
        Ok(lines) => {
            state.log.extend(lines);
            match state.choice {
                Some(Choice::Local) | Some(Choice::Host) => {
                    state
                        .log
                        .push(format!("waiting for the hub on {LOCAL_HUB}"));
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

/// Decodes a join code and refuses one that has expired. The message is what the form shows.
fn join_code(text: &str) -> Result<JoinToken, String> {
    let token = JoinToken::decode(text)
        .map_err(|_| "Not a join code: it starts with apzj1.".to_string())?;
    if token.expires <= now_secs() {
        return Err("this code has expired: ask the hub for a new one".into());
    }
    Ok(token)
}

/// Unix seconds now (zero if the clock is before the epoch).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The live reading under the join code field: where it joins and how long it lasts, or why it cannot be used.
fn join_code_note<'a>(text: &str) -> Option<Element<'a, Msg>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(match join_code(text) {
        Ok(token) => {
            let minutes = token.expires.saturating_sub(now_secs()).div_ceil(60);
            ui::faint(format!(
                "Joins {} · expires in {minutes} min",
                token.hub_url
            ))
            .into()
        }
        Err(error) => ui::error_line(&error),
    })
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
        ui::sans("AgentPlaza", 13.0, crate::theme::palette().ink).into(),
        ui::sans(
            "How do you want to run it?",
            30.0,
            crate::theme::palette().ink,
        )
        .into(),
    ];
    if let Some(choice) = state.remembered {
        let label = format!("Continue: {}", choice.title());
        page.push(ui::btn(
            &label,
            Some(choice.icon()),
            Kind::Primary,
            (!busy).then_some(Msg::Pick(choice)),
        ));
    }
    page.push(
        column![
            row![
                option_card(Choice::Local, busy),
                option_card(Choice::Host, busy)
            ]
            .spacing(12),
            row![
                option_card(Choice::Join, busy),
                option_card(Choice::Workstation, busy)
            ]
            .spacing(12),
            row![
                option_card(Choice::Connect, busy),
                Space::new().width(Length::Fill)
            ]
            .spacing(12),
        ]
        .spacing(12)
        .into(),
    );
    if let CliLink::Linkable { link, .. } = &state.cli {
        let line = format!(
            "Command line: add the `agentplaza` command ({})",
            link.display()
        );
        page.push(
            row![
                ui::faint(line),
                Space::new().width(Length::Fill),
                ui::btn("Add", None, Kind::Ghost, (!busy).then_some(Msg::LinkCli)),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if let Some(note) = &state.cli_note {
        page.push(ui::faint(note.clone()).into());
    }
    column(page).spacing(20).into()
}

/// One square option card: icon, title, one serif sentence. Clicking it picks the option.
fn option_card(choice: Choice, busy: bool) -> Element<'static, Msg> {
    let icon: Element<'static, Msg> = ui::icon(choice.icon(), 22.0, crate::theme::palette().ink);
    let body = column![
        icon,
        ui::sans(choice.title(), 14.0, crate::theme::palette().ink),
        ui::prose(choice.sentence())
    ]
    .spacing(12);
    let card = button(body)
        .width(Length::Fill)
        .height(Length::Fixed(180.0))
        .padding(18)
        .style(|_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let background: Color = if hovered {
                crate::theme::palette().hover
            } else {
                crate::theme::palette().sheet
            };
            button::Style {
                background: Some(background.into()),
                text_color: crate::theme::palette().ink,
                border: Border {
                    color: crate::theme::palette().rule,
                    width: 1.0,
                    radius: 0.0.into(),
                },
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
    let ws_busy = choice == Choice::Workstation && state.ws.busy;
    let head = row![
        column![
            ui::sans(choice.title(), 26.0, crate::theme::palette().ink),
            ui::faint(choice.sentence())
        ]
        .spacing(4),
        Space::new().width(Length::Fill),
        ui::btn(
            "Back",
            None,
            Kind::Ghost,
            (!busy && !ws_busy && state.fixing.is_none()).then_some(Msg::Back)
        ),
    ]
    .align_y(Alignment::Center);
    let mut page: Vec<Element<'_, Msg>> = vec![head.into()];
    match choice {
        Choice::Local => page.push(requirements(state)),
        Choice::Host => {
            page.push(ui::label("Bind address").into());
            page.push(
                text_input("0.0.0.0", &state.bind)
                    .style(crate::theme::input)
                    .on_input(Msg::Bind)
                    .on_submit(Msg::Go)
                    .into(),
            );
            page.push(ui::faint("Other machines reach this hub at this machine's public address, on the port it shows once started.").into());
            page.push(requirements(state));
        }
        Choice::Join => {
            page.push(ui::label("Join code").into());
            page.push(ui::prose("On the hub, open Nodes and press “Add a node”: it shows a one-time join code (apzj1.…), valid for an hour. Paste it here. The code names the hub, so this machine connects to that hub and trusts only its certificate.").into());
            page.push(
                text_input("apzj1.…", &state.join_token)
                    .style(crate::theme::input)
                    .on_input(Msg::JoinToken)
                    .on_submit(Msg::Go)
                    .into(),
            );
            if let Some(note) = join_code_note(&state.join_token) {
                page.push(note);
            }
        }
        Choice::Connect => page.push(hubs::view(&state.hubs).map(Msg::Hubs)),
        Choice::Workstation => {
            page.push(workstation_setup::view(&state.ws).map(Msg::Workstation));
            return column(page).spacing(14).into();
        }
    }
    let action = match choice {
        Choice::Local => "Start",
        Choice::Host => "Start hub",
        Choice::Join => "Join",
        Choice::Workstation => "Join",
        Choice::Connect => "Connect",
    };
    let gated = setup_mode(Some(choice)).is_some();
    let ready = !gated || can_start(state);
    if !matches!(choice, Choice::Connect | Choice::Workstation) {
        page.push(ui::btn(
            action,
            Some(choice.icon()),
            Kind::Primary,
            (!busy && ready).then_some(Msg::Go),
        ));
    }
    if gated && !ready && !state.checking {
        page.push(ui::faint("Install or set up what is missing first.").into());
    }
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

/// The requirements of Start locally and Host a hub: one row each, with its state and, when it can
/// be installed or set up here, the button that asks first. Then the progress of the last fix.
fn requirements(state: &State) -> Element<'_, Msg> {
    let mut page: Vec<Element<'_, Msg>> = vec![ui::label("Requirements").into()];
    if state.checking {
        page.push(ui::faint("Checking what is installed...").into());
    }
    for check in &state.checks {
        let (stamp, tone) = match check.state {
            setup::State::Ok => ("OK", Tone::Ok),
            setup::State::Missing => ("MISSING", Tone::Warn),
            setup::State::Blocked => ("NEEDS YOU", Tone::Danger),
            setup::State::Warning => ("WARNING", Tone::Muted),
        };
        let action: Element<'_, Msg> = match (check.state, check.fix.is_some()) {
            (setup::State::Missing, true) => {
                let text = "Set up";
                ui::btn(
                    text,
                    None,
                    Kind::Ghost,
                    (state.fixing.is_none()).then_some(Msg::Ask(check.need)),
                )
            }
            _ => Space::new().into(),
        };
        let line = row![
            ui::sans(check.label, 13.0, crate::theme::palette().ink),
            ui::faint(check.detail.clone()),
            Space::new().width(Length::Fill),
            action,
            ui::badge(stamp, tone),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let mut block: Vec<Element<'_, Msg>> = vec![line.into()];
        if state.confirm == Some(check.need) {
            let buttons = row![
                ui::btn("Go ahead", None, Kind::Primary, Some(Msg::Fix(check.need))),
                ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Cancel)),
            ]
            .spacing(12);
            let question = check.fix.clone().unwrap_or_default();
            block.push(
                container(column![ui::prose(question), buttons].spacing(12))
                    .padding(14)
                    .width(Length::Fill)
                    .style(confirm_box)
                    .into(),
            );
        }
        page.push(column(block).spacing(8).into());
    }
    if let Some(need) = state.fixing {
        let label = state
            .checks
            .iter()
            .find(|check| check.need == need)
            .map_or("requirement", |check| check.label);
        page.push(ui::faint(format!("Working on {label}...")).into());
    }
    page.push(ui::btn(
        "Check again",
        None,
        Kind::Ghost,
        (!state.checking && state.fixing.is_none()).then_some(Msg::Check),
    ));
    if !state.setup_log.is_empty() {
        let start = state.setup_log.len().saturating_sub(12);
        page.push(log_lines(&state.setup_log[start..]));
    }
    column(page).spacing(10).into()
}

/// The box that asks before a fix runs: the same paper and rule as the option cards.
fn confirm_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// The progress lines, mono and faint.
fn log_lines(lines: &[String]) -> Element<'static, Msg> {
    let rows: Vec<Element<'static, Msg>> = lines
        .iter()
        .map(|line| ui::mono(line.clone(), 11.5, crate::theme::palette().ink_3).into())
        .collect();
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
    serde_json::from_str::<Remembered>(text)
        .ok()
        .map(|remembered| remembered.choice)
}

fn load_remembered() -> Option<Choice> {
    let text = std::fs::read_to_string(config_path()?).ok()?;
    decode_remembered(&text)
}

fn save_remembered(choice: Choice) -> std::io::Result<()> {
    let remembered: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&encode_remembered(choice)).unwrap_or_default();
    for (key, value) in remembered {
        save_setting(&key, value)?;
    }
    Ok(())
}

/// Sets one key of `desktop.json`, keeping the other keys (the remembered choice and the theme share the file).
pub fn save_setting(key: &str, value: serde_json::Value) -> std::io::Result<()> {
    let path = config_path().ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    let mut settings = read_settings(&path);
    settings.insert(key.to_string(), value);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::Value::Object(settings).to_string())
}

/// A key of `desktop.json`, if it is set.
pub fn saved_setting(key: &str) -> Option<serde_json::Value> {
    read_settings(&config_path()?).remove(key)
}

fn read_settings(path: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_choice_round_trips() {
        for choice in [
            Choice::Local,
            Choice::Host,
            Choice::Join,
            Choice::Workstation,
            Choice::Connect,
        ] {
            assert_eq!(decode_remembered(&encode_remembered(choice)), Some(choice));
        }
        assert_eq!(decode_remembered("not json"), None);
    }
}
