//! The Overview screen (parity with the web console's `overview` view): the plaza at a glance,
//! the orchestrator's chat box, the activity feed and the fleet.
//!
//! The web view is pushed the transcript bus. A screen has no event stream here, so the feed is
//! polled: every load reads each agent's transcript after the last entry seen.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::alignment::Vertical;
use iced::font::{Style as FontStyle, Weight};
use iced::keyboard::{Key, key::Named};
use iced::widget::text::{Span, Wrapping};
use iced::widget::{
    Space, button, column, container, rich_text, row, space, span, text, text_editor,
};
use iced::{Background, Border, Color, Element, Font, Length, Padding, Task, Theme};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::Id;
use crate::console::{Ctx, client, ui};
use crate::theme::{self};

const FEED_MAX: usize = 40;
const FLEET_COLS: usize = 4;
/// The web's `info` tone (dark ground): a working agent.
const INFO: Color = Color::from_rgb8(0x6c, 0xc8, 0xff);
const SANS_BOLD: Font = Font {
    weight: Weight::Bold,
    ..theme::SANS
};
const SANS_ITALIC: Font = Font {
    style: FontStyle::Italic,
    ..theme::SANS
};
const SANS_MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..theme::SANS
};

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    overview: Option<Overview>,
    agents: Vec<Agent>,
    feed: Vec<Item>,
    cursors: HashMap<u64, u64>,
    goal: text_editor::Content,
    sending: bool,
    note: Option<String>,
    chat_error: Option<String>,
    /// A load is in flight; hub events that arrive meanwhile only mark `again`.
    loading: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// One poll of the hub: the overview, the fleet and the new transcript entries.
    Loaded(Result<Snapshot, String>),
    /// An edit to the orchestrator goal.
    Goal(text_editor::Action),
    /// Send the goal to the orchestrator.
    Send,
    /// The chat message was accepted or refused.
    Sent(Result<Value, String>),
    /// A hub event: the roster or the transcript changed.
    Event(crate::console::events::Event),
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

/// The hub's events while the screen is open: the web refreshes the fleet on `agent` and `work`
/// frames and the feed on `transcript` frames.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

/// Loads the screen's data (called when it opens, on every poll tick and on a hub event).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let client = ctx.client.clone();
    let cursors = state.cursors.clone();
    client::task(
        move || {
            let overview: Overview = client.get("/v1/overview")?;
            let agents: Vec<Agent> = client.get("/v1/agents")?;
            let mut feed = Vec::with_capacity(agents.len());
            for a in &agents {
                let path = match cursors.get(&a.cid) {
                    Some(after) => {
                        format!("/v1/agents/{}/transcript?after={after}&limit=200", a.cid)
                    }
                    None => format!("/v1/agents/{}/transcript?after=0&limit=1000", a.cid),
                };
                let entries: Vec<Entry> = client.get(&path)?;
                feed.push((a.cid, entries));
            }
            Ok(Snapshot {
                overview,
                agents,
                feed,
            })
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
                        state.feed.push(Item {
                            at_ms: e.at_ms,
                            cid,
                            line,
                        });
                    }
                }
            }
            state.feed.sort_by_key(|a| std::cmp::Reverse(a.at_ms));
            state.feed.truncate(FEED_MAX);
            state.overview = Some(snap.overview);
            state.agents = snap.agents;
            state.error = None;
            return landed(state, ctx);
        }
        Msg::Loaded(Err(e)) => {
            state.overview = None;
            state.error = Some(e);
            return landed(state, ctx);
        }
        Msg::Event(event) => {
            if !matches!(event.kind.as_str(), "agent" | "work" | "transcript") {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            return load(state, ctx);
        }
        Msg::Goal(action) => state.goal.perform(action),
        Msg::Send => {
            let text = state.goal.text().trim().to_string();
            let Some(cid) = state
                .agents
                .iter()
                .find(|a| a.tier == "orchestrator")
                .map(|a| a.cid)
            else {
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
                move || {
                    client
                        .post::<Value>(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))
                },
                Msg::Sent,
            );
        }
        Msg::Sent(Ok(_)) => {
            state.sending = false;
            state.goal = text_editor::Content::new();
            state.note = Some("Sent to the orchestrator".into());
        }
        Msg::Sent(Err(e)) => {
            state.sending = false;
            state.chat_error = Some(e);
        }
    }
    Task::none()
}

