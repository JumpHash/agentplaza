//! One agent's page (parity with the web console's agent detail): the back link and actions, the
//! profile card, the transcript and chat (or the memory library), and the side column: model,
//! harness, permissions (`agent_side`), then details and queue. Restart, wipe and retire ask first;
//! cancel and stop act at once.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::font::Weight;
use iced::keyboard;
use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_editor, Space};
use iced::{Alignment, Border, Color, Element, Font, Length, Task, Theme};
use serde_json::{json, Value};

use super::{agent_side, Id};
use crate::console::floor3d::world::layout::{agent_hue, role_hue};
use crate::console::{client, ui, Ctx};
use crate::theme::{CRIT, HOVER, INFO, MONO, OK, SERIF, WARN};

/// The profile and infractions refresh at most this often; the transcript on every poll.
const EXTRAS_EVERY: Duration = Duration::from_secs(15);
const SIDE_WIDTH: f32 = 320.0;
const TRANSCRIPT_HEIGHT: f32 = 480.0;
/// The portrait's square, and the infractions shown before "Show all".
const PORTRAIT: f32 = 168.0;
const INFRACTIONS_SHOWN: usize = 10;

const MONO_SEMIBOLD: Font = Font { weight: Weight::Semibold, ..MONO };
const MONO_BOLD: Font = Font { weight: Weight::Bold, ..MONO };

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Transcript,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Restart,
    Wipe { memory: bool, transcript: bool, identity: bool },
    Retire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wipe {
    Memory,
    Transcript,
    Identity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Cancel,
    Stop,
}

#[derive(Debug, Default)]
pub struct State {
    pub cid: u64,
    pub error: Option<String>,
    /// The line a finished action leaves (the web's toast): "Restarting", "Stopped", …
    pub notice: Option<String>,
    pub agent: Option<Value>,
    pub profile: Option<Value>,
    pub infractions: Vec<Value>,
    pub infractions_all: bool,
    pub entries: Vec<Value>,
    pub last_seq: u64,
    pub extras_at: Option<Instant>,
    pub fetching: bool,
    pub busy: bool,
    pub sending: bool,
    pub tab: Tab,
    pub composer: text_editor::Content,
    pub dialog: Option<Dialog>,
    /// The tool blocks and results the reader has opened, by (entry seq, block index).
    pub open: HashSet<(u64, usize)>,
    pub side: agent_side::Side,
}

/// One poll: the agent, the transcript after the last seq, and (when due) the profile and infractions.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub cid: u64,
    pub agent: Value,
    pub entries: Vec<Value>,
    pub extras: Option<(Option<Value>, Vec<Value>)>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Tab(Tab),
    Polled(Result<Snapshot, String>),
    Edit(text_editor::Action),
    Send,
    Sent(Result<Value, String>),
    Run(Act),
    Ask(Dialog),
    Toggle(Wipe),
    Dismiss,
    Proceed,
    Done { retired: bool, note: &'static str, result: Result<(), String> },
    Expand((u64, usize)),
    ShowAll,
    Side(agent_side::SideMsg),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let side = agent_side::load(&mut state.side, state.cid, ctx).map(Msg::Side);
    if state.fetching {
        return side;
    }
    state.fetching = true;
    let extras = match state.extras_at {
        None => true,
        Some(at) => at.elapsed() >= EXTRAS_EVERY,
    };
    if extras {
        state.extras_at = Some(Instant::now());
    }
    let cid = state.cid;
    let after = state.last_seq;
    let client = ctx.client.clone();
    let poll = client::task(move || fetch(&client, cid, after, extras), Msg::Polled);
    Task::batch([poll, side])
}

