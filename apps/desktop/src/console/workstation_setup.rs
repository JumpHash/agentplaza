//! **Add as a workstation**, the startup chooser's option for a machine the agents work on through the hub.
//!
//! The form asks for the hub's address, the join code (made on the hub's Workstations screen), the name, the account
//! the agents work in, the certificate pin for a hub with its own certificate, and, for **Also run as a node** (one
//! install, both roles), the node's join code. **Show the plan** runs the dry run: the account and the service
//! commands, read from the machine and changed by nothing. **Join** runs the same library calls as
//! `agentplaza workstation join`, after the same administrator check. The result shows the station, its account, its
//! link and what the platform asks of the owner (macOS permissions, the Wayland portal).

use apz_trust::token::JoinToken;
use apz_workstation::account::{self, AccountChoice};
use apz_workstation::enrol::{self, Enrol};
use apz_workstation::facts::host_os;
use apz_workstation::plan;
use iced::widget::{checkbox, column, container, text_input};
use iced::{Element, Length, Task};

use super::services::{self, Mode};
use super::ui::{self, Kind};

/// The account the agents work in, as the form offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// A separate OS user for the agents: the default.
    Agent,
    /// The owner's own account: the agents act as the owner.
    Owner,
}

/// The form, the plan once shown, the join's progress, and its result.
#[derive(Debug, Clone)]
pub struct State {
    pub hub: String,
    pub code: String,
    pub name: String,
    pub agent_user: String,
    pub pick: Pick,
    pub pin: String,
    pub also_node: bool,
    pub node_code: String,
    /// The plan as the owner reads it. Any edit clears it, so Join always runs the plan that was shown.
    pub plan: Option<Vec<String>>,
    /// The dry run is running.
    pub planning: bool,
    /// The join is running.
    pub busy: bool,
    /// The join's progress lines.
    pub log: Vec<String>,
    pub error: Option<String>,
    /// The result of a join, once it finished.
    pub done: Option<Outcome>,
}

impl Default for State {
    fn default() -> Self {
        State {
            hub: String::new(),
            code: String::new(),
            name: String::new(),
            agent_user: account::DEFAULT_AGENT_USER.to_string(),
            pick: Pick::Agent,
            pin: String::new(),
            also_node: false,
            node_code: String::new(),
            plan: None,
            planning: false,
            busy: false,
            log: Vec::new(),
            error: None,
            done: None,
        }
    }
}

/// What a finished join reports.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub station: String,
    pub name: String,
    pub hub: String,
    pub account: String,
    pub link: String,
    pub node: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Hub(String),
    Code(String),
    Name(String),
    AgentUser(String),
    Pin(String),
    NodeCode(String),
    Pick(Pick),
    AlsoNode(bool),
    /// Run the dry run and show the plan.
    ShowPlan,
    /// The plan lines, or why there is none.
    Planned(Result<Vec<String>, String>),
    /// Join with the shown plan.
    Join,
    /// The join's outcome and its progress lines.
    Joined(Result<Outcome, String>, Vec<String>),
    /// Back to an empty form for another machine.
    Reset,
}

/// The form's values, checked.
#[derive(Debug, Clone)]
struct Params {
    hub: String,
    code: String,
    name: Option<String>,
    choice: AccountChoice,
    pin: Option<String>,
    node_code: Option<String>,
}

impl State {
    /// Any edit makes the shown plan stale.
    fn edited(&mut self) {
        self.plan = None;
        self.error = None;
    }
}

