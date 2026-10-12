//! The Conversations screen (parity with the web console's `conversations` view).
//!
//! Agent-to-agent threads grouped by the pair of agents: a list of pairs (newest first) beside the
//! selected thread (oldest message first, the newest shown at the bottom).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::operation::snap_to;
use iced::widget::{button, column, container, row, scrollable, Id as WidgetId, Space};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde_json::Value;

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme::{INK, PAPER, RULE, SHEET, SIGNAL};

/// The thread's scrollable, so a newly opened thread can start at its newest message.
const THREAD: &str = "conversations-thread";

#[derive(Debug, Default)]
pub struct State {
    /// The last load's failure, if it failed.
    pub error: Option<String>,
    loaded: bool,
    conversations: Vec<Conversation>,
    /// `(cid, display name)` from `GET /v1/agents`.
    agents: Vec<(u64, String)>,
    /// The key (`a-b`, ordered) of the pair the reader picked; the newest pair when unset.
    selected: Option<String>,
    /// The key of the thread that is shown, to tell a newly opened thread from a redraw.
    shown: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// Open the thread with this pair key.
    Select(String),
    /// The conversations and the agents, or the failure.
    Loaded(Result<(Value, Value), String>),
}

/// One message from `GET /v1/conversations`.
#[derive(Debug, Clone)]
struct Conversation {
    from_cid: u64,
    to_cid: u64,
    text: String,
    created_at: i64,
    delivered: bool,
}

/// The messages between two agents, oldest first.
#[derive(Debug, Clone)]
struct Pair {
    key: String,
    a: u64,
    b: u64,
    messages: Vec<Conversation>,
    last: Conversation,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the conversations and the agents (called when the screen opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let conversations = client.get::<Value>("/v1/conversations")?;
            // The agents only name the threads: without them the list still shows, with ids.
            let agents = client.get::<Value>("/v1/agents").unwrap_or(Value::Null);
            Ok((conversations, agents))
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, _ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Select(key) => {
            state.selected = Some(key);
            sync_thread(state)
        }
        Msg::Loaded(result) => {
            state.loaded = true;
            match result {
                Ok((conversations, agents)) => {
                    state.error = None;
                    state.conversations = parse_conversations(&conversations);
                    state.agents = parse_agents(&agents);
                }
                Err(err) => state.error = Some(err),
            }
            sync_thread(state)
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let pairs = group_pairs(&state.conversations);
    let current = current_index(&pairs, state.selected.as_deref());

    let list: Element<'a, Msg> = if !state.loaded {
        ui::loading()
    } else if let Some(err) = &state.error {
        column![ui::empty("alert", "Could not load conversations.", ""), ui::error_line(err)].into()
    } else if pairs.is_empty() {
        ui::empty("conversations", "No conversations yet.", "")
    } else {
        column(pairs.iter().enumerate().map(|(i, p)| pair_item(p, i == current, &state.agents))).into()
    };
    let list_col = container(scrollable(list).height(Length::Fill)).width(Length::Fixed(340.0)).height(Length::Fill);

    let thread: Element<'a, Msg> = match pairs.get(current) {
        Some(p) if state.error.is_none() => thread_view(p, &state.agents),
        _ => Space::new().width(Length::Fill).into(),
    };

    let divider = container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill).style(|_: &Theme| container::Style {
        background: Some(RULE.into()),
        ..Default::default()
    });

    let body: Element<'a, Msg> = row![list_col, divider, thread].height(Length::Fixed(640.0)).into();
    ui::sheet(ui::view_head("Conversations", vec![]), body)
}

/// Scrolls the thread to its newest message when a different thread is shown; a redraw keeps the
/// reader's place.
fn sync_thread(state: &mut State) -> Task<Msg> {
    let pairs = group_pairs(&state.conversations);
    let key = pairs.get(current_index(&pairs, state.selected.as_deref())).map(|p| p.key.clone());
    if key == state.shown {
        return Task::none();
    }
    state.shown = key;
    snap_to(WidgetId::new(THREAD), scrollable::RelativeOffset::END)
}

/// The index of the selected pair, or the newest (0) when none is selected or it is gone.
fn current_index(pairs: &[Pair], selected: Option<&str>) -> usize {
    pairs.iter().position(|p| Some(p.key.as_str()) == selected).unwrap_or(0)
}

/// Groups the messages by the unordered pair of agents; pairs newest first.
fn group_pairs(list: &[Conversation]) -> Vec<Pair> {
    let mut oldest_first: Vec<&Conversation> = list.iter().collect();
    oldest_first.sort_by_key(|c| c.created_at);
    let mut pairs: Vec<Pair> = Vec::new();
    let mut index: HashMap<(u64, u64), usize> = HashMap::new();
    for c in oldest_first {
        let (a, b) = (c.from_cid.min(c.to_cid), c.from_cid.max(c.to_cid));
        let i = *index.entry((a, b)).or_insert_with(|| {
            pairs.push(Pair { key: format!("{a}-{b}"), a, b, messages: Vec::new(), last: c.clone() });
            pairs.len() - 1
        });
        pairs[i].messages.push(c.clone());
        pairs[i].last = c.clone();
    }
    pairs.sort_by(|x, y| y.last.created_at.cmp(&x.last.created_at));
    pairs
}