fn fetch(client: &client::Client, cid: u64, after: u64, extras: bool) -> Result<Snapshot, String> {
    let agent: Value = client.get(&format!("/v1/agents/{cid}"))?;
    let mut entries: Vec<Value> = client.get(&format!("/v1/agents/{cid}/transcript?after={after}&limit=1000"))?;
    entries.sort_by_key(|e| u(e, "seq"));
    let extras = extras.then(|| {
        let profile = client.get::<Value>(&format!("/v1/agents/{cid}/profile")).ok();
        let infractions = client.get::<Vec<Value>>(&format!("/v1/agents/{cid}/infractions")).unwrap_or_default();
        (profile, infractions)
    });
    Ok(Snapshot { cid, agent, entries, extras })
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Tab(tab) => {
            state.tab = tab;
            Task::none()
        }
        Msg::Polled(result) => {
            state.fetching = false;
            match result {
                Ok(snap) if snap.cid == state.cid => {
                    state.error = None;
                    state.agent = Some(snap.agent);
                    for e in snap.entries {
                        let seq = u(&e, "seq");
                        if seq > state.last_seq {
                            state.last_seq = seq;
                            state.entries.push(e);
                        }
                    }
                    if let Some((profile, infractions)) = snap.extras {
                        state.profile = profile;
                        state.infractions = infractions;
                    }
                }
                Ok(_) => {}
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Edit(action) => {
            state.composer.perform(action);
            Task::none()
        }
        Msg::Send => {
            let text = state.composer.text().trim().to_string();
            if text.is_empty() || state.sending || offline(state) {
                return Task::none();
            }
            state.sending = true;
            let client = ctx.client.clone();
            let path = format!("/v1/agents/{}/chat", state.cid);
            client::task(move || client.post::<Value>(&path, &json!({ "text": text })), Msg::Sent)
        }
        Msg::Sent(result) => {
            state.sending = false;
            match result {
                Ok(_) => state.composer = text_editor::Content::new(),
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Run(act) => {
            let (verb, note) = match act {
                Act::Cancel => ("cancel", "Cancelled"),
                Act::Stop => ("stop", "Stopped"),
            };
            let path = format!("/v1/agents/{}/{verb}", state.cid);
            dispatch(state, ctx, path, json!({}), false, note)
        }
        Msg::Ask(dialog) => {
            state.dialog = Some(dialog);
            Task::none()
        }
        Msg::Toggle(which) => {
            if let Some(Dialog::Wipe { memory, transcript, identity }) = state.dialog.as_mut() {
                let flag = match which {
                    Wipe::Memory => memory,
                    Wipe::Transcript => transcript,
                    Wipe::Identity => identity,
                };
                *flag = !*flag;
            }
            Task::none()
        }
        Msg::Dismiss => {
            state.dialog = None;
            Task::none()
        }
        Msg::Proceed => {
            let cid = state.cid;
            match state.dialog.take() {
                Some(Dialog::Restart) => dispatch(state, ctx, format!("/v1/agents/{cid}/restart"), json!({}), false, "Restarting"),
                Some(Dialog::Wipe { memory, transcript, identity }) => {
                    let body = json!({ "memory": memory, "transcript": transcript, "identity": identity });
                    dispatch(state, ctx, format!("/v1/agents/{cid}/wipe"), body, false, "Wiped")
                }
                Some(Dialog::Retire) => dispatch(state, ctx, format!("/v1/agents/{cid}/retire"), json!({}), true, "Retired"),
                None => Task::none(),
            }
        }
        Msg::Done { retired, note, result } => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    if retired {
                        return Task::done(Msg::Go(Id::Agents));
                    }
                    state.notice = Some(note.to_string());
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Expand(key) => {
            if !state.open.remove(&key) {
                state.open.insert(key);
            }
            Task::none()
        }
        Msg::ShowAll => {
            state.infractions_all = !state.infractions_all;
            Task::none()
        }
        Msg::Side(m) => agent_side::update(&mut state.side, m, ctx).map(Msg::Side),
    }
}

/// Posts one head action; the head buttons stay disabled until it answers.
fn dispatch(state: &mut State, ctx: &Ctx, path: String, body: Value, retired: bool, note: &'static str) -> Task<Msg> {
    state.busy = true;
    state.notice = None;
    let client = ctx.client.clone();
    client::task(move || client.post::<Value>(&path, &body), move |r| Msg::Done { retired, note, result: r.map(|_| ()) })
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let name = state.agent.as_ref().map(agent_name).unwrap_or_else(|| format!("Agent {}", state.cid));
    let busy = state.busy;
    let pick = |m: Msg| if busy { None } else { Some(m) };
    let working = state.agent.as_ref().is_some_and(|a| s(a, "status") == "working");
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if state.agent.is_some() {
        if working {
            actions.push(ui::btn("Cancel work", Some("ban"), ui::Kind::Secondary, pick(Msg::Run(Act::Cancel))));
        }
        actions.push(ui::btn("Restart", None, ui::Kind::Secondary, pick(Msg::Ask(Dialog::Restart))));
        actions.push(ui::btn("Stop", Some("stop"), ui::Kind::Danger, pick(Msg::Run(Act::Stop))));
        actions.push(ui::btn("Wipe…", None, ui::Kind::Secondary, pick(Msg::Ask(Dialog::Wipe { memory: false, transcript: false, identity: false }))));
        actions.push(ui::btn("Retire", None, ui::Kind::Danger, pick(Msg::Ask(Dialog::Retire))));
    }

    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(n) = &state.notice {
        body.push(ui::faint(n.as_str()).into());
    }
    if let Some(d) = &state.dialog {
        body.push(dialog_card(d, &name, busy));
    }
    match state.agent.as_ref() {
        Some(a) => {
            if let Some(e) = &state.error {
                body.push(ui::error_line(e));
            }
            body.push(profile_card(state, a));
        }
        None => match &state.error {
            Some(e) => body.push(ui::empty("alert", "Could not load this agent", e)),
            None => body.push(ui::loading()),
        },
    }

    let tabs = ui::tabs(&[(Tab::Transcript, "Transcript"), (Tab::Memory, "Memory")], state.tab, Msg::Tab);
    let content: Element<'a, Msg> = match state.tab {
        Tab::Transcript => column![transcript_card(state), composer_view(state)].spacing(16).width(Length::Fill).into(),
        Tab::Memory => agent_side::memory_view(&state.side).map(Msg::Side),
    };
    let main = column![tabs, content].spacing(16).width(Length::Fill);

    let mut side: Vec<Element<'a, Msg>> = vec![agent_side::view(&state.side, ctx).map(Msg::Side)];
    if let Some(a) = state.agent.as_ref() {
        side.push(details_card(a));
        side.push(queue_card(a));
    }
    let side = container(column(side).spacing(16)).width(Length::Fixed(SIDE_WIDTH));
    body.push(row![main, side].spacing(24).into());

    ui::sheet(head(actions), scrollable(column(body).spacing(16).width(Length::Fill)).into())
}

/// The web's head: the back link on its own line, the actions right-aligned under it.
fn head<'a>(actions: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let back = button(text("Agents").font(SERIF).size(13.0).color(ui::INK_3))
        .padding(0)
        .style(|_: &Theme, _: button::Status| button::Style { background: None, text_color: ui::INK_3, border: Border::default(), ..Default::default() })
        .on_press(Msg::Go(Id::Agents));
    let mut line = row![Space::new().width(Length::Fill)].spacing(8).align_y(Alignment::Center);
    for action in actions {
        line = line.push(action);
    }
    container(column![back, line].spacing(8)).padding([24, 0]).width(Length::Fill).into()
}

fn dialog_card<'a>(d: &Dialog, name: &str, busy: bool) -> Element<'a, Msg> {
    let proceed = if busy { None } else { Some(Msg::Proceed) };
    match d {
        Dialog::Restart => ui::card(
            &format!("Restart {name}?"),
            column![
                ui::faint("Its VM restarts."),
                row![ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)), ui::btn("Restart", None, ui::Kind::Primary, proceed)].spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Wipe { memory, transcript, identity } => ui::card(
            &format!("Wipe {name}"),
            column![
                ui::faint("The agent's VM restarts with a fresh workspace. This cannot be undone."),
                checkbox(true).label("Workspace (always)"),
                checkbox(*memory).label("Memory notes").on_toggle(|_| Msg::Toggle(Wipe::Memory)),
                checkbox(*transcript).label("Conversation transcript").on_toggle(|_| Msg::Toggle(Wipe::Transcript)),
                checkbox(*identity).label("Name and reputation").on_toggle(|_| Msg::Toggle(Wipe::Identity)),
                row![ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)), ui::btn("Wipe", None, ui::Kind::Danger, proceed)].spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Retire => ui::card(
            &format!("Retire {name}?"),
            column![
                ui::faint("Its VM stops for good and it leaves the roster."),
                row![ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)), ui::btn("Retire", None, ui::Kind::Danger, proceed)].spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
    }
}

/// The profile: the portrait with its stamp, the name and role, the money, standing, conduct and
/// record tiles, then the infractions (`agent-profile.ts`). The portrait is the agent's hue and
/// initial until the desktop draws portraits.
fn profile_card<'a>(state: &'a State, a: &'a Value) -> Element<'a, Msg> {
    let p = state.profile.as_ref();
    let pv = |k: &str| f(p, k);
    let orchestrator = s(a, "tier") == "orchestrator";
    let hue = if orchestrator { 25.0 } else { role_hue(s(a, "role")) };
    let (status_text, status_tone) = match s(a, "status") {
        "working" => ("Working", ui::Tone::Info),
        "idle" => ("Idle", ui::Tone::Ok),
        _ => ("Offline", ui::Tone::Muted),
    };

    let name = agent_name(a).to_uppercase();
    let initial = name.chars().next().map(String::from).unwrap_or_else(|| "A".to_string());
    let stamp = container(text(if orchestrator { "ORCHESTRATOR" } else { "CITIZEN" }).font(MONO_BOLD).size(11.0).color(ui::SIGNAL))
        .padding([4, 8])
        .style(frame(Some(ui::SHEET), ui::SIGNAL));
    let portrait = container(ui::mono(initial, 64.0, ui::INK))
        .width(Length::Fixed(PORTRAIT))
        .height(Length::Fixed(PORTRAIT))
        .center_x(Length::Fixed(PORTRAIT))
        .center_y(Length::Fixed(PORTRAIT))
        .style(frame(Some(mix(hsl(agent_hue(u(a, "cid")), 0.6, 0.75), ui::SHEET, 0.45)), ui::INK));
    let portrait = column![portrait, stamp].spacing(10).align_x(Alignment::Center);

    let stripe = hsl(hue, 0.55, 0.4);
    let role_chip = row![
        container(Space::new().width(Length::Fixed(4.0)).height(Length::Fill)).style(move |_: &Theme| container::Style {
            background: Some(stripe.into()),
            ..Default::default()
        }),
        container(text(s(a, "role_label").to_uppercase()).font(MONO_SEMIBOLD).size(11.5).color(ui::INK)).padding([6, 10]).style(frame(Some(ui::SHEET), ui::INK)),
    ]
    .height(Length::Shrink);
    let tags = row![role_chip, ui::mono(s(a, "model"), 12.0, ui::INK_2), ui::faint(format!("agent #{}", u(a, "cid")))]
        .spacing(12)
        .align_y(Alignment::Center);
    let ident = column![row![text(name).font(MONO_BOLD).size(34.0).color(ui::INK), ui::badge(status_text, status_tone)].spacing(12).align_y(Alignment::Center), tags]
        .spacing(12)
        .width(Length::Fill);

    let usage = a.get("usage");
    let tokens = f(usage, "input_tokens") + f(usage, "output_tokens");
    let conf = pv("confidence");
    let rep: Element<'a, Msg> = match p {
        Some(_) if conf >= 0.05 => meter(pv("reputation"), 100.0, OK, format!("{} / 100", pv("reputation").round())),
        Some(_) => figure_faint("Unrated"),
        None => figure("—".into()),
    };
    let rep_sub = match p {
        Some(_) if conf >= 0.05 => format!("{}% confident", (conf * 100.0).round()),
        Some(_) => "no rated work yet".to_string(),
        None => String::new(),
    };
    let conduct: Element<'a, Msg> = match p {
        Some(_) => meter(pv("conduct"), 100.0, conduct_tone(pv("conduct")), conduct_text(pv("conduct")).to_string()),
        None => figure("—".into()),
    };
    let tk = |k: &str| f(p.and_then(|p| p.get("tasks")), k);
    let fines = p.and_then(|p| p.get("fines"));
    let tiles_a = row![
        tile("Balance", p.map(|_| figure(format!("{} cr", compact(pv("wallet"))))).unwrap_or_else(|| figure("—".into())), match p {
            Some(_) => format!("earned {} · spent {}", compact(pv("earned")), compact(pv("spent"))),
            None => String::new(),
        }),
        tile("Reputation", rep, rep_sub),
        tile("Conduct", conduct, match p {
            Some(_) => format!("{} violations · {} refusals", violations(p).len(), pv("denials").round()),
            None => String::new(),
        }),
    ]
    .spacing(12);
    let tiles_b = row![
        tile("Tasks", p.map(|_| figure(format!("{} verified", tk("verified")))).unwrap_or_else(|| figure("—".into())), match p {
            Some(_) => format!("{} done · {} failed · {} posted", tk("done"), tk("failed"), tk("posted")),
            None => String::new(),
        }),
        tile("Network", p.map(|_| figure(format!("{} contacts", pv("contacts")))).unwrap_or_else(|| figure("—".into())), match p {
            Some(_) => format!("{} memories", pv("memory_notes")),
            None => String::new(),
        }),
        tile("Tokens", figure(compact(tokens)), match p {
            Some(p) => format!("joined {}", ago(u(p, "joined_ms"))),
            None => String::new(),
        }),
    ]
    .spacing(12);

    let mut record: Vec<Element<'a, Msg>> = Vec::new();
    let viol = violations(p);
    if p.is_some() && (!viol.is_empty() || f(fines, "count") > 0.0) {
        record.push(ui::label("Record").into());
        for v in viol.iter().take(5) {
            record.push(row![ui::badge(s(v, "law"), ui::Tone::Danger), ui::prose(s(v, "detail")), ui::faint(ago(u(v, "at_ms")))].spacing(8).into());
        }
        if f(fines, "count") > 0.0 {
            record.push(ui::faint(format!("{} fines, {} cr in total", f(fines, "count"), compact(f(fines, "total")))).into());
        }
    } else if p.is_some() {
        record.push(ui::faint("A clean record: no violations, no fines.").into());
    }

    let n = state.infractions.len();
    let headline: Element<'a, Msg> = if state.extras_at.is_none() {
        ui::faint("Loading…").into()
    } else if n == 0 {
        ui::badge("No infractions", ui::Tone::Ok)
    } else {
        ui::badge(&format!("{} infraction{}", n, if n == 1 { "" } else { "s" }), ui::Tone::Danger)
    };
    let mut inf: Vec<Element<'a, Msg>> = vec![ui::label("Infractions").into(), headline];
    let shown = if state.infractions_all { n } else { INFRACTIONS_SHOWN };
    for r in state.infractions.iter().take(shown) {
        inf.push(
            row![
                ui::link(pick_str(s(r, "law_title"), s(r, "law")), Msg::Go(Id::Rules), false),
                ui::mono(s(r, "tool"), 12.0, ui::INK_2),
                ui::faint(s(r, "detail")),
                ui::faint(ago(u(r, "at_ms"))),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if n > INFRACTIONS_SHOWN {
        inf.push(ui::link(if state.infractions_all { "Show fewer" } else { "Show all" }, Msg::ShowAll, false));
    }

    let info = column![ident, tiles_a, tiles_b, column(record).spacing(6), column(inf).spacing(6)].spacing(14).width(Length::Fill);
    container(row![portrait, info].spacing(24))
        .padding(20)
        .width(Length::Fill)
        .style(frame(Some(ui::SHEET), ui::INK))
        .into()
}

/// A profile tile: its label, its figure (text or a meter) and its caption, on paper.
fn tile<'a>(label: &'static str, value: Element<'a, Msg>, sub: String) -> Element<'a, Msg> {
    container(column![text(label.to_uppercase()).font(MONO_SEMIBOLD).size(11.0).color(ui::INK_2), value, ui::mono(sub, 11.0, ui::INK_3)].spacing(6))
        .padding([12, 14])
        .width(Length::Fill)
        .style(frame(Some(ui::PAPER), ui::RULE))
        .into()
}

fn figure<'a>(value: String) -> Element<'a, Msg> {
    text(value).font(MONO_BOLD).size(22.0).color(ui::INK).into()
}

fn figure_faint<'a>(value: &'static str) -> Element<'a, Msg> {
    ui::faint(value).into()
}