/// A load landed: one more runs if hub events came in while it was in flight.
fn landed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let stats = match &state.overview {
        Some(o) => stat_row(o),
        None => empty("alert", "apid is unreachable", "Start apid, then reload."),
    };
    let mut page = column![stats].spacing(26);
    if let Some(e) = &state.error {
        page = page.push(ui::error_line(e));
    }
    page = page.push(
        row![
            container(orchestrator(state)).width(Length::FillPortion(1)),
            container(activity(state)).width(Length::FillPortion(1)),
        ]
        .spacing(22),
    );
    page = page.push(fleet(&state.agents));
    ui::sheet(ui::view_head("Overview", vec![]), page.into())
}

fn stat_row<'a>(o: &Overview) -> Element<'a, Msg> {
    let open =
        o.tasks.get("open").copied().unwrap_or(0) + o.tasks.get("claimed").copied().unwrap_or(0);
    let done = o.tasks.get("done").copied().unwrap_or(0);
    row![
        stat(
            "Agents",
            o.agents.total.to_string(),
            format!(
                "{} working · {} offline",
                o.agents.working, o.agents.offline
            )
        ),
        stat("Open tasks", open.to_string(), format!("{done} done")),
        stat(
            "Worker model",
            model_name(&o.default_model),
            model_provider(&o.default_model)
        ),
        stat(
            "Orchestrator model",
            model_name(&o.orchestrator_model),
            model_provider(&o.orchestrator_model)
        ),
    ]
    .spacing(22)
    .into()
}

/// A stat tile: the figure in bold mono first, its label under it, an italic serif sub-line.
fn stat<'a>(label: &str, value: String, sub: String) -> Element<'a, Msg> {
    let body = column![
        text(value)
            .font(ui::SANS_BOLD)
            .size(34.0)
            .color(crate::theme::palette().ink),
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(10.5)
            .color(crate::theme::palette().ink_3),
        text(sub)
            .font(SANS_ITALIC)
            .size(13.0)
            .color(crate::theme::palette().ink_2),
    ]
    .spacing(6);
    container(body)
        .padding(Padding {
            top: 14.0,
            right: 16.0,
            bottom: 16.0,
            left: 16.0,
        })
        .width(Length::FillPortion(1))
        .height(Length::Fill)
        .style(rule_box)
        .into()
}

fn orchestrator<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(orch) = state.agents.iter().find(|a| a.tier == "orchestrator") else {
        return card(
            card_title("Orchestrator"),
            empty(
                "orchestrator",
                "No orchestrator connected",
                "Add the orchestrator role to the fleet to delegate work by goal.",
            ),
            false,
        );
    };
    let (status, tone, filled) = match orch.status.as_str() {
        "working" => ("Working", INFO, false),
        "idle" => ("Idle", crate::theme::palette().ok, true),
        "asleep" => ("Asleep", crate::theme::palette().ink_2, false),
        "dozing" => ("Asleep (light)", crate::theme::palette().ink_2, false),
        _ => ("Offline", crate::theme::palette().ink_2, false),
    };
    let head = row![
        ui::icon::<Msg>("orchestrator", 15.0, crate::theme::palette().ink),
        text(name(orch).to_string())
            .font(ui::SANS_SEMIBOLD)
            .size(11.5)
            .color(crate::theme::palette().ink),
        ui::faint(orch.role_label.clone()),
        marker_tag(status, tone, filled),
    ]
    .spacing(10)
    .align_y(Vertical::Center);

    let goal = text_editor(&state.goal)
        .placeholder("Tell the orchestrator what you want done…")
        .on_action(Msg::Goal)
        .key_binding(|press| {
            if matches!(press.key, Key::Named(Named::Enter))
                && (press.modifiers.control() || press.modifiers.logo())
            {
                return Some(text_editor::Binding::Custom(Msg::Send));
            }
            text_editor::Binding::from_key_press(press)
        })
        .font(theme::SANS)
        .size(15.0)
        .padding(Padding {
            top: 6.0,
            right: 0.0,
            bottom: 7.0,
            left: 0.0,
        })
        .height(Length::Fixed(84.0))
        .style(|_: &Theme, _| text_editor::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            placeholder: crate::theme::palette().ink_3,
            value: crate::theme::palette().ink,
            selection: crate::theme::palette().signal,
        });

    let mut body = column![
        text(format!(
            "Plans the work and delegates it to the fleet · {}",
            orch.model
        ))
        .font(theme::SANS)
        .size(15.0)
        .color(crate::theme::palette().ink_2),
        column![goal, rule(crate::theme::palette().ink)],
        row![
            ui::faint("Ctrl+Enter to send"),
            space::horizontal(),
            link("Open transcript", 14.0, Msg::Go(Id::Agent(orch.cid))),
            ui::btn(
                "Send",
                Some("send"),
                ui::Kind::Primary,
                (!state.sending).then_some(Msg::Send)
            ),
        ]
        .spacing(14)
        .align_y(Vertical::Center),
    ]
    .spacing(14);
    if let Some(n) = &state.note {
        body = body.push(ui::faint(n.as_str()));
    }
    if let Some(e) = &state.chat_error {
        body = body.push(ui::error_line(e));
    }
    card(head.into(), body.into(), false)
}

