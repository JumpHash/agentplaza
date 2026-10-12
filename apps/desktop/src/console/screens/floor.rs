//! The Floor screen (parity with the web console's `floor` view): the 3D floor in a shader widget,
//! with name labels, the agent card and the legend over it.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use iced::widget::{column, container, pin, row, shader, stack, Space};
use iced::{Alignment, Border, Color, Element, Length, Point, Shadow, Subscription, Task, Theme};
use serde_json::Value;

use super::Id;
use crate::console::floor3d::scene::{Color as SceneColor, Frame};
use crate::console::floor3d::world::view::{FloorProgram, Label};
use crate::console::floor3d::world::{AgentInfo, Floor};
use crate::console::{client, events, ui, Ctx};
use crate::theme::OK;

/// The card's width and the field of view the orbit pans with (degrees, as the camera has it).
const CARD_W: f32 = 284.0;
const FOV_DEG: f32 = 40.0;
/// The stage's height on the sheet.
const STAGE_H: f32 = 640.0;

pub struct State {
    pub floor: Floor,
    /// The frame the shader draws; rebuilt on every animation tick.
    pub frame: Arc<Frame>,
    /// The stage's size in pixels, as the shader reports it.
    pub size: (f32, f32),
    pub error: Option<String>,
    /// The previous animation tick, for the step's dt.
    pub last: Option<Instant>,
    /// Device pixels per logical pixel, for the labels' textures (2 for crisp text: the window's
    /// scale factor is not known to the widget).
    pub scale: f32,
}

impl Default for State {
    fn default() -> State {
        let mut floor = Floor::new();
        let frame = Arc::new(floor.frame(1.0));
        State { floor, frame, size: (0.0, 0.0), error: None, last: None, scale: 2.0 }
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").field("error", &self.error).field("size", &self.size).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// An animation tick from the window, with its instant.
    Frame(Instant),
    /// The agent list from `GET /v1/agents`.
    Agents(Result<Value, String>),
    /// One event from the hub's stream: it moves the robots.
    Event(events::Event),
    /// The stage changed size.
    Resized(f32, f32),
    /// Left drag: orbit, by pixels, in a view `height` tall.
    Rotate { dx: f32, dy: f32, height: f32 },
    /// Right or middle drag: pan.
    Pan { dx: f32, dy: f32, height: f32 },
    /// Wheel: zoom by `steps` (positive zooms out).
    Dolly(f32),
    /// The pointer moved over the stage (`None` when it left).
    Hover(Option<Point>),
    /// A click on the stage (`None` when it was outside).
    Select(Option<Point>),
    ResetView,
    /// Navigate to another screen.
    Go(Id),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Animation frames from the window, while the Floor is the shown screen.
pub fn subscription() -> Subscription<Msg> {
    // On a software adapter, 30 frames a second: every frame costs the CPU the whole picture.
    if crate::console::floor3d::render::light() {
        return iced::time::every(std::time::Duration::from_millis(33)).map(Msg::Frame);
    }
    iced::window::frames().map(Msg::Frame)
}

/// The hub's events, while the Floor is open.
pub fn events(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// Loads the agents (called when the screen opens and on every poll tick, every 4 s).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || client.call("GET", "/v1/agents", None), Msg::Agents)
}

pub fn update(state: &mut State, msg: Msg, _ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Frame(now) => {
            let started = Instant::now();
            let dt = state.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32()).min(0.05);
            state.last = Some(now);
            state.floor.orbit.update();
            state.floor.step(dt, now_ms(now));
            let (w, h) = state.size;
            if w > 0.0 && h > 0.0 {
                let mut frame = state.floor.frame(w / h);
                frame.overlays = state.floor.label_overlays(w, h, state.scale);
                state.frame = Arc::new(frame);
            }
            perf(started.elapsed(), now);
        }
        Msg::Agents(Ok(v)) => {
            state.error = None;
            let list = agent_list(&v);
            state.floor.sync(&list);
            if !state.floor.fitted && !list.is_empty() {
                state.floor.reset_view();
                state.floor.fitted = true;
            }
        }
        Msg::Agents(Err(e)) => state.error = Some(e),
        Msg::Resized(w, h) => state.size = (w, h),
        Msg::Rotate { dx, dy, height } => state.floor.orbit.rotate(dx, dy, height),
        Msg::Pan { dx, dy, height } => state.floor.orbit.pan(dx, dy, height, FOV_DEG),
        Msg::Dolly(steps) => state.floor.orbit.dolly(steps),
        Msg::Hover(at) => {
            let (w, h) = state.size;
            let hit = at.and_then(|p| state.floor.pick(p.x, p.y, w, h));
            state.floor.hovered = hit;
        }
        Msg::Select(at) => {
            let (w, h) = state.size;
            let hit = at.and_then(|p| state.floor.pick(p.x, p.y, w, h));
            state.floor.selected = hit;
        }
        Msg::Event(e) => state.floor.on_event(&e.kind, &e.data),
        Msg::ResetView => state.floor.reset_view(),
        Msg::Go(_) => {}
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(
        "Floor",
        vec![ui::btn("Reset view", Some("refresh"), ui::Kind::Secondary, Some(Msg::ResetView))],
    );

    let mut layers: Vec<Element<'a, Msg>> = vec![
        shader(FloorProgram { primitive: Arc::clone(&state.frame) }).width(Length::Fill).height(Length::Fill).into(),
    ];
    let (w, h) = state.size;
    if w > 0.0 && h > 0.0 {
        let labels = state.floor.labels(w, h);
        if let Some(label) = state.floor.selected.and_then(|sel| labels.iter().find(|l| l.cid == sel)) {
            layers.push(card(state, label, w, h));
        }
    }
    layers.push(
        container(legend())
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Start)
            .align_y(Alignment::End)
            .padding(12)
            .into(),
    );

    let stage = container(stack(layers))
        .width(Length::Fill)
        .height(Length::Fixed(STAGE_H))
        .style(|_: &Theme| container::Style {
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        });

    let mut body = column![].spacing(12);
    if let Some(e) = &state.error {
        body = body.push(ui::error_line(e));
    }
    ui::sheet(head, body.push(stage).into())
}

