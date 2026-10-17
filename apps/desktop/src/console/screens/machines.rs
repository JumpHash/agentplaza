//! The Machines screen (parity with the web console's `machines` view): the control plane wired to
//! each machine, then a section per machine with its live usage, where its resources go, its runs
//! (serial, kill) and its technical details, folded away.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::advanced::widget::operate;
use iced::advanced::widget::operation::{Operation, Outcome, Scrollable};
use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::text::Wrapping;
use iced::widget::{Space, column, container, mouse_area, row, scrollable, shader};
use iced::{
    Alignment, Border, Color, Element, Length, Pixels, Point, Rectangle, Size, Subscription, Task,
    Theme, Vector, mouse,
};
use serde_json::Value;

use apz_engine::input::Action;
use apz_engine::{Cmd, Engine, Feed, Motion};

use super::Id;
use super::floor;
use crate::console::engine::fleet::{self, FleetResponse};
use crate::console::engine::kit;
use crate::console::engine::machine_stage::View;
use crate::console::floor_engine::EngineProgram;
use crate::console::ui::{INK, INK_2, INK_3, Kind, RULE, RULE_FAINT, SHEET, SIGNAL, Tone};
use crate::console::{Ctx, client, render_card, ui};
use crate::theme::SANS;

/// A machine's row in the topology, and the topology's fleet column.
const ROW_H: f32 = 56.0;
const FLEET_W: f32 = 260.0;
const PLANE_W: f32 = 200.0;
/// The CPU sparkline: its height, and how many of the latest samples it draws.
const SPARK_H: f32 = 72.0;
const SPARK_POINTS: usize = 60;

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    /// The last good read of the fleet; None before the first load and while the node is offline.
    data: Option<Data>,
    /// Why the last read failed, while the node is offline.
    offline: Option<String>,
    /// The machine the topology picked.
    selected: Option<String>,
    /// Each machine's Agents-table sort, kept across refreshes.
    sorts: HashMap<String, Sort>,
    /// Runs with their VMs open; `run/index` serials open; machines with technical details open.
    expanded: BTreeSet<String>,
    serial_open: BTreeSet<String>,
    tech_open: BTreeSet<String>,
    /// The VMs last read for each open run, so a refresh does not blank them.
    run_vms: HashMap<String, Result<Vec<VmRow>, String>>,
    /// The machine whose 3D view is open (one at a time); its engine is dropped on collapse.
    stage: Option<StageView>,
    /// The grid (the fleet on the 3D floor, the default) or the list (a section per machine).
    mode: Mode,
    /// The grid's own state: its engine, fleet poll and popup.
    grid: floor::State,
}

/// The screen's two views of the fleet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Grid,
    List,
}

impl State {
    pub fn in_grid(&self) -> bool {
        self.mode == Mode::Grid
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// The grid's messages (its 3D floor and popup).
    Grid(floor::Msg),
    /// The Grid | List switch.
    SetMode(Mode),
    Loaded(Result<Value, String>),
    /// The topology picked a machine.
    Pick(String),
    PickChecked(String, f32, f32),
    /// The pick's scroll has run and a frame has passed: measure the section again.
    PickMeasure(String, f32),
    Sort(String, SortKey),
    ToggleRun(String),
    RunLoaded(String, Result<Value, String>),
    ToggleSerial(String),
    ToggleTech(String),
    Kill(String),
    Killed(Result<(), String>),
    /// Opens or closes one machine's 3D view.
    ToggleStage(String),
    /// The fleet for the 3D view (`GET /v1/fleet`, as the Floor reads it).
    StageFleet(Result<Value, String>),
    /// The 3D view's clock.
    StageFrame(Instant),
    /// A pointer action on the 3D view.
    Stage(Action),
}

/// The Agents table's columns that sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Mem,
}

#[derive(Debug, Clone, Copy)]
struct Sort {
    key: SortKey,
    descending: bool,
}

const DEFAULT_SORT: Sort = Sort {
    key: SortKey::Cpu,
    descending: true,
};

#[derive(Debug, Clone)]
struct Data {
    overview: Option<Overview>,
    machines: Vec<Machine>,
    /// Agents by CID, for the names of the VMs.
    agents: HashMap<u64, AgentRef>,
}

#[derive(Debug, Clone)]
struct Overview {
    total: u64,
    working: u64,
    hub: bool,
}

#[derive(Debug, Clone)]
struct AgentRef {
    name: String,
    role_label: String,
}

/// An Apple machine's chip, as the hub reports it (only on Macs).
#[derive(Debug, Clone)]
struct Apple {
    chip: String,
    p_cores: u64,
    e_cores: u64,
    gpu_cores: u64,
    unified_memory_gib: u64,
}

#[derive(Debug, Clone)]
struct Machine {
    id: String,
    name: String,
    kind: String,
    vendor: String,
    model: Option<String>,
    apple: Option<Apple>,
    class: String,
    arch: String,
    backend: String,
    irqchip: String,
    vsock_mode: String,
    noded_port: Option<u64>,
    cpus: u64,
    memory_mib: f64,
    gpus: Vec<String>,
    runs: Vec<Run>,
    agents: u64,
    reachable: Option<bool>,
    stats: Option<Stats>,
    /// The workstation's part, when the machine is a station (`GET /v1/stations`).
    station: Option<Station>,
}

/// A workstation's part of its machine: its status, OS and form, its leases and the badge for the owner's account.
#[derive(Debug, Clone)]
struct Station {
    status: String,
    os: String,
    form: String,
    leases: u64,
    active: u64,
    badge: Option<String>,
}

#[derive(Debug, Clone)]
struct Stats {
    cpu: f64,
    load: [f64; 3],
    mem_total: f64,
    mem_used: f64,
    uptime_s: f64,
    vms_running: u64,
    guest_mem: f64,
    history: Vec<f64>,
    plaza: Option<Plaza>,
}

#[derive(Debug, Clone)]
struct Plaza {
    services: Vec<Proc>,
    vms: Vec<VmUsage>,
    total: Total,
}

#[derive(Debug, Clone)]
struct Total {
    services_cpu: f64,
    services_rss: f64,
    vms_cpu: f64,
    vms_rss: f64,
}

/// A process of the plaza on a machine; `cpu` is of the whole machine.
#[derive(Debug, Clone)]
struct Proc {
    name: String,
    pid: u64,
    cpu: f64,
    rss: f64,
}

/// An agent VM's process, and its guest memory as the balloon reports it.
#[derive(Debug, Clone)]
struct VmUsage {
    run: String,
    vm: u64,
    cid: Option<u64>,
    role: Option<String>,
    cpu: f64,
    rss: f64,
    usable: Option<f64>,
    guest_used: Option<f64>,
}

#[derive(Debug, Clone)]
struct Run {
    id: String,
    state: String,
    role: Option<String>,
    created_at: f64,
    vms: Vec<VmRow>,
}

#[derive(Debug, Clone)]
struct VmRow {
    index: u64,
    state: String,
    guest_cid: Option<u64>,
    serial: Option<String>,
}

/// The 3D view's size in logical px: the widget and the engine's stage share it.
const STAGE_W: f32 = 480.0;
const STAGE_H: f32 = 300.0;

/// One machine's 3D view: its engine, whether the engine has the machine on its stage, and the motion of its last frame.
struct StageView {
    id: String,
    engine: Engine,
    shown: bool,
    motion: Motion,
}

impl std::fmt::Debug for StageView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageView").field("id", &self.id).finish()
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) | Msg::Grid(floor::Msg::Go(id)) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick): the overview, the
/// machines and the agents, in one task.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    match state.mode {
        Mode::Grid => floor::load(&mut state.grid, ctx).map(Msg::Grid),
        Mode::List => load_list(state, ctx),
    }
}

/// Loads the list's data in one task (the 3D view's fleet too, while it is open).
fn load_list(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let main = client::task(
        move || {
            let overview = client.get::<Value>("/v1/overview").ok();
            let machines = client.get::<Value>("/v1/machines")?;
            let agents = client.get::<Value>("/v1/agents").ok();
            // The stations are optional: a failure reads as none.
            let stations = client.call("GET", "/v1/stations", None).ok();
            Ok(
                serde_json::json!({ "overview": overview, "machines": machines, "agents": agents, "stations": stations }),
            )
        },
        Msg::Loaded,
    );
    if state.stage.is_some() {
        Task::batch([main, stage_fleet(ctx)])
    } else {
        main
    }
}

/// Reads the fleet for the 3D view (`GET /v1/fleet`).
fn stage_fleet(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.call("GET", "/v1/fleet", None),
        Msg::StageFleet,
    )
}

