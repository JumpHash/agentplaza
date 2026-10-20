//! A project's Maintenance tab (the web's Maintenance view): what its watchers keep while it runs in maintenance. The
//! proposals waiting for the owner (Do it, Later, or No with a reason), the strip with the sweep and its focus and
//! interval, the map of the product, the signals that feed it (added here or sent to the ingest address), and the
//! sources the agents may read. The mode switch lives on the project's page.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, column, container, row, text, text_input};
use iced::{Alignment, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, ui};
use crate::theme;

/// The signals the list shows; the hub keeps 200.
const SHOWN_SIGNALS: usize = 30;
/// The longest focus the hub takes.
const FOCUS_MAX: usize = 1000;
/// The kinds of signal a person adds, as the hub knows them.
const SIGNAL_KINDS: [&str; 6] = ["feedback", "report", "analytics", "error", "usage", "note"];
/// The intervals the − and + buttons step through, in hours: hourly, then longer spans, up to 30 days.
const EVERY_STEPS: [i64; 13] = [1, 2, 3, 4, 6, 8, 12, 24, 48, 72, 168, 336, 720];
const BUILDING: &str = "This project is being built. In maintenance, agents read its analytics, user reports and feedback, keep a map of it and propose work; nothing runs until you pick it.";

// ---- The view, as the hub answers it -------------------------------------------------------------------------------

/// `GET /v1/projects/{id}/maintenance`. Every field defaults, so one the hub leaves out never fails the read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Maintenance {
    pub config: Config,
    pub map: ProductMap,
    /// The newest 200, newest first.
    pub signals: Vec<Signal>,
    pub signals_total: i64,
    /// Signals no sweep has read yet.
    pub unseen: i64,
    /// Newest first.
    pub proposals: Vec<Proposal>,
    pub sources: Sources,
    pub sweep: Sweep,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub every_hours: i64,
    pub focus: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProductMap {
    pub summary: String,
    pub areas: Vec<Area>,
}

/// One part of the product: its health is `good`, `watch`, `poor` or `unknown`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Area {
    pub name: String,
    pub summary: String,
    pub paths: Vec<String>,
    pub health: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Signal {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub text: String,
    pub from: String,
    pub at_ms: i64,
}

