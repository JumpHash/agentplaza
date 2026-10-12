//! The Overview screen (parity with the web console's `overview` view): the plaza at a glance,
//! the orchestrator's chat box, the activity feed and the fleet.
//!
//! The web view is pushed the transcript bus. A screen has no event stream here, so the feed is
//! polled: every load reads each agent's transcript after the last entry seen.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::alignment::Vertical;
use iced::widget::{button, column, container, row, scrollable, space, text_input};
use iced::{Element, Length, Task};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme::{self, INK, INK_2, INK_3, OK, SIGNAL};

const FEED_MAX: usize = 40;
const FLEET_COLS: usize = 4;

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    overview: Option<Overview>,
    agents: Vec<Agent>,
    feed: Vec<Item>,
    cursors: HashMap<u64, u64>,
    goal: String,
    sending: bool,
    note: Option<String>,
    chat_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// One poll of the hub: the overview, the fleet and the new transcript entries.
    Loaded(Result<Snapshot, String>),
    /// The orchestrator goal text changed.
    Goal(String),
    /// Send the goal to the orchestrator.
    Send,
    /// The chat message was accepted or refused.
    Sent(Result<Value, String>),
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    overview: Overview,
    agents: Vec<Agent>,
    feed: Vec<(u64, Vec<Entry>)>,
}

#[derive(Debug, Clone, Deserialize)]
struct Overview {
    agents: Counts,
    tasks: HashMap<String, u64>,
    default_model: String,
    orchestrator_model: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Counts {
    total: u64,
    working: u64,
    offline: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct Agent {
    cid: u64,
    name: String,
    role_label: String,
    tier: String,
    status: String,
    model: String,
    work: Option<Work>,
}

#[derive(Debug, Clone, Deserialize)]
struct Work {
    prompt: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    seq: u64,
    at_ms: u64,
    kind: String,
    data: Map<String, Value>,
}

/// One line of the activity feed.
#[derive(Debug, Clone)]
struct Item {
    at_ms: u64,
    cid: u64,
    line: String,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let cursors = state.cursors.clone();
    client::task(
        move || {
            let overview: Overview = client.get("/v1/overview")?;
            let agents: Vec<Agent> = client.get("/v1/agents")?;
            let mut feed = Vec::with_capacity(agents.len());
            for a in &agents {
                let path = match cursors.get(&a.cid) {
                    Some(after) => format!("/v1/agents/{}/transcript?after={after}&limit=200", a.cid),
                    None => format!("/v1/agents/{}/transcript?after=0&limit=1000", a.cid),
                };
                let entries: Vec<Entry> = client.get(&path)?;
                feed.push((a.cid, entries));
            }
            Ok(Snapshot { overview, agents, feed })
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => {}
        Msg::Loaded(Ok(snap)) => {
            for (cid, entries) in snap.feed {
                for e in entries {
                    let cursor = state.cursors.entry(cid).or_insert(0);
                    *cursor = (*cursor).max(e.seq);
                    if let Some(line) = describe(&e) {
                        state.feed.push(Item { at_ms: e.at_ms, cid, line });
                    }
                }
            }
            state.feed.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
            state.feed.truncate(FEED_MAX);
            state.overview = Some(snap.overview);
            state.agents = snap.agents;
            state.error = None;
        }
        Msg::Loaded(Err(e)) => {
            state.overview = None;
            state.error = Some(e);
        }
        Msg::Goal(text) => state.goal = text,
        Msg::Send => {
            let text = state.goal.trim().to_string();
            let Some(cid) = state.agents.iter().find(|a| a.tier == "orchestrator").map(|a| a.cid) else {
                return Task::none();
            };
            if state.sending || text.is_empty() {
                return Task::none();
            }
            state.sending = true;
            state.note = None;
            state.chat_error = None;
            let client = ctx.client.clone();
            return client::task(
                move || client.post::<Value>(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text })),
                Msg::Sent,
            );
        }
        Msg::Sent(Ok(_)) => {
            state.sending = false;
            state.goal.clear();
            state.note = Some("Sent to the orchestrator".into());
        }
        Msg::Sent(Err(e)) => {
            state.sending = false;
            state.chat_error = Some(e);
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let stats = match &state.overview {
        Some(o) => stat_row(o),
        None => ui::empty("alert", "apid is unreachable", "Start apid, then reload."),
    };
    let mut page = column![stats].spacing(18);
    if let Some(e) = &state.error {
        page = page.push(ui::error_line(e));
    }
    page = page.push(
        row![
            container(orchestrator(state)).width(Length::FillPortion(1)),
            container(activity(state)).width(Length::FillPortion(1)),
        ]
        .spacing(16),
    );
    page = page.push(fleet(&state.agents));
    ui::sheet(ui::view_head("Overview", vec![]), page.into())
}

fn stat_row<'a>(o: &Overview) -> Element<'a, Msg> {
    let open = o.tasks.get("open").copied().unwrap_or(0) + o.tasks.get("claimed").copied().unwrap_or(0);
    let done = o.tasks.get("done").copied().unwrap_or(0);
    row![
        stat(
            "Agents",
            o.agents.total.to_string(),
            format!("{} working · {} offline", o.agents.working, o.agents.offline),
        ),
        stat("Open tasks", open.to_string(), format!("{done} done")),
        stat("Worker model", model_name(&o.default_model), model_provider(&o.default_model)),
        stat("Orchestrator model", model_name(&o.orchestrator_model), model_provider(&o.orchestrator_model)),
    ]
    .spacing(16)
    .into()
}

fn stat<'a>(label: &str, value: String, sub: String) -> Element<'a, Msg> {
    container(column![ui::label(label), ui::mono(value, 26.0, INK), ui::faint(sub)].spacing(8))
        .padding(14.0)
        .width(Length::FillPortion(1))
        .style(theme::boxed)
        .into()
}

fn orchestrator<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(orch) = state.agents.iter().find(|a| a.tier == "orchestrator") else {
        return ui::card(
            "Orchestrator",
            ui::empty("orchestrator", "No orchestrator connected", "Add the orchestrator role to the fleet to delegate work by goal."),
        );
    };
    let (status, tone) = match orch.status.as_str() {
        "working" => ("Working", SIGNAL),
        "idle" => ("Idle", OK),
        _ => ("Offline", INK_3),
    };
    let mut body = column![
        row![
            ui::icon::<Msg>("orchestrator", 16.0, INK),
            ui::mono(name(orch), 15.0, INK),
            ui::faint(orch.role_label.clone()),
            ui::mono(status, 11.0, tone),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
        ui::prose(format!("Plans the work and delegates it to the fleet · {}", orch.model)).size(13.0).color(INK_2),
        text_input("Tell the orchestrator what you want done…", &state.goal)
            .on_input(Msg::Goal)
            .on_submit(Msg::Send)
            .padding(10.0)
            .style(theme::input),
        row![
            ui::faint("Enter to send"),
            space::horizontal(),
            ui::btn("Open transcript", None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(orch.cid)))),
            ui::btn("Send", Some("send"), ui::Kind::Primary, (!state.sending).then_some(Msg::Send)),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    ]
    .spacing(12);
    if let Some(n) = &state.note {
        body = body.push(ui::faint(n.as_str()));
    }
    if let Some(e) = &state.chat_error {
        body = body.push(ui::error_line(e));
    }
    ui::card("Orchestrator", body.into())
}

fn activity<'a>(state: &'a State) -> Element<'a, Msg> {
    let now = now_ms();
    let mut list = column![].spacing(10);
    if state.feed.is_empty() {
        list = list.push(ui::faint("Waiting for activity…"));
    }
    for item in &state.feed {
        let who = state
            .agents
            .iter()
            .find(|a| a.cid == item.cid)
            .map(name)
            .unwrap_or_else(|| format!("agent {}", item.cid));
        list = list.push(
            row![
                ui::faint(ago(now, item.at_ms)).width(Length::Fixed(64.0)),
                ui::btn(&who, None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(item.cid)))),
                ui::mono(item.line.clone(), 12.0, INK_2),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
        );
    }
    ui::card("Activity", scrollable(list).height(Length::Fixed(360.0)).into())
}

