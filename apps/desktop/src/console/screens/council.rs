//! The Council screen (parity with the web console's `council` view): the motions the plaza's
//! roles vote on, their tallies and votes, and the owner's approve or reject on the ones that
//! wait for the owner.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, column, container, pick_list, row, text, text_editor};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, roles, ui};

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
const STATES: [&str; 8] = [
    "open",
    "no_quorum",
    "carried",
    "rejected",
    "tie_to_owner",
    "awaiting_owner",
    "ratified",
    "vetoed",
];

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
    pub reason: text_editor::Content,
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
    Reason(text_editor::Action),
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
            state.reason = text_editor::Content::new();
            state.decide_error = None;
            Task::none()
        }
        Msg::Reason(action) => {
            state.reason.perform(action);
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
            let body = decide_body(pending.verdict, &state.reason.text());
            let path = format!("/v1/motions/{}/decide", pending.id);
            let client = ctx.client.clone();
            client::task(move || client.post::<Value>(&path, &body), Msg::Decided)
        }
        Msg::Decided(Ok(_)) => {
            state.busy = false;
            state.pending = None;
            state.reason = text_editor::Content::new();
            load(state, ctx)
        }
        Msg::Decided(Err(error)) => {
            state.busy = false;
            state.decide_error = Some(error);
            Task::none()
        }
    }
}

/// One state the list can be narrowed to; the first choice is every state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StateChoice {
    value: Option<String>,
    label: String,
}

impl std::fmt::Display for StateChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    // The state filter sits in the head, as the web's `State` select does.
    let options: Vec<StateChoice> = std::iter::once(StateChoice {
        value: None,
        label: "All states".to_string(),
    })
    .chain(STATES.iter().map(|s| StateChoice {
        value: Some((*s).to_string()),
        label: s.replace('_', " "),
    }))
    .collect();
    let selected = options.iter().find(|c| c.value == state.filter).cloned();
    let filter = pick_list(options, selected, |c: StateChoice| Msg::Filter(c.value))
        .style(crate::theme::pick_list)
        .width(Length::Fixed(140.0));
    let head = ui::view_head("Council", vec![filter.into()]);

    let body: Element<'a, Msg> = match (&state.error, &state.motions) {
        (Some(error), _) => ui::empty("alert", "Council unreachable", error),
        (None, None) => ui::loading(),
        (None, Some(motions)) if motions.is_empty() => ui::empty(
            "council",
            "No motions",
            "Council seats propose and vote on changes to how the plaza runs.",
        ),
        (None, Some(motions)) => {
            let shown: Vec<&Motion> = motions
                .iter()
                .filter(|m| state.filter.as_deref().is_none_or(|f| m.state == f))
                .collect();
            if shown.is_empty() {
                ui::empty(
                    "council",
                    "No motions in this state",
                    "Pick another state to see the rest.",
                )
            } else {
                column(shown.into_iter().map(motion_card).collect::<Vec<_>>())
                    .spacing(12)
                    .width(Length::Fill)
                    .into()
            }
        }
    };
    // The owner's decision is a dialog over the screen, as the web opens its `modal()`.
    let dialog = state.pending.as_ref().and_then(|pending| {
        let motion = state
            .motions
            .as_ref()?
            .iter()
            .find(|m| m.id == pending.id)?;
        Some(decide_dialog(state, pending, motion))
    });
    ui::with_modal(ui::sheet(head, body), dialog)
}

/// The decision dialog (the web's `decide()`): the motion, the reason, and Cancel with the verdict button.
fn decide_dialog<'a>(state: &'a State, pending: &Pending, motion: &'a Motion) -> Element<'a, Msg> {
    let verdict = pending.verdict;
    let title = match verdict {
        Verdict::Approve => "Approve motion",
        Verdict::Reject => "Reject motion",
    };
    let reason = text_editor(&state.reason)
        .style(crate::theme::editor)
        .placeholder("Reason")
        .height(Length::Fixed(96.0))
        .padding(8.0)
        .on_action(Msg::Reason);
    let mut body = column![
        ui::faint(motion.text.clone()),
        text("Reason")
            .size(12.0)
            .color(crate::theme::palette().ink_3),
        reason,
    ]
    .spacing(8)
    .width(Length::Fill);
    if let Some(error) = &state.decide_error {
        body = body.push(ui::error_line(error));
    }
    let submit = ui::btn_sm(
        verdict.verb(),
        None,
        verdict.kind(),
        (!state.busy).then_some(Msg::Submit),
    );
    let foot = vec![
        ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::Cancel)),
        submit,
    ];
    ui::modal(title, body.into(), foot, Msg::Cancel, 460.0)
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

/// A relative time since `ms` (the web's `ago`); "—" for no time.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(ms, |d| d.as_millis() as f64);
    let s = ((now - ms) / 1000.0).round().max(0.0);
    if s < 45.0 {
        "just now".to_string()
    } else if s < 3600.0 {
        format!("{} min ago", (s / 60.0).round().max(1.0))
    } else if s < 86400.0 {
        format!("{} h ago", (s / 3600.0).round())
    } else {
        format!("{} d ago", (s / 86400.0).round())
    }
}