/// The agent card (`renderCard` and `positionCard`): the selected robot's name, role, status,
/// model, activity and an Open button. Beside the robot, kept inside the stage.
fn card<'a>(state: &'a State, l: &Label, w: f32, h: f32) -> Element<'a, Msg> {
    let (status, model) = state
        .floor
        .bot(l.cid)
        .map(|b| (b.agent.status.clone(), b.agent.model.clone()))
        .unwrap_or_default();
    let tone = match status.as_str() {
        "working" => ui::Tone::Signal,
        "offline" => ui::Tone::Muted,
        _ => ui::Tone::Ok,
    };
    let body = column![
        row![ui::mono(l.name.clone(), 13.0, ui::INK), Space::new().width(Length::Fill), role_tag(&l.role, l.hue)]
            .spacing(8)
            .align_y(Alignment::Center),
        ui::mono(l.line.clone(), 11.0, ui::INK_3),
        row![ui::label("Status"), ui::badge(&status, tone)].spacing(10).align_y(Alignment::Center),
        row![ui::label("Model"), ui::mono(model, 11.0, ui::INK)].spacing(10).align_y(Alignment::Center),
        ui::btn("Open", None, ui::Kind::Primary, Some(Msg::Go(Id::Agent(l.cid)))),
    ]
    .spacing(10);
    let card = container(body)
        .padding(12)
        .width(Length::Fixed(CARD_W))
        .style(move |_: &Theme| boxed_style(false));
    let left = clamp(l.x + 24.0, 8.0, (w - CARD_W - 24.0).max(8.0));
    let top = clamp(l.y - 120.0, 8.0, (h - 260.0).max(8.0));
    pin(card).x(left).y(top).into()
}

