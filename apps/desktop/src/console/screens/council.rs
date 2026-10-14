//! The Council screen (parity with the web console's `council` view): the motions the plaza's
//! roles vote on, their tallies and votes, and the owner's approve or reject on the ones that
//! wait for the owner.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{column, container, row, text_input, Space};
use iced::{Alignment, Element, Length, Padding, Task, Theme};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme::{self};

/// A motion as the hub reports it (`GET /v1/motions`).
#[derive(Debug, Clone, Deserialize)]
pub struct Motion {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub tally: Tally,
    #[serde(default)]
    pub votes: Vec<Vote>,
    #[serde(default)]
    pub quorum: Quorum,
    /// Milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: f64,
}

/// A motion's quorum (`quorum` on the motion): the rule in words, with its numbers.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Quorum {
    #[serde(default)]
    pub rule: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Tally {
    #[serde(default)]
    pub yes: u32,
    #[serde(default)]
    pub no: u32,
    #[serde(default)]
    pub abstain: u32,
    #[serde(default)]
    pub voted: u32,
    #[serde(default)]
    pub seats: u32,
}

/// One seat's vote: the seat that cast it (its role), the agent that acted for it, the word and the reason.
#[derive(Debug, Clone, Deserialize)]
pub struct Vote {
    #[serde(default)]
    pub seat: String,
    #[serde(default)]
    pub agent_cid: u64,
    #[serde(default)]
    pub vote: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub motions: Vec<Motion>,
}

/// The owner's decision on a motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Reject,
}

impl Verdict {
    fn verb(self) -> &'static str {
        match self {
            Verdict::Approve => "Approve",
            Verdict::Reject => "Reject",
        }
    }

    fn kind(self) -> ui::Kind {
        match self {
            Verdict::Approve => ui::Kind::Primary,
            Verdict::Reject => ui::Kind::Danger,
        }
    }
}

/// The `POST /v1/motions/{id}/decide` body: `approve` as the hub reads it, and the note, or null
/// when the reason is blank (the hub refuses an empty note, but takes an absent one).
fn decide_body(verdict: Verdict, reason: &str) -> Value {
    let note = reason.trim();
    json!({ "approve": verdict == Verdict::Approve, "note": if note.is_empty() { Value::Null } else { json!(note) } })
}

/// The states the hub reports (`State::as_str`), in the order a motion moves through them.
const STATES: [&str; 8] = ["open", "no_quorum", "carried", "rejected", "tie_to_owner", "awaiting_owner", "ratified", "vetoed"];

/// The decision form that is open: which motion, and which verdict.
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: String,
    pub verdict: Verdict,
}

#[derive(Debug, Default)]
pub struct State {
    /// The motions as last loaded; None until the first load.
    pub motions: Option<Vec<Motion>>,
    /// The load error, shown in place of the list.
    pub error: Option<String>,
    /// The open decision form, if any.
    pub pending: Option<Pending>,
    pub reason: String,
    /// A decision is on its way to the hub.
    pub busy: bool,
    pub decide_error: Option<String>,
    /// The state the list shows; None shows every motion.
    pub filter: Option<String>,
    /// A live refresh is in flight; an event that came meanwhile runs once more when it lands.
    refreshing: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Envelope, String>),
    /// Open the decision form for a motion.
    Open(String, Verdict),
    Reason(String),
    Cancel,
    Submit,
    Decided(Result<Value, String>),
    /// Show only the motions in this state (None: all of them).
    Filter(Option<String>),
    /// A hub event: a motion change reloads the screen.
    Event(crate::console::events::Event),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || client.get::<Envelope>("/v1/motions"), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let landed = matches!(msg, Msg::Loaded(_));
    let task = apply(state, msg, ctx);
    if !landed {
        return task;
    }
    state.refreshing = false;
    if std::mem::take(&mut state.again) {
        state.refreshing = true;
        return Task::batch([task, load(state, ctx)]);
    }
    task
}