/// The 3D view's clock: frames while it moves (30 a second on a software adapter, as the Floor), about 12 a second while
/// it only turns, none at rest.
pub fn subscription(state: &State) -> Subscription<Msg> {
    if state.mode == Mode::Grid {
        return floor::subscription(&state.grid).map(Msg::Grid);
    }
    match state.stage.as_ref().map(|s| s.motion) {
        Some(Motion::Active) if crate::console::engine::render::light() => {
            iced::time::every(Duration::from_millis(33)).map(Msg::StageFrame)
        }
        Some(Motion::Active) => iced::window::frames().map(Msg::StageFrame),
        Some(Motion::Ambient) => iced::time::every(Duration::from_millis(33)).map(Msg::StageFrame),
        _ => Subscription::none(),
    }
}

/// Reads one run with its VMs (the serial lines come with them).
fn fetch_run(ctx: &Ctx, id: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/runs/{id}");
    let key = id.clone();
    client::task(
        move || client.get::<Value>(&path),
        move |r| Msg::RunLoaded(key.clone(), r),
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::Grid(floor::Msg::Go(_)) => Task::none(),
        Msg::Grid(floor::Msg::OpenMachine(id)) => {
            // The popup closes with the grid; the list shows the machine's section, and its 3D view with it.
            let _ = floor::update(&mut state.grid, floor::Msg::Escape, ctx);
            open_in_list(state, &id);
            let stage = if state.stage.as_ref().is_some_and(|s| s.id == id) {
                Task::none()
            } else {
                update(state, Msg::ToggleStage(id.clone()), ctx)
            };
            Task::batch([stage, load(state, ctx).chain(Task::done(Msg::Pick(id)))])
        }
        Msg::Grid(m) => floor::update(&mut state.grid, m, ctx).map(Msg::Grid),
        Msg::SetMode(mode) => {
            if state.mode == mode {
                Task::none()
            } else {
                state.mode = mode;
                load(state, ctx)
            }
        }
        Msg::Loaded(Ok(v)) => {
            let data = parse_data(&v);
            let run_ids: BTreeSet<String> = data
                .machines
                .iter()
                .flat_map(|m| m.runs.iter().map(|r| r.id.clone()))
                .collect();
            state.expanded.retain(|id| run_ids.contains(id));
            if !data
                .machines
                .iter()
                .any(|m| Some(&m.id) == state.selected.as_ref())
            {
                state.selected = data.machines.first().map(|m| m.id.clone());
            }
            state.offline = None;
            let fetches: Vec<Task<Msg>> = state
                .expanded
                .iter()
                .map(|id| fetch_run(ctx, id.clone()))
                .collect();
            state.data = Some(data);
            Task::batch(fetches)
        }
        Msg::Loaded(Err(e)) => {
            state.data = None;
            state.offline = Some(e);
            Task::none()
        }
        Msg::Pick(id) => {
            let measure = SectionOffset {
                section: section_id(&id),
                scroll: crate::console::shell::SCREEN_SCROLL.clone(),
                section_top: None,
                view_top: None,
                scrolled: 0.0,
                visible: false,
            };
            state.selected = Some(id.clone());
            // Measure the section against the screen's scroll, scroll so its top is the view's top, then check it.
            operate(measure).then(move |target| {
                eprintln!("agentplaza-desktop: pick {id}: scroll to {target:.1} px");
                let scroll = crate::console::shell::SCREEN_SCROLL.clone();
                iced::widget::operation::scroll_to(
                    scroll,
                    scrollable::AbsoluteOffset { x: 0.0, y: target },
                )
                .chain(settle(id.clone(), target))
            })
        }
        Msg::PickMeasure(id, target) => pick_check(id, target),
        Msg::PickChecked(id, _target, needed) => {
            let gap = needed;
            let verdict = if gap.abs() <= PICK_TOLERANCE {
                "within 2 px: PASS"
            } else {
                "more than 2 px off: FAIL"
            };
            eprintln!(
                "agentplaza-desktop: pick {id}: section top {gap:+.1} px from the view top, {verdict}"
            );
            Task::none()
        }
        Msg::Sort(id, key) => {
            let cur = state.sorts.get(&id).copied().unwrap_or(DEFAULT_SORT);
            let next = if cur.key == key {
                Sort {
                    key,
                    descending: !cur.descending,
                }
            } else {
                Sort {
                    key,
                    descending: true,
                }
            };
            state.sorts.insert(id, next);
            Task::none()
        }
        Msg::ToggleRun(id) => {
            if state.expanded.remove(&id) {
                Task::none()
            } else {
                state.expanded.insert(id.clone());
                fetch_run(ctx, id)
            }
        }
        Msg::RunLoaded(id, r) => {
            if state.expanded.contains(&id) {
                let vms: Result<Vec<VmRow>, String> =
                    r.map(|v| items(&v, "vms").iter().map(parse_vm_row).collect());
                state.run_vms.insert(id, vms);
            }
            Task::none()
        }
        Msg::ToggleSerial(key) => {
            toggle(&mut state.serial_open, key);
            Task::none()
        }
        Msg::ToggleTech(id) => {
            toggle(&mut state.tech_open, id);
            Task::none()
        }
        Msg::Kill(id) => {
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("DELETE", &format!("/v1/runs/{id}"), None)
                        .map(|_| ())
                },
                Msg::Killed,
            )
        }
        Msg::ToggleStage(id) => {
            if state.stage.as_ref().is_some_and(|s| s.id == id) {
                state.stage = None;
                Task::none()
            } else {
                state.stage = Some(StageView {
                    id,
                    engine: Engine::detached(),
                    shown: false,
                    motion: Motion::Active,
                });
                stage_fleet(ctx)
            }
        }
        Msg::StageFleet(Ok(v)) => {
            if let (Some(s), Ok(fleet)) = (
                state.stage.as_mut(),
                serde_json::from_value::<FleetResponse>(v),
            ) {
                s.engine.set_data(Feed {
                    fleet: Some(fleet),
                    agents: None,
                    fleet_error: None,
                });
                if !s.shown {
                    s.engine.command(Cmd::ShowMachine { id: s.id.clone() });
                    s.shown = true;
                }
                s.motion = Motion::Active;
            }
            Task::none()
        }
        Msg::StageFleet(Err(_)) => Task::none(),
        Msg::StageFrame(now) => {
            if let Some(s) = state.stage.as_mut() {
                s.motion = s.engine.advance(now, (STAGE_W, STAGE_H)).motion;
            }
            Task::none()
        }
        Msg::Stage(action) => {
            if let Some(s) = state.stage.as_mut() {
                s.engine.apply(action);
                s.motion = Motion::Active;
            }
            Task::none()
        }
        Msg::Killed(Ok(())) => load(state, ctx),
        Msg::Killed(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
    }
}

/// The grid's "Open machine": the list shows, with the machine selected (its section is picked once the list loads).
fn open_in_list(state: &mut State, id: &str) {
    state.mode = Mode::List;
    state.selected = Some(id.to_string());
}

fn toggle(set: &mut BTreeSet<String>, key: String) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let switch = ui::seg(
        &[(Mode::Grid, "Grid"), (Mode::List, "List")],
        state.mode,
        Msg::SetMode,
    );
    match state.mode {
        Mode::Grid => {
            let actions = vec![floor::reset_button().map(Msg::Grid), switch];
            ui::sheet(
                ui::view_head("Machines", actions),
                floor::body(&state.grid).map(Msg::Grid),
            )
        }
        Mode::List => ui::sheet(ui::view_head("Machines", vec![switch]), body(state)),
    }
}

fn body<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    match &state.data {
        None if state.offline.is_some() => parts.push(ui::empty(
            "machine",
            "The node is offline",
            "Start the node daemon to see its machines and VMs.",
        )),
        None => parts.push(ui::loading()),
        Some(data) => {
            parts.push(topology(state, data));
            if data.machines.is_empty() {
                parts.push(ui::empty(
                    "machine",
                    "No machines.",
                    "The plaza has no machines to show.",
                ));
            } else {
                for m in &data.machines {
                    parts.push(ui::hairline());
                    parts.push(section(state, data, m));
                }
            }
        }
    }
    column(parts).spacing(32).into()
}

// ---- The topology: the control plane, wired to each machine ---------------------------------