/// Checks the form and returns its values, or the first thing wrong with them.
fn params(state: &State) -> Result<Params, String> {
    let hub = state.hub.trim().trim_end_matches('/').to_string();
    if !(hub.starts_with("https://") || hub.starts_with("http://")) {
        return Err(
            "The hub address starts with https:// (or http:// for a hub on this machine).".into(),
        );
    }
    let code = state.code.trim().to_string();
    if code.is_empty() || code.chars().any(char::is_whitespace) {
        return Err("Paste the join code from the hub's Workstations screen (one word).".into());
    }
    let choice = match state.pick {
        Pick::Agent => AccountChoice::Agent {
            user: state.agent_user.trim().to_string(),
        },
        Pick::Owner => AccountChoice::Owner,
    };
    choice.validate(&account::current_user())?;
    let pin = Some(state.pin.trim().to_string()).filter(|p| !p.is_empty());
    if let Some(pin) = &pin {
        enrol::hex_pin_to_b64(pin)?;
    }
    let node_code = if state.also_node {
        let code = state.node_code.trim().to_string();
        JoinToken::decode(&code)
            .map_err(|_| "The node's join code starts with apzj1.".to_string())?;
        Some(code)
    } else {
        None
    };
    Ok(Params {
        hub,
        code,
        name: Some(state.name.trim().to_string()).filter(|n| !n.is_empty()),
        choice,
        pin,
        node_code,
    })
}

/// The dry run: the lines the owner reads before Join.
fn plan_lines(p: &Params) -> Result<Vec<String>, String> {
    let dir = enrol::default_dir()?;
    let plans = enrol::enrolment_plans(&dir, &p.choice)?;
    let mut lines = vec!["Joining the hub, then:".to_string()];
    for (title, plan) in &plans {
        lines.extend(plan::block(title, plan));
    }
    if p.node_code.is_some() {
        lines.push(
            "Also run as a node: the node's join code joins it to its hub after the workstation."
                .to_string(),
        );
    }
    if enrol::needs_admin(&plans) {
        lines.push("This needs administrator rights, and the app is not running as one: Join will not run until it can.".to_string());
    }
    Ok(lines)
}

/// The join, as `agentplaza workstation join` does it: the administrator check, the enrolment, the account, the
/// service, and the node when asked. Progress goes to `log`.
fn run_join(p: Params, log: &mut Vec<String>) -> Result<Outcome, String> {
    let dir = enrol::default_dir()?;
    let plans = enrol::enrolment_plans(&dir, &p.choice)?;
    if enrol::needs_admin(&plans) {
        return Err(
            "Adding the account and the service needs administrator rights. Nothing was joined. Run the app as an administrator, or run `agentplaza workstation join` as one."
                .to_string(),
        );
    }
    let record = enrol::join_here(
        &dir,
        &Enrol {
            hub: p.hub,
            code: p.code,
            name: p.name,
            account: p.choice.clone(),
            pin: p.pin,
        },
    )?;
    log.push(format!(
        "joined {} as station {} ({})",
        record.hub, record.station, record.name
    ));
    enrol::finish(&dir, &p.choice)?;
    log.push(match &p.choice {
        AccountChoice::Agent { user } => format!("account {user} is ready"),
        AccountChoice::Owner => "working as your own account".to_string(),
    });
    log.push("the workstation service is installed".to_string());
    let node = match &p.node_code {
        Some(code) => {
            let id = services::join(code)?;
            log.push(format!("joined as node {id}"));
            log.extend(services::start(Mode::Node)?);
            Some(id)
        }
        None => None,
    };
    let snapshot = enrol::status_here(&dir)?;
    Ok(Outcome {
        station: record.station,
        name: record.name,
        hub: record.hub,
        account: account::describe(&record.account),
        link: enrol::link_line(&snapshot),
        node,
        notes: enrol::permission_notes(host_os()),
    })
}