fn activity<'a>(state: &'a State) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if state.feed.is_empty() {
        container(ui::faint("Waiting for activity…"))
            .padding(Padding {
                top: 6.0,
                right: 20.0,
                bottom: 6.0,
                left: 20.0,
            })
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        let now = now_ms();
        let mut list = column![].spacing(0);
        for item in &state.feed {
            let who = state
                .agents
                .iter()
                .find(|a| a.cid == item.cid)
                .map(name)
                .unwrap_or_else(|| format!("agent {}", item.cid));
            list = list.push(
                container(
                    row![
                        ui::faint(ago(now, item.at_ms)).width(Length::Fixed(72.0)),
                        link(&who, 13.0, Msg::Go(Id::Agent(item.cid))),
                        text(item.line.clone())
                            .font(theme::SANS)
                            .size(13.0)
                            .color(crate::theme::palette().ink_2)
                            .wrapping(Wrapping::None)
                            .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(Vertical::Center),
                )
                .padding(Padding {
                    top: 6.0,
                    right: 20.0,
                    bottom: 6.0,
                    left: 20.0,
                })
                .width(Length::Fill),
            );
        }
        // The web feed scrolls past 360px and is shorter than that when it has less to show.
        container(ui::scroll(container(list).padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 8.0,
            left: 0.0,
        })))
        .max_height(360.0)
        .into()
    };
    card(card_title("Activity"), body, true)
}

fn fleet<'a>(agents: &'a [Agent]) -> Element<'a, Msg> {
    if agents.is_empty() {
        return card(
            card_title("Fleet"),
            empty(
                "agents",
                "No agents yet",
                "Agents appear here as their VMs connect.",
            ),
            false,
        );
    }
    let mut grid = column![].spacing(8);
    for chunk in agents.chunks(FLEET_COLS) {
        let mut line = row![].spacing(8);
        for a in chunk {
            line = line.push(tile(a));
        }
        for _ in chunk.len()..FLEET_COLS {
            line = line.push(space::horizontal());
        }
        grid = grid.push(line);
    }
    card(
        card_title("Fleet"),
        container(grid).padding(16).width(Length::Fill).into(),
        true,
    )
}

fn tile<'a>(a: &'a Agent) -> Element<'a, Msg> {
    let (tone, filled) = match a.status.as_str() {
        "working" => (INFO, true),
        "idle" => (crate::theme::palette().ok, true),
        _ => (crate::theme::palette().ink_3, false),
    };
    let sub = match &a.work {
        Some(w) => clip(&w.prompt, 60),
        None if a.status == "offline" => "offline".to_string(),
        None if a.status == "asleep" => "asleep".to_string(),
        None if a.status == "dozing" => "asleep (light)".to_string(),
        None => a.role_label.clone(),
    };
    let body = column![
        row![
            marker(tone, filled),
            text(name(a))
                .font(SANS_BOLD)
                .size(13.0)
                .color(crate::theme::palette().ink)
        ]
        .spacing(8)
        .align_y(Vertical::Center),
        ui::faint(sub),
    ]
    .spacing(2);
    button(body)
        .on_press(Msg::Go(Id::Agent(a.cid)))
        .style(tile_style)
        .padding(12.0)
        .width(Length::FillPortion(1))
        .into()
}