fn topology<'a>(state: &'a State, data: &'a Data) -> Element<'a, Msg> {
    let fleet_h = data.machines.len().max(1) as f32 * ROW_H;
    let wires = iced::widget::canvas(Wires {
        live: data.machines.iter().map(|m| is_live(&m.runs)).collect(),
        labels: data
            .machines
            .iter()
            .map(|m| format!("{} AGENTS · {} VMS", m.agents, machine_vms(m)))
            .collect(),
    })
    .width(Length::Fill)
    .height(Length::Fixed(fleet_h));

    let blocks: Vec<Element<'a, Msg>> = data.machines.iter().map(|m| block(state, m)).collect();
    let fleet = column(blocks).width(Length::Fixed(FLEET_W));

    let overview_text = match &data.overview {
        Some(o) => format!("{} agents · {} working", o.total, o.working),
        None => "Unreachable".to_string(),
    };
    let hub = data.overview.as_ref().is_some_and(|o| o.hub);
    let plane = container(
        column![
            ui::label("Control plane"),
            ui::faint(overview_text),
            row![
                ui::badge(
                    "apid",
                    if data.overview.is_some() {
                        Tone::Ok
                    } else {
                        Tone::Danger
                    }
                ),
                ui::badge("hub", if hub { Tone::Ok } else { Tone::Muted }),
            ]
            .spacing(8),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fixed(PLANE_W))
    .style(boxed);

    row![plane, wires, fleet].align_y(Alignment::Center).into()
}

/// The straight rules from the control plane's middle to each machine's row, drawn over the
/// topology; a live machine's rule is the signal orange, the rest are muted.
struct Wires {
    live: Vec<bool>,
    labels: Vec<String>,
}

impl canvas::Program<Msg> for Wires {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let (w, h) = (bounds.width, bounds.height);
        let rows = self.live.len().max(1) as f32;
        let hub = Point::new(0.0, h / 2.0);
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        for (i, (live, label)) in self.live.iter().zip(self.labels.iter()).enumerate() {
            let end = Point::new(w, (i as f32 + 0.5) * h / rows);
            let (color, width) = if *live { (SIGNAL, 2.0) } else { (INK_3, 1.0) };
            frame.stroke(
                &Path::line(hub, end),
                Stroke::default().with_color(color).with_width(width),
            );
            frame.fill_text(canvas::Text {
                content: label.clone(),
                position: Point::new(w * 0.5, (hub.y + end.y) * 0.5 - 14.0),
                color: INK_2,
                size: Pixels(11.0),
                font: SANS,
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// One machine's block in the fleet column: its name, its kind and state. Picking it selects it.
fn block<'a>(state: &'a State, m: &'a Machine) -> Element<'a, Msg> {
    let live = is_live(&m.runs);
    let down = m.reachable == Some(false);
    let tone = if down {
        Tone::Danger
    } else if live {
        Tone::Ok
    } else {
        Tone::Muted
    };
    let status = if let Some(st) = &m.station {
        st.status.as_str()
    } else if down {
        "unreachable"
    } else if live {
        "running"
    } else {
        "idle"
    };
    let selected = state.selected.as_deref() == Some(m.id.as_str());
    let content = column![
        row![
            swatch(tone.color(), 8.0),
            ui::sans(m.name.to_string(), 13.0, INK)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::faint(format!("{} · {status}", kind_label(&m.kind))),
    ]
    .spacing(4);
    let cell = container(content)
        .padding([8, 12])
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .align_y(Alignment::Center)
        .style(block_style(selected));
    mouse_area(cell).on_press(Msg::Pick(m.id.clone())).into()
}

fn block_style(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| container::Style {
        background: if selected { Some(SHEET.into()) } else { None },
        border: Border {
            color: if selected { SIGNAL } else { RULE },
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

// ---- A machine's device: its drawing in line art (the web's ui/devices.ts) ---------------------

/// A machine's section's widget id: the topology finds the section by it, so no offset is guessed.
fn section_id(machine: &str) -> iced::widget::Id {
    iced::widget::Id::from(format!("machine-section-{machine}"))
}

/// The section's top from the view's top after a pick: `target` is the offset the pick scrolled to, and `needed` the offset
/// How far (px) a picked section's top may sit from the view's top and still pass the check.
const PICK_TOLERANCE: f32 = 2.0;

/// Waits about one frame after the scroll has run, then measures again: the second operation, in its own update.
fn settle(id: String, target: f32) -> Task<Msg> {
    Task::perform(
        async {
            std::thread::sleep(Duration::from_millis(50));
        },
        move |()| Msg::PickMeasure(id, target),
    )
}

/// Measures the picked section again once its scroll has run, and reports it as `PickChecked`.
fn pick_check(id: String, target: f32) -> Task<Msg> {
    let measure = SectionOffset {
        section: section_id(&id),
        scroll: crate::console::shell::SCREEN_SCROLL.clone(),
        section_top: None,
        view_top: None,
        scrolled: 0.0,
        visible: true,
    };
    operate(measure).map(move |needed| Msg::PickChecked(id.clone(), target, needed))
}

/// Measures a machine's section in the screen's scroll: the offset that puts its top at the view's top.
/// Both tops are window positions, so the scroll's current offset is added back.
struct SectionOffset {
    section: iced::widget::Id,
    scroll: iced::widget::Id,
    section_top: Option<f32>,
    view_top: Option<f32>,
    scrolled: f32,
    /// Measures the gap still visible after the scroll (true), not the offset the scroll must reach (false).
    visible: bool,
}

impl Operation<f32> for SectionOffset {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<f32>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::widget::Id>, bounds: Rectangle) {
        if id == Some(&self.section) {
            self.section_top = Some(bounds.y);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.scroll) {
            let max = (content.height - bounds.height).max(0.0);
            eprintln!(
                "agentplaza-desktop: pick scroll: view top {:.1}, view height {:.1}, content height {:.1}, max offset {:.1}, translation {:.1}",
                bounds.y, bounds.height, content.height, max, translation.y
            );
            self.view_top = Some(bounds.y);
            self.scrolled = translation.y;
        }
    }

    fn finish(&self) -> Outcome<f32> {
        match (self.section_top, self.view_top) {
            (Some(section), Some(view)) => {
                // The bounds come untranslated (they stay put as the scroll moves), so the offset the scroll must reach is
                // section - view, and the gap still visible is that less the scroll's translation.
                let offset = section - view;
                let value = if self.visible {
                    offset - self.scrolled
                } else {
                    offset
                };
                eprintln!(
                    "agentplaza-desktop: pick measure: section top {section:.1}, view top {view:.1}, translation {:.1}, {} {value:.1}",
                    self.scrolled,
                    if self.visible {
                        "visible gap"
                    } else {
                        "offset"
                    }
                );
                Outcome::Some(value)
            }
            _ => {
                eprintln!(
                    "agentplaza-desktop: pick measure: section or view not found ({:?}, {:?})",
                    self.section_top, self.view_top
                );
                Outcome::None
            }
        }
    }
}

const EDGE_GREEN: Color = Color::from_rgb8(0x5a, 0x9a, 0x78);
const GOLD: Color = Color::from_rgb8(0xc9, 0xa2, 0x27);

/// The device shapes: the web's looks (`lookOf`), with an Apple laptop as a MacBook and other Apple machines as a Mac mini.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    Server,
    Workstation,
    Edge,
    Laptop,
    MacBook,
    Mini,
    Phone,
}

fn look_of(kind: &str, vendor: &str) -> Look {
    if vendor != "apple" || kind == "phone" {
        return match kind {
            "workstation" => Look::Workstation,
            "edge" => Look::Edge,
            "laptop" => Look::Laptop,
            "phone" => Look::Phone,
            _ => Look::Server,
        };
    }
    if kind == "laptop" {
        Look::MacBook
    } else {
        Look::Mini
    }
}

/// A pen over the web's 120 x 90 device grid, scaled to the figure's width; strokes take `ink`.
struct Pen {
    frame: canvas::Frame,
    scale: f32,
    ink: Color,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> Point {
        Point::new(x * self.scale, y * self.scale)
    }

    fn stroke(&self, width: f32) -> Stroke<'static> {
        Stroke::default()
            .with_color(self.ink)
            .with_width(width * self.scale)
    }

    /// A box with rounded corners, filled when `fill` is set, and outlined.
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Option<Color>) {
        let path = Path::rounded_rectangle(
            self.at(x, y),
            Size::new(w * self.scale, h * self.scale),
            (radius * self.scale).into(),
        );
        if let Some(c) = fill {
            self.frame.fill(&path, c);
        }
        self.frame.stroke(&path, self.stroke(1.5));
    }

    /// A box with no outline, only its fill.
    fn solid(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let path = Path::rectangle(self.at(x, y), Size::new(w * self.scale, h * self.scale));
        self.frame.fill(&path, color);
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
        let path = Path::line(self.at(x1, y1), self.at(x2, y2));
        self.frame.stroke(&path, self.stroke(width));
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Option<Color>, width: f32) {
        let path = Path::circle(self.at(cx, cy), r * self.scale);
        if let Some(c) = fill {
            self.frame.fill(&path, c);
        }
        self.frame.stroke(&path, self.stroke(width));
    }

    fn poly(&mut self, points: &[(f32, f32)], fill: Color) {
        let pts: Vec<Point> = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        let path = Path::new(|b| {
            b.move_to(pts[0]);
            for p in &pts[1..] {
                b.line_to(*p);
            }
            b.close();
        });
        self.frame.fill(&path, fill);
        self.frame.stroke(&path, self.stroke(1.5));
    }

    /// A status LED: signal green when the machine runs VMs, paper otherwise.
    fn led(&mut self, cx: f32, cy: f32, active: bool) {
        let fill = if active {
            Tone::Ok.color()
        } else {
            crate::theme::palette().paper
        };
        self.circle(cx, cy, 1.6, Some(fill), 1.0);
    }
}

/// A small GPU card beside the device, its riser joining it at y 46.
fn gpu_card(p: &mut Pen, riser: f32) {
    p.line(riser, 46.0, 86.0, 46.0, 1.5);
    p.rect(86.0, 36.0, 28.0, 24.0, 0.0, Some(SHEET));
    p.circle(98.0, 46.0, 6.0, None, 1.5);
    p.circle(98.0, 46.0, 1.5, None, 1.5);
    p.solid(90.0, 56.0, 20.0, 3.0, GOLD);
}

/// A 2U rack chassis: two front bays with drive slots, vents and an LED row. Returns its riser's x.
fn server(p: &mut Pen, active: bool) -> f32 {
    p.rect(8.0, 18.0, 68.0, 44.0, 0.0, Some(SHEET));
    p.rect(
        13.0,
        23.0,
        40.0,
        14.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    for x in [17.0, 21.0, 25.0] {
        p.line(x, 27.0, x, 33.0, 1.5);
    }
    p.rect(
        13.0,
        41.0,
        40.0,
        15.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    for y in [45.0, 48.0, 51.0, 54.0] {
        p.line(17.0, y, 49.0, y, 1.0);
        p.line(58.0, y, 73.0, y, 1.0);
    }
    for x in [58.0, 63.0, 68.0, 73.0] {
        p.led(x, 30.0, active);
    }
    76.0
}

/// A tower: a front panel with a drive bay, a power button, LEDs and side vents.
fn workstation(p: &mut Pen, active: bool) -> f32 {
    p.rect(26.0, 8.0, 42.0, 76.0, 0.0, Some(SHEET));
    p.poly(
        &[(68.0, 8.0), (76.0, 3.0), (76.0, 79.0), (68.0, 84.0)],
        crate::theme::palette().paper,
    );
    for y in [30.0, 36.0, 42.0] {
        p.line(70.0, y - 1.25, 75.0, y - 4.4, 1.0);
    }
    p.rect(
        32.0,
        14.0,
        30.0,
        18.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    p.line(36.0, 22.0, 58.0, 22.0, 1.0);
    p.line(36.0, 26.0, 58.0, 26.0, 1.0);
    for y in [70.0, 73.0, 76.0, 79.0] {
        p.line(34.0, y, 62.0, y, 1.0);
    }
    p.circle(52.0, 60.0, 3.5, Some(crate::theme::palette().paper), 1.5);
    p.led(36.0, 60.0, active);
    p.led(42.0, 60.0, active);
    76.0
}

/// A single-board computer: a green board, a CPU die, a GPIO header, USB and ethernet.
fn edge(p: &mut Pen, active: bool) -> f32 {
    p.ink = EDGE_GREEN;
    p.rect(10.0, 30.0, 70.0, 40.0, 0.0, Some(SHEET));
    for (x, y) in [(14.0, 34.0), (76.0, 34.0), (14.0, 66.0), (76.0, 66.0)] {
        p.circle(x, y, 1.5, None, 1.0);
    }
    p.ink = INK_2;
    p.rect(
        30.0,
        40.0,
        20.0,
        20.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    p.rect(36.0, 46.0, 8.0, 8.0, 0.0, None);
    for i in 0..10 {
        let x = 52.0 + i as f32 * 2.2;
        p.circle(x, 34.0, 0.9, None, 0.8);
        p.circle(x, 37.0, 0.9, None, 0.8);
    }
    p.rect(
        69.0,
        48.0,
        11.0,
        9.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    p.rect(
        69.0,
        58.0,
        11.0,
        9.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    p.rect(
        54.0,
        60.0,
        13.0,
        10.0,
        0.0,
        Some(crate::theme::palette().paper),
    );
    p.led(18.0, 62.0, active);
    p.led(23.0, 62.0, active);
    80.0
}

/// A flat rounded box with a front light and a top line: a Mac mini or Studio.
fn mini(p: &mut Pen, active: bool) -> f32 {
    p.rect(10.0, 30.0, 66.0, 30.0, 8.0, Some(SHEET));
    p.line(18.0, 35.0, 66.0, 35.0, 1.0);
    p.line(20.0, 50.0, 50.0, 50.0, 1.0);
    p.line(20.0, 53.0, 50.0, 53.0, 1.0);
    p.led(66.0, 45.0, active);
    76.0
}

/// A laptop, lid open: a screen with a camera dot, a base wedge with a keyboard line and a light.
fn laptop(p: &mut Pen, active: bool) -> f32 {
    p.rect(26.0, 14.0, 68.0, 46.0, 4.0, Some(SHEET));
    p.rect(
        31.0,
        19.0,
        58.0,
        36.0,
        2.0,
        Some(crate::theme::palette().paper),
    );
    p.circle(60.0, 16.5, 0.9, None, 0.8);
    p.poly(
        &[(12.0, 60.0), (108.0, 60.0), (113.0, 70.0), (7.0, 70.0)],
        SHEET,
    );
    p.line(24.0, 65.0, 96.0, 65.0, 1.0);
    p.led(102.0, 66.0, active);
    0.0
}

/// A MacBook: a thin lid with a camera notch and a lit screen, on a slim aluminium base.
fn macbook(p: &mut Pen, active: bool) -> f32 {
    p.rect(24.0, 14.0, 72.0, 48.0, 6.0, Some(SHEET));
    p.rect(
        29.0,
        19.0,
        62.0,
        38.0,
        3.0,
        Some(crate::theme::palette().paper),
    );
    p.rect(
        55.0,
        15.5,
        10.0,
        2.5,
        1.2,
        Some(crate::theme::palette().paper),
    );
    p.rect(12.0, 62.0, 96.0, 6.0, 3.0, Some(SHEET));
    p.line(44.0, 65.0, 76.0, 65.0, 1.0);
    p.led(104.0, 65.0, active);
    0.0
}

/// A phone standing upright: a slim body, a screen, a speaker grille and a home bar.
fn phone(p: &mut Pen, active: bool) -> f32 {
    p.rect(40.0, 8.0, 40.0, 74.0, 7.0, Some(SHEET));
    p.rect(
        44.0,
        14.0,
        32.0,
        60.0,
        2.0,
        Some(crate::theme::palette().paper),
    );
    p.line(55.0, 11.0, 65.0, 11.0, 1.0);
    p.line(54.0, 78.0, 66.0, 78.0, 1.5);
    p.led(72.0, 11.0, active);
    0.0
}

struct Device {
    look: Look,
    gpu: bool,
    active: bool,
}

impl canvas::Program<Msg> for Device {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut pen = Pen {
            frame: canvas::Frame::new(renderer, bounds.size()),
            scale: bounds.width / 120.0,
            ink: INK_2,
        };
        let riser = match self.look {
            Look::Server => server(&mut pen, self.active),
            Look::Workstation => workstation(&mut pen, self.active),
            Look::Edge => edge(&mut pen, self.active),
            Look::Laptop => laptop(&mut pen, self.active),
            Look::MacBook => macbook(&mut pen, self.active),
            Look::Mini => mini(&mut pen, self.active),
            Look::Phone => phone(&mut pen, self.active),
        };
        let portable = matches!(self.look, Look::Laptop | Look::MacBook | Look::Phone);
        if self.gpu && !portable {
            gpu_card(&mut pen, riser);
        }
        vec![pen.frame.into_geometry()]
    }
}

/// The machine's device drawing, 160 x 120 (the web draws it at 160 px wide on a 120 x 90 grid). The line art sits under
/// the exterior's 3D card: the card is transparent where no mesh draws (its clear is 0, blitted over the page), so the
/// line art shows through it; where the GPU cannot draw, the line art alone shows.
fn device<'a>(m: &'a Machine, slot: usize) -> Element<'a, Msg> {
    let active = is_live(&m.runs);
    let drawing = Device {
        look: look_of(&m.kind, &m.vendor),
        gpu: !m.gpus.is_empty(),
        active,
    };
    let line_art: Element<'a, Msg> = iced::widget::canvas(drawing)
        .width(Length::Fixed(160.0))
        .height(Length::Fixed(120.0))
        .into();
    match render_card::exterior(slot, &m.kind, &m.vendor, m.gpus.len(), active) {
        Some(card) => iced::widget::stack(vec![line_art, card])
            .width(Length::Fixed(160.0))
            .height(Length::Fixed(120.0))
            .into(),
        None => line_art,
    }
}

// ---- A machine's section ----------------------------------------------------------------------

/// The machine's 3D view: the engine's stage, drawn through the shared shader program at the stage's size.
fn stage_view<'a>(s: &'a StageView) -> Element<'a, Msg> {
    let program = EngineProgram {
        primitive: s.engine.scene(),
        wrap: Msg::Stage,
    };
    shader(program)
        .width(Length::Fixed(STAGE_W))
        .height(Length::Fixed(STAGE_H))
        .into()
}

/// The machine's four flat views (`View::FLAT`) in a row under its 3D stage: each a card with its view's name under it,
/// then one caption with the machine's size in centimetres and its kit model. Returns None when the kit has no model
/// for the machine (the cards have none either).
fn flat_views<'a>(m: &'a Machine) -> Option<Element<'a, Msg>> {
    let active = is_live(&m.runs);
    let machine = render_card::card_machine(&m.kind, &m.vendor, m.gpus.len(), active)?;
    let model = kit::model(fleet::kit_model_of(&machine))?;
    let mut columns: Vec<Element<'a, Msg>> = Vec::with_capacity(View::FLAT.len());
    for (index, view) in View::FLAT.into_iter().enumerate() {
        let card = render_card::flat_view(index, &m.kind, &m.vendor, m.gpus.len(), active, view)?;
        columns.push(column![card, ui::label(view.name())].spacing(4).into());
    }
    // The kit's size is in kit units, 1 unit = 10 cm.
    let cm = model.size * 10.0;
    let caption = format!(
        "{} × {} × {} cm · {}",
        cm.x.round(),
        cm.y.round(),
        cm.z.round(),
        model.id
    );
    Some(
        column![row(columns).spacing(12), ui::faint(caption)]
            .spacing(8)
            .into(),
    )
}

fn section<'a>(state: &'a State, data: &'a Data, m: &'a Machine) -> Element<'a, Msg> {
    let slot = data.machines.iter().position(|x| x.id == m.id).unwrap_or(0);
    let figure: Element<'a, Msg> = device(m, slot);
    let open = state.stage.as_ref().filter(|s| s.id == m.id);
    let orbit_label = if open.is_some() {
        "Hide 3D view"
    } else {
        "Show 3D view"
    };
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(m.name.to_string(), 18.0, INK).into(),
        ui::faint(sentence(m)).into(),
        ui::btn(
            orbit_label,
            None,
            ui::Kind::Secondary,
            Some(Msg::ToggleStage(m.id.clone())),
        ),
        live_stats(m),
        resources(state, data, m),
        ui::label("Runs").into(),
        runs(state, m),
        technical(state, m),
    ];
    if let Some(s) = open {
        let mut stage: Vec<Element<'a, Msg>> = vec![stage_view(s)];
        stage.extend(flat_views(m));
        parts.insert(3, column(stage).spacing(12).into());
    }
    if let Some(st) = &m.station {
        parts.insert(2, station_facts(st));
    }
    container(row![figure, column(parts).spacing(16).width(Length::Fill)].spacing(24))
        .id(section_id(&m.id))
        .width(Length::Fill)
        .into()
}

/// The machine's tallies: CPU with its sparkline, memory, load and the agents' VMs.
fn live_stats<'a>(m: &'a Machine) -> Element<'a, Msg> {
    let Some(s) = &m.stats else {
        return ui::faint("No live stats.").into();
    };
    let tallies = row![
        tally(
            "CPU",
            format!("{}%", s.cpu.round()),
            None,
            Some(spark(&s.history))
        ),
        tally(
            "Memory",
            format!("{} / {}", gib(s.mem_used), gib(s.mem_total)),
            Some("GiB used of GiB".to_string()),
            Some(stacked(
                &[
                    (s.mem_used, INK),
                    ((s.mem_total - s.mem_used).max(0.0), RULE_FAINT)
                ],
                s.mem_total,
                6.0
            )),
        ),
        tally(
            "Load",
            format!("{:.1} · {:.1} · {:.1}", s.load[0], s.load[1], s.load[2]),
            Some("1 / 5 / 15 min".to_string()),
            None,
        ),
        tally(
            "Agents' VMs",
            s.vms_running.to_string(),
            Some(format!("{} GiB reserved", gib(s.guest_mem))),
            None
        ),
    ]
    .spacing(12);
    column![tallies, ui::faint(format!("Up {}", uptime(s.uptime_s)))]
        .spacing(12)
        .into()
}