pub fn update(state: &mut State, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Joined(result, lines) => {
            state.busy = false;
            state.log = lines;
            match result {
                Ok(outcome) => state.done = Some(outcome),
                Err(error) => state.error = Some(error),
            }
            Task::none()
        }
        Msg::Planned(result) => {
            state.planning = false;
            match result {
                Ok(lines) => state.plan = Some(lines),
                Err(error) => state.error = Some(error),
            }
            Task::none()
        }
        _ if state.busy || state.planning => Task::none(),
        Msg::Hub(value) => {
            state.hub = value;
            state.edited();
            Task::none()
        }
        Msg::Code(value) => {
            state.code = value;
            state.edited();
            Task::none()
        }
        Msg::Name(value) => {
            state.name = value;
            state.edited();
            Task::none()
        }
        Msg::AgentUser(value) => {
            state.agent_user = value;
            state.edited();
            Task::none()
        }
        Msg::Pin(value) => {
            state.pin = value;
            state.edited();
            Task::none()
        }
        Msg::NodeCode(value) => {
            state.node_code = value;
            state.edited();
            Task::none()
        }
        Msg::Pick(pick) => {
            state.pick = pick;
            state.edited();
            Task::none()
        }
        Msg::AlsoNode(on) => {
            state.also_node = on;
            state.edited();
            Task::none()
        }
        Msg::ShowPlan => match params(state) {
            Ok(p) => {
                state.planning = true;
                state.error = None;
                Task::perform(async move { plan_lines(&p) }, Msg::Planned)
            }
            Err(error) => {
                state.error = Some(error);
                Task::none()
            }
        },
        Msg::Join => {
            if state.plan.is_none() {
                state.error = Some("Show the plan first: Join runs the plan you have read.".into());
                return Task::none();
            }
            match params(state) {
                Ok(p) => {
                    state.busy = true;
                    state.error = None;
                    state.log.clear();
                    Task::perform(
                        async move {
                            let mut log = Vec::new();
                            let result = run_join(p, &mut log);
                            (result, log)
                        },
                        |(result, log)| Msg::Joined(result, log),
                    )
                }
                Err(error) => {
                    state.error = Some(error);
                    Task::none()
                }
            }
        }
        Msg::Reset => {
            *state = State {
                hub: state.hub.clone(),
                ..State::default()
            };
            Task::none()
        }
    }
}

pub fn view(state: &State) -> Element<'_, Msg> {
    if let Some(done) = &state.done {
        return result_view(done);
    }
    let ink = crate::theme::palette().ink;
    let mut page: Vec<Element<'_, Msg>> = vec![
        ui::label("Hub address").into(),
        text_input("https://hub.example:7443", &state.hub)
            .on_input(Msg::Hub)
            .into(),
        ui::label("Join code").into(),
        ui::faint("On the hub, open Workstations and make a join code: it is shown once.").into(),
        text_input("join code", &state.code)
            .on_input(Msg::Code)
            .into(),
        ui::label("Name").into(),
        text_input("this machine's name", &state.name)
            .on_input(Msg::Name)
            .into(),
        ui::label("The account the agents work in").into(),
        ui::seg(
            &[
                (Pick::Agent, "A NEW USER FOR THE AGENTS"),
                (Pick::Owner, "MY OWN ACCOUNT"),
            ],
            state.pick,
            Msg::Pick,
        ),
    ];
    match state.pick {
        Pick::Agent => {
            page.push(
                text_input(account::DEFAULT_AGENT_USER, &state.agent_user)
                    .on_input(Msg::AgentUser)
                    .into(),
            );
            page.push(
                ui::faint("Created on this machine when you join, with its own home and work folder. Creating it asks for administrator rights.").into(),
            );
        }
        Pick::Owner => {
            let warning = account::owner_warning(&account::current_user());
            page.push(
                ui::faint(warning)
                    .color(crate::theme::palette().warn)
                    .into(),
            );
        }
    }
    page.push(ui::label("Certificate pin (optional)").into());
    page.push(
        text_input(
            "64 hex digits, for a hub with its own certificate",
            &state.pin,
        )
        .on_input(Msg::Pin)
        .into(),
    );
    page.push(
        checkbox(state.also_node)
            .label("Also run as a node (one install, both roles)")
            .on_toggle(Msg::AlsoNode)
            .into(),
    );
    if state.also_node {
        page.push(
            text_input("apzj1.…", &state.node_code)
                .on_input(Msg::NodeCode)
                .into(),
        );
        page.push(ui::faint("The node's own join code, from its hub's Nodes screen.").into());
    }
    let idle = !state.busy && !state.planning;
    page.push(ui::btn(
        "Show the plan",
        None,
        Kind::Secondary,
        idle.then_some(Msg::ShowPlan),
    ));
    if state.planning {
        page.push(ui::loading());
    }
    if let Some(lines) = &state.plan {
        let rows: Vec<Element<'_, Msg>> = lines
            .iter()
            .map(|line| ui::mono(line.clone(), 11.5, ink).into())
            .collect();
        page.push(ui::card("The plan", column(rows).spacing(2).into()));
        page.push(ui::btn(
            "Join",
            Some("terminal"),
            Kind::Primary,
            idle.then_some(Msg::Join),
        ));
    }
    if state.busy {
        page.push(ui::loading());
    }
    if !state.log.is_empty() {
        let rows: Vec<Element<'_, Msg>> = state
            .log
            .iter()
            .map(|line| ui::mono(line.clone(), 11.5, ink).into())
            .collect();
        page.push(column(rows).spacing(2).into());
    }
    if let Some(error) = &state.error {
        page.push(ui::error_line(error));
    }
    container(column(page).spacing(12))
        .width(Length::Fill)
        .into()
}