/// A fleet tile: a ruled box on the surface, lifted on hover.
fn tile_style(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            crate::theme::palette().rule_faint
        } else {
            crate::theme::palette().hover
        })),
        text_color: crate::theme::palette().ink,
        border: Border {
            color: if hovered {
                crate::theme::palette().ink_3
            } else {
                crate::theme::palette().rule
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// A card in the web's language: a mono head over a rule, the body below; a 1px rule frames it.
/// The body is flush (no padding) when the content brings its own.
fn card<'a>(head: Element<'a, Msg>, body: Element<'a, Msg>, flush: bool) -> Element<'a, Msg> {
    let head = container(head)
        .padding(Padding {
            top: 12.0,
            right: 16.0,
            bottom: 10.0,
            left: 16.0,
        })
        .width(Length::Fill);
    let body = container(body)
        .padding(if flush { 0.0 } else { 16.0 })
        .width(Length::Fill);
    container(column![head, rule(crate::theme::palette().rule), body])
        .width(Length::Fill)
        .style(rule_box)
        .into()
}

/// A card's title: mono, semibold, uppercase.
fn card_title<'a>(title: &str) -> Element<'a, Msg> {
    text(title.to_string())
        .font(ui::SANS_SEMIBOLD)
        .size(11.5)
        .color(crate::theme::palette().ink)
        .into()
}

/// An empty state: an icon, a bold mono line, a faint serif sentence; centred.
fn empty<'a>(icon_name: &str, title: &str, sentence: &str) -> Element<'a, Msg> {
    container(
        column![
            ui::icon::<Msg>(icon_name, 16.0, crate::theme::palette().ink_3),
            text(title.to_string())
                .font(ui::SANS_BOLD)
                .size(13.0)
                .color(crate::theme::palette().ink),
            text(sentence.to_string())
                .font(theme::SANS)
                .size(14.0)
                .color(crate::theme::palette().ink_3),
        ]
        .spacing(10)
        .align_x(iced::alignment::Horizontal::Center),
    )
    .padding(Padding {
        top: 40.0,
        right: 16.0,
        bottom: 40.0,
        left: 16.0,
    })
    .width(Length::Fill)
    .align_x(iced::alignment::Horizontal::Center)
    .into()
}

/// A status tag: a square marker and an uppercase mono label in its colour.
fn marker_tag<'a>(label: &str, color: Color, filled: bool) -> Element<'a, Msg> {
    row![
        marker(color, filled),
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(10.5)
            .color(color)
    ]
    .spacing(7)
    .align_y(Vertical::Center)
    .into()
}

/// The web's 8px square mark: outlined, or filled when it stands for ok, info or a working agent.
fn marker<'a>(color: Color, filled: bool) -> Element<'a, Msg> {
    container(
        Space::new()
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: filled.then_some(Background::Color(color)),
        border: Border {
            color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// A text link: serif, underlined, as the web's links are.
fn link<'a>(label: &str, size: f32, on_press: Msg) -> Element<'a, Msg> {
    let spans: Vec<Span<'_, ()>> = vec![
        span(label.to_string())
            .underline(true)
            .font(theme::SANS)
            .size(size)
            .color(crate::theme::palette().ink),
    ];
    button(rich_text(spans))
        .on_press(on_press)
        .padding(0)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: None,
            text_color: if matches!(status, button::Status::Hovered) {
                crate::theme::palette().signal
            } else {
                crate::theme::palette().ink
            },
            ..Default::default()
        })
        .into()
}

/// A full-width ruled line.
fn rule<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// A ruled box with no fill: the web's `.card` and `.stat` frame.
fn rule_box(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// The name an agent chose for itself, or "Unnamed" while it has none.
fn name(a: &Agent) -> String {
    if a.name.trim().is_empty() {
        "Unnamed".to_string()
    } else {
        a.name.clone()
    }
}

/// The model's name: everything after the provider (`openrouter/anthropic/claude-x` keeps `anthropic/claude-x`).
fn model_name(model: &str) -> String {
    model
        .split_once('/')
        .map_or(model, |(_, rest)| rest)
        .to_string()
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
        "work_start" => Some(format!(
            "started {}: {}",
            text("kind"),
            clip(&text("prompt"), 80)
        )),
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
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