fn tally<'a>(
    label: &str,
    figure: String,
    caption: Option<String>,
    extra: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    // A figure too long for its tally is ellipsized; a long figure (a load triple) sets smaller so it stays on one line.
    // Labels, figures and captions are single lines: they never wrap.
    let figure = ui::ellipsize(&figure, 18);
    let size = if figure.chars().count() > 12 {
        14.0
    } else {
        20.0
    };
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::label(label).wrapping(Wrapping::None).into(),
        ui::sans(figure, size, INK).wrapping(Wrapping::None).into(),
    ];
    if let Some(extra) = extra {
        parts.push(extra);
    }
    if let Some(caption) = caption {
        parts.push(ui::faint(caption).wrapping(Wrapping::None).into());
    }
    container(column(parts).spacing(8))
        .padding(12)
        .width(Length::FillPortion(1))
        .style(boxed)
        .into()
}

/// The last 60 CPU samples as one ink line over a ruled baseline.
struct Spark(Vec<f64>);

impl canvas::Program<Msg> for Spark {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let (w, h) = (bounds.width, bounds.height);
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.stroke(
            &Path::line(Point::new(0.0, h - 0.5), Point::new(w, h - 0.5)),
            Stroke::default().with_color(RULE).with_width(1.0),
        );
        let pts = &self.0[self.0.len().saturating_sub(SPARK_POINTS)..];
        if pts.len() >= 2 {
            let steps = (pts.len() - 1) as f32;
            let path = Path::new(|b| {
                for (i, cpu) in pts.iter().enumerate() {
                    let x = i as f32 / steps * w;
                    let y = h - 4.0 - (cpu.clamp(0.0, 100.0) as f32 / 100.0) * (h - 8.0);
                    if i == 0 {
                        b.move_to(Point::new(x, y));
                    } else {
                        b.line_to(Point::new(x, y));
                    }
                }
            });
            frame.stroke(&path, Stroke::default().with_color(INK).with_width(2.0));
        }
        vec![frame.into_geometry()]
    }
}