fn label(agents: &[(u64, String)], cid: u64) -> String {
    agents.iter().find(|(id, _)| *id == cid).map(|(_, name)| name.clone()).unwrap_or_else(|| format!("agent {cid}"))
}

fn name_link<'a>(cid: u64, agents: &[(u64, String)]) -> Element<'a, Msg> {
    ui::btn(&label(agents, cid), None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(cid))))
}

fn pair_item<'a>(p: &Pair, active: bool, agents: &[(u64, String)]) -> Element<'a, Msg> {
    let names = format!("{} · {}", label(agents, p.a), label(agents, p.b));
    let meta = format!("{} · {} messages", ago(p.last.created_at, now_ms()), p.messages.len());
    let (background, edge) = if active { (PAPER, SIGNAL) } else { (SHEET, RULE) };
    button(column![ui::mono(names, 12.0, INK), ui::prose(clip(&p.last.text, 90)), ui::faint(meta)].spacing(4))
        .width(Length::Fill)
        .padding(12)
        .on_press(Msg::Select(p.key.clone()))
        .style(move |_: &Theme, _: button::Status| button::Style {
            background: Some(background.into()),
            text_color: INK,
            border: Border { color: edge, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

fn thread_view<'a>(p: &Pair, agents: &[(u64, String)]) -> Element<'a, Msg> {
    // The first sender's messages sit on the left, the other agent's on the right.
    let first = p.messages.first().map_or(p.a, |m| m.from_cid);
    let head = row![name_link(p.a, agents), ui::faint(" · "), name_link(p.b, agents)].spacing(6).align_y(Alignment::Center);
    let mut items: Vec<Element<'a, Msg>> = vec![head.into()];
    items.extend(p.messages.iter().map(|m| bubble(m, m.from_cid != first, agents)));
    scrollable(column(items).spacing(14).padding(16))
        .id(WidgetId::new(THREAD))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn bubble<'a>(m: &Conversation, right: bool, agents: &[(u64, String)]) -> Element<'a, Msg> {
    let mut meta = row![name_link(m.from_cid, agents), ui::faint(ago(m.created_at, now_ms()))].spacing(10).align_y(Alignment::Center);
    if !m.delivered {
        meta = meta.push(ui::badge("undelivered", ui::Tone::Warn));
    }
    let card: Element<'a, Msg> = container(column![meta, ui::markdown(&m.text)].spacing(8))
        .padding(12)
        .max_width(620)
        .style(|_: &Theme| container::Style {
            background: Some(SHEET.into()),
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into();
    let gap = Space::new().width(Length::Fill);
    if right {
        row![gap, card].into()
    } else {
        row![card, gap].into()
    }
}

/// Parses `GET /v1/conversations` (a bare array, or `{ "conversations": [...] }`).
fn parse_conversations(v: &Value) -> Vec<Conversation> {
    items(v, "conversations")
        .iter()
        .filter_map(|c| {
            let from_cid = c.get("from_cid").and_then(Value::as_u64)?;
            let to_cid = c.get("to_cid").and_then(Value::as_u64)?;
            Some(Conversation {
                from_cid,
                to_cid,
                text: c.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                created_at: c.get("created_at").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                delivered: c.get("delivered").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// Parses `GET /v1/agents` into `(cid, name)`; an agent without a name is "Unnamed", as on the web.
fn parse_agents(v: &Value) -> Vec<(u64, String)> {
    items(v, "agents")
        .iter()
        .filter_map(|a| {
            let cid = a.get("cid").and_then(Value::as_u64)?;
            let raw = a.get("name").and_then(Value::as_str).unwrap_or("");
            let name = if raw.trim().is_empty() { "Unnamed".to_string() } else { raw.to_string() };
            Some((cid, name))
        })
        .collect()
}

/// The array under `key`, or the value itself when it is a bare array.
fn items<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    v.as_array().or_else(|| v.get(key).and_then(Value::as_array)).map(Vec::as_slice).unwrap_or(&[])
}

/// The web's `ago`: the age of a millisecond timestamp, as "just now", "5m ago", "3d ago".
fn ago(ms: i64, now: i64) -> String {
    if ms == 0 {
        return "\u{2014}".to_string();
    }
    let s = ((now - ms) as f64 / 1000.0).round().max(0.0) as i64;
    match s {
        s if s < 5 => "just now".to_string(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// Clips to `max` characters, ending with an ellipsis, as the web's `clip`.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}\u{2026}", text.chars().take(max.saturating_sub(1)).collect::<String>())
    } else {
        text.to_string()
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