fn fleet<'a>(agents: &'a [Agent]) -> Element<'a, Msg> {
    if agents.is_empty() {
        return ui::card("Fleet", ui::empty("agents", "No agents yet", "Agents appear here as their VMs connect."));
    }
    let mut grid = column![].spacing(10);
    for chunk in agents.chunks(FLEET_COLS) {
        let mut line = row![].spacing(10);
        for a in chunk {
            line = line.push(tile(a));
        }
        for _ in chunk.len()..FLEET_COLS {
            line = line.push(space::horizontal());
        }
        grid = grid.push(line);
    }
    ui::card("Fleet", grid.into())
}

fn tile<'a>(a: &'a Agent) -> Element<'a, Msg> {
    let tone = match a.status.as_str() {
        "working" => SIGNAL,
        "idle" => OK,
        _ => INK_3,
    };
    let sub = match &a.work {
        Some(w) => clip(&w.prompt, 60),
        None if a.status == "offline" => "offline".to_string(),
        None => a.role_label.clone(),
    };
    let body = column![
        row![ui::mono("●", 12.0, tone), ui::mono(name(a), 13.0, INK)]
            .spacing(8)
            .align_y(Vertical::Center),
        ui::faint(sub),
    ]
    .spacing(4);
    button(body)
        .on_press(Msg::Go(Id::Agent(a.cid)))
        .style(theme::register_row(false))
        .padding(10.0)
        .width(Length::FillPortion(1))
        .into()
}

/// The name an agent chose for itself, or "Unnamed" while it has none.
fn name(a: &Agent) -> String {
    if a.name.trim().is_empty() {
        "Unnamed".to_string()
    } else {
        a.name.clone()
    }
}

/// The model's name: its last path segment.
fn model_name(model: &str) -> String {
    model.rsplit('/').next().unwrap_or("—").to_string()
}

/// The model's provider: its first path segment.
fn model_provider(model: &str) -> String {
    model.split('/').next().unwrap_or("").to_string()
}

/// The feed line for a transcript entry, if the feed shows that kind.
fn describe(e: &Entry) -> Option<String> {
    let d = &e.data;
    let text = |key: &str| d.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    match e.kind.as_str() {
        "work_start" => Some(format!("started {}: {}", text("kind"), clip(&text("prompt"), 80))),
        "work_end" => Some(format!("finished ({})", text("status"))),
        "tool_request" => Some(format!("called {}", text("name"))),
        "error" => Some(format!("error: {}", clip(&text("message"), 80))),
        _ => None,
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn ago(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at) / 1000;
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