fn spark<'a>(history: &[f64]) -> Element<'a, Msg> {
    iced::widget::canvas(Spark(history.to_vec()))
        .width(Length::Fill)
        .height(Length::Fixed(SPARK_H))
        .into()
}

/// A bar split into segments, each a share of `span`; the rest of the bar stays empty.
fn stacked<'a>(segments: &[(f64, Color)], span: f64, height: f32) -> Element<'a, Msg> {
    let mut children: Vec<Element<'a, Msg>> = Vec::new();
    for &(value, color) in segments {
        let portion = (value / span.max(f64::MIN_POSITIVE) * 1000.0)
            .round()
            .clamp(0.0, 1000.0) as u16;
        if portion == 0 {
            continue;
        }
        let cell = container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::FillPortion(portion))
            .height(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(color.into()),
                ..Default::default()
            });
        children.push(cell.into());
    }
    row(children)
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .into()
}

/// A small square in a colour, for legends and status marks.
fn swatch<'a>(color: Color, size: f32) -> Element<'a, Msg> {
    container(
        Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(color.into()),
        ..Default::default()
    })
    .into()
}

/// Where the machine's resources go: CPU and memory split four ways (services, agents, the rest
/// of the system, free), then the plaza's services and the agents' VMs.
fn resources<'a>(state: &'a State, data: &'a Data, m: &'a Machine) -> Element<'a, Msg> {
    let heading = ui::label("Where the resources go");
    let Some(stats) = &m.stats else {
        return column![
            heading,
            ui::faint("This node doesn't report a breakdown yet.")
        ]
        .spacing(8)
        .into();
    };
    let Some(plaza) = &stats.plaza else {
        return column![
            heading,
            ui::faint("This node doesn't report a breakdown yet.")
        ]
        .spacing(8)
        .into();
    };
    let t = &plaza.total;
    let cpu_parts = [
        ("Plaza services", t.services_cpu, INK),
        ("Agents", t.vms_cpu, SIGNAL),
        (
            "Rest of the system",
            (stats.cpu - (t.services_cpu + t.vms_cpu)).max(0.0),
            INK_3,
        ),
        ("Free", (100.0 - stats.cpu).max(0.0), RULE_FAINT),
    ];
    let mem_parts = [
        ("Plaza services", t.services_rss, INK),
        ("Agents", t.vms_rss, SIGNAL),
        (
            "Rest of the system",
            (stats.mem_used - (t.services_rss + t.vms_rss)).max(0.0),
            INK_3,
        ),
        (
            "Free",
            (stats.mem_total - stats.mem_used).max(0.0),
            RULE_FAINT,
        ),
    ];

    // The services table.
    let mut service_lines: Vec<Element<'a, Msg>> = vec![
        grid(vec![
            (4, false, ui::label("Service").into()),
            (2, true, ui::label("PID").into()),
            (2, true, ui::label("CPU").into()),
            (2, true, ui::label("Memory").into()),
        ]),
        ui::hairline(),
    ];
    for p in &plaza.services {
        service_lines.push(grid(vec![
            (4, false, ui::faint(p.name.clone()).into()),
            (2, true, ui::mono(p.pid.to_string(), 12.0, INK_2).into()),
            (2, true, ui::sans(pct(p.cpu), 12.0, INK).into()),
            (2, true, ui::sans(human(p.rss), 12.0, INK).into()),
        ]));
        service_lines.push(ui::hairline());
    }

    // The agents' table, sorted as the machine's sort says.
    let sort = state.sorts.get(&m.id).copied().unwrap_or(DEFAULT_SORT);
    let mut vms: Vec<&VmUsage> = plaza.vms.iter().collect();
    vms.sort_by(|a, b| {
        let (x, y) = match sort.key {
            SortKey::Cpu => (a.cpu, b.cpu),
            SortKey::Mem => (a.rss, b.rss),
        };
        let order = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
        if sort.descending {
            order.reverse()
        } else {
            order
        }
    });
    let agents_block: Element<'a, Msg> = if plaza.vms.is_empty() {
        ui::faint("No agent VMs on this node.").into()
    } else {
        let head = |label: &str, key: SortKey| -> Element<'a, Msg> {
            let active = sort.key == key;
            let text = if active {
                format!("{label} {}", if sort.descending { "↓" } else { "↑" })
            } else {
                label.to_string()
            };
            ui::btn(&text, None, Kind::Ghost, Some(Msg::Sort(m.id.clone(), key)))
        };
        let mut lines: Vec<Element<'a, Msg>> = vec![
            grid(vec![
                (3, false, ui::label("Agent").into()),
                (2, false, ui::label("VM").into()),
                (2, true, head("CPU", SortKey::Cpu)),
                (2, true, head("Host memory", SortKey::Mem)),
                (3, true, ui::label("Guest used / usable").into()),
            ]),
            ui::hairline(),
        ];
        for v in &vms {
            let agent = v.cid.and_then(|c| data.agents.get(&c));
            let name = match (agent, v.cid) {
                (Some(a), _) if a.name.trim().is_empty() => "Unnamed".to_string(),
                (Some(a), _) => a.name.clone(),
                (None, None) => "Unattached".to_string(),
                (None, Some(c)) => format!("CID {c}"),
            };
            let role = agent
                .map(|a| a.role_label.clone())
                .or_else(|| v.role.clone())
                .unwrap_or_else(|| "—".to_string());
            let who: Element<'a, Msg> = column![ui::sans(name, 12.0, INK), ui::faint(role)]
                .spacing(2)
                .into();
            lines.push(grid(vec![
                (3, false, who),
                (
                    2,
                    false,
                    ui::sans(format!("{}/{}", v.run, v.vm), 12.0, INK_2).into(),
                ),
                (2, true, ui::sans(pct(v.cpu), 12.0, INK).into()),
                (2, true, ui::sans(human(v.rss), 12.0, INK).into()),
                (3, true, guest_cell(v)),
            ]));
            lines.push(ui::hairline());
        }
        let sum_cpu: f64 = plaza.vms.iter().map(|v| v.cpu).sum();
        let sum_rss: f64 = plaza.vms.iter().map(|v| v.rss).sum();
        let guest_total = if plaza.vms.iter().any(|v| v.guest_used.is_some()) {
            let used: f64 = plaza.vms.iter().filter_map(|v| v.guest_used).sum();
            let usable: f64 = plaza.vms.iter().filter_map(|v| v.usable).sum();
            format!("{} / {}", human(used), human(usable))
        } else {
            "—".to_string()
        };
        lines.push(grid(vec![
            (
                3,
                false,
                ui::sans(format!("{} VMs", plaza.vms.len()), 12.0, INK).into(),
            ),
            (2, false, Space::new().into()),
            (2, true, ui::sans(pct(sum_cpu), 12.0, INK).into()),
            (2, true, ui::sans(human(sum_rss), 12.0, INK).into()),
            (3, true, ui::sans(guest_total, 12.0, INK).into()),
        ]));
        column(lines).into()
    };

    column![
        heading,
        breakdown("CPU", &cpu_parts, 100.0, pct),
        breakdown("Memory", &mem_parts, stats.mem_total, gib_text),
        ui::label("Services"),
        column(service_lines),
        ui::label("Agents"),
        agents_block,
    ]
    .spacing(12)
    .into()
}

