//! One agent's page (parity with the web console's agent detail): the profile card, the transcript
//! and chat, restart / stop / cancel, wipe and retire. The right column is the agent side panels
//! (`agent_side`: model, harness, permissions, details, queue, memory).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::keyboard;
use iced::widget::{checkbox, column, container, row, scrollable, text_editor, Space};
use iced::{Element, Length, Task};
use serde_json::{json, Value};

use super::{agent_side, Id};
use crate::console::{client, ui, Ctx};

/// The profile and infractions refresh at most this often; the transcript on every poll.
const EXTRAS_EVERY: Duration = Duration::from_secs(15);
const SIDE_WIDTH: f32 = 320.0;
const TRANSCRIPT_HEIGHT: f32 = 480.0;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Transcript,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
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
    Restart,
    Stop,
}

#[derive(Debug, Default)]
pub struct State {
    pub cid: u64,
    pub error: Option<String>,
    pub agent: Option<Value>,
    pub profile: Option<Value>,
    pub infractions: Vec<Value>,
    pub entries: Vec<Value>,
    pub last_seq: u64,
    pub extras_at: Option<Instant>,
    pub fetching: bool,
    pub busy: bool,
    pub sending: bool,
    pub tab: Tab,
    pub composer: text_editor::Content,
    pub dialog: Option<Dialog>,
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
    Done { retired: bool, result: Result<(), String> },
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
            let verb = match act {
                Act::Cancel => "cancel",
                Act::Restart => "restart",
                Act::Stop => "stop",
            };
            let path = format!("/v1/agents/{}/{}", state.cid, verb);
            dispatch(state, ctx, path, json!({}), false)
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
                Some(Dialog::Wipe { memory, transcript, identity }) => {
                    let body = json!({ "memory": memory, "transcript": transcript, "identity": identity });
                    dispatch(state, ctx, format!("/v1/agents/{cid}/wipe"), body, false)
                }
                Some(Dialog::Retire) => dispatch(state, ctx, format!("/v1/agents/{cid}/retire"), json!({}), true),
                None => Task::none(),
            }
        }
        Msg::Done { retired, result } => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    if retired {
                        return Task::done(Msg::Go(Id::Agents));
                    }
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Side(m) => agent_side::update(&mut state.side, m, ctx).map(Msg::Side),
    }
}

/// Posts one head action; the head buttons stay disabled until it answers.
fn dispatch(state: &mut State, ctx: &Ctx, path: String, body: Value, retired: bool) -> Task<Msg> {
    state.busy = true;
    let client = ctx.client.clone();
    client::task(move || client.post::<Value>(&path, &body), move |r| Msg::Done { retired, result: r.map(|_| ()) })
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
        actions.push(ui::btn("Restart", None, ui::Kind::Secondary, pick(Msg::Run(Act::Restart))));
        actions.push(ui::btn("Stop", Some("stop"), ui::Kind::Danger, pick(Msg::Run(Act::Stop))));
        actions.push(ui::btn("Wipe…", None, ui::Kind::Secondary, pick(Msg::Ask(Dialog::Wipe { memory: false, transcript: false, identity: false }))));
        actions.push(ui::btn("Retire", None, ui::Kind::Danger, pick(Msg::Ask(Dialog::Retire))));
    }
    let head = ui::view_head(&name, actions);

    let mut body: Vec<Element<'a, Msg>> = vec![ui::btn("Agents", None, ui::Kind::Ghost, Some(Msg::Go(Id::Agents)))];
    if let Some(e) = &state.error {
        body.push(ui::error_line(e));
    }
    if let Some(d) = &state.dialog {
        body.push(dialog_card(d, &name, busy));
    }
    match state.agent.as_ref() {
        Some(a) => body.push(profile_card(state, a)),
        None if state.error.is_none() => body.push(ui::loading()),
        None => {}
    }
    body.push(
        row![tab_btn("Transcript", Tab::Transcript, state.tab), tab_btn("Memory", Tab::Memory, state.tab)]
            .spacing(8)
            .into(),
    );
    match state.tab {
        Tab::Transcript => {
            body.push(ui::card("Transcript", transcript_view(state)));
            body.push(composer_view(state));
        }
        Tab::Memory => body.push(ui::empty("memory", "Memory", "The agent's memory notes are in the Memory panel beside this page.")),
    }
    let main = column(body).spacing(16).width(Length::Fill);
    let side = container(agent_side::view(&state.side, ctx).map(Msg::Side)).width(Length::Fixed(SIDE_WIDTH));
    ui::sheet(head, scrollable(row![main, side].spacing(24)).into())
}

