//! The Floor screen (parity with the web console's `floor` view): the 3D floor in a shader widget,
//! with name labels, the agent card, the machine popup and the legend over it.
//!
//! The screen shows one of two scenes, both drawn by the same shader:
//! - the Machines view (`View::Machines`): the fleet from `GET /v1/fleet` on the Floor's sky and
//!   ground (`State::machines` + `State::fleet`). Hover lifts a machine, a click opens its popup,
//!   and the popup's ENTER opens the machine's view;
//! - a machine's view (`View::Machine`): the machine's board (`State::floor`): its model built
//!   from the fleet, with the rooms of the agents on it and their robots. `View::Plaza` shows the
//!   default workstation's board with every agent when the fleet cannot be read.
//!
//! Captures: `APZ_FLOOR_MACHINE=<id>` opens that machine's view at start, and `APZ_FLOOR_POPUP=<id>`
//! opens that machine's popup in the Machines view.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glam::Vec3;
use iced::widget::{column, container, opaque, pin, row, shader, stack, Space};
use iced::{Alignment, Border, Color, Element, Length, Point, Shadow, Subscription, Task, Theme};
use serde_json::Value;

use super::Id;
use crate::console::floor3d::controls::FOV_DEG;
use crate::console::floor3d::fleet::{self, build_fleet, FleetResponse, FleetScene, Machine};
use crate::console::floor3d::hardware::{self, Params, Status};
use crate::console::floor3d::scene::{Color as SceneColor, Frame};
use crate::console::floor3d::world::view::{machine_overlays, FloorProgram, Label};
use crate::console::floor3d::world::{display_name, AgentInfo, Floor};
use crate::console::{client, events, ui, Ctx};
use crate::theme::OK;

/// The card's width, and the machine popup's.
const CARD_W: f32 = 284.0;
const POPUP_W: f32 = 300.0;
/// The popup's height, for keeping it inside the stage (its rows are not measured).
const POPUP_H: f32 = 420.0;
/// The hue of the popup's kind tag stripe.
const KIND_HUE: f32 = 200.0;
/// How many agent names the popup lists.
const POPUP_NAMES: usize = 6;
/// The fleet poll.
const FLEET_EVERY: Duration = Duration::from_secs(5);

/// Which scene the screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// The fleet: every machine, with its popup.
    Machines,
    /// One machine's view: its board.
    Machine(String),
    /// The default workstation's board, shown directly because the fleet could not be read.
    Plaza,
}

pub struct State {
    /// The per-machine view: the machine's board with its agents.
    pub floor: Floor,
    /// The agents from `GET /v1/agents`: the board's rooms and robots are the ones on its machine.
    agents: Vec<AgentInfo>,
    /// The Machines view's scene: the Floor's sky and ground, with the fleet's group in its root.
    pub machines: Floor,
    /// The fleet scene, built from the first snapshot.
    pub fleet: Option<FleetScene>,
    /// The latest `GET /v1/fleet` snapshot: the popup's facts and the scene's update.
    fleet_data: Option<FleetResponse>,
    fleet_error: Option<String>,
    /// A fleet request is in flight; one was sent at least once; one was answered.
    fleet_asking: bool,
    fleet_started: bool,
    fleet_seen: bool,
    /// The shown view.
    pub view: View,
    /// The machine whose popup is open (Machines view only).
    popup: Option<String>,
    /// The machine under the pointer (Machines view only).
    hovered_machine: Option<String>,
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
    /// The user has orbited, panned or zoomed: a resize no longer refits the view.
    pub moved: bool,
}