/// One stacked bar with its legend: each part's share of the bar, and the bar is at least `whole`.
fn breakdown<'a>(
    title: &str,
    parts: &[(&str, f64, Color)],
    whole: f64,
    fmt: fn(f64) -> String,
) -> Element<'a, Msg> {
    let sum: f64 = parts.iter().map(|p| p.1).sum();
    let span = whole.max(sum).max(1.0);
    let segments: Vec<(f64, Color)> = parts
        .iter()
        .map(|&(_, value, color)| (value, color))
        .collect();
    let mut legend: Vec<Element<'a, Msg>> = Vec::new();
    for &(label, value, color) in parts {
        legend.push(
            row![
                swatch(color, 8.0),
                ui::faint(label.to_string()),
                ui::sans(fmt(value), 11.0, INK)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    column![
        ui::faint(title.to_string()),
        stacked(&segments, span, 10.0),
        row(legend).spacing(16)
    ]
    .spacing(8)
    .into()
}

/// A VM's guest memory as "used / usable", with a thin bar of the share used.
fn guest_cell<'a>(v: &VmUsage) -> Element<'a, Msg> {
    let used = v.guest_used.map_or("—".to_string(), human);
    let usable = v.usable.map_or("—".to_string(), human);
    let mut parts: Vec<Element<'a, Msg>> =
        vec![ui::sans(format!("{used} / {usable}"), 12.0, INK).into()];
    if let (Some(used), Some(usable)) = (v.guest_used, v.usable)
        && usable > 0.0
    {
        parts.push(stacked(
            &[
                (used.min(usable), INK_2),
                ((usable - used).max(0.0), RULE_FAINT),
            ],
            usable,
            4.0,
        ));
    }
    column(parts).spacing(4).into()
}

/// One table line: cells of a portion each, right-aligned when `end`.
fn grid<'a>(cells: Vec<(u16, bool, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let children: Vec<Element<'a, Msg>> = cells
        .into_iter()
        .map(|(portion, end, content)| {
            container(content)
                .width(Length::FillPortion(portion))
                .align_x(if end {
                    Alignment::End
                } else {
                    Alignment::Start
                })
                .into()
        })
        .collect();
    row(children)
        .spacing(12)
        .padding([8, 12])
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// The machine's runs: a row each, opened to its VMs when clicked, with a kill.
fn runs<'a>(state: &'a State, m: &'a Machine) -> Element<'a, Msg> {
    if m.runs.is_empty() {
        return ui::faint("No runs.").into();
    }
    let mut lines: Vec<Element<'a, Msg>> = vec![
        grid(vec![
            (3, false, ui::label("Run").into()),
            (2, false, ui::label("Role").into()),
            (2, false, ui::label("State").into()),
            (2, true, ui::label("VMs").into()),
            (2, true, ui::label("Started").into()),
            (2, true, Space::new().into()),
        ]),
        ui::hairline(),
    ];
    for run in &m.runs {
        let kill = ui::btn(
            "Kill",
            Some("stop"),
            Kind::Danger,
            Some(Msg::Kill(run.id.clone())),
        );
        let line = grid(vec![
            (3, false, ui::mono(run.id.clone(), 12.0, INK).into()),
            (
                2,
                false,
                ui::faint(run.role.clone().unwrap_or_else(|| "fleet".to_string())).into(),
            ),
            (2, false, ui::badge(&run.state, state_tone(&run.state))),
            (2, true, ui::faint(format!("{} VMs", run.vms.len())).into()),
            (
                2,
                true,
                ui::faint(format!("started {}", ago(run.created_at))).into(),
            ),
            (2, true, kill),
        ]);
        lines.push(
            mouse_area(line)
                .on_press(Msg::ToggleRun(run.id.clone()))
                .into(),
        );
        if state.expanded.contains(&run.id) {
            lines.push(vm_details(state, run));
        }
        lines.push(ui::hairline());
    }
    column(lines).into()
}

/// An open run's VMs, read on open and on every refresh.
fn vm_details<'a>(state: &'a State, run: &'a Run) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match state.run_vms.get(&run.id) {
        None => ui::faint("Loading…").into(),
        Some(Err(e)) => ui::faint(e.clone()).into(),
        Some(Ok(vms)) if vms.is_empty() => ui::faint("No VMs.").into(),
        Some(Ok(vms)) => column(
            vms.iter()
                .map(|vm| vm_line(state, &run.id, vm))
                .collect::<Vec<_>>(),
        )
        .spacing(8)
        .into(),
    };
    container(body).padding([8, 24]).width(Length::Fill).into()
}