/// The result: the station, its account and link, and what the platform asks of the owner.
fn result_view(done: &Outcome) -> Element<'_, Msg> {
    let ink = crate::theme::palette().ink;
    let mut facts = vec![
        ui::prose(format!("Hub: {}", done.hub)).into(),
        ui::prose(format!("Station: {} ({})", done.station, done.name)).into(),
        ui::prose(format!("Working as: {}", done.account)).into(),
        ui::prose(format!("Link: {}", done.link)).into(),
    ];
    if let Some(node) = &done.node {
        facts.push(ui::prose(format!("Also a node: {node}")).into());
    }
    let notes: Vec<Element<'_, Msg>> = done
        .notes
        .iter()
        .map(|note| ui::prose(note.clone()).into())
        .collect();
    column![
        ui::mono("This machine is a workstation", 22.0, ink),
        ui::card("Station", column(facts).spacing(6).into()),
        ui::card(
            "What the platform asks of you",
            column(notes).spacing(8).into()
        ),
        ui::btn("Add another", None, Kind::Ghost, Some(Msg::Reset)),
    ]
    .spacing(14)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> State {
        State {
            hub: "https://hub.example:7443/".into(),
            code: "JOIN-9".into(),
            ..State::default()
        }
    }

    #[test]
    fn the_form_starts_with_the_separate_agentplaza_user() {
        let state = State::default();
        assert_eq!(state.pick, Pick::Agent);
        assert_eq!(state.agent_user, "agentplaza");
        assert!(!state.also_node);
    }

    #[test]
    fn a_complete_form_becomes_the_join_parameters() {
        let p = params(&filled()).unwrap();
        assert_eq!(p.hub, "https://hub.example:7443");
        assert_eq!(p.code, "JOIN-9");
        assert_eq!(
            p.choice,
            AccountChoice::Agent {
                user: "agentplaza".into()
            }
        );
        assert!(p.name.is_none());
        assert!(p.node_code.is_none());
    }

    #[test]
    fn a_hub_without_a_scheme_or_a_code_with_spaces_is_refused() {
        let mut state = filled();
        state.hub = "hub.example".into();
        assert!(params(&state).is_err());
        let mut state = filled();
        state.code = "two words".into();
        assert!(params(&state).is_err());
    }

    #[test]
    fn the_owner_choice_is_the_owners_own_account() {
        let mut state = filled();
        state.pick = Pick::Owner;
        assert_eq!(params(&state).unwrap().choice, AccountChoice::Owner);
    }

    #[test]
    fn a_bad_agent_user_or_pin_is_refused_before_anything_runs() {
        let mut state = filled();
        state.agent_user = "root".into();
        assert!(params(&state).is_err());
        let mut state = filled();
        state.pin = "abcd".into();
        assert!(params(&state).is_err());
    }

    #[test]
    fn an_edit_clears_the_shown_plan_so_join_runs_only_what_was_read() {
        let mut state = filled();
        state.plan = Some(vec!["x".into()]);
        let _ = update(&mut state, Msg::Code("JOIN-10".into()));
        assert!(state.plan.is_none());
    }

    #[test]
    fn join_without_a_shown_plan_does_nothing_but_say_so() {
        let mut state = filled();
        let _ = update(&mut state, Msg::Join);
        assert!(!state.busy);
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("Show the plan"))
        );
    }

    #[test]
    fn a_node_code_is_checked_only_when_the_node_option_is_on() {
        let mut state = filled();
        state.node_code = "not-a-code".into();
        assert!(params(&state).is_ok());
        state.also_node = true;
        assert!(params(&state).is_err());
    }
}
