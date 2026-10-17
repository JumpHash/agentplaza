//! The Machines grid (parity with the web console's `floor` view): the fleet's machines on the 3D floor in a shader
//! widget, with their name tags, the machine popup and the legend over it.
//!
//! The grid shows the Machines view only: the fleet from `GET /v1/fleet` on the Floor's sky and ground
//! (`State::machines` + `State::fleet`). Hover lifts a machine and a click opens its popup (Esc closes it); the popup
//! lists the machine's agents, and each name opens that agent's screen.
//!
//! `APZ_PERF=1` also logs to stderr: each frame whose wall gap from the last one is over 16 ms (with its work time),
//! and each update over 16 ms, with times from the screen's start.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glam::Vec3;
use iced::widget::{Space, column, container, opaque, pin, row, shader, stack};
use iced::{Alignment, Border, Color, Element, Length, Point, Shadow, Subscription, Task, Theme};
use serde_json::Value;

use super::Id;
use crate::console::engine::fleet::{FleetResponse, Machine};
use crate::console::engine::scene::Color as SceneColor;
use crate::console::engine::style::{Lamp, Palette};
use crate::console::engine::world::display_name;
use crate::console::floor_engine::EngineProgram;
use crate::console::{Ctx, client, ui};
use apz_engine::input::Action;
use apz_engine::stage::{Stage, ms_since, perf_line, perf_log_on, perf_t};
use apz_engine::{Engine, Motion};

/// The machine popup's width, and its height, for keeping it inside the stage (its rows are not measured).
const POPUP_W: f32 = 300.0;
const POPUP_H: f32 = 420.0;
/// The hue of the popup's kind tag stripe.
const KIND_HUE: f32 = 200.0;
/// How many agent names the popup lists.
const POPUP_NAMES: usize = 6;
/// The fleet poll.
const FLEET_EVERY: Duration = Duration::from_secs(5);

/// The grid's state: the engine (its stage is what the Floor shows and how it moves), the motion of the
/// last frame (how often the window should draw next), plus the screen's own timing origin. The screen reads and
/// drives the stage through `Deref`.
pub struct State(pub Engine, Motion);

impl std::ops::Deref for State {
    type Target = Stage;
    fn deref(&self) -> &Stage {
        &self.0.stage
    }
}

impl std::ops::DerefMut for State {
    fn deref_mut(&mut self) -> &mut Stage {
        &mut self.0.stage
    }
}

impl Default for State {
    fn default() -> State {
        perf_t(Instant::now()); // the timing lines' origin: the screen's start
        let mut engine = Engine::detached();
        engine.set_scale(2.0);
        State(engine, Motion::Active) // the first frame draws
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.stage.fmt(f)
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// An animation tick from the window, with its instant.
    Frame(Instant),
    /// The agent list from `GET /v1/agents`.
    Agents(Result<Value, String>),
    /// The fleet from `GET /v1/fleet`.
    Fleet(Result<Value, String>),
    /// The fleet poll's tick (every 5 s).
    FleetTick,
    /// The stage changed size.
    Resized(f32, f32),
    /// Left drag: orbit, by pixels, in a view `height` tall.
    Rotate {
        dx: f32,
        dy: f32,
        height: f32,
    },
    /// Right or middle drag: pan.
    Pan {
        dx: f32,
        dy: f32,
        height: f32,
    },
    /// Wheel: zoom by `steps` (positive zooms out).
    Dolly(f32),
    /// The pointer moved over the stage (`None` when it left).
    Hover(Option<Point>),
    /// A click on the stage (`None` when it was outside).
    Select(Option<Point>),
    ResetView,
    /// Esc: closes the machine popup.
    Escape,
    /// Navigate to another screen.
    Go(Id),
    /// The popup's "Open machine": the Machines screen shows the machine's section and its 3D view.
    OpenMachine(String),
}

/// Animation frames from the window, the fleet poll and Esc, while the grid is the shown view. The frames follow
/// the last frame's motion: every frame while something is active, 12 a second while only the decoration turns,
/// none at rest (a change or input brings them back on the next update).
pub fn subscription(state: &State) -> Subscription<Msg> {
    let frames = match state.1 {
        // On a software adapter, 30 frames a second: every frame costs the CPU the whole picture.
        Motion::Active if crate::console::engine::render::light() => {
            iced::time::every(Duration::from_millis(33)).map(Msg::Frame)
        }
        Motion::Active => iced::window::frames().map(Msg::Frame),
        // The idle fleet at 30 frames a second, as the web draws it (12 read as stutter).
        Motion::Ambient => iced::time::every(Duration::from_millis(33)).map(Msg::Frame),
        Motion::Rest => Subscription::none(),
    };
    let poll = iced::time::every(FLEET_EVERY).map(|_| Msg::FleetTick);
    let esc = iced::event::listen_with(|event, _status, _window| match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        }) => Some(Msg::Escape),
        _ => None,
    });
    Subscription::batch([frames, poll, esc])
}