fn vm_line<'a>(state: &'a State, run: &str, vm: &'a VmRow) -> Element<'a, Msg> {
    let key = format!("{run}/{}", vm.index);
    let guest = vm.guest_cid.map_or("—".to_string(), |c| c.to_string());
    let head: Element<'a, Msg> = row![
        ui::sans(format!("VM {}", vm.index), 12.0, INK),
        ui::badge(&vm.state, state_tone(&vm.state)),
        ui::faint(format!("guest CID {guest}")),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into();
    let mut lines: Vec<Element<'a, Msg>> = vec![head];
    if vm.serial.is_some() {
        let open = state.serial_open.contains(&key);
        lines.push(ui::btn(
            if open { "Serial ▾" } else { "Serial ▸" },
            None,
            Kind::Ghost,
            Some(Msg::ToggleSerial(key.clone())),
        ));
        if let (true, Some(serial)) = (open, &vm.serial) {
            lines.push(
                ui::scroll(
                    container(ui::mono(serial.clone(), 11.0, INK_2))
                        .padding(8)
                        .style(boxed),
                )
                .width(Length::Fill)
                .into(),
            );
        }
    }
    column(lines).spacing(6).into()
}

/// The machine's technical details, folded away.
fn technical<'a>(state: &'a State, m: &'a Machine) -> Element<'a, Msg> {
    let open = state.tech_open.contains(&m.id);
    let toggle = ui::btn(
        if open {
            "Technical details ▾"
        } else {
            "Technical details ▸"
        },
        None,
        Kind::Ghost,
        Some(Msg::ToggleTech(m.id.clone())),
    );
    if !open {
        return toggle;
    }
    let port = m.noded_port.map_or("—".to_string(), |p| p.to_string());
    let rows = [
        ("Class", m.class.clone()),
        ("Arch", m.arch.clone()),
        ("Backend", m.backend.clone()),
        ("Irqchip", m.irqchip.clone()),
        ("vsock", m.vsock_mode.clone()),
        ("noded port", port),
    ];
    let mut lines: Vec<Element<'a, Msg>> = vec![toggle];
    for (key, value) in rows {
        lines.push(
            row![
                container(ui::faint(key.to_string())).width(Length::Fixed(160.0)),
                ui::sans(value, 12.0, INK)
            ]
            .spacing(12)
            .into(),
        );
    }
    column(lines).spacing(8).into()
}

// ---- Helpers ----------------------------------------------------------------------------------

fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: RULE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn is_live(runs: &[Run]) -> bool {
    runs.iter()
        .any(|r| r.vms.iter().any(|vm| state_tone(&vm.state) == Tone::Ok))
}

fn machine_vms(m: &Machine) -> usize {
    m.runs.iter().map(|r| r.vms.len()).sum()
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "server" => "Server",
        "workstation" => "Workstation",
        "laptop" => "Laptop",
        "edge" => "Edge",
        "phone" => "Phone",
        other => other,
    }
}

fn state_tone(state: &str) -> Tone {
    let s = state.to_lowercase();
    if ["run", "ready", "up", "active"]
        .iter()
        .any(|w| s.contains(*w))
    {
        Tone::Ok
    } else if ["start", "boot", "pend", "creat", "launch"]
        .iter()
        .any(|w| s.contains(*w))
    {
        Tone::Info
    } else if ["fail", "err", "crash", "kill", "dead"]
        .iter()
        .any(|w| s.contains(*w))
    {
        Tone::Danger
    } else {
        Tone::Muted
    }
}

/// "Apple M3 Max · 16 cores (12P + 4E) · 40-core GPU · 64 GB unified".
fn apple_line(a: &Apple) -> String {
    format!(
        "{} · {} cores ({}P + {}E) · {}-core GPU · {} GB unified",
        a.chip,
        a.p_cores + a.e_cores,
        a.p_cores,
        a.e_cores,
        a.gpu_cores,
        a.unified_memory_gib
    )
}

/// "macOS mini", "Windows laptop": the station's OS and form, as the hub reports them.
fn station_name_line(st: &Station) -> String {
    let os = match st.os.as_str() {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    let form = if st.form == "vm" {
        "VM"
    } else {
        st.form.as_str()
    };
    [os, form]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The station's status, its leases (and how many are active) and the badge for the owner's account.
fn station_facts<'a>(st: &'a Station) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!(
            "{} · {} leases, {} active",
            st.status, st.leases, st.active
        ))
        .into(),
    ];
    if let Some(badge) = &st.badge {
        lines.push(ui::faint(badge.clone()).into());
    }
    column(lines).spacing(4).into()
}

fn sentence(m: &Machine) -> String {
    if let Some(st) = &m.station {
        let spec = format!(
            "{}-core {} · {} GiB memory",
            m.cpus,
            m.arch,
            (m.memory_mib / 1024.0).round()
        );
        let mut parts = vec![kind_label(&m.kind).to_string(), station_name_line(st), spec];
        if !m.gpus.is_empty() {
            parts.push(m.gpus.join(", "));
        }
        return parts.join(" · ");
    }
    let chip = match &m.apple {
        Some(a) => apple_line(a),
        None => format!(
            "{}-core {} on {} · {} GiB memory",
            m.cpus,
            m.arch,
            m.backend.to_uppercase(),
            (m.memory_mib / 1024.0).round()
        ),
    };
    let mut parts = vec![kind_label(&m.kind).to_string()];
    if let Some(model) = &m.model {
        parts.push(model.clone());
    }
    parts.push(chip);
    let head = parts.join(" · ");
    if m.gpus.is_empty() {
        head
    } else {
        format!("{head} · {}", m.gpus.join(", "))
    }
}