/// A card's surface for a motion (the web's `.cn-card`): the sheet, ruled, the skin's large radius, no shadow.
fn card_surface(_: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        ..container::Style::default()
    }
}

/// A part of the tally bar: `portion` thousandths of its width, filled in `colour`.
fn tally_part<'a>(portion: u16, colour: Color) -> Element<'a, Msg> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::FillPortion(portion))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(colour.into()),
            ..Default::default()
        })
        .into()
}

/// The tally bar (the web's `.cn-tally`): yes, no, abstain, each as its share of the seats, on the rule's track.
fn tally_bar<'a>(t: &Tally) -> Element<'a, Msg> {
    let share = |n: u32| -> u16 {
        if t.seats == 0 {
            0
        } else {
            ((n as f32 / t.seats as f32) * 1000.0).round().min(1000.0) as u16
        }
    };
    let parts = [
        (share(t.yes), crate::theme::palette().ok),
        (share(t.no), crate::theme::palette().crit),
        (share(t.abstain), crate::theme::palette().ink_3),
    ];
    let used: u16 = parts.iter().map(|(n, _)| *n).sum::<u16>().min(1000);
    let mut bar = row![].width(Length::Fill).height(Length::Fill);
    for (n, colour) in parts {
        if n > 0 {
            bar = bar.push(tally_part(n, colour));
        }
    }
    if used < 1000 {
        bar = bar.push(
            Space::new()
                .width(Length::FillPortion(1000 - used))
                .height(Length::Fill),
        );
    }
    container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(12.0))
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            border: Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// One seat's vote (the web's `.cn-vote`): the seat's role, the vote as a chip, the reason; above a rule.
fn vote_row<'a>(v: &Vote) -> Element<'a, Msg> {
    let line = row![
        container(ui::faint(roles::label(&v.seat))).width(Length::Fixed(160.0)),
        ui::badge(&v.vote, vote_tone(&v.vote)),
        container(ui::sans(
            v.reason.clone(),
            12.5,
            crate::theme::palette().ink,
        ))
        .width(Length::Fill),
    ]
    .spacing(8)
    .width(Length::Fill)
    .align_y(Alignment::Center);
    column![
        ui::hairline(),
        container(line).padding([8.0, 0.0]).width(Length::Fill),
    ]
    .into()
}

fn motion_card<'a>(m: &Motion) -> Element<'a, Msg> {
    let t = &m.tally;
    let p = crate::theme::palette();
    let head = row![
        ui::badge(&m.kind, ui::Tone::Signal),
        ui::badge(&m.state, state_tone(&m.state)),
        Space::new().width(Length::Fill),
        ui::faint(ago(m.created_at)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut stack = column![
        head,
        text(m.text.clone())
            .font(crate::theme::SANS_SEMIBOLD)
            .size(14.0)
            .color(p.ink)
            .width(Length::Fill),
        tally_bar(t),
        ui::sans(
            format!(
                "{} yes · {} no · {} abstain · {}/{}",
                t.yes, t.no, t.abstain, t.voted, t.seats
            ),
            12.5,
            p.ink_2,
        ),
        ui::faint(format!("Quorum: {}", m.quorum.rule)),
    ]
    .spacing(8)
    .width(Length::Fill);

    if !m.votes.is_empty() {
        stack = stack
            .push(column(m.votes.iter().map(vote_row).collect::<Vec<_>>()).width(Length::Fill));
    }

    if needs_owner(&m.state) {
        stack = stack.push(
            row![
                Space::new().width(Length::Fill),
                ui::btn_sm(
                    "Reject",
                    Some("x"),
                    ui::Kind::Danger,
                    Some(Msg::Open(m.id.clone(), Verdict::Reject))
                ),
                ui::btn_sm(
                    "Approve",
                    Some("check"),
                    ui::Kind::Primary,
                    Some(Msg::Open(m.id.clone(), Verdict::Approve))
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    container(stack)
        .padding(16)
        .width(Length::Fill)
        .style(card_surface)
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
        assert_eq!(
            decide_body(Verdict::Approve, "fine").to_string(),
            r#"{"approve":true,"note":"fine"}"#
        );
        assert_eq!(
            decide_body(Verdict::Reject, "  no  ").to_string(),
            r#"{"approve":false,"note":"no"}"#
        );
        assert_eq!(
            decide_body(Verdict::Reject, "   ").to_string(),
            r#"{"approve":false,"note":null}"#
        );
    }

    #[test]
    fn hub_vote_and_motion_rows_parse_their_seat_agent_and_quorum_rule() {
        let vote: Vote = serde_json::from_value(json!({ "seat": "architect", "agent_cid": 42, "vote": "yes", "reason": "sound", "at": 1 })).unwrap();
        assert_eq!(
            (vote.seat.as_str(), vote.agent_cid, vote.vote.as_str()),
            ("architect", 42, "yes")
        );
        let motion: Motion = serde_json::from_value(json!({
            "id": "m1",
            "state": "open",
            "quorum": { "rule": "a motion is counted once 3 of the 4 filled seats have voted", "seats": 4, "filled": 4, "required": 3, "can_decide": true }
        }))
        .unwrap();
        assert!(motion.quorum.rule.contains("3 of the 4 filled seats"));
    }
}