/// Loads the agents (called when the screen opens and on every poll tick, every 4 s), and the
/// fleet the first time the Floor opens.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let agents = client::task(move || client.call("GET", "/v1/agents", None), Msg::Agents);
    if state.fleet_started {
        return agents;
    }
    Task::batch([agents, fleet_request(state, ctx)])
}

/// Sends `GET /v1/fleet` (one at a time).
fn fleet_request(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.fleet_asking = true;
    state.fleet_started = true;
    let client = ctx.client.clone();
    client::task(move || client.call("GET", "/v1/fleet", None), Msg::Fleet)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let mut task = Task::none();
    let perf_msg = perf_log_on().then(|| {
        (
            format!("{msg:?}").chars().take(32).collect::<String>(),
            Instant::now(),
        )
    });
    // Any message but a frame changes the screen: the engine marks it, and the next frame draws at full rate.
    if !matches!(msg, Msg::Frame(_)) {
        state.0.touch();
        state.1 = Motion::Active;
    }
    match msg {
        Msg::Frame(now) => {
            let started = Instant::now();
            // The machines follow the console's theme (the stage rebuilds only when it changed).
            let style = state.style;
            state.0.set_style(style, !crate::theme::is_light());
            let size = state.size;
            let out = state.0.advance(now, size);
            state.1 = out.motion;
            perf(started.elapsed(), now);
            frame_log(now, started.elapsed());
        }
        Msg::Agents(Ok(v)) => state.apply_agents(apz_engine::agent_list(&v)),
        Msg::Agents(Err(e)) => state.error = Some(e),
        Msg::Fleet(res) => {
            state.fleet_asking = false;
            match res
                .and_then(|v| serde_json::from_value::<FleetResponse>(v).map_err(|e| e.to_string()))
            {
                Ok(data) => state.apply_fleet(data),
                Err(e) => state.fleet_failed(e),
            }
        }
        Msg::FleetTick => {
            if !state.fleet_asking {
                task = fleet_request(state, ctx);
            }
        }
        Msg::Resized(w, h) => state.resize(w, h),
        Msg::Rotate { dx, dy, height } => state.rotate(dx, dy, height),
        Msg::Pan { dx, dy, height } => state.pan(dx, dy, height),
        Msg::Dolly(steps) => state.dolly(steps),
        Msg::Hover(at) => state.hover(at.map(|p| (p.x, p.y))),
        Msg::Select(at) => state.select(at.map(|p| (p.x, p.y))),
        Msg::ResetView => state.reset_view(),
        Msg::Escape => state.popup = None,
        Msg::Go(_) | Msg::OpenMachine(_) => {}
    }
    if let Some((name, t0)) = perf_msg
        && ms_since(t0) > 16.0
    {
        perf_line(&format!(
            "update {name} took {:.1} ms at {:.3} s",
            ms_since(t0),
            perf_t(t0)
        ));
    }
    task
}

/// The engine's picture alone (the shader the Floor draws in, with its pointer input): the labs draw it under their
/// own controls.
pub fn stage<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    shader(EngineProgram {
        primitive: Arc::clone(&state.frame),
        wrap: floor_msg,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The floor's engine actions as its messages.
fn floor_msg(action: Action) -> Msg {
    match action {
        Action::Resized { width, height } => Msg::Resized(width, height),
        Action::Rotate { dx, dy, height } => Msg::Rotate { dx, dy, height },
        Action::Pan { dx, dy, height } => Msg::Pan { dx, dy, height },
        Action::Hover(p) => Msg::Hover(p.map(|(x, y)| Point::new(x, y))),
        Action::Select((x, y)) => Msg::Select(Some(Point::new(x, y))),
        Action::Dolly(steps) => Msg::Dolly(steps),
    }
}

/// The grid's header action: the view's reset.
pub fn reset_button<'a>() -> Element<'a, Msg> {
    ui::btn(
        "Reset view",
        Some("refresh"),
        ui::Kind::Secondary,
        Some(Msg::ResetView),
    )
}

/// The grid's body: the stage with its popup and legend, under the errors (the screen's sheet holds the header).
pub fn body<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut layers: Vec<Element<'a, Msg>> = vec![
        shader(EngineProgram {
            primitive: Arc::clone(&state.frame),
            wrap: floor_msg,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    ];
    let (w, h) = state.size;
    if w > 0.0
        && h > 0.0
        && let Some(layer) = popup_layer(state, w, h)
    {
        layers.push(layer);
    }
    layers.push(
        container(legend(&state.palette()))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Start)
            .align_y(Alignment::End)
            .padding(12)
            .into(),
    );

    let stage = container(stack(layers))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });

    let mut body = column![].spacing(12).height(Length::Fill);
    if let Some(e) = &state.error {
        body = body.push(ui::error_line(e));
    }
    if let Some(e) = &state.fleet_error {
        body = body.push(ui::error_line(&format!("Machines: {e}")));
    }
    body.push(stage).into()
}