/// A meter: a 2px-ruled track filled to `value` of `max`, its caption under it.
fn meter<'a>(value: f64, max: f64, fill: Color, caption: String) -> Element<'a, Msg> {
    let pct = (value / max * 100.0).clamp(0.0, 100.0).round() as u16;
    let mut bar = row![].height(Length::Fill);
    if pct > 0 {
        bar = bar.push(
            container(Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::FillPortion(pct))
                .height(Length::Fill)
                .style(move |_: &Theme| container::Style { background: Some(fill.into()), ..Default::default() }),
        );
    }
    if pct < 100 {
        bar = bar.push(Space::new().width(Length::FillPortion(100 - pct)).height(Length::Fill));
    }
    let track = container(bar).width(Length::Fill).height(Length::Fixed(14.0)).style(frame(Some(ui::SHEET), ui::INK));
    column![track, text(caption).font(MONO_SEMIBOLD).size(12.0).color(ui::INK)].spacing(4).width(Length::Fill).into()
}

fn transcript_card<'a>(state: &'a State) -> Element<'a, Msg> {
    // Entries group into their work item's section; a rule divides one section from the next.
    let mut groups: Vec<Vec<&'a Value>> = Vec::new();
    for e in &state.entries {
        match groups.last_mut() {
            Some(group) if group.first().is_some_and(|first| u(first, "work") == u(e, "work")) => group.push(e),
            _ => groups.push(vec![e]),
        }
    }
    let count = groups.len();
    let mut sections: Vec<Element<'a, Msg>> = Vec::new();
    for (i, group) in groups.into_iter().enumerate() {
        let mut parts: Vec<Element<'a, Msg>> = Vec::new();
        for e in group {
            parts.extend(entry_view(&state.open, e));
        }
        sections.push(column(parts).spacing(12).width(Length::Fill).into());
        if i + 1 < count {
            sections.push(ui::hairline());
        }
    }
    let lines: Element<'a, Msg> = column(sections).spacing(16).width(Length::Fill).into();
    container(scrollable(lines).height(Length::Fixed(TRANSCRIPT_HEIGHT)))
        .padding(16)
        .width(Length::Fill)
        .style(frame(None, ui::RULE))
        .into()
}