fn uptime(s: f64) -> String {
    if s < 60.0 {
        return format!("{}s", s.floor());
    }
    let mins = (s / 60.0).floor() as u64;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h {}m", mins % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

fn gib(mib: f64) -> String {
    format!("{:.1}", mib / 1024.0)
}

fn gib_text(mib: f64) -> String {
    format!("{} GiB", gib(mib))
}

fn human(mib: f64) -> String {
    if mib >= 1024.0 {
        gib_text(mib)
    } else {
        format!("{} MiB", mib.round())
    }
}

fn pct(n: f64) -> String {
    // A rounding residue below a tenth of a point reads as zero, never as a signed "-0.0%".
    let n = if n.abs() < 0.05 { 0.0 } else { n };
    format!("{n:.1}%")
}

/// A relative time since `ms` (as the web's `ago`); "—" for no time.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    let s = ((now - ms) / 1000.0).round().max(0.0) as u64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

// ---- Reading the hub's JSON -------------------------------------------------------------------

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn opt_num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn opt_count(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn num_at(v: &Value, path: &str) -> f64 {
    v.pointer(path).and_then(Value::as_f64).unwrap_or(0.0)
}

fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn parse_data(v: &Value) -> Data {
    Data {
        overview: v
            .get("overview")
            .filter(|o| o.is_object())
            .map(parse_overview),
        machines: items(v, "machines")
            .iter()
            .filter_map(parse_machine)
            .chain(items(v, "stations").iter().filter_map(parse_station))
            .collect(),
        agents: items(v, "agents").iter().filter_map(parse_agent).collect(),
    }
}

fn parse_overview(o: &Value) -> Overview {
    Overview {
        total: o
            .pointer("/agents/total")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        working: o
            .pointer("/agents/working")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        hub: o.get("hub").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn parse_agent(a: &Value) -> Option<(u64, AgentRef)> {
    let cid = a.get("cid").and_then(Value::as_u64)?;
    let role = text(a, "role");
    let label = text(a, "role_label");
    let role_label = if label.is_empty() { role } else { label };
    Some((
        cid,
        AgentRef {
            name: text(a, "name"),
            role_label,
        },
    ))
}

fn parse_machine(m: &Value) -> Option<Machine> {
    let id = text(m, "id");
    if id.is_empty() {
        return None;
    }
    Some(Machine {
        name: text(m, "name"),
        kind: text(m, "kind"),
        vendor: text(m, "vendor"),
        model: opt_text(m, "model"),
        apple: m.get("apple").filter(|a| a.is_object()).map(|a| Apple {
            chip: text(a, "chip"),
            p_cores: count(a, "p_cores"),
            e_cores: count(a, "e_cores"),
            gpu_cores: count(a, "gpu_cores"),
            unified_memory_gib: count(a, "unified_memory_gib"),
        }),
        class: text(m, "class"),
        arch: text(m, "arch"),
        backend: text(m, "backend"),
        irqchip: text(m, "irqchip"),
        vsock_mode: text(m, "vsock_mode"),
        noded_port: opt_count(m, "noded_port"),
        cpus: count(m, "cpus"),
        memory_mib: num(m, "memory_mib"),
        gpus: items(m, "gpus").iter().map(|g| text(g, "name")).collect(),
        runs: items(m, "runs").iter().map(parse_run).collect(),
        agents: count(m, "agents"),
        reachable: m.get("reachable").and_then(Value::as_bool),
        stats: m.get("stats").filter(|s| s.is_object()).map(parse_stats),
        station: None,
        id,
    })
}

/// A workstation from `GET /v1/stations`, as a machine: its facts, its GPUs with their VRAM, its leases (the active and
/// the opening ones are its agents) and the "agents act as" badge when the owner's account is used.
fn parse_station(s: &Value) -> Option<Machine> {
    let id = text(s, "id");
    if id.is_empty() {
        return None;
    }
    let facts = s.get("facts").filter(|f| f.is_object()).unwrap_or(s);
    let leases = items(s, "leases");
    let state_count =
        |state: &str| leases.iter().filter(|l| text(l, "state") == state).count() as u64;
    let active = state_count("active");
    let badge = match facts.get("account") {
        Some(a) if text(a, "mode") == "owner" => Some(format!("agents act as {}", text(a, "name"))),
        _ => None,
    };
    let gpus: Vec<String> = items(facts, "gpus")
        .iter()
        .map(|g| match count(g, "vram_mb") {
            0 => text(g, "name"),
            mib => format!(
                "{} ({} GiB VRAM)",
                text(g, "name"),
                (mib as f64 / 1024.0).round()
            ),
        })
        .collect();
    Some(Machine {
        name: text(s, "name"),
        kind: "workstation".to_string(),
        vendor: String::new(),
        model: None,
        apple: None,
        class: String::new(),
        arch: text(facts, "arch"),
        backend: String::new(),
        irqchip: String::new(),
        vsock_mode: String::new(),
        noded_port: None,
        cpus: count(facts, "cpus"),
        memory_mib: num(facts, "ram_mb"),
        gpus,
        runs: Vec::new(),
        agents: active + state_count("opening"),
        reachable: Some(s.get("online").and_then(Value::as_bool).unwrap_or(false)),
        stats: None,
        station: Some(Station {
            status: text(s, "status"),
            os: text(facts, "os"),
            form: text(facts, "form"),
            leases: leases.len() as u64,
            active,
            badge,
        }),
        id,
    })
}

fn parse_stats(s: &Value) -> Stats {
    let load = items(s, "load");
    let at = |i: usize| load.get(i).and_then(Value::as_f64).unwrap_or(0.0);
    Stats {
        cpu: num(s, "cpu_percent"),
        load: [at(0), at(1), at(2)],
        mem_total: num(s, "mem_total_mib"),
        mem_used: num(s, "mem_used_mib"),
        uptime_s: num(s, "uptime_s"),
        vms_running: count(s, "vms_running"),
        guest_mem: num(s, "guest_mem_mib"),
        history: items(s, "history")
            .iter()
            .map(|p| num(p, "cpu_percent"))
            .collect(),
        plaza: s.get("plaza").filter(|p| p.is_object()).map(parse_plaza),
    }
}

fn parse_plaza(p: &Value) -> Plaza {
    Plaza {
        services: items(p, "services").iter().map(parse_proc).collect(),
        vms: items(p, "vms").iter().map(parse_vm_usage).collect(),
        total: Total {
            services_cpu: num_at(p, "/total/services_cpu_percent"),
            services_rss: num_at(p, "/total/services_rss_mib"),
            vms_cpu: num_at(p, "/total/vms_cpu_percent"),
            vms_rss: num_at(p, "/total/vms_rss_mib"),
        },
    }
}

fn parse_proc(v: &Value) -> Proc {
    Proc {
        name: text(v, "name"),
        pid: count(v, "pid"),
        cpu: num(v, "cpu_percent"),
        rss: num(v, "rss_mib"),
    }
}

fn parse_vm_usage(v: &Value) -> VmUsage {
    VmUsage {
        run: text(v, "run"),
        vm: count(v, "vm"),
        cid: opt_count(v, "cid"),
        role: opt_text(v, "role"),
        cpu: num(v, "cpu_percent"),
        rss: num(v, "rss_mib"),
        usable: opt_num(v, "usable_mib"),
        guest_used: opt_num(v, "guest_used_mib"),
    }
}

fn parse_run(r: &Value) -> Run {
    Run {
        id: text(r, "id"),
        state: text(r, "state"),
        role: opt_text(r, "role"),
        created_at: num(r, "created_at"),
        vms: items(r, "vms").iter().map(parse_vm_row).collect(),
    }
}

fn parse_vm_row(v: &Value) -> VmRow {
    VmRow {
        index: count(v, "index"),
        state: text(v, "state"),
        guest_cid: opt_count(v, "guest_cid"),
        serial: opt_text(v, "serial"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn machines_show_a_station_as_a_workstation_with_its_leases_and_account() {
        let station = |mode: &str| {
            json!({
                "id": "s1", "name": "studio-mini", "status": "online", "online": true,
                "facts": {
                    "os": "macos", "os_version": "15.1", "arch": "arm64", "hostname": "studio", "form": "mini",
                    "cpus": 10, "ram_mb": 32768,
                    "gpus": [{"vendor": "apple", "name": "Apple M4 Pro GPU", "vram_mb": 0}],
                    "account": {"mode": mode, "name": "zarathos"}
                },
                "leases": [
                    {"id": "l1", "agent": "a1", "state": "active", "kind": "pixel", "task": "t1"},
                    {"id": "l2", "agent": "a2", "state": "queued", "kind": "pixel", "task": "t2"}
                ]
            })
        };
        let owned = parse_station(&station("owner")).expect("a station with an id");
        assert_eq!(owned.kind, "workstation");
        assert_eq!(owned.agents, 1);
        assert_eq!(owned.reachable, Some(true));
        let line = sentence(&owned);
        assert!(
            line.starts_with("Workstation · macOS mini · 10-core arm64"),
            "{line}"
        );
        assert!(line.contains("Apple M4 Pro GPU"), "{line}");
        let st = owned.station.as_ref().expect("a station");
        assert_eq!((st.leases, st.active), (2, 1));
        assert_eq!(st.badge.as_deref(), Some("agents act as zarathos"));
        let agent_user = parse_station(&station("agent_user")).expect("a station with an id");
        assert_eq!(
            agent_user.station.as_ref().and_then(|s| s.badge.clone()),
            None
        );
    }

    #[test]
    fn the_grid_is_the_default_and_an_open_machine_switches_to_the_list() {
        let mut state = State::default();
        assert_eq!(state.mode, Mode::Grid);
        assert!(state.in_grid());
        open_in_list(&mut state, "m2");
        assert_eq!(state.mode, Mode::List);
        assert_eq!(state.selected.as_deref(), Some("m2"));
    }
}