impl Default for State {
    fn default() -> State {
        let mut floor = Floor::new();
        let mut machines = Floor::machines();
        let view = match std::env::var("APZ_FLOOR_MACHINE").ok().filter(|id| !id.is_empty()) {
            Some(id) => View::Machine(id),
            None => View::Machines,
        };
        let popup = std::env::var("APZ_FLOOR_POPUP").ok().filter(|id| !id.is_empty());
        let frame = Arc::new(if view == View::Machines { machines.frame(1.0) } else { floor.frame(1.0) });
        State {
            floor,
            agents: Vec::new(),
            machines,
            fleet: None,
            fleet_data: None,
            fleet_error: None,
            fleet_asking: false,
            fleet_started: false,
            fleet_seen: false,
            view,
            popup,
            hovered_machine: None,
            frame,
            size: (0.0, 0.0),
            error: None,
            last: None,
            scale: 2.0,
            moved: false,
        }
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
    /// The fleet from `GET /v1/fleet`.
    Fleet(Result<Value, String>),
    /// The fleet poll's tick (every 5 s).
    FleetTick,
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
    /// Esc: closes the machine popup.
    Escape,
    /// ENTER in a machine's popup: opens that machine's view.
    Enter(String),
    /// Back to the machines.
    Back,
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

/// Animation frames from the window, the fleet poll and Esc, while the Floor is the shown screen.
pub fn subscription() -> Subscription<Msg> {
    // On a software adapter, 30 frames a second: every frame costs the CPU the whole picture.
    let frames = if crate::console::floor3d::render::light() {
        iced::time::every(Duration::from_millis(33)).map(Msg::Frame)
    } else {
        iced::window::frames().map(Msg::Frame)
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

/// The hub's events, while the Floor is open.
pub fn events(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
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

/// The scene the shown view draws.
fn active(state: &mut State) -> &mut Floor {
    match state.view {
        View::Machines => &mut state.machines,
        _ => &mut state.floor,
    }
}

/// Frames the whole plaza for the stage's size (`resetView`). Nothing happens until the stage has a size.
fn reset_plaza(state: &mut State) {
    let (w, h) = state.size;
    if w > 0.0 && h > 0.0 {
        state.floor.reset_view(w / h);
        state.floor.fitted = true;
    }
}

/// The first fit of the board: once there is a board and the stage has a size.
fn fit_plaza(state: &mut State) {
    if state.floor.board.is_some() {
        reset_plaza(state);
    }
}

/// The machine whose board the Machine view shows: the one the view names, else the hub, else the
/// first. None without a fleet.
fn board_machine(state: &State) -> Option<&Machine> {
    let data = state.fleet_data.as_ref()?;
    let named = match &state.view {
        View::Machine(id) => data.machines.iter().find(|m| &m.id == id),
        _ => None,
    };
    named.or_else(|| data.machines.iter().find(|m| m.role == "hub")).or_else(|| data.machines.first())
}

/// The params a machine's board is built with: its GPUs, its Apple chip's cores and memory, and
/// whether it is a Mac Studio (the model's `when` parts read them).
fn board_params(m: &Machine) -> Params {
    let mut params = Params::default().with("gpus", m.gpus.len() as f64);
    if let Some(apple) = &m.apple {
        params = params
            .with("p_cores", apple.p_cores as f64)
            .with("e_cores", apple.e_cores as f64)
            .with("gpu_cores", apple.gpu_cores as f64)
            .with("memory_gib", apple.unified_memory_gib as f64);
    }
    let studio = m.model.as_deref().is_some_and(|name| name.contains("Studio"));
    params.with("studio", if studio { 1.0 } else { 0.0 })
}

/// Shows the Machine view's board: the machine's own model with its params, or the default
/// workstation without a fleet, and the agents on the machine (the hub's own when it names none).
/// The board is rebuilt only when its model, params or status change; the rooms and robots follow
/// the agents every time.
fn refresh_board(state: &mut State) {
    if matches!(state.view, View::Machines) {
        return;
    }
    let (name, params, on, status) = match board_machine(state) {
        Some(m) => {
            let (_, board) = hardware::models_for(&m.kind, m.vendor.as_deref());
            let on: Vec<AgentInfo> = state.agents.iter().filter(|a| m.agents.contains(&a.cid)).cloned().collect();
            (board, board_params(m), on, fleet::status_of(&m.status))
        }
        None => ("board/workstation", Params::default(), state.agents.clone(), Status::Online),
    };
    let Some(model) = hardware::model(name) else { return };
    let key = format!("{name}|{:?}|{status:?}", params.values);
    if state.floor.board.as_ref().map(|b| b.key.as_str()) != Some(key.as_str()) {
        state.floor.show_board(key, model, params, status);
        // A new board is framed afresh.
        state.moved = false;
    }
    state.floor.sync(&on);
    if !state.floor.fitted {
        fit_plaza(state);
    }
}

/// Frames the fleet's bounding sphere for the stage's size. Nothing happens until both are there.
fn fit_machines(state: &mut State) {
    let (w, h) = state.size;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    if let Some(fleet) = &state.fleet {
        let f = fleet.frame();
        state.machines.fit_sphere(f.center, f.radius, w / h);
    }
}

/// Frames the shown view.
fn fit_active(state: &mut State) {
    match state.view {
        View::Machines => fit_machines(state),
        _ => fit_plaza(state),
    }
}

/// Shows another view: its camera is framed afresh and the popup closes.
fn set_view(state: &mut State, view: View) {
    state.view = view;
    state.popup = None;
    state.moved = false;
    refresh_board(state);
    fit_active(state);
}

/// The machine under the stage point `at` (the ray through the Machines' camera).
fn machine_at(state: &State, at: Point) -> Option<String> {
    let (w, h) = state.size;
    let fleet = state.fleet.as_ref()?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (origin, dir) = state.machines.orbit.camera(w / h).ray(at.x, at.y, w, h);
    fleet.pick(origin.to_array(), dir.normalize_or_zero().to_array())
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let mut task = Task::none();
    match msg {
        Msg::Frame(now) => {
            let started = Instant::now();
            let dt = state.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32()).min(0.05);
            state.last = Some(now);
            let (w, h) = state.size;
            match state.view {
                View::Machines => {
                    state.machines.orbit.update();
                    state.machines.step(dt, now_ms(now));
                    if let (Some(fleet), Some(data)) = (state.fleet.as_mut(), state.fleet_data.as_ref()) {
                        fleet.update(dt, data);
                    }
                    if w > 0.0 && h > 0.0 {
                        let camera = state.machines.orbit.camera(w / h);
                        let mut frame = state.machines.frame(w / h);
                        if let Some(fleet) = &state.fleet {
                            frame.overlays = machine_overlays(fleet, &camera, w, h, state.scale, state.hovered_machine.as_deref());
                        }
                        state.frame = Arc::new(frame);
                    }
                }
                _ => {
                    state.floor.orbit.update();
                    state.floor.step(dt, now_ms(now));
                    if w > 0.0 && h > 0.0 {
                        let mut frame = state.floor.frame(w / h);
                        frame.overlays = state.floor.label_overlays(w, h, state.scale);
                        state.frame = Arc::new(frame);
                    }
                }
            }
            perf(started.elapsed(), now);
        }
        Msg::Agents(Ok(v)) => {
            state.error = None;
            state.agents = agent_list(&v);
            refresh_board(state);
        }
        Msg::Agents(Err(e)) => state.error = Some(e),
        Msg::Fleet(res) => {
            state.fleet_asking = false;
            match res.and_then(|v| serde_json::from_value::<FleetResponse>(v).map_err(|e| e.to_string())) {
                Ok(data) => {
                    state.fleet_error = None;
                    state.fleet_seen = true;
                    if state.fleet.is_none() {
                        let fleet = build_fleet(&data, &state.machines.fleet_palette());
                        state.machines.root.add(&fleet.group);
                        state.fleet = Some(fleet);
                    }
                    state.fleet_data = Some(data);
                    // The fallback gives way to the machines once they can be read.
                    if state.view == View::Plaza {
                        set_view(state, View::Machines);
                    } else if state.view == View::Machines && !state.moved {
                        fit_machines(state);
                    } else if matches!(state.view, View::Machine(_)) {
                        // The machine's own model, once the fleet names it.
                        refresh_board(state);
                    }
                }
                Err(e) => {
                    state.fleet_error = Some(e);
                    // Nothing read yet: the plaza is shown directly.
                    if !state.fleet_seen && state.view == View::Machines {
                        set_view(state, View::Plaza);
                    }
                }
            }
        }
        Msg::FleetTick => {
            if !state.fleet_asking {
                task = fleet_request(state, ctx);
            }
        }
        Msg::Resized(w, h) => {
            state.size = (w, h);
            if !state.moved {
                fit_active(state);
            }
        }
        Msg::Rotate { dx, dy, height } => {
            state.moved = true;
            active(state).orbit.rotate(dx, dy, height);
        }
        Msg::Pan { dx, dy, height } => {
            state.moved = true;
            active(state).orbit.pan(dx, dy, height, FOV_DEG);
        }
        Msg::Dolly(steps) => {
            state.moved = true;
            active(state).orbit.dolly(steps);
        }
        Msg::Hover(at) => match state.view {
            View::Machines => {
                let hit = at.and_then(|p| machine_at(state, p));
                if let Some(fleet) = state.fleet.as_mut() {
                    fleet.hover(hit.as_deref());
                }
                state.hovered_machine = hit;
            }
            _ => {
                let (w, h) = state.size;
                let hit = at.and_then(|p| state.floor.pick(p.x, p.y, w, h));
                state.floor.hovered = hit;
            }
        },
        Msg::Select(at) => match state.view {
            View::Machines => state.popup = at.and_then(|p| machine_at(state, p)),
            _ => {
                let (w, h) = state.size;
                let hit = at.and_then(|p| state.floor.pick(p.x, p.y, w, h));
                state.floor.selected = hit;
            }
        },
        Msg::Event(e) => state.floor.on_event(&e.kind, &e.data),
        Msg::ResetView => {
            state.moved = false;
            match state.view {
                View::Machines => fit_machines(state),
                _ => reset_plaza(state),
            }
        }
        Msg::Escape => state.popup = None,
        Msg::Enter(id) => set_view(state, View::Machine(id)),
        Msg::Back => set_view(state, View::Machines),
        Msg::Go(_) => {}
    }
    task
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let (title, mut actions) = match &state.view {
        View::Machines => ("Machines".to_string(), Vec::new()),
        View::Machine(id) => (
            format!("Machines › {}", machine_name(state, id)),
            vec![ui::btn("Back to machines", None, ui::Kind::Secondary, Some(Msg::Back))],
        ),
        View::Plaza => ("Floor".to_string(), Vec::new()),
    };
    actions.push(ui::btn("Reset view", Some("refresh"), ui::Kind::Secondary, Some(Msg::ResetView)));
    let head = ui::view_head(&title, actions);

    let mut layers: Vec<Element<'a, Msg>> = vec![
        shader(FloorProgram { primitive: Arc::clone(&state.frame) }).width(Length::Fill).height(Length::Fill).into(),
    ];
    let (w, h) = state.size;
    if w > 0.0 && h > 0.0 {
        match state.view {
            View::Machines => {
                if let Some(layer) = popup_layer(state, w, h) {
                    layers.push(layer);
                }
            }
            _ => {
                let labels = state.floor.labels(w, h);
                if let Some(label) = state.floor.selected.and_then(|sel| labels.iter().find(|l| l.cid == sel)) {
                    layers.push(card(state, label, w, h));
                }
            }
        }
    }
    layers.push(
        container(legend(matches!(state.view, View::Machines)))
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
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        });

    let mut body = column![].spacing(12).height(Length::Fill);
    if let Some(e) = &state.error {
        body = body.push(ui::error_line(e));
    }
    if let Some(e) = &state.fleet_error {
        body = body.push(ui::error_line(&format!("Machines: {e}")));
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

/// The popup of the open machine, beside its tag (none while the fleet or the machine is missing).
fn popup_layer<'a>(state: &'a State, w: f32, h: f32) -> Option<Element<'a, Msg>> {
    let id = state.popup.as_ref()?;
    let fleet = state.fleet.as_ref()?;
    let machine = state.fleet_data.as_ref()?.machines.iter().find(|m| m.id == *id)?;
    let tag = fleet.tags().into_iter().find(|t| t.id == *id)?;
    let (x, y) = state.machines.orbit.camera(w / h).to_screen(Vec3::from(tag.pos), w, h)?;
    Some(popup(state, machine, x, y, w, h))
}

/// The machine popup (the agent card's look): the name and kind, the status, the facts, the
/// agents, and ENTER (the primary button) to open the machine's view.
fn popup<'a>(state: &'a State, m: &'a Machine, x: f32, y: f32, w: f32, h: f32) -> Element<'a, Msg> {
    let mut facts_col = column![].spacing(5);
    for (label_text, value) in facts(m, now_millis()) {
        facts_col = facts_col.push(
            row![container(ui::label(label_text)).width(Length::Fixed(76.0)), ui::mono(value, 10.5, ui::INK)]
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
        let name = state.floor.bot(*cid).map_or_else(|| format!("agent {cid}"), |b| display_name(*cid, &b.agent.name));
        names = names.push(ui::btn(&name, None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(*cid)))));
    }
    if m.agents.len() > POPUP_NAMES {
        names = names.push(ui::faint(format!("and {} more", m.agents.len() - POPUP_NAMES)));
    }

    let model = format!("{} · {}", m.kind, m.model.as_deref().unwrap_or("model unknown"));
    let body = column![
        row![ui::mono(m.name.clone(), 13.0, ui::INK), Space::new().width(Length::Fill), role_tag(&m.kind, KIND_HUE)]
            .spacing(8)
            .align_y(Alignment::Center),
        ui::mono(model, 11.0, ui::INK_3),
        row![ui::label("Status"), ui::badge(&m.status, status_tone(&m.status))].spacing(10).align_y(Alignment::Center),
        facts_col,
        row![ui::label("Agents"), ui::mono(format!("{working} working, {idle} idle"), 11.0, ui::INK)]
            .spacing(10)
            .align_y(Alignment::Center),
        names,
        ui::btn("ENTER", None, ui::Kind::Primary, Some(Msg::Enter(m.id.clone()))),
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

/// The legend's keys, bottom-left of the stage: the machines' statuses, or the agents' states.
fn legend<'a>(machines: bool) -> Element<'a, Msg> {
    let body = if machines {
        row![
            key(status_colour(Status::Online), "Online"),
            key(status_colour(Status::Draining), "Draining"),
            key(status_colour(Status::Offline), "Offline"),
            key(status_colour(Status::Revoked), "Revoked"),
        ]
    } else {
        row![key(ui::SIGNAL, "Working"), key(OK, "Idle"), key(ui::INK_3, "Offline")]
    }
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

/// A machine status's LED colour, as the iced colour.
fn status_colour(status: Status) -> Color {
    let rgb = fleet::status_led(status).to_srgb();
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

/// The machine's name, or its id while the fleet is not read.
fn machine_name(state: &State, id: &str) -> String {
    state
        .fleet_data
        .as_ref()
        .and_then(|d| d.machines.iter().find(|m| m.id == id))
        .map_or_else(|| id.to_string(), |m| m.name.clone())
}

/// The popup's facts as label and value rows, from the machine's snapshot at `now` (epoch ms).
fn facts(m: &Machine, now: u64) -> Vec<(&'static str, String)> {
    let cpu = match &m.apple {
        Some(a) if m.cpu.model.is_empty() => a.chip.clone(),
        _ => m.cpu.model.clone(),
    };
    let cores = match &m.apple {
        Some(a) if m.cpu.cores == 0 => format!("{} performance, {} efficiency", a.p_cores, a.e_cores),
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
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Milliseconds on a monotonic clock that starts at the first frame.
fn now_ms(at: Instant) -> f64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_a_last_seen_stamp() {
        let now = 1_000_000_000;
        assert_eq!(ago(0, now), "never");
        assert_eq!(ago(now - 2_000, now), "just now");
        assert_eq!(ago(now - 42_000, now), "42s ago");
        assert_eq!(ago(now - 180_000, now), "3m ago");
        assert_eq!(ago(now - 7_200_000, now), "2h ago");
        assert_eq!(ago(now - 3 * 86_400_000, now), "3d ago");
    }

    #[test]
    fn facts_list_the_machine_and_its_gpus() {
        let m: Machine = serde_json::from_value(serde_json::json!({
            "id": "n0",
            "name": "zarathos-pc",
            "role": "hub",
            "kind": "workstation",
            "status": "online",
            "arch": "x86_64",
            "os": "linux",
            "backend": "kvm",
            "cpu": { "model": "AMD Ryzen 9 5900X", "cores": 12, "threads": 24 },
            "memory_mib": 131072,
            "gpus": [{ "name": "NVIDIA GeForce RTX 3090", "vendor": "NVIDIA", "memory_mib": 24576 }, { "name": "Intel UHD", "vendor": "Intel", "memory_mib": null }],
            "agents": [200, 201],
            "working": 1,
            "last_seen_ms": 0
        }))
        .expect("a machine");
        let facts = facts(&m, 0);
        let get = |k: &str| facts.iter().find(|(label, _)| *label == k).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(get("CPU"), "AMD Ryzen 9 5900X");
        assert_eq!(get("Cores"), "12 cores, 24 threads");
        assert_eq!(get("Memory"), "128 GiB");
        assert_eq!(get("GPUs"), "NVIDIA GeForce RTX 3090 (24 GiB), Intel UHD");
        assert_eq!(get("Last seen"), "never");
    }
}