fn composer_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let offline = offline(state);
    let mut editor = text_editor(&state.composer)
        .placeholder("Message this agent…")
        .height(Length::Fixed(96.0))
        .key_binding(|kp| {
            if kp.key == keyboard::Key::Named(keyboard::key::Named::Enter) && kp.modifiers.control() {
                Some(text_editor::Binding::Custom(Msg::Send))
            } else {
                text_editor::Binding::from_key_press(kp)
            }
        });
    if !offline {
        editor = editor.on_action(Msg::Edit);
    }
    let can_send = !offline && !state.sending && !state.composer.text().trim().is_empty();
    let send = ui::btn("Send", Some("send"), ui::Kind::Primary, if can_send { Some(Msg::Send) } else { None });
    column![editor, row![Space::new().width(Length::Fill), send]].spacing(8).into()
}

/// One transcript entry as the web renders it: turns, tool calls and results (collapsed until
/// opened), decisions, errors. Blocks key their open state by the entry's seq and block index.
fn entry_view<'a>(open: &HashSet<(u64, usize)>, e: &'a Value) -> Vec<Element<'a, Msg>> {
    let d = e.get("data").unwrap_or(&Value::Null);
    let seq = u(e, "seq");
    match s(e, "kind") {
        "work_start" => vec![ui::badge(s(d, "kind"), ui::Tone::Info), ui::prose(s(d, "prompt")).into()],
        "work_end" => {
            let st = s(d, "status");
            let tone = if st.contains("fail") || st.contains("error") {
                ui::Tone::Danger
            } else if st.contains("cancel") || st.contains("stop") {
                ui::Tone::Warn
            } else {
                ui::Tone::Ok
            };
            vec![ui::badge(if st.is_empty() { "done" } else { st }, tone)]
        }
        "model_response" => {
            let mut out: Vec<Element<'a, Msg>> = Vec::new();
            if let Some(blocks) = d.get("content").and_then(Value::as_array) {
                for (idx, b) in blocks.iter().enumerate() {
                    out.extend(block_view(open, seq, idx, b));
                }
            }
            out
        }
        "tool_decision" => match s(d, "decision") {
            "allow" => vec![ui::badge("ran in VM", ui::Tone::Info)],
            "done" => vec![ui::badge("ran on host", ui::Tone::Muted)],
            "deny" => vec![ui::badge(&format!("denied: {}", s(d, "reason")), ui::Tone::Danger)],
            _ => Vec::new(),
        },
        "tool_result" => {
            let failed = d.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let body = pick_str(s(d, "content"), s(d, "output"));
            let key = (seq, 0);
            let opened = open.contains(&key);
            let (label, tone) = if failed { ("Error", CRIT) } else { ("Output", OK) };
            let mut out = vec![disclosure(label.to_string(), tone, opened, Msg::Expand(key))];
            if opened {
                out.push(pre(body.to_string()));
            }
            out
        }
        "error" => vec![ui::error_line(pick_str(s(d, "message"), "Error"))],
        _ => Vec::new(),
    }
}