fn tab_btn<'a>(label: &str, tab: Tab, current: Tab) -> Element<'a, Msg> {
    let kind = if tab == current { ui::Kind::Primary } else { ui::Kind::Ghost };
    ui::btn(label, None, kind, Some(Msg::Tab(tab)))
}

fn dialog_card<'a>(d: &Dialog, name: &str, busy: bool) -> Element<'a, Msg> {
    let proceed = if busy { None } else { Some(Msg::Proceed) };
    match d {
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
            &format!("Retire {name}"),
            column![
                ui::faint("Its VM stops for good and it leaves the roster."),
                row![ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)), ui::btn("Retire", None, ui::Kind::Danger, proceed)].spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
    }
}

fn profile_card<'a>(state: &'a State, a: &'a Value) -> Element<'a, Msg> {
    let p = state.profile.as_ref();
    let name = agent_name(a);
    let (status_text, status_tone) = match s(a, "status") {
        "working" => ("Working", ui::Tone::Info),
        "idle" => ("Idle", ui::Tone::Ok),
        _ => ("Offline", ui::Tone::Muted),
    };
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "A".to_string());
    let portrait = container(ui::mono(initial, 40.0, ui::INK))
        .width(Length::Fixed(120.0))
        .height(Length::Fixed(120.0))
        .center_x(Length::Fixed(120.0))
        .center_y(Length::Fixed(120.0));
    let tier = if s(a, "tier") == "orchestrator" { "ORCHESTRATOR" } else { "CITIZEN" };
    let ident = column![
        row![ui::prose(name.clone()), ui::badge(status_text, status_tone)].spacing(12),
        ui::mono(format!("{} · {} · agent #{}", s(a, "role_label"), s(a, "model"), u(a, "cid")), 12.0, ui::INK_2),
        ui::label(tier),
    ]
    .spacing(6);

    let pv = |k: &str| f(p, k);
    let tk = |k: &str| f(p.and_then(|p| p.get("tasks")), k);
    let usage = a.get("usage");
    let tokens = f(usage, "input_tokens") + f(usage, "output_tokens");
    let (balance, balance_sub) = match p {
        Some(_) => (format!("{} cr", compact(pv("wallet"))), format!("earned {} · spent {}", compact(pv("earned")), compact(pv("spent")))),
        None => ("—".to_string(), String::new()),
    };
    let conf = pv("confidence");
    let (rep, rep_sub) = match p {
        Some(_) if conf >= 0.05 => (format!("{} / 100", pv("reputation").round()), format!("{}% confident", (conf * 100.0).round())),
        Some(_) => ("Unrated".to_string(), "no rated work yet".to_string()),
        None => ("—".to_string(), String::new()),
    };
    let (conduct, conduct_sub) = match p {
        Some(_) => (conduct_text(pv("conduct")).to_string(), format!("{} violations · {} refusals", violations(p).len(), pv("denials").round())),
        None => ("—".to_string(), String::new()),
    };
    let (tasks, tasks_sub) = match p {
        Some(_) => (format!("{} verified", tk("verified")), format!("{} done · {} failed · {} posted", tk("done"), tk("failed"), tk("posted"))),
        None => ("—".to_string(), String::new()),
    };
    let (network, network_sub) = match p {
        Some(_) => (format!("{} contacts", pv("contacts")), format!("{} memories", pv("memory_notes"))),
        None => ("—".to_string(), String::new()),
    };
    let tokens_sub = p.map(|p| format!("joined {}", ago(u(p, "joined_ms")))).unwrap_or_default();
    let tiles_a = row![
        tile("Balance", balance, balance_sub),
        tile("Reputation", rep, rep_sub),
        tile("Conduct", conduct, conduct_sub),
    ]
    .spacing(12);
    let tiles_b = row![
        tile("Tasks", tasks, tasks_sub),
        tile("Network", network, network_sub),
        tile("Tokens", compact(tokens), tokens_sub),
    ]
    .spacing(12);

    let mut record: Vec<Element<'a, Msg>> = Vec::new();
    let viol = violations(p);
    if p.is_some() && (!viol.is_empty() || f(p.and_then(|p| p.get("fines")), "count") > 0.0) {
        record.push(ui::label("Record").into());
        for v in viol.iter().take(5) {
            record.push(
                row![ui::badge(s(v, "law"), ui::Tone::Danger), ui::prose(s(v, "detail")), ui::faint(ago(u(v, "at_ms")))]
                    .spacing(8)
                    .into(),
            );
        }
        let fines = p.and_then(|p| p.get("fines"));
        if f(fines, "count") > 0.0 {
            record.push(
                ui::faint(format!("{} fines, {} cr in total", f(fines, "count"), compact(f(fines, "total")))).into(),
            );
        }
    } else if p.is_some() {
        record.push(ui::faint("A clean record: no violations, no fines.").into());
    }

    let headline: Element<'a, Msg> = if state.extras_at.is_none() {
        ui::faint("Loading…").into()
    } else if state.infractions.is_empty() {
        ui::badge("No infractions", ui::Tone::Ok)
    } else {
        let n = state.infractions.len();
        ui::badge(&format!("{} infraction{}", n, if n == 1 { "" } else { "s" }), ui::Tone::Danger)
    };
    let mut inf: Vec<Element<'a, Msg>> = vec![ui::label("Infractions").into(), headline];
    for r in state.infractions.iter().take(10) {
        inf.push(
            row![
                ui::prose(pick_str(s(r, "law_title"), s(r, "law"))),
                ui::mono(s(r, "tool"), 12.0, ui::INK_2),
                ui::faint(s(r, "detail")),
                ui::faint(ago(u(r, "at_ms"))),
            ]
            .spacing(10)
            .into(),
        );
    }

    let info = column![ident, tiles_a, tiles_b, column(record).spacing(6), column(inf).spacing(6)].spacing(14).width(Length::Fill);
    ui::card("Profile", row![portrait, info].spacing(24).into())
}