/// The screen's messages. `update` wraps it to keep the live refresh's in-flight flag.
fn apply(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Event(event) => {
            if !matches!(event.kind.as_str(), "motion") {
                return Task::none();
            }
            if state.refreshing {
                state.again = true;
                Task::none()
            } else {
                state.refreshing = true;
                load(state, ctx)
            }
        }
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(envelope)) => {
            state.motions = Some(envelope.motions);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Open(id, verdict) => {
            state.pending = Some(Pending { id, verdict });
            state.reason.clear();
            state.decide_error = None;
            Task::none()
        }
        Msg::Reason(reason) => {
            state.reason = reason;
            Task::none()
        }
        Msg::Cancel => {
            state.pending = None;
            state.decide_error = None;
            Task::none()
        }
        Msg::Filter(filter) => {
            state.filter = filter;
            Task::none()
        }
        Msg::Submit => {
            let Some(pending) = state.pending.clone() else {
                return Task::none();
            };
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.decide_error = None;
            let body = decide_body(pending.verdict, &state.reason);
            let path = format!("/v1/motions/{}/decide", pending.id);
            let client = ctx.client.clone();
            client::task(move || client.post::<Value>(&path, &body), Msg::Decided)
        }
        Msg::Decided(Ok(_)) => {
            state.busy = false;
            state.pending = None;
            state.reason.clear();
            load(state, ctx)
        }
        Msg::Decided(Err(error)) => {
            state.busy = false;
            state.decide_error = Some(error);
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match (&state.error, &state.motions) {
        (Some(error), _) => ui::empty("alert", "Council unreachable", error),
        (None, None) => ui::loading(),
        (None, Some(motions)) if motions.is_empty() => {
            ui::empty("council", "No motions", "Council seats propose and vote on changes to how the plaza runs.")
        }
        (None, Some(motions)) => {
            let shown: Vec<&Motion> = motions.iter().filter(|m| state.filter.as_deref().is_none_or(|f| m.state == f)).collect();
            let list: Element<'a, Msg> = if shown.is_empty() {
                ui::empty("council", "No motions in this state", "Pick another state to see the rest.")
            } else {
                column(shown.into_iter().map(|m| motion_card(m, state)).collect::<Vec<_>>()).spacing(14).width(Length::Fill).into()
            };
            column![filter_row(state.filter.as_deref()), list].spacing(14).width(Length::Fill).into()
        }
    };
    ui::sheet(ui::view_head("Council", vec![]), body)
}

/// The state filter: an "All" chip, then one chip per state; the chosen one is filled.
fn filter_row<'a>(active: Option<&str>) -> Element<'a, Msg> {
    let all = filter_chip("All", None, active);
    let states = STATES.iter().copied().map(|s| filter_chip(&s.replace('_', " "), Some(s), active));
    ui::chip_row(std::iter::once(all).chain(states).collect())
}

fn filter_chip<'a>(label: &str, value: Option<&str>, active: Option<&str>) -> Element<'a, Msg> {
    let kind = if value == active { ui::Kind::Primary } else { ui::Kind::Secondary };
    ui::btn_sm(label, None, kind, Some(Msg::Filter(value.map(str::to_string))))
}

/// Whether a motion waits for the owner's decision (the web's `needsOwner`).
fn needs_owner(state: &str) -> bool {
    matches!(state, "tie_to_owner" | "awaiting_owner")
}

/// The tone of a motion's state badge (the web's `stateTone`): explicit, so an unknown state is muted.
fn state_tone(state: &str) -> ui::Tone {
    match state {
        "open" => ui::Tone::Info,
        "carried" | "ratified" => ui::Tone::Ok,
        "rejected" | "vetoed" => ui::Tone::Danger,
        "tie_to_owner" | "awaiting_owner" => ui::Tone::Warn,
        _ => ui::Tone::Muted,
    }
}