fn block_view<'a>(open: &HashSet<(u64, usize)>, seq: u64, idx: usize, b: &'a Value) -> Vec<Element<'a, Msg>> {
    match s(b, "type") {
        "text" => vec![ui::markdown(s(b, "text"))],
        "tool_use" => {
            let key = (seq, idx);
            let opened = open.contains(&key);
            let mut out = vec![chip(format!("→ {}", s(b, "name")), opened, Msg::Expand(key))];
            if opened {
                let input = serde_json::to_string_pretty(b.get("input").unwrap_or(&Value::Null)).unwrap_or_default();
                out.push(pre(input));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// A transcript disclosure: a marker and a mono label in a tone, that opens and shuts its body.
fn disclosure<'a>(label: String, tone: Color, opened: bool, msg: Msg) -> Element<'a, Msg> {
    let marker = if opened { "▾" } else { "▸" };
    button(row![ui::mono(marker, 11.0, ui::INK_3), ui::mono(label, 12.0, tone)].spacing(6))
        .padding(0)
        .style(|_: &Theme, _: button::Status| button::Style { background: None, text_color: ui::INK, border: Border::default(), ..Default::default() })
        .on_press(msg)
        .into()
}

/// A tool call's name as the web's chip: bordered, mono, opening its input.
fn chip<'a>(label: String, opened: bool, msg: Msg) -> Element<'a, Msg> {
    let marker = if opened { "▾" } else { "▸" };
    button(row![ui::mono(marker, 10.0, ui::INK_3), ui::mono(label, 11.0, ui::INK_2)].spacing(6))
        .padding([2, 8])
        .style(|_: &Theme, _: button::Status| button::Style {
            background: Some(HOVER.into()),
            text_color: ui::INK_2,
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

/// An opened body in mono, on a ruled panel.
fn pre<'a>(body: String) -> Element<'a, Msg> {
    container(ui::mono(body, 12.0, ui::INK_2)).padding(8).width(Length::Fill).style(frame(None, ui::RULE)).into()
}

/// The details card: the agent's ids, its VM, when it connected and was seen, its tokens.
fn details_card<'a>(a: &'a Value) -> Element<'a, Msg> {
    let usage = a.get("usage");
    let run = match s(a, "run") {
        "" => "—",
        run => run,
    };
    let vm = a.get("vm").and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_else(|| "—".to_string());
    let rows: Vec<Element<'a, Msg>> = vec![
        kv("CID", ui::mono(u(a, "cid").to_string(), 13.0, ui::INK).into()),
        kv("Run", ui::mono(run, 13.0, ui::INK).into()),
        kv("VM", ui::mono(vm, 13.0, ui::INK).into()),
        kv("Connected", ui::prose(when(a, "connected_at_ms")).into()),
        kv("Last seen", ui::prose(when(a, "last_seen_ms")).into()),
        kv("Tokens in", ui::mono(compact(f(usage, "input_tokens")), 13.0, ui::INK).into()),
        kv("Tokens out", ui::mono(compact(f(usage, "output_tokens")), 13.0, ui::INK).into()),
    ];
    ui::card("Details", column(rows).spacing(8).into())
}

fn kv<'a>(key: &'static str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![container(ui::label(key)).width(Length::Fixed(96.0)), value].spacing(12).align_y(Alignment::Center).into()
}

/// The queue card: the work running now, and how many items wait behind it.
fn queue_card<'a>(a: &'a Value) -> Element<'a, Msg> {
    let work: Element<'a, Msg> = match a.get("work").filter(|w| !w.is_null()) {
        Some(w) => column![ui::badge(s(w, "kind"), ui::Tone::Info), ui::prose(preview(s(w, "prompt"), 160))].spacing(6).into(),
        None => ui::faint("Nothing running").into(),
    };
    ui::card("Queue", column![work, ui::faint(format!("{} queued", u(a, "queue")))].spacing(8).into())
}