/// The legend's keys, bottom-left of the stage.
fn legend<'a>() -> Element<'a, Msg> {
    let body = row![key(ui::SIGNAL, "Working"), key(OK, "Idle"), key(ui::INK_3, "Offline")]
        .spacing(14)
        .align_y(Alignment::Center);
    container(body)
        .padding([6, 10])
        .style(|_: &Theme| container::Style {
            background: Some(ui::SHEET.into()),
            border: Border { color: ui::INK, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

fn key<'a>(colour: Color, name: &str) -> Element<'a, Msg> {
    let swatch = container(Space::new().width(10.0).height(10.0)).style(move |_: &Theme| container::Style {
        background: Some(colour.into()),
        border: Border { color: ui::INK, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    });
    row![swatch, ui::mono(name.to_string(), 10.5, ui::INK)].spacing(6).align_y(Alignment::Center).into()
}

/// The role as a square tag with a 3 px stripe in the district's colour.
fn role_tag<'a>(role: &str, hue: f32) -> Element<'a, Msg> {
    let rgb = SceneColor::hsl(hue, 0.55, 0.34).to_srgb();
    let stripe = container(Space::new().width(3.0).height(16.0)).style(move |_: &Theme| container::Style {
        background: Some(Color::from_rgb(rgb[0], rgb[1], rgb[2]).into()),
        ..Default::default()
    });
    let text = role.replace('_', " ").to_uppercase();
    container(row![stripe, ui::mono(text, 9.5, ui::INK)].spacing(4).align_y(Alignment::Center))
        .padding([1, 5])
        .style(|_: &Theme| container::Style {
            border: Border { color: ui::INK, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// The print look shared by labels and the card: sheet, 1px ink border, a hard shadow.
fn boxed_style(highlighted: bool) -> container::Style {
    container::Style {
        background: Some(ui::SHEET.into()),
        border: Border { color: if highlighted { ui::SIGNAL } else { ui::INK }, width: 1.0, radius: 0.0.into() },
        // The print shadow (2px 2px 0 ink). iced drops the text of a box whose shadow has exactly
        // zero blur, so a hair of blur keeps it crisp and the text drawn.
        shadow: Shadow { color: ui::INK, offset: iced::Vector::new(2.0, 2.0), blur_radius: 0.6 },
        ..Default::default()
    }
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

/// Milliseconds on a monotonic clock that starts at the first frame.
fn now_ms(at: Instant) -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    at.saturating_duration_since(*EPOCH.get_or_init(|| at)).as_secs_f64() * 1000.0
}

/// The agents from the hub's list: a bare array, or `{ "agents": [...] }`.
fn agent_list(v: &Value) -> Vec<AgentInfo> {
    let items = v.as_array().or_else(|| v.get("agents").and_then(Value::as_array)).cloned().unwrap_or_default();
    items
        .iter()
        .filter_map(|a| {
            let text = |key: &str| a.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
            let status = text("status");
            Some(AgentInfo {
                cid: a.get("cid")?.as_u64()?,
                name: text("name"),
                role: text("role"),
                status: if status.is_empty() { "idle".to_string() } else { status },
                tier: text("tier"),
                model: text("model"),
            })
        })
        .collect()
}

/// `APZ_PERF`: frames per second and the floor's CPU time per frame, every two seconds.
fn perf(cpu: std::time::Duration, now: Instant) {
    use std::sync::Mutex;
    static STATS: Mutex<Option<(Instant, u32, std::time::Duration)>> = Mutex::new(None);
    if std::env::var_os("APZ_PERF").is_none() {
        return;
    }
    let mut stats = STATS.lock().unwrap_or_else(|p| p.into_inner());
    let (since, frames, total) = stats.get_or_insert((now, 0, std::time::Duration::ZERO));
    *frames += 1;
    *total += cpu;
    let span = now.saturating_duration_since(*since);
    if span.as_secs_f32() >= 2.0 {
        eprintln!(
            "agentplaza-desktop: floor {:.1} fps, {:.2} ms CPU per frame (step + scene)",
            *frames as f32 / span.as_secs_f32(),
            total.as_secs_f64() * 1000.0 / f64::from(*frames)
        );
        *stats = None;
    }
}