fn tile<'a>(label: &'static str, value: String, sub: String) -> Element<'a, Msg> {
    container(column![ui::label(label), ui::mono(value, 16.0, ui::INK), ui::faint(sub)].spacing(4))
        .width(Length::Fill)
        .into()
}

fn transcript_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    for e in &state.entries {
        lines.extend(entry_view(e));
    }
    if lines.is_empty() {
        lines.push(ui::faint("No messages yet.").into());
    }
    scrollable(column(lines).spacing(12).width(Length::Fill)).height(Length::Fixed(TRANSCRIPT_HEIGHT)).into()
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

/// One transcript entry as the web renders it: turns, tool calls and results compact, errors marked.
fn entry_view<'a>(e: &'a Value) -> Vec<Element<'a, Msg>> {
    let d = e.get("data").unwrap_or(&Value::Null);
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
                for b in blocks {
                    out.extend(block_view(b));
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
            vec![
                ui::badge(if failed { "Error" } else { "Output" }, if failed { ui::Tone::Danger } else { ui::Tone::Ok }),
                ui::mono(preview(body, 400), 12.0, ui::INK).into(),
            ]
        }
        "error" => vec![ui::error_line(pick_str(s(d, "message"), "Error"))],
        _ => Vec::new(),
    }
}

fn block_view<'a>(b: &'a Value) -> Vec<Element<'a, Msg>> {
    match s(b, "type") {
        "text" => vec![ui::markdown(s(b, "text"))],
        "tool_use" => {
            let input = b.get("input").map(|i| i.to_string()).unwrap_or_default();
            vec![
                ui::mono(format!("→ {}", s(b, "name")), 12.0, ui::INK_2).into(),
                ui::faint(preview(&input, 160)).into(),
            ]
        }
        _ => Vec::new(),
    }
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
fn ago(ms: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