/// A bordered panel, optionally filled: the web's cards, tiles and mono frames.
fn frame(fill: Option<Color>, edge: Color) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| container::Style {
        background: fill.map(Into::into),
        border: Border { color: edge, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

/// Mixes `over` into `base` by `amount` (0 keeps `base`, 1 is `over`).
fn mix(base: Color, over: Color, amount: f32) -> Color {
    Color { r: base.r * amount + over.r * (1.0 - amount), g: base.g * amount + over.g * (1.0 - amount), b: base.b * amount + over.b * (1.0 - amount), a: 1.0 }
}

/// CSS `hsl()`: hue in degrees, saturation and lightness 0 to 1.
fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::from_rgb(r + m, g + m, b + m)
}

fn offline(state: &State) -> bool {
    state.agent.as_ref().is_some_and(|a| s(a, "status") == "offline")
}

fn agent_name(a: &Value) -> String {
    let name = s(a, "name");
    if name.is_empty() {
        format!("Agent {}", u(a, "cid"))
    } else {
        name.to_string()
    }
}

fn conduct_text(score: f64) -> &'static str {
    if score >= 90.0 {
        "Exemplary"
    } else if score >= 70.0 {
        "Good standing"
    } else if score >= 40.0 {
        "Under watch"
    } else {
        "In trouble"
    }
}