fn vote_tone(vote: &str) -> ui::Tone {
    match vote {
        "yes" => ui::Tone::Ok,
        "no" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// The human label for a role (the web's `roleLabel`); an unknown role shows as its raw name.
fn role_label(role: &str) -> String {
    let label = match role {
        "orchestrator" => "Orchestrator",
        "architect" => "Architect",
        "auditor" => "Auditor",
        "breeder" => "Breeder",
        "builder" => "Builder",
        "edge-operator" => "Edge operator",
        "finance" => "Finance",
        "incident" => "Incident",
        "legal-agents-counsel" => "Legal · agents' counsel",
        "legal-agents-defender" => "Legal · agents' defender",
        "legal-agents-ombudsman" => "Legal · agents' ombudsman",
        "legal-gov-judge" => "Legal · government judge",
        "legal-gov-prosecutor" => "Legal · government prosecutor",
        "librarian" => "Librarian",
        "mayor" => "Mayor",
        "mayor-office" => "Mayor · office",
        "planner" => "Planner",
        "qa" => "QA",
        "release" => "Release",
        "researcher" => "Researcher",
        "reviewer" => "Reviewer",
        "security-reviewer" => "Security reviewer",
        other => return other.to_string(),
    };
    label.to_string()
}

/// A relative time since `ms` (the web's `ago`); "—" for no time.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
    let s = ((now - ms) / 1000.0).round().max(0.0) as u64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The tally bar: yes, no, abstain, then the seats not yet voted, each as its share of the seats.
fn tally_bar<'a>(t: &Tally) -> Element<'a, Msg> {
    let rest = t.seats.saturating_sub(t.yes + t.no + t.abstain);
    let mut bar = row![].width(Length::Fill).height(Length::Fixed(8.0));
    for (n, colour) in [(t.yes, crate::theme::palette().ok), (t.no, crate::theme::palette().crit), (t.abstain, crate::theme::palette().ink_3), (rest, crate::theme::palette().rule_faint)] {
        if n > 0 {
            bar = bar.push(
                container(Space::new())
                    .width(Length::FillPortion(n as u16))
                    .height(Length::Fill)
                    .style(move |_: &Theme| container::Style { background: Some(colour.into()), ..Default::default() }),
            );
        }
    }
    container(bar).width(Length::Fill).style(|_: &Theme| container::Style { background: Some(crate::theme::palette().rule_faint.into()), ..Default::default() }).into()
}

fn vote_row<'a>(v: &Vote) -> Element<'a, Msg> {
    row![
        ui::faint(role_label(&v.seat)),
        ui::badge(&v.vote, vote_tone(&v.vote)),
        container(ui::faint(v.reason.clone())).width(Length::Fill),
    ]
    .spacing(12)
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}

fn motion_card<'a>(m: &Motion, state: &'a State) -> Element<'a, Msg> {
    let t = &m.tally;
    let head = row![
        ui::badge(&m.kind, ui::Tone::Signal),
        ui::badge(&m.state, state_tone(&m.state)),
        Space::new().width(Length::Fill),
        ui::faint(ago(m.created_at)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut stack = column![
        container(ui::prose(m.text.clone())).max_width(620),
        tally_bar(t),
        ui::faint(format!("{} yes · {} no · {} abstain · {}/{}", t.yes, t.no, t.abstain, t.voted, t.seats)),
        ui::faint(format!("Quorum: {}", m.quorum.rule)),
    ]
    .spacing(12)
    .width(Length::Fill);

    if !m.votes.is_empty() {
        stack = stack.push(column(m.votes.iter().map(vote_row).collect::<Vec<_>>()).spacing(6).width(Length::Fill));
    }

    if needs_owner(&m.state) {
        stack = stack.push(
            row![
                Space::new().width(Length::Fill),
                ui::btn("Reject", Some("x"), ui::Kind::Danger, Some(Msg::Open(m.id.clone(), Verdict::Reject))),
                ui::btn("Approve", Some("check"), ui::Kind::Primary, Some(Msg::Open(m.id.clone(), Verdict::Approve))),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if let Some(pending) = state.pending.as_ref().filter(|p| p.id == m.id) {
            let input = text_input("Reason", &state.reason)
                .on_input(Msg::Reason)
                .on_submit(Msg::Submit)
                .style(theme::input);
            let submit = ui::btn(pending.verdict.verb(), None, pending.verdict.kind(), (!state.busy).then_some(Msg::Submit));
            let mut form = column![
                ui::label(&format!("{} motion", pending.verdict.verb())),
                input,
                row![Space::new().width(Length::Fill), ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Cancel)), submit]
                    .spacing(8)
                    .align_y(Alignment::Center),
            ]
            .spacing(10)
            .width(Length::Fill);
            if let Some(error) = &state.decide_error {
                form = form.push(ui::error_line(error));
            }
            stack = stack.push(form);
        }
    }

    container(column![
        container(head).padding(Padding { top: 12.0, right: 16.0, bottom: 10.0, left: 16.0 }).width(Length::Fill),
        ui::hairline(),
        container(stack).padding(16).width(Length::Fill),
    ])
    .width(Length::Fill)
    .style(ui::frame(None, crate::theme::palette().rule))
    .into()
}

pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_body_serialises_approve_and_note_as_the_hub_reads_them() {
        assert_eq!(decide_body(Verdict::Approve, "fine").to_string(), r#"{"approve":true,"note":"fine"}"#);
        assert_eq!(decide_body(Verdict::Reject, "  no  ").to_string(), r#"{"approve":false,"note":"no"}"#);
        assert_eq!(decide_body(Verdict::Reject, "   ").to_string(), r#"{"approve":false,"note":null}"#);
    }

    #[test]
    fn a_vote_row_parses_its_seat_and_agent() {
        let vote: Vote = serde_json::from_value(json!({ "seat": "architect", "agent_cid": 42, "vote": "yes", "reason": "sound", "at": 1 })).unwrap();
        assert_eq!(vote.seat, "architect");
        assert_eq!(vote.agent_cid, 42);
        assert_eq!(vote.vote, "yes");
    }

    #[test]
    fn a_motion_parses_its_quorum_rule() {
        let motion: Motion = serde_json::from_value(json!({
            "id": "m1",
            "state": "open",
            "quorum": { "rule": "a motion is counted once 3 of the 4 filled seats have voted", "seats": 4, "filled": 4, "required": 3, "can_decide": true }
        }))
        .unwrap();
        assert!(motion.quorum.rule.contains("3 of the 4 filled seats"));
    }

    #[test]
    fn owner_states_wait_for_the_owner_and_are_toned_explicitly() {
        assert!(needs_owner("awaiting_owner"));
        assert!(needs_owner("tie_to_owner"));
        assert!(!needs_owner("ratified"));
        assert!(!needs_owner("no_quorum"));
    }
}