/// The popup of the open machine, beside its tag (none while the fleet or the machine is missing).
fn popup_layer<'a>(state: &'a State, w: f32, h: f32) -> Option<Element<'a, Msg>> {
    let id = state.popup.as_ref()?;
    let fleet = state.fleet.as_ref()?;
    let machine = state
        .fleet_data
        .as_ref()?
        .machines
        .iter()
        .find(|m| m.id == *id)?;
    let tag = fleet.tags().into_iter().find(|t| t.id == *id)?;
    let (x, y) = state
        .machines
        .orbit
        .camera(w / h)
        .to_screen(Vec3::from(tag.pos), w, h)?;
    Some(popup(state, machine, x, y, w, h))
}

/// The machine popup (the agent card's look): the name and kind, the status, the facts and the agents. Each agent
/// name opens that agent's screen.
fn popup<'a>(state: &'a State, m: &'a Machine, x: f32, y: f32, w: f32, h: f32) -> Element<'a, Msg> {
    let mut facts_col = column![].spacing(5);
    for (label_text, value) in facts(m, now_millis()) {
        facts_col = facts_col.push(
            row![
                container(ui::label(label_text)).width(Length::Fixed(76.0)),
                ui::sans(value, 10.5, crate::theme::palette().ink)
            ]
            .spacing(8)
            .align_y(Alignment::Start),
        );
    }

    let working = m.working as usize;
    let idle = m.agents.len().saturating_sub(working);
    let mut names = column![].spacing(2);
    if m.agents.is_empty() {
        names = names.push(ui::faint("No agents run here."));
    }
    for cid in m.agents.iter().take(POPUP_NAMES) {
        let name = state
            .agents
            .iter()
            .find(|a| a.cid == *cid)
            .map_or_else(|| format!("agent {cid}"), |a| display_name(*cid, &a.name));
        names = names.push(ui::btn(
            &name,
            None,
            ui::Kind::Ghost,
            Some(Msg::Go(Id::Agent(*cid))),
        ));
    }
    if m.agents.len() > POPUP_NAMES {
        names = names.push(ui::faint(format!(
            "and {} more",
            m.agents.len() - POPUP_NAMES
        )));
    }

    let model = format!(
        "{} · {}",
        m.kind,
        m.model.as_deref().unwrap_or("model unknown")
    );
    let body = column![
        row![
            ui::sans(m.name.clone(), 13.0, crate::theme::palette().ink),
            Space::new().width(Length::Fill),
            role_tag(&m.kind, KIND_HUE)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::sans(model, 11.0, crate::theme::palette().ink_3),
        // "Open machine" sits by the status, near the top, so a long card never hides it.
        row![
            ui::label("Status"),
            ui::badge(&m.status, status_tone(&m.status)),
            Space::new().width(Length::Fill),
            ui::btn(
                "Open machine",
                None,
                ui::Kind::Primary,
                Some(Msg::OpenMachine(m.id.clone())),
            )
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        facts_col,
        row![
            ui::label("Agents"),
            ui::sans(
                format!("{working} working, {idle} idle"),
                11.0,
                crate::theme::palette().ink
            )
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        names,
    ]
    .spacing(10);
    let card = container(body)
        .padding(12)
        .width(Length::Fixed(POPUP_W))
        .style(move |_: &Theme| boxed_style(false));
    let left = clamp(x + 24.0, 8.0, (w - POPUP_W - 24.0).max(8.0));
    let top = clamp(y - 120.0, 8.0, (h - POPUP_H).max(8.0));
    pin(opaque(card)).x(left).y(top).into()
}

/// The legend's keys, bottom-left of the stage: the machines' status lights, in the stage's palette.
fn legend<'a>(palette: &Palette) -> Element<'a, Msg> {
    let body = row![
        key(lamp_colour(palette, Lamp::Working), "Working"),
        key(lamp_colour(palette, Lamp::Idle), "Idle"),
        key(lamp_colour(palette, Lamp::Strained), "Draining"),
        key(lamp_colour(palette, Lamp::Offline), "Offline"),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    container(body)
        .padding([6, 10])
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet.into()),
            border: Border {
                color: crate::theme::palette().ink,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn key<'a>(colour: Color, name: &str) -> Element<'a, Msg> {
    let swatch =
        container(Space::new().width(10.0).height(10.0)).style(move |_: &Theme| container::Style {
            background: Some(colour.into()),
            border: Border {
                color: crate::theme::palette().ink,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    row![
        swatch,
        ui::sans(name.to_string(), 10.5, crate::theme::palette().ink)
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// The role as a square tag with a 3 px stripe in the district's colour.
fn role_tag<'a>(role: &str, hue: f32) -> Element<'a, Msg> {
    let rgb = SceneColor::hsl(hue, 0.55, 0.34).to_srgb();
    let stripe =
        container(Space::new().width(3.0).height(16.0)).style(move |_: &Theme| container::Style {
            background: Some(Color::from_rgb(rgb[0], rgb[1], rgb[2]).into()),
            ..Default::default()
        });
    let text = role.replace('_', " ");
    container(
        row![stripe, ui::sans(text, 9.5, crate::theme::palette().ink)]
            .spacing(4)
            .align_y(Alignment::Center),
    )
    .padding([1, 5])
    .style(|_: &Theme| container::Style {
        border: Border {
            color: crate::theme::palette().ink,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// A status light's colour in `palette`, as the iced colour.
fn lamp_colour(palette: &Palette, lamp: Lamp) -> Color {
    let rgb = palette.lamp(lamp).to_srgb();
    Color::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// The badge tone of a machine's status.
fn status_tone(status: &str) -> ui::Tone {
    match status {
        "online" => ui::Tone::Ok,
        "draining" => ui::Tone::Warn,
        "revoked" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// The print look shared by the popup and the legend: sheet, 1px ink border, a hard shadow.
fn boxed_style(highlighted: bool) -> container::Style {
    container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: if highlighted {
                crate::theme::palette().signal
            } else {
                crate::theme::palette().ink
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        // The print shadow (2px 2px 0 ink). iced drops the text of a box whose shadow has exactly
        // zero blur, so a hair of blur keeps it crisp and the text drawn.
        shadow: Shadow {
            color: crate::theme::palette().ink,
            offset: iced::Vector::new(2.0, 2.0),
            blur_radius: 0.6,
        },
        ..Default::default()
    }
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

/// The popup's facts as label and value rows, from the machine's snapshot at `now` (epoch ms).
fn facts(m: &Machine, now: u64) -> Vec<(&'static str, String)> {
    let cpu = match &m.apple {
        Some(a) if m.cpu.model.is_empty() => a.chip.clone(),
        _ => m.cpu.model.clone(),
    };
    let cores = match &m.apple {
        Some(a) if m.cpu.cores == 0 => {
            format!("{} performance, {} efficiency", a.p_cores, a.e_cores)
        }
        _ => format!("{} cores, {} threads", m.cpu.cores, m.cpu.threads),
    };
    let gpus = if m.gpus.is_empty() {
        "none".to_string()
    } else {
        m.gpus
            .iter()
            .map(|g| match g.memory_mib {
                Some(mib) => format!("{} ({})", g.name, gib(mib)),
                None => g.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    vec![
        ("CPU", cpu),
        ("Cores", cores),
        ("Memory", gib(m.memory_mib)),
        ("GPUs", gpus),
        ("Arch", m.arch.clone()),
        ("Backend", m.backend.clone()),
        ("OS", m.os.clone()),
        ("Role", m.role.clone()),
        ("Last seen", ago(m.last_seen_ms, now)),
    ]
}

/// A memory size in MiB, in GiB.
fn gib(mib: u64) -> String {
    format!("{:.0} GiB", mib as f64 / 1024.0)
}

/// How long ago a last-seen stamp (epoch ms) was: "just now", "42s ago", "3m ago", … ("never" at 0).
fn ago(ms: u64, now: u64) -> String {
    if ms == 0 {
        return "never".to_string();
    }
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The wall clock in epoch ms (0 before the epoch).
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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

/// `APZ_PERF`: a frame whose wall gap from the previous frame is over 16 ms, with the time its work took.
fn frame_log(now: Instant, work: std::time::Duration) {
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    if !perf_log_on() {
        return;
    }
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let gap = last.replace(now).map(|t| now.saturating_duration_since(t));
    if let Some(gap) = gap.filter(|g| g.as_secs_f64() > 0.016) {
        perf_line(&format!(
            "frame {:.1} ms at {:.3} s (work {:.1} ms)",
            gap.as_secs_f64() * 1000.0,
            perf_t(now),
            work.as_secs_f64() * 1000.0
        ));
    }
}