fn conduct_tone(score: f64) -> Color {
    if score >= 90.0 {
        OK
    } else if score >= 70.0 {
        INFO
    } else if score >= 40.0 {
        WARN
    } else {
        CRIT
    }
}

fn violations(p: Option<&Value>) -> &[Value] {
    p.and_then(|p| p.get("violations")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn f(v: Option<&Value>, key: &str) -> f64 {
    v.and_then(|v| v.get(key)).and_then(Value::as_f64).unwrap_or(0.0)
}

fn pick_str<'a>(first: &'a str, fallback: &'a str) -> &'a str {
    if first.is_empty() {
        fallback
    } else {
        first
    }
}

fn preview(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn compact(n: f64) -> String {
    let a = n.abs();
    if a >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if a >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{}", n.round())
    }
}

/// How long ago an epoch-millisecond instant was, in the web's short words.
pub(super) fn ago(ms: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// The `ago` of a record's field, or a dash when the record does not carry it.
fn when(a: &Value, key: &str) -> String {
    a.get(key).and_then(Value::as_u64).map(ago).unwrap_or_else(|| "—".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_matches_css_hsl() {
        let red = hsl(0.0, 1.0, 0.5);
        assert!((red.r - 1.0).abs() < 1e-4 && red.g.abs() < 1e-4 && red.b.abs() < 1e-4);
        let grey = hsl(200.0, 0.0, 0.5);
        assert!((grey.r - 0.5).abs() < 1e-4 && (grey.g - 0.5).abs() < 1e-4 && (grey.b - 0.5).abs() < 1e-4);
    }

    #[test]
    fn conduct_bands_match_the_web() {
        assert_eq!(conduct_text(90.0), "Exemplary");
        assert_eq!(conduct_text(70.0), "Good standing");
        assert_eq!(conduct_text(40.0), "Under watch");
        assert_eq!(conduct_text(39.9), "In trouble");
    }

    #[test]
    fn figures_compact_like_the_web() {
        assert_eq!(compact(999.0), "999");
        assert_eq!(compact(1500.0), "1.5k");
        assert_eq!(compact(2_500_000.0), "2.5M");
    }

    #[test]
    fn previews_cut_on_characters() {
        assert_eq!(preview("héllo", 3), "hél…");
        assert_eq!(preview("abc", 3), "abc");
    }
}