/// A change the watchers propose. `state` is `proposed`, `later`, `approved`, `declined` or `withdrawn`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Proposal {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub impact: Impact,
    /// 1 (little) to 5 (a lot), and why.
    pub value: Worth,
    pub effort: Effort,
    pub evidence: Vec<Evidence>,
    /// Data the watchers lack, and where it would come from.
    pub instrument: Option<Instrument>,
    pub state: String,
    pub created_ms: i64,
    pub note: String,
    /// The task it became, and that task's state.
    pub task: Option<String>,
    pub task_state: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Impact {
    pub changes: String,
    pub users: String,
    /// `low`, `medium` or `high`.
    pub risk: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Worth {
    pub score: i64,
    pub why: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Effort {
    /// `s`, `m`, `l` or `xl`.
    pub size: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Evidence {
    pub signal: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Instrument {
    pub kind: String,
    pub name: String,
    pub data: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sources {
    /// Whether an ingest address is set.
    pub ingest: bool,
    pub connectors: Vec<Connector>,
    pub mcp: Vec<McpServer>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Connector {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct McpServer {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sweep {
    pub last_ms: i64,
    pub last_cid: u64,
    pub last_summary: String,
    /// Set while a sweep runs.
    pub running: Option<Running>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Running {
    pub cid: u64,
}

/// `POST .../ingest`: the address's path, shown once.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ingest {
    pub path: String,
}

// ---- The tab's state and messages ----------------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct State {
    /// The view; None until the first load.
    pub data: Option<Maintenance>,
    pub error: Option<String>,
    /// A line for the last action ("Approved.").
    pub note: Option<String>,
    /// A write is in flight.
    pub busy: bool,
    /// The focus as typed; None shows the saved one.
    pub focus_draft: Option<String>,
    /// The proposal being declined, with the reason so far.
    pub declining: Option<Declining>,
    /// Whether the decided proposals are listed.
    pub decided_open: bool,
    /// The signal being added.
    pub new_signal: NewSignal,
    /// The ingest address just made, shown once.
    pub ingest_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Declining {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct NewSignal {
    pub kind: String,
    pub title: String,
    pub text: String,
}

impl Default for NewSignal {
    fn default() -> Self {
        NewSignal {
            kind: "feedback".into(),
            title: String::new(),
            text: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Maintenance, String>),
    /// Sweep now.
    Sweep,
    /// The sweep was asked for (its line), or refused (the hub's reason).
    Swept(Result<String, String>),
    Focus(String),
    SaveFocus,
    FocusSaved(Result<String, String>),
    /// Sweeps every n hours.
    Every(i64),
    Approve(String),
    Later(String),
    /// "No": opens the reason for a proposal.
    OpenDecline(String),
    DeclineReason(String),
    SubmitDecline,
    CancelDecline,
    Declined(Result<String, String>),
    ToggleDecided,
    SignalKind(String),
    SignalTitle(String),
    SignalText(String),
    AddSignal,
    SignalAdded(Result<String, String>),
    DeleteSignal(String),
    NewIngest,
    Ingest(Result<Ingest, String>),
    StopIngest,
    /// A write went through (its line) or failed; the view is read again.
    Done(Result<String, String>),
}

/// The navigation this tab asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

fn maintenance_path(project: &str) -> String {
    format!("/v1/projects/{}/maintenance", client::enc(project))
}

/// Reads the project's maintenance view.
pub fn load(project: &str, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = maintenance_path(project);
    client::task(move || c.get::<Maintenance>(&path), Msg::Loaded)
}

/// `project` is the project's id.
pub fn update(state: &mut State, msg: Msg, project: &str, ctx: &Ctx) -> Task<Msg> {
    let root = maintenance_path(project);
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(data)) => {
            state.data = Some(data);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Sweep => write(
            state,
            ctx,
            move |c| {
                c.post::<Value>(&format!("{root}/sweep"), &json!({}))
                    .map(|_| "Sweep started.".to_string())
            },
            Msg::Swept,
        ),
        Msg::Swept(res) => {
            state.busy = false;
            state.note = Some(match res {
                Ok(line) => line,
                Err(e) => e,
            });
            load(project, ctx)
        }
        Msg::Focus(typed) => {
            state.focus_draft = Some(typed.chars().take(FOCUS_MAX).collect());
            Task::none()
        }
        Msg::SaveFocus => {
            let Some(focus) = state.focus_draft.clone() else {
                return Task::none();
            };
            write(
                state,
                ctx,
                move |c| {
                    c.patch::<Value>(&root, &json!({ "focus": focus }))
                        .map(|_| "Focus saved.".to_string())
                },
                Msg::FocusSaved,
            )
        }
        Msg::FocusSaved(res) => {
            state.busy = false;
            if res.is_ok() {
                state.focus_draft = None;
            }
            settle(state, res);
            load(project, ctx)
        }
        Msg::Every(hours) => write(
            state,
            ctx,
            move |c| {
                c.patch::<Value>(&root, &json!({ "every_hours": hours }))
                    .map(|_| format!("Sweeps every {}.", hours_label(hours)))
            },
            Msg::Done,
        ),
        Msg::Approve(pid) => write(
            state,
            ctx,
            move |c| {
                c.post::<Value>(&format!("{root}/proposals/{pid}/approve"), &json!({}))
                    .map(|_| "Approved.".to_string())
            },
            Msg::Done,
        ),
        Msg::Later(pid) => write(
            state,
            ctx,
            move |c| {
                c.post::<Value>(&format!("{root}/proposals/{pid}/later"), &json!({}))
                    .map(|_| "Kept for later.".to_string())
            },
            Msg::Done,
        ),
        Msg::OpenDecline(pid) => {
            state.declining = Some(Declining {
                id: pid,
                reason: String::new(),
            });
            Task::none()
        }
        Msg::DeclineReason(reason) => {
            if let Some(d) = state.declining.as_mut() {
                d.reason = reason;
            }
            Task::none()
        }
        Msg::CancelDecline => {
            state.declining = None;
            Task::none()
        }
        Msg::SubmitDecline => {
            let Some(d) = state.declining.clone() else {
                return Task::none();
            };
            write(
                state,
                ctx,
                move |c| {
                    c.post::<Value>(
                        &format!("{root}/proposals/{}/decline", d.id),
                        &json!({ "reason": d.reason.trim() }),
                    )
                    .map(|_| "Declined.".to_string())
                },
                Msg::Declined,
            )
        }
        Msg::Declined(res) => {
            state.busy = false;
            if res.is_ok() {
                state.declining = None;
            }
            settle(state, res);
            load(project, ctx)
        }
        Msg::ToggleDecided => {
            state.decided_open = !state.decided_open;
            Task::none()
        }
        Msg::SignalKind(kind) => {
            state.new_signal.kind = kind;
            Task::none()
        }
        Msg::SignalTitle(title) => {
            state.new_signal.title = title;
            Task::none()
        }
        Msg::SignalText(text) => {
            state.new_signal.text = text;
            Task::none()
        }
        Msg::AddSignal => {
            let signal = state.new_signal.clone();
            if signal.text.trim().is_empty() {
                state.error = Some("A signal needs some text.".into());
                return Task::none();
            }
            write(
                state,
                ctx,
                move |c| {
                    let body = json!({
                        "kind": signal.kind,
                        "title": signal.title.trim(),
                        "text": signal.text.trim(),
                    });
                    c.post::<Value>(&format!("{root}/signals"), &body)
                        .map(|_| "Signal added.".to_string())
                },
                Msg::SignalAdded,
            )
        }
        Msg::SignalAdded(res) => {
            state.busy = false;
            if res.is_ok() {
                state.new_signal.title.clear();
                state.new_signal.text.clear();
            }
            settle(state, res);
            load(project, ctx)
        }
        Msg::DeleteSignal(sid) => write(
            state,
            ctx,
            move |c| {
                c.delete(&format!("{root}/signals/{sid}"))
                    .map(|_| "Signal deleted.".to_string())
            },
            Msg::Done,
        ),
        Msg::NewIngest => {
            if !start(state) {
                return Task::none();
            }
            let c = ctx.client.clone();
            let path = format!("{root}/ingest");
            client::task(move || c.post::<Ingest>(&path, &json!({})), Msg::Ingest)
        }
        Msg::Ingest(res) => {
            state.busy = false;
            match res {
                Ok(ingest) => {
                    state.ingest_path = Some(ingest.path);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(project, ctx)
        }
        Msg::StopIngest => {
            state.ingest_path = None;
            write(
                state,
                ctx,
                move |c| {
                    c.delete(&format!("{root}/ingest"))
                        .map(|_| "Ingest address stopped.".to_string())
                },
                Msg::Done,
            )
        }
        Msg::Done(res) => {
            state.busy = false;
            settle(state, res);
            load(project, ctx)
        }
    }
}

/// Starts a write, unless one is under way: a click while it runs sends nothing.
fn start(state: &mut State) -> bool {
    if state.busy {
        return false;
    }
    state.busy = true;
    true
}

/// Runs one write as a task, busy until it answers; `done` gets its line or its error.
fn write(
    state: &mut State,
    ctx: &Ctx,
    job: impl FnOnce(client::Client) -> Result<String, String> + Send + 'static,
    done: fn(Result<String, String>) -> Msg,
) -> Task<Msg> {
    if !start(state) {
        return Task::none();
    }
    let c = ctx.client.clone();
    client::task(move || job(c), done)
}

/// Shows a write's line, or its error.
fn settle(state: &mut State, res: Result<String, String>) {
    match res {
        Ok(line) => {
            state.note = Some(line);
            state.error = None;
        }
        Err(e) => state.error = Some(e),
    }
}

// ---- Plain helpers (tested below) ----------------------------------------------------------------------------------

/// A proposal the owner has still to decide: proposed, or later.
fn is_waiting(p: &Proposal) -> bool {
    matches!(p.state.as_str(), "proposed" | "later")
}

/// How many proposals wait for the owner.
pub fn waiting_count(props: &[Proposal]) -> usize {
    props.iter().filter(|p| is_waiting(p)).count()
}

/// The proposals waiting for the owner, best first: value high to low, then risk low to high, then newest.
pub fn waiting_in_order(props: &[Proposal]) -> Vec<&Proposal> {
    let mut open: Vec<&Proposal> = props.iter().filter(|p| is_waiting(p)).collect();
    open.sort_by(|a, b| {
        b.value
            .score
            .cmp(&a.value.score)
            .then_with(|| risk_rank(&a.impact.risk).cmp(&risk_rank(&b.impact.risk)))
            .then_with(|| b.created_ms.cmp(&a.created_ms))
    });
    open
}

/// The proposals already decided (approved, declined, withdrawn), in the hub's order.
pub fn decided_of(props: &[Proposal]) -> Vec<&Proposal> {
    props.iter().filter(|p| !is_waiting(p)).collect()
}

fn risk_rank(risk: &str) -> u8 {
    match risk {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        _ => 3,
    }
}

/// A health's dot: good is ok, watch is warn, poor is danger, anything else muted.
pub fn health_tone(health: &str) -> ui::Tone {
    match health {
        "good" => ui::Tone::Ok,
        "watch" => ui::Tone::Warn,
        "poor" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

fn risk_tone(risk: &str) -> ui::Tone {
    match risk {
        "low" => ui::Tone::Ok,
        "medium" => ui::Tone::Warn,
        "high" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// The interval the − or + button sets next: one step up or down the ladder, never below one hour.
fn every_step(hours: i64, up: bool) -> i64 {
    if up {
        EVERY_STEPS
            .iter()
            .copied()
            .find(|&s| s > hours)
            .unwrap_or(720)
    } else {
        EVERY_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&s| s < hours)
            .unwrap_or(1)
    }
}

fn hours_label(hours: i64) -> String {
    if hours == 1 {
        "1 hour".into()
    } else {
        format!("{hours} hours")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Text on one line, its spaces collapsed.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A signal's line: its title, or the start of its text when it has none.
fn signal_line(s: &Signal) -> String {
    let title = s.title.trim();
    if title.is_empty() {
        ui::ellipsize(&one_line(&s.text), 80)
    } else {
        title.to_string()
    }
}

/// An evidence line: the signal it cites, then the note.
fn evidence_line(e: &Evidence) -> String {
    match (e.signal.as_deref().filter(|s| !s.is_empty()), e.note.trim()) {
        (Some(signal), "") => signal.to_string(),
        (Some(signal), note) => format!("{signal} · {note}"),
        (None, note) => note.to_string(),
    }
}

/// The last sweep, as the strip says it: "never", or how long ago and by which agent.
fn last_sweep_text(sweep: &Sweep) -> String {
    if sweep.last_ms <= 0 {
        "never".into()
    } else if sweep.last_cid > 0 {
        format!("{} by agent {}", ago(sweep.last_ms), sweep.last_cid)
    } else {
        ago(sweep.last_ms)
    }
}

/// A relative time: "just now", "5m ago", "3h ago", "2d ago".
fn ago(ms: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    ago_at(now, ms)
}

fn ago_at(now: i64, ms: i64) -> String {
    let s = (now - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

// ---- The view ------------------------------------------------------------------------------------------------------

/// The tab. `in_maintenance` is whether the project runs in maintenance (the mode switch is on its page).
pub fn view<'a>(state: &'a State, _project: &'a str, in_maintenance: bool) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    let Some(data) = &state.data else {
        parts.push(ui::loading());
        return column(parts).spacing(16).into();
    };
    if !in_maintenance {
        parts.push(ui::faint(BUILDING).into());
    }
    let busy = state.busy;
    parts.push(strip(state, data, busy));
    parts.push(waiting_card(state, data, busy));
    parts.push(decided_section(state, data));
    parts.push(map_card(&data.map));
    parts.push(signals_card(state, data, busy));
    parts.push(ingest_card(state, data, busy));
    parts.push(sources_card(data));
    column(parts).spacing(16).into()
}

/// The strip: what waits, the signals, the last sweep; Sweep now; the focus; how often sweeps run.
fn strip<'a>(state: &'a State, data: &'a Maintenance, busy: bool) -> Element<'a, Msg> {
    let running = data.sweep.running.is_some();
    let facts: Element<'a, Msg> = row(vec![
        ink_line(format!(
            "Waiting for you: {}",
            plural(waiting_count(&data.proposals), "proposal", "proposals")
        )),
        ink_line(format!(
            "Signals: {} ({} new)",
            data.signals_total, data.unseen
        )),
        ink_line(format!("Last sweep: {}", last_sweep_text(&data.sweep))),
        ui::btn_sm(
            "Sweep now",
            None,
            ui::Kind::Secondary,
            (!busy && !running).then_some(Msg::Sweep),
        ),
    ])
    .spacing(20)
    .wrap()
    .into();
    let mut lines: Vec<Element<'a, Msg>> = vec![facts];
    if let Some(r) = &data.sweep.running {
        lines.push(ui::faint(format!("A sweep is under way, by agent {}.", r.cid)).into());
    } else if !data.sweep.last_summary.is_empty() {
        lines.push(ui::faint(data.sweep.last_summary.clone()).into());
    }

    let draft = state
        .focus_draft
        .as_deref()
        .unwrap_or(data.config.focus.as_str());
    let changed = state
        .focus_draft
        .as_deref()
        .is_some_and(|d| d != data.config.focus);
    lines.push(
        column![
            ui::label("What matters now"),
            row![
                text_input("Anything the agents should look at first", draft)
                    .on_input(Msg::Focus)
                    .on_submit(Msg::SaveFocus)
                    .padding(8)
                    .style(theme::input),
                ui::btn_sm(
                    "Save",
                    None,
                    ui::Kind::Secondary,
                    (changed && !busy).then_some(Msg::SaveFocus),
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .into(),
    );

    let hours = data.config.every_hours;
    let (down, up) = (every_step(hours, false), every_step(hours, true));
    lines.push(
        row![
            ui::btn_sm(
                "−",
                None,
                ui::Kind::Secondary,
                (!busy && down != hours).then_some(Msg::Every(down)),
            ),
            ink_line(format!("Every {}", hours_label(hours))),
            ui::btn_sm(
                "+",
                None,
                ui::Kind::Secondary,
                (!busy && up != hours).then_some(Msg::Every(up)),
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
    );
    ui::card("Maintenance", column(lines).spacing(14).into())
}

/// The proposals waiting for the owner, best first.
fn waiting_card<'a>(state: &'a State, data: &'a Maintenance, busy: bool) -> Element<'a, Msg> {
    let open = waiting_in_order(&data.proposals);
    let count = open.len();
    let body: Element<'a, Msg> = if open.is_empty() {
        ui::faint("Nothing waits for you.").into()
    } else {
        let mut items: Vec<Element<'a, Msg>> = Vec::new();
        for (i, p) in open.into_iter().enumerate() {
            if i > 0 {
                items.push(ui::hairline());
            }
            items.push(proposal_block(state, p, busy));
        }
        column(items).spacing(16).into()
    };
    ui::card(&format!("Waiting for you ({count})"), body)
}

/// One proposal: its kind, risk, title and weight, what it changes and why it is worth it, its evidence, and
/// what it needs; then the owner's three answers (or the reason form, under No).
fn proposal_block<'a>(state: &'a State, p: &'a Proposal, busy: bool) -> Element<'a, Msg> {
    let head: Element<'a, Msg> = row![
        ui::badge::<Msg>(&p.kind, ui::Tone::Muted),
        ui::badge::<Msg>(
            &format!("Risk {}", p.impact.risk),
            risk_tone(&p.impact.risk)
        ),
        text(p.title.clone())
            .font(ui::SANS_SEMIBOLD)
            .size(theme::TYPE_ITEM)
            .color(theme::palette().ink),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .wrap()
    .into();
    let meta: Element<'a, Msg> = row![
        ui::faint(format!("Value {}/5", p.value.score)),
        ui::faint(format!("Size {}", p.effort.size)),
    ]
    .spacing(14)
    .into();
    let mut lines: Vec<Element<'a, Msg>> = vec![head, meta];
    if !p.summary.is_empty() {
        lines.push(ui::prose(p.summary.clone()).into());
    }
    if !p.impact.changes.is_empty() {
        lines.push(field("What changes:", p.impact.changes.clone()));
    }
    if !p.impact.users.is_empty() {
        lines.push(field("Who:", p.impact.users.clone()));
    }
    if !p.value.why.is_empty() {
        lines.push(field("Why it is worth it:", p.value.why.clone()));
    }
    let evidence: Vec<String> = p
        .evidence
        .iter()
        .map(evidence_line)
        .filter(|l| !l.is_empty())
        .collect();
    if !evidence.is_empty() {
        lines.push(field("Evidence:", evidence.join("\n")));
    }
    if let Some(i) = &p.instrument {
        lines.push(ui::faint(format!("Needs: {} ({}) — {}", i.name, i.kind, i.data)).into());
    }
    lines.push(answers(state, p, busy));
    column(lines).spacing(10).into()
}

/// Do it, Later, No; under No, the reason and Decline.
fn answers<'a>(state: &'a State, p: &'a Proposal, busy: bool) -> Element<'a, Msg> {
    if let Some(d) = state.declining.as_ref().filter(|d| d.id == p.id) {
        let reason = text_input("Why not (optional)", &d.reason)
            .on_input(Msg::DeclineReason)
            .on_submit(Msg::SubmitDecline)
            .padding(8)
            .style(theme::input);
        return column![
            ui::label("Why not"),
            reason,
            row![
                ui::btn_sm(
                    "Decline",
                    None,
                    ui::Kind::Secondary,
                    (!busy).then_some(Msg::SubmitDecline),
                ),
                ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDecline)),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .into();
    }
    row![
        ui::btn_sm(
            "Do it",
            None,
            ui::Kind::Primary,
            (!busy).then_some(Msg::Approve(p.id.clone())),
        ),
        ui::btn_sm(
            "Later",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::Later(p.id.clone())),
        ),
        ui::btn_sm(
            "No",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::OpenDecline(p.id.clone())),
        ),
    ]
    .spacing(8)
    .into()
}

/// The decided proposals, under a toggle that shows their count.
fn decided_section<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let done = decided_of(&data.proposals);
    let toggle: Element<'a, Msg> = ui::disclosure(
        format!("Decided ({})", done.len()),
        ui::Tone::Muted.color(),
        state.decided_open,
        Msg::ToggleDecided,
    );
    if !state.decided_open {
        return toggle;
    }
    let mut items: Vec<Element<'a, Msg>> = vec![toggle];
    for p in done {
        let head: Element<'a, Msg> = row![
            text(p.title.clone())
                .font(ui::SANS_SEMIBOLD)
                .size(theme::TYPE_UI)
                .color(theme::palette().ink),
            ui::faint(p.state.clone()),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into();
        let mut lines: Vec<Element<'a, Msg>> = vec![head];
        if !p.note.is_empty() {
            lines.push(ui::faint(p.note.clone()).into());
        }
        if let Some(task) = p.task.as_deref().filter(|t| !t.is_empty()) {
            let mut task_row: Vec<Element<'a, Msg>> = vec![
                ui::faint("Task").into(),
                ui::text_link(task, Msg::Go(Id::Task(task.to_string()))),
            ];
            if let Some(ts) = p.task_state.as_deref().filter(|t| !t.is_empty()) {
                task_row.push(ui::faint(ts.to_string()).into());
            }
            lines.push(row(task_row).spacing(10).align_y(Alignment::Center).into());
        }
        items.push(column(lines).spacing(4).into());
    }
    column(items).spacing(12).into()
}

/// The map: the summary, then each area with its health, summary and paths.
fn map_card<'a>(map: &'a ProductMap) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if !map.summary.is_empty() {
        lines.push(ui::prose(map.summary.clone()).into());
    }
    if map.areas.is_empty() && map.summary.is_empty() {
        lines.push(ui::faint("No map yet: the first sweep maps the project.").into());
    }
    for a in &map.areas {
        let mut info: Vec<Element<'a, Msg>> = vec![
            row![
                text(a.name.clone())
                    .font(ui::SANS_SEMIBOLD)
                    .size(theme::TYPE_ITEM)
                    .color(theme::palette().ink),
                ui::faint(a.health.clone()),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        ];
        if !a.summary.is_empty() {
            info.push(ink_line(a.summary.clone()));
        }
        let paths = a.paths.join("  ");
        if !paths.is_empty() {
            info.push(ui::mono(paths, theme::TYPE_META, theme::palette().ink_3).into());
        }
        lines.push(
            row![
                health_dot(&a.health),
                column(info).spacing(4).width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Start)
            .into(),
        );
    }
    ui::card("The map", column(lines).spacing(14).into())
}

/// A small square in a health's colour.
fn health_dot<'a>(health: &str) -> Element<'a, Msg> {
    let color = health_tone(health).color();
    container(
        Space::new()
            .width(Length::Fixed(10.0))
            .height(Length::Fixed(10.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(color.into()),
        ..Default::default()
    })
    .into()
}

/// The signals: the form to add one, then the newest of them, each deletable.
fn signals_card<'a>(state: &'a State, data: &'a Maintenance, busy: bool) -> Element<'a, Msg> {
    let ns = &state.new_signal;
    let kinds: Element<'a, Msg> = row(SIGNAL_KINDS.iter().map(|k| {
        let kind = if ns.kind == *k {
            ui::Kind::Primary
        } else {
            ui::Kind::Secondary
        };
        ui::btn_sm(k, None, kind, Some(Msg::SignalKind(k.to_string())))
    }))
    .spacing(6)
    .wrap()
    .into();
    let add: Element<'a, Msg> = column![
        ui::label("Add a signal"),
        kinds,
        text_input("Title (optional)", &ns.title)
            .on_input(Msg::SignalTitle)
            .padding(8)
            .style(theme::input),
        text_input("What was said, seen or measured", &ns.text)
            .on_input(Msg::SignalText)
            .on_submit(Msg::AddSignal)
            .padding(8)
            .style(theme::input),
        ui::btn_sm(
            "Add",
            None,
            ui::Kind::Secondary,
            (!busy && !ns.text.trim().is_empty()).then_some(Msg::AddSignal),
        ),
    ]
    .spacing(8)
    .into();

    let mut list: Vec<Element<'a, Msg>> = Vec::new();
    if data.signals.is_empty() {
        list.push(ui::faint("No signals yet.").into());
    }
    for s in data.signals.iter().take(SHOWN_SIGNALS) {
        let meta = [s.from.clone(), ago(s.at_ms)]
            .into_iter()
            .filter(|m| !m.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        list.push(
            row![
                ui::badge::<Msg>(&s.kind, ui::Tone::Muted),
                column![ink_line(signal_line(s)), ui::faint(meta)]
                    .spacing(2)
                    .width(Length::Fill),
                ui::btn_sm(
                    "×",
                    None,
                    ui::Kind::Ghost,
                    (!busy).then_some(Msg::DeleteSignal(s.id.clone())),
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if data.signals_total > SHOWN_SIGNALS as i64 {
        list.push(
            ui::faint(format!(
                "Showing the newest {SHOWN_SIGNALS} of {}.",
                data.signals_total
            ))
            .into(),
        );
    }
    ui::card(
        "Signals",
        column![add, column(list).spacing(10)].spacing(16).into(),
    )
}

/// The ingest address: made once (its path shown once), replaced or stopped after.
fn ingest_card<'a>(state: &'a State, data: &'a Maintenance, busy: bool) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(path) = &state.ingest_path {
        lines.push(
            ui::faint("Send user reports or webhooks here (POST, JSON {text, kind?, title?}). It is shown once.")
                .into(),
        );
        lines.push(ui::mono(path.clone(), theme::TYPE_CODE, theme::palette().ink).into());
    }
    let buttons: Element<'a, Msg> = if data.sources.ingest {
        lines.insert(0, ui::faint("An ingest address is set.").into());
        row![
            ui::btn_sm(
                "Make a new one",
                None,
                ui::Kind::Secondary,
                (!busy).then_some(Msg::NewIngest),
            ),
            ui::btn_sm(
                "Stop it",
                None,
                ui::Kind::DangerOutline,
                (!busy).then_some(Msg::StopIngest),
            ),
        ]
        .spacing(8)
        .into()
    } else {
        row![ui::btn_sm(
            "Make an ingest address",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::NewIngest),
        )]
        .into()
    };
    lines.push(buttons);
    ui::card("Ingest address", column(lines).spacing(10).into())
}

/// The connectors and MCP servers the agents can read, or the note that there are none.
fn sources_card<'a>(data: &'a Maintenance) -> Element<'a, Msg> {
    let s = &data.sources;
    let body: Element<'a, Msg> = if s.connectors.is_empty() && s.mcp.is_empty() {
        ui::faint(
            "No analytics, error or support tools are connected: the agents may propose some.",
        )
        .into()
    } else {
        let mut lines: Vec<Element<'a, Msg>> = Vec::new();
        if !s.connectors.is_empty() {
            let names: Vec<String> = s
                .connectors
                .iter()
                .map(|c| named(&c.label, &c.id))
                .collect();
            lines.push(field("Connectors", names.join(", ")));
        }
        if !s.mcp.is_empty() {
            let names: Vec<String> = s.mcp.iter().map(|m| named(&m.name, &m.id)).collect();
            lines.push(field("MCP servers", names.join(", ")));
        }
        column(lines).spacing(10).into()
    };
    ui::card("Sources", body)
}

/// A source's name: its label, or its id while it has none.
fn named(label: &str, id: &str) -> String {
    if label.is_empty() {
        id.to_string()
    } else {
        label.to_string()
    }
}

/// A labelled value: the label in the small caps, the value under it.
fn field<'a>(label: &str, value: String) -> Element<'a, Msg> {
    column![ui::label(label), ink_line(value)].spacing(4).into()
}

/// A line of interface text in ink.
fn ink_line<'a>(content: String) -> Element<'a, Msg> {
    ui::sans(content, theme::TYPE_UI, theme::palette().ink).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(id: &str, state: &str, score: i64, risk: &str, created_ms: i64) -> Proposal {
        Proposal {
            id: id.into(),
            state: state.into(),
            value: Worth {
                score,
                why: String::new(),
            },
            impact: Impact {
                risk: risk.into(),
                ..Default::default()
            },
            created_ms,
            ..Default::default()
        }
    }

    fn ids(list: &[&Proposal]) -> Vec<String> {
        list.iter().map(|p| p.id.clone()).collect()
    }

    #[test]
    fn the_waiting_proposals_go_best_first() {
        let props = [
            proposal("a", "proposed", 3, "low", 100),
            proposal("b", "proposed", 5, "high", 50),
            proposal("c", "proposed", 5, "low", 10),
            proposal("d", "later", 3, "low", 200),
            proposal("e", "proposed", 3, "low", 300),
            proposal("f", "approved", 5, "low", 999),
        ];
        assert_eq!(
            ids(&waiting_in_order(&props)),
            ["c", "b", "e", "d", "a"],
            "value first, then the lower risk, then the newest"
        );
    }

    #[test]
    fn the_waiting_count_and_the_decided_split() {
        let props = [
            proposal("a", "proposed", 1, "low", 1),
            proposal("b", "later", 1, "low", 2),
            proposal("c", "approved", 1, "low", 3),
            proposal("d", "declined", 1, "low", 4),
            proposal("e", "withdrawn", 1, "low", 5),
        ];
        assert_eq!(waiting_count(&props), 2);
        assert_eq!(ids(&decided_of(&props)), ["c", "d", "e"]);
        assert_eq!(
            waiting_in_order(&props).len() + decided_of(&props).len(),
            props.len()
        );
    }

    #[test]
    fn health_has_a_colour_each() {
        assert_eq!(health_tone("good"), ui::Tone::Ok);
        assert_eq!(health_tone("watch"), ui::Tone::Warn);
        assert_eq!(health_tone("poor"), ui::Tone::Danger);
        assert_eq!(health_tone("unknown"), ui::Tone::Muted);
        assert_eq!(health_tone(""), ui::Tone::Muted);
    }

    #[test]
    fn the_interval_steps_and_stops_at_its_ends() {
        assert_eq!(every_step(6, true), 8);
        assert_eq!(every_step(6, false), 4);
        assert_eq!(every_step(5, true), 6);
        assert_eq!(every_step(720, true), 720);
        assert_eq!(every_step(1, false), 1);
        assert_eq!(every_step(0, true), 1);
        assert_eq!(hours_label(1), "1 hour");
        assert_eq!(hours_label(24), "24 hours");
    }

    #[test]
    fn ago_reads_in_the_largest_unit() {
        let now: i64 = 1_800_000_000_000;
        assert_eq!(ago_at(now, now - 20_000), "just now");
        assert_eq!(ago_at(now, now - 5 * 60_000), "5m ago");
        assert_eq!(ago_at(now, now - 3 * 3_600_000), "3h ago");
        assert_eq!(ago_at(now, now - 2 * 86_400_000), "2d ago");
    }

    #[test]
    fn a_signal_reads_as_its_title_or_the_start_of_its_text() {
        let titled = Signal {
            title: "Checkout fails".into(),
            text: "long text".into(),
            ..Default::default()
        };
        assert_eq!(signal_line(&titled), "Checkout fails");
        let untitled = Signal {
            text: "word ".repeat(40),
            ..Default::default()
        };
        let line = signal_line(&untitled);
        assert!(line.chars().count() <= 80, "cut to 80 characters");
        assert!(line.ends_with('…'));
    }

    #[test]
    fn counts_are_plural_when_they_are_not_one() {
        assert_eq!(plural(0, "proposal", "proposals"), "0 proposals");
        assert_eq!(plural(1, "proposal", "proposals"), "1 proposal");
        assert_eq!(plural(2, "proposal", "proposals"), "2 proposals");
    }
}
