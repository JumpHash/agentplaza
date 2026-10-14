//! The Floor's stage: what the Floor shows and how it moves, platform-agnostic. The host (the desktop's
//! screen, later the browser) owns the window and the fleet's transport; this owns the scenes (the machines
//! view, the board, the ENTER flight), the view state and the capture hooks. Moved here from
//! `apps/desktop/src/console/screens/floor.rs` (Phase 1): the logic is unchanged.

use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use crate::controls::{Orbit, FOV_DEG};
use crate::entry_scene::{Entry, Handoff, Parts, Phase, Pose};
use crate::fleet::{self, FleetResponse, FleetScene, Machine};
use crate::hardware::{self, Params, Status};
use crate::hardware_lab as hw_lab;
use crate::robots::lab;
use crate::rooms::lab as room_lab;
use crate::scene::Frame;
use crate::world::view::machine_overlays;
use crate::world::{AgentInfo, Floor};

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

/// How much of the scene still moves after a tick. `Rest < Ambient < Active`, so `max` combines the sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Motion {
    /// Nothing animates: the host may stop drawing until input or data arrives.
    Rest,
    /// Only clock-driven decoration turns (stars, planet, dust, pulses, fans, packets, atoms): a pure function of the
    /// clock, so it may be drawn at a low rate.
    Ambient,
    /// Something is in motion (damping, tweens, the ENTER flight, walking robots, board work): draw at full rate.
    Active,
}

pub struct Stage {
    /// The per-machine view: the machine's board with its agents.
    pub floor: Floor,
    /// The agents from `GET /v1/agents`: the board's rooms and robots are the ones on its machine.
    pub agents: Vec<AgentInfo>,
    /// The Machines view's scene: the Floor's sky and ground, with the fleet's group in its root.
    pub machines: Floor,
    /// The fleet scene, built from the first snapshot.
    pub fleet: Option<FleetScene>,
    /// The latest `GET /v1/fleet` snapshot: the popup's facts and the scene's update.
    pub fleet_data: Option<FleetResponse>,
    pub fleet_error: Option<String>,
    /// A fleet request is in flight; one was sent at least once; one was answered.
    pub fleet_asking: bool,
    pub fleet_started: bool,
    pub fleet_seen: bool,
    /// The shown view.
    pub view: View,
    /// The machine whose popup is open (Machines view only).
    pub popup: Option<String>,
    /// The machine under the pointer (Machines view only).
    pub hovered_machine: Option<String>,
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
    /// `APZ_ROBOT_LAB`: a line-up of robots on the machines' ground, instead of the plaza's robots.
    pub lab: Option<lab::Lab>,
    /// `APZ_ROOM_LAB`: the room lab, rooms on the machines' ground (see `rooms::lab`).
    pub room_lab: Option<room_lab::Lab>,
    /// `APZ_HW_LAB`: one hardware model on the machines' ground (see `hardware_lab`).
    pub hardware_lab: Option<hw_lab::Lab>,
    /// The ENTER sequence (`floor3d/entry_scene.rs`) while it plays or reverses.
    pub entry: Option<Entry>,
    /// A direct load's build wave has been started (once per screen; an ENTER counts too).
    pub direct_waved: bool,
    /// Reduced motion: the ambient visuals hold their t = 0 pose, and `tick` never reports `Motion::Ambient`.
    pub reduced_motion: bool,
    /// The capture hooks' key presses (`APZ_AUTO_ENTER`, `APZ_AUTO_BACK`), when either is set.
    pub auto: Option<Auto>,
    /// One machine on the studio stage (`Cmd::ShowMachine`), when it is shown; it takes over the view.
    pub machine: Option<crate::machine_stage::MachineStage>,
}

/// The capture hooks' key presses: ENTER at `enter` seconds and Back at `back` seconds, counted from the screen's start.
pub struct Auto {
    pub enter: Option<f64>,
    pub back: Option<f64>,
    pub select: Option<f64>,
    pub started: Instant,
    pub entered: bool,
    pub backed: bool,
    pub selected: bool,
}

impl Auto {
    pub fn from_env() -> Option<Auto> {
        let secs = |name: &str| crate::env_var(name).and_then(|s| s.trim().parse::<f64>().ok()).filter(|t| *t >= 0.0);
        let (enter, back, select) = (secs("APZ_AUTO_ENTER"), secs("APZ_AUTO_BACK"), secs("APZ_AUTO_SELECT"));
        (enter.is_some() || back.is_some() || select.is_some()).then(|| Auto {
            enter,
            back,
            select,
            started: Instant::now(),
            entered: false,
            backed: false,
            selected: false,
        })
    }
}

impl Default for Stage {
    fn default() -> Stage {
        let mut floor = Floor::new();
        let mut machines = Floor::machines();
        let lab = lab::build(&mut machines);
        let room_lab = room_lab::build(&mut machines);
        let hardware_lab = hw_lab::build(&mut machines);
        let lab_mode = lab.is_some() || room_lab.is_some() || hardware_lab.is_some();
        let view = match crate::env_var("APZ_FLOOR_MACHINE").filter(|id| !id.is_empty()) {
            Some(id) => View::Machine(id),
            None => View::Machines,
        };
        let popup = crate::env_var("APZ_FLOOR_POPUP").filter(|id| !id.is_empty());
        let frame = Arc::new(if view == View::Machines { machines.frame(1.0) } else { floor.frame(1.0) });
        Stage {
            floor,
            agents: Vec::new(),
            machines,
            lab,
            room_lab,
            hardware_lab,
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
            // A lab keeps its own framing: the fleet does not refit the camera.
            moved: lab_mode,
            entry: None,
            direct_waved: false,
            machine: None,
            reduced_motion: false,
            auto: Auto::from_env(),
        }
    }
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage").field("error", &self.error).field("size", &self.size).finish_non_exhaustive()
    }
}

const BOARD_STEP: Duration = Duration::from_millis(6);

/// The params a machine's board is built with: its GPUs, its Apple chip's cores and memory, and
/// whether it is a Mac Studio (the model's `when` parts read them).
pub fn board_params(m: &Machine) -> Params {
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

/// Whether `APZ_PERF` is set: the frame and prebuild timing lines are printed.
pub fn perf_log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_var("APZ_PERF").is_some())
}

/// Seconds from the Floor's origin (its first `perf_t` call, made when the Floor state is built) to `now`.
pub fn perf_t(now: Instant) -> f64 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    now.saturating_duration_since(*ORIGIN.get_or_init(|| now)).as_secs_f64()
}

/// Milliseconds since `t0`.
pub fn ms_since(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

/// One `APZ_PERF` timing line on stderr.
pub fn perf_line(text: &str) {
    if perf_log_on() {
        eprintln!("agentplaza-desktop: {text}");
    }
}

/// The machine under the stage point `at` (the ray through the Machines' camera).
pub fn machine_at(state: &Stage, at: (f32, f32)) -> Option<String> {
    let (w, h) = state.size;
    let fleet = state.fleet.as_ref()?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (origin, dir) = state.machines.orbit.camera(w / h).ray(at.0, at.1, w, h);
    fleet.pick(origin.to_array(), dir.normalize_or_zero().to_array())
}

/// Milliseconds on a monotonic clock that starts at the first frame.
fn now_ms(at: Instant) -> f64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    at.saturating_duration_since(*EPOCH.get_or_init(|| at)).as_secs_f64() * 1000.0
}

impl Stage {
pub fn active(&mut self) -> &mut Floor {
    match self.view {
        View::Machines => &mut self.machines,
        _ => &mut self.floor,
    }
}

/// Frames the whole plaza for the stage's size (`resetView`). Nothing happens until the stage has a size.
pub fn reset_plaza(&mut self) {
    if self.driving() {
        return; // the ENTER sequence owns the camera
    }
    let (w, h) = self.size;
    if w > 0.0 && h > 0.0 {
        self.floor.reset_view(w / h);
        self.floor.fitted = true;
        // Snapshot knobs: `APZ_FLOOR_ZOOM=<factor>` multiplies the camera distance (2 frames the board
        // twice as far), and `APZ_FLOOR_ORBIT=<azimuth_deg>,<polar_deg>` sets the eye's direction from the
        // target (the polar angle from +Y). Both are for the detail and cutaway checks.
        if let Some(zoom) = crate::env_var("APZ_FLOOR_ZOOM").and_then(|s| s.parse::<f32>().ok()).filter(|z| *z > 0.0) {
            self.floor.orbit.radius *= zoom;
        }
        if let Some((az, polar)) = crate::env_var("APZ_FLOOR_ORBIT").and_then(|s| {
            let (az, polar) = s.split_once(',')?;
            Some((az.trim().parse::<f32>().ok()?, polar.trim().parse::<f32>().ok()?))
        }) {
            self.floor.orbit.theta = az.to_radians();
            self.floor.orbit.phi = polar.to_radians();
        }
    }
}

/// The first fit of the board: once there is a board and the stage has a size.
pub fn fit_plaza(&mut self) {
    if self.floor.board.is_some() {
        self.reset_plaza();
    }
}

/// The machine whose board is shown: the one named, else the hub, else the first. None without a fleet.
pub fn board_of<'a>(&'a self, named: Option<&str>) -> Option<&'a Machine> {
    let data = self.fleet_data.as_ref()?;
    let named = named.and_then(|id| data.machines.iter().find(|m| m.id == id));
    named.or_else(|| data.machines.iter().find(|m| m.role == "hub")).or_else(|| data.machines.first())
}


/// Shows the Machine view's board: the machine's own model with its params, or the default
/// workstation without a fleet, and the agents on the machine (the hub's own when it names none).
/// The board is rebuilt only when its model, params or status change; the rooms and robots follow
/// the agents every time.
pub fn refresh_board(&mut self) {
    // The popup's machine is built while its popup shows (so ENTER only starts the sequence): the Machine view
    // names its machine, the Plaza names none (the hub, or the first).
    let named = match &self.view {
        View::Machines => match self.popup.clone() {
            Some(id) => Some(id),
            None => return,
        },
        View::Machine(id) => Some(id.clone()),
        View::Plaza => None,
    };
    let in_machines = matches!(self.view, View::Machines);
    let t0 = Instant::now();
    let (name, params, on, status) = match self.board_of(named.as_deref()) {
        Some(m) => {
            let (_, board) = hardware::models_for(&m.kind, m.vendor.as_deref());
            let on: Vec<AgentInfo> = self.agents.iter().filter(|a| m.agents.contains(&a.cid)).cloned().collect();
            (board, board_params(m), on, fleet::status_of(&m.status))
        }
        None => ("board/workstation", Params::default(), self.agents.clone(), Status::Online),
    };
    let Some(model) = hardware::model(name) else { return };
    let key = format!("{name}|{:?}|{status:?}", params.values);
    let mut built = false;
    // The machine the board is for, as the web's route names it: its rooms are seeded by it, and its built parts are cached by it.
    self.floor.machine_id = named.clone().unwrap_or_default();
    if self.floor.board.as_ref().map(|b| b.key.as_str()) != Some(key.as_str()) {
        self.floor.show_board(key, model, params, status);
        // A new board is framed afresh (a prebuild, while the Machines view shows, leaves the view as it is).
        if !in_machines {
            self.moved = false;
        }
        built = true;
    }
    // While the popup shows, the rooms and the bake wait for `work_board`'s frames (a board in view is built at once).
    if in_machines {
        self.floor.begin_sync(&on);
    } else {
        self.floor.sync(&on);
    }
    if perf_log_on() {
        perf_line(&format!(
            "refresh_board: built {built}, {:.1} ms, rooms queued {}",
            ms_since(t0),
            self.floor.rooms_pending()
        ));
    }
    if !in_machines && !self.floor.fitted {
        self.fit_plaza();
    }
}

/// Frames the fleet's bounding sphere for the stage's size. Nothing happens until both are there.
pub fn fit_machines(&mut self) {
    let (w, h) = self.size;
    // A robot lab keeps the camera its options set (`cam`), and a hardware lab its own framing, not the fleet's.
    if w <= 0.0 || h <= 0.0 || self.lab.is_some() || self.hardware_lab.is_some() {
        return;
    }
    if let Some(fleet) = &self.fleet {
        let f = fleet.frame();
        self.machines.fit_sphere(f.center, f.radius, w / h);
        if !self.machines.orbit.drifting() {
            self.machines.orbit.start_drift(); // the framed view starts the idle sway
        }
    }
}

/// Frames the shown view.
pub fn fit_active(&mut self) {
    match self.view {
        View::Machines => self.fit_machines(),
        _ => self.fit_plaza(),
    }
}

/// Shows another view: its camera is framed afresh and the popup closes.
pub fn set_view(&mut self, view: View) {
    self.view = view;
    self.popup = None;
    self.moved = false;
    self.refresh_board();
    self.machines.orbit.stop_drift(); // a new view starts its own sway
    self.fit_active();
}

/// Whether the ENTER sequence drives the camera.
pub fn driving(&self) -> bool {
    self.entry.as_ref().is_some_and(Entry::driving)
}

/// The time `work_board` may spend per frame: one district's rooms (5 to 18 ms each on the desktop's test machine) or the bake.

/// The popup's board, built while the Machines view shows: its queued rooms one district per frame, then its statics
/// baked. Nothing draws the board in this view, so the work waits for no frame of the sequence.
pub fn work_board(&mut self) {
    if self.floor.rooms_pending() || self.floor.shared_waiting() {
        self.floor.step_rooms(BOARD_STEP);
    } else if self.floor.statics_dirty && self.popup.is_some() {
        self.floor.frame(1.0);
    }
}

/// Builds the board's queued rooms and bake now, for a moment that cannot wait for frames (ENTER).
pub fn finish_board(&mut self) {
    self.floor.finish_rooms();
    if self.floor.statics_dirty {
        self.floor.frame(1.0);
    }
}

/// Opens the popup of machine `id` (a click, or the `APZ_AUTO_SELECT` hook) and prebuilds its board: the popup's
/// machine is queued now and built while the popup shows, so ENTER only starts the sequence.
pub fn open_popup(&mut self, id: Option<String>) {
    let t0 = Instant::now();
    perf_line(&format!("popup open at {:.3} s", perf_t(t0)));
    self.popup = id;
    self.refresh_board();
    perf_line(&format!("popup prebuild {:.1} ms, ended at {:.3} s", ms_since(t0), perf_t(Instant::now())));
}

/// ENTER in a machine's popup: the sequence flies from the Machines' camera into the machine
/// (`floor3d/entry_scene.rs`). The board was built while the popup showed, so this only starts the timeline.
pub fn enter(&mut self, id: String) {
    if self.fleet.is_none() || self.entry.is_some() {
        return;
    }
    self.machines.orbit.stop_drift(); // the ENTER flight takes the camera
    let (w, h) = self.size;
    let aspect = if h > 0.0 { w / h } else { 1.0 };
    let cam = self.machines.orbit.camera(aspect);
    let handoff = Handoff {
        machine: id.clone(),
        from: Pose { eye: cam.eye, target: cam.target },
        fog: (self.machines.world.fog_near, self.machines.world.fog_far),
        back: self.machines.orbit.clone(),
    };
    perf_line(&format!("ENTER at {:.3} s", perf_t(Instant::now())));
    // The board of this machine: prebuilt (the popup built it), so this is a no-op unless the fleet changed.
    self.popup = Some(id.clone());
    self.refresh_board();
    self.finish_board(); // the rooms and the bake still queued are done now: the sequence's first frame does not wait
    self.popup = None;
    self.hovered_machine = None;
    self.view = View::Machine(id);
    self.direct_waved = true;
    let board_fog = (self.floor.world.fog_near, self.floor.world.fog_far);
    self.entry = Some(Entry::handoff(handoff, board_fog));
}

/// Back or Esc: reverses the sequence from where it stands. With no sequence, the board is left at once.
pub fn leave(&mut self) {
    if let Some(entry) = self.entry.as_mut() {
        entry.back();
        return;
    }
    if !matches!(self.view, View::Machines) {
        self.set_view(View::Machines);
    }
}

/// The capture hooks: ENTER in the popup's machine once the fleet is read (retried until it is), and Back once the
/// sequence has started. Each is pressed once.
pub fn auto_tick(&mut self) {
    let Some(auto) = self.auto.as_ref() else { return };
    let t = auto.started.elapsed().as_secs_f64();
    let enter_due = !auto.entered && auto.enter.is_some_and(|at| t >= at);
    let back_due = !auto.backed && auto.back.is_some_and(|at| t >= at);
    let select_due = !auto.selected && auto.select.is_some_and(|at| t >= at);
    if select_due && self.fleet.is_some() && self.view == View::Machines {
        if let Some(id) = self.board_of(None).map(|m| m.id.clone()) {
            self.open_popup(Some(id));
            if let Some(auto) = self.auto.as_mut() {
                auto.selected = true;
            }
        }
    }
    if enter_due && self.fleet.is_some() && self.entry.is_none() {
        if let Some(id) = self.popup.clone().or_else(|| self.board_of(None).map(|m| m.id.clone())) {
            self.enter(id);
            if let Some(auto) = self.auto.as_mut() {
                auto.entered = true;
            }
        }
    }
    if back_due && self.entry.is_some() {
        self.leave();
        if let Some(auto) = self.auto.as_mut() {
            auto.backed = true;
        }
    }
}

/// The reverse has played out: the Machines view takes the screen back at the camera it left (after an ENTER), or
/// refits (after a direct load, as a fresh view does).
pub fn left(&mut self, back: Option<Orbit>) {
    self.view = View::Machines;
    self.popup = None;
    self.hovered_machine = None;
    match back {
        Some(orbit) => {
            self.machines.orbit = orbit;
            self.moved = true;
            self.machines.orbit.start_drift(); // the reverse flight's end restarts the idle sway
        }
        None => {
            self.moved = false;
            self.fit_machines();
        }
    }
}

/// `Msg::Resized`: the stage's new size; an unmoved view refits to it.
pub fn resize(&mut self, w: f32, h: f32) {
    self.size = (w, h);
    if !self.moved {
        self.fit_active();
    }
}

/// Orbits the shown view by a drag.
pub fn rotate(&mut self, dx: f32, dy: f32, height: f32) {
    if let Some(m) = self.machine.as_mut() {
        m.rotate(dx, dy, height);
        return;
    }
    self.moved = true;
    self.machines.orbit.stop_drift(); // input stops the idle sway
    self.active().orbit.rotate(dx, dy, height);
}

/// Pans the shown view by a drag.
pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
    self.moved = true;
    self.machines.orbit.stop_drift(); // input stops the idle sway
    self.active().orbit.pan(dx, dy, height, FOV_DEG);
}

/// Zooms the shown view by a wheel step count.
pub fn dolly(&mut self, steps: f32) {
    if let Some(m) = self.machine.as_mut() {
        m.dolly(steps);
        return;
    }
    self.moved = true;
    self.machines.orbit.stop_drift(); // input stops the idle sway
    self.active().orbit.dolly(steps);
}

/// Frames the shown view afresh (the Reset view control).
pub fn reset_view(&mut self) {
    self.moved = false;
    match self.view {
        View::Machines => {
            self.machines.orbit.stop_drift();
            self.fit_machines(); // the reset starts the idle sway again
        }
        _ => self.reset_plaza(),
    }
}

/// The pointer hovers a machine (Machines view) or a board's part (other views); `None` leaves it.
pub fn hover(&mut self, at: Option<(f32, f32)>) {
    if at.is_some() {
        self.machines.orbit.stop_drift(); // pointer movement over the stage stops the idle sway
    }
    match self.view {
        View::Machines => {
            let hit = at.and_then(|p| machine_at(self, p));
            if let Some(fleet) = self.fleet.as_mut() {
                fleet.hover(hit.as_deref());
            }
            self.hovered_machine = hit;
        }
        _ => {
            let (w, h) = self.size;
            let hit = at.and_then(|p| self.floor.pick(p.0, p.1, w, h));
            self.floor.hovered = hit;
        }
    }
}

/// A click: opens the popup of the machine under the pointer (Machines view) or selects a board's part.
pub fn select(&mut self, at: Option<(f32, f32)>) {
    self.machines.orbit.stop_drift(); // a click stops the idle sway
    match self.view {
        View::Machines => {
            let id = at.and_then(|p| machine_at(self, p));
            self.open_popup(id);
        }
        _ => {
            let (w, h) = self.size;
            let hit = at.and_then(|p| self.floor.pick(p.0, p.1, w, h));
            self.floor.selected = hit;
        }
    }
}

/// One animation tick: the shown scene steps by `dt` (the time since the last tick, at most 50 ms) and the
/// frame it draws is built. The ENTER sequence's phase is followed here too. Returns how much of the scene still
/// moves after this tick: `Active` while damping, tweens, walking robots, the ENTER flight or board work run,
/// `Ambient` while only the clock-driven decoration turns, `Rest` when nothing animates.
pub fn tick(&mut self, now: Instant) -> Motion {
    if self.machine.is_some() {
        return self.tick_machine(now);
    }
    let elapsed = self.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
    let dt = elapsed.min(0.05);
    self.last = Some(now);
    // The ambient visuals run on the clock in seconds (`now_ms`), not on the capped step, so a throttled tick draws
    // the pose a full-rate one would. Reduced motion holds them at t = 0.
    let (ambient, ambient_dt) =
        if self.reduced_motion { (0.0, 0.0) } else { ((now_ms(now) / 1000.0) as f32, elapsed.min(0.5)) };
    let (w, h) = self.size;
    let mut motion = Motion::Rest;
    match self.view {
        View::Machines => {
            if self.machines.orbit.update() {
                motion = Motion::Active;
            }
            if self.machines.orbit.drift_to(now) {
                motion = motion.max(Motion::Ambient); // the idle sway turns the camera on the clock
            }
            self.machines.step(dt, now_ms(now), ambient, ambient_dt);
            motion = motion.max(self.machines.motion());
            if let Some(lab) = self.lab.as_mut() {
                lab.step(dt);
                motion = Motion::Active; // the lab's robots animate their clips
            }
            if let Some(lab) = self.room_lab.as_mut() {
                lab.step(ambient);
                motion = motion.max(Motion::Ambient); // the rooms' screens glow on the clock
            }
            if let Some(lab) = self.hardware_lab.as_mut() {
                lab.step(ambient_dt, ambient);
                motion = motion.max(Motion::Ambient); // the model's fans, LEDs and pulses run on the clock
            }
            if let (Some(fleet), Some(data)) = (self.fleet.as_mut(), self.fleet_data.as_ref()) {
                fleet.update(dt, ambient, ambient_dt, data);
            }
            if let Some(fleet) = &self.fleet {
                motion = motion.max(fleet.motion());
            }
            // Queued rooms and the popup board's bake: work still pending is motion.
            let board_busy = self.floor.rooms_pending() || (self.floor.statics_dirty && self.popup.is_some());
            let shared_waiting = self.floor.shared_waiting();
            self.work_board();
            if board_busy {
                motion = Motion::Active;
            } else if shared_waiting {
                // A shared layout computing off the frame: poll at the ambient rate until it lands.
                motion = motion.max(Motion::Ambient);
            }
            if w > 0.0 && h > 0.0 {
                let camera = self.machines.orbit.camera(w / h);
                let mut frame = self.machines.frame(w / h);
                // The robot lab's `bg` is the Machines view's sky too (the lab's line-up is on the machines' ground).
                if let Some(bg) = self.lab.as_ref().and_then(lab::Lab::background) {
                    frame.world.background = bg;
                    frame.world.flat_background = true;
                }
                frame.animated = self.fleet.as_ref().zip(self.fleet_data.as_ref()).map(|(f, d)| Arc::new(f.instances(d)));
                if let Some(fleet) = &self.fleet {
                    frame.overlays = machine_overlays(fleet, &camera, w, h, self.scale, self.hovered_machine.as_deref());
                }
                self.frame = Arc::new(frame);
            }
        }
        _ => {
            if self.floor.orbit.update() {
                motion = Motion::Active;
            }
            self.floor.step(dt, now_ms(now), ambient, ambient_dt);
            motion = motion.max(self.floor.motion());
            // The shared layout lands after ENTER (it computes off the frame): poll for it here too, and build the rooms
            // it queues, or the board keeps the engine's own layout and its robots never get the navigation.
            if self.floor.rooms_pending() || self.floor.shared_waiting() {
                self.floor.step_rooms(BOARD_STEP);
                motion = motion.max(if self.floor.rooms_pending() { Motion::Active } else { Motion::Ambient });
            }
            // A direct load (no ENTER) plays the build wave once its board is built and the stage has a size.
            if self.entry.is_none() && !self.direct_waved && self.floor.board.is_some() && w > 0.0 && h > 0.0 {
                self.direct_waved = true;
                let fog = (self.floor.world.fog_near, self.floor.world.fog_far);
                self.entry = Some(Entry::direct(fog));
            }
            let aspect = if h > 0.0 { w / h } else { 1.0 };
            let phase = match self.entry.as_mut() {
                Some(entry) => entry.step(
                    f64::from(dt),
                    &mut Parts {
                        floor: &mut self.floor,
                        machines: &mut self.machines,
                        fleet: self.fleet.as_mut(),
                        data: self.fleet_data.as_ref(),
                        aspect,
                        ambient,
                        ambient_dt,
                    },
                ),
                None => Phase::Done,
            };
            // The ENTER flight is Active until it is Done (finished, or none).
            if !matches!(phase, Phase::Done) {
                motion = Motion::Active;
            }
            match phase {
                Phase::Finished => {
                    // The plaza is at its default view now: a resize refits it, as a fresh view does.
                    self.moved = false;
                }
                Phase::Left => {
                    let back = self.entry.take().and_then(Entry::into_return);
                    self.left(back);
                }
                Phase::Playing | Phase::Done => {}
            }
            if w > 0.0 && h > 0.0 {
                let mut frame = self.floor.frame(w / h);
                frame.overlays = self.floor.label_overlays(w, h, self.scale);
                // The robot lab's `bg` (dark or light) replaces the floor's sky colour.
                if let Some(bg) = self.lab.as_ref().and_then(lab::Lab::background) {
                    frame.world.background = bg;
                    frame.world.flat_background = true;
                }
                self.frame = Arc::new(frame);
            }
        }
    }
    if self.reduced_motion && motion == Motion::Ambient {
        Motion::Rest
    } else {
        motion
    }
}

/// The agents from `GET /v1/agents`, parsed by the host: the board follows them.
pub fn apply_agents(&mut self, list: Vec<AgentInfo>) {
    self.error = None;
    self.agents = list;
    self.refresh_board();
}

/// The fleet snapshot from `GET /v1/fleet`, parsed by the host: the scene is built once, and the view follows it.
pub fn apply_fleet(&mut self, data: FleetResponse) {
    self.fleet_error = None;
    self.fleet_seen = true;
    // A lab shows its robots alone: no machines join the line-up.
    if self.fleet.is_none() && self.lab.is_none() && self.room_lab.is_none() && self.hardware_lab.is_none() {
        let fleet = fleet::build_fleet(&data, &self.machines.fleet_palette());
        self.machines.root.add(&fleet.group);
        self.machines.statics_dirty = true; // the fleet's static parts are baked
        self.fleet = Some(fleet);
    }
    self.fleet_data = Some(data);
    // The fallback gives way to the machines once they can be read.
    if self.view == View::Plaza {
        self.set_view(View::Machines);
    } else if self.view == View::Machines && !self.moved {
        self.fit_machines();
    } else if matches!(self.view, View::Machine(_)) {
        // The machine's own model, once the fleet names it.
        self.refresh_board();
    }
}

/// A fleet request that failed (or could not be read): the error is kept, and the plaza is shown
/// directly when nothing was read yet.
pub fn fleet_failed(&mut self, error: String) {
    self.fleet_error = Some(error);
    // Nothing read yet: the plaza is shown directly.
    if !self.fleet_seen && self.view == View::Machines {
        self.set_view(View::Plaza);
    }
}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The default stage in the Machines view, with two online machines and their fleet read.
    fn online_fleet_stage() -> Stage {
        let mut s = Stage::default();
        s.size = (800.0, 600.0);
        let data: FleetResponse = serde_json::from_str(
            r#"{"machines":[{"id":"n0","status":"online"},{"id":"n1","status":"online"}]}"#,
        )
        .unwrap();
        s.apply_fleet(data);
        s
    }

    /// A stage whose fleet is one machine of `kind` (its popup opens on it), with the GPUs or Apple chip its kind has.
    fn one_machine_stage(kind: &str) -> Stage {
        let gpus = match kind {
            "workstation" => 4,
            "server" => 8,
            _ => 0,
        };
        let gpu_json = (0..gpus).map(|_| r#"{"name":"GPU","vendor":"nvidia"}"#).collect::<Vec<_>>().join(",");
        let apple = if kind == "apple-soc" {
            r#","apple":{"chip":"M4 Max","p_cores":12,"e_cores":4,"gpu_cores":40,"unified_memory_gib":64}"#
        } else {
            ""
        };
        let json = format!(
            r#"{{"machines":[{{"id":"m0","role":"hub","kind":"{kind}","status":"online","gpus":[{gpu_json}]{apple}}}]}}"#
        );
        let mut s = Stage::default();
        s.size = (800.0, 600.0);
        s.apply_fleet(serde_json::from_str(&json).unwrap());
        s
    }

    /// Opening a machine and the ticks after it run on the page's thread: each call must stay under 50 ms in release.
    #[test]
    fn opening_a_machine_keeps_every_call_under_50_ms_per_kind() {
        for kind in ["workstation", "server", "laptop", "phone", "edge", "apple-soc"] {
            let mut s = one_machine_stage(kind);
            let t = Instant::now();
            s.open_popup(Some("m0".to_string()));
            let open = ms_since(t);
            let mut slowest = open;
            let mut now = Instant::now();
            for _ in 0..200 {
                now += Duration::from_millis(16);
                let t = Instant::now();
                s.work_board();
                slowest = slowest.max(ms_since(t));
                let t = Instant::now();
                s.tick(now);
                slowest = slowest.max(ms_since(t));
            }
            println!("{kind}: open_popup {open:.1} ms, slowest call {slowest:.1} ms");
            if !cfg!(debug_assertions) {
                assert!(slowest < 50.0, "{kind}: a call took {slowest:.1} ms on the page's thread");
            }
        }
    }

    #[test]
    fn an_online_fleet_at_rest_is_ambient_once_300_ms_pass() {
        let mut s = online_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        assert_eq!(s.tick(t0 + Duration::from_millis(300)), Motion::Ambient);
    }

    #[test]
    fn reduced_motion_holds_the_ambient_visuals_and_reports_rest() {
        let mut s = online_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        s.reduced_motion = true;
        assert_eq!(s.tick(t0 + Duration::from_millis(600)), Motion::Rest);
    }

    #[test]
    fn enter_is_active_until_the_entry_is_done() {
        let mut s = online_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        s.enter("n0".to_string());
        assert!(s.entry.is_some(), "ENTER starts a flight");
        let mut now = t0;
        let mut active_ticks = 0;
        let mut rested = false;
        for _ in 0..10_000 {
            now += Duration::from_millis(16);
            match s.tick(now) {
                Motion::Active => {
                    assert!(!rested, "ENTER went Active again after it had reported rest");
                    active_ticks += 1;
                }
                _ => rested = true,
            }
        }
        assert!(active_ticks > 1, "the flight ran for {active_ticks} ticks");
        assert!(rested, "the ENTER flight never finished");
    }

    #[test]
    fn inside_a_machine_only_its_board_draws() {
        let mut s = Stage::default();
        s.size = (800.0, 600.0);
        let data: FleetResponse = serde_json::from_str(
            r#"{"machines":[{"id":"n0","kind":"edge","status":"online","agents":[1,2,3,4]},{"id":"n1","kind":"laptop","status":"online"},
                {"id":"n2","kind":"phone","status":"online"},{"id":"n3","kind":"workstation","status":"online"},
                {"id":"n4","kind":"server","status":"online"}]}"#,
        )
        .unwrap();
        s.apply_fleet(data);
        let agents: Vec<AgentInfo> = (1..=4u64)
            .map(|cid| AgentInfo { cid, name: format!("a{cid}"), role: ["builder", "reviewer", "ops", "librarian"][cid as usize - 1].into(), status: "idle".into(), tier: "worker".into(), model: String::new(), species: None })
            .collect();
        s.apply_agents(agents);
        let t0 = Instant::now();
        s.tick(t0);
        s.open_popup(Some("n0".to_string()));
        s.enter("n0".to_string());
        let mut now = t0;
        for _ in 0..3000 {
            now += Duration::from_millis(16);
            s.tick(now);
        }
        let frame = s.frame.clone();
        let b = s.floor.board.as_ref().expect("the board").layout.bounds;
        let reach = 0.5 * b.w.hypot(b.d) * 1.15;
        let mut far = Vec::new();
        let all = frame.statics.iter().chain(frame.draws.iter());
        for d in all {
            let p = d.model.transform_point3(glam::Vec3::ZERO);
            if p.x.hypot(p.z) > reach && p.y < 40.0 {
                far.push((d.material.color, p));
            }
        }
        for b in frame.anim_batches.iter() {
            let _ = b;
        }
        assert!(far.is_empty(), "{} draws lie far outside the board, e.g. {:?}", far.len(), &far[..far.len().min(6)]);
    }

    #[test]
    fn an_orbit_drag_is_active_while_damping_then_ambient() {
        let mut s = online_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        s.rotate(40.0, 0.0, 600.0);
        let mut now = t0 + Duration::from_millis(16);
        assert_eq!(s.tick(now), Motion::Active);
        let mut frames = 0;
        loop {
            now += Duration::from_millis(16);
            frames += 1;
            assert!(frames < 2_000, "the drag never settled");
            if s.tick(now) != Motion::Active {
                break;
            }
        }
        assert_eq!(s.tick(now + Duration::from_millis(16)), Motion::Ambient);
    }

    /// The online fleet, framed by a Reset view (which starts the idle sway).
    fn framed_fleet_stage() -> Stage {
        let mut s = online_fleet_stage();
        s.reset_view();
        s
    }

    /// Drags the view, then steps until its damping settles. The sway must not resume.
    fn drag_and_settle(s: &mut Stage, now: &mut Instant) {
        s.rotate(40.0, 0.0, 600.0);
        for _ in 0..2_000 {
            *now += Duration::from_millis(16);
            if s.tick(*now) != Motion::Active {
                return;
            }
        }
        panic!("the drag never settled");
    }

    #[test]
    fn the_idle_sway_turns_the_camera_on_the_clock_and_is_ambient() {
        let mut s = framed_fleet_stage();
        assert!(s.machines.orbit.drifting());
        let t0 = Instant::now();
        s.tick(t0);
        let framed = s.machines.orbit.theta;
        assert_eq!(s.tick(t0 + Duration::from_secs(1)), Motion::Ambient);
        let turned = s.machines.orbit.theta;
        assert!(turned != framed, "the sway did not turn the camera");
        assert!((turned - framed).abs() <= crate::controls::DRIFT_AMP, "turned {turned} from {framed}");
    }

    #[test]
    fn a_pointer_drag_stops_the_idle_sway() {
        let mut s = framed_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        let mut now = t0 + Duration::from_secs(1);
        s.tick(now);
        drag_and_settle(&mut s, &mut now);
        assert!(!s.machines.orbit.drifting());
        let held = s.machines.orbit.theta;
        s.tick(now + Duration::from_secs(5));
        assert!((s.machines.orbit.theta - held).abs() < 1e-3, "the view kept swaying");
    }

    #[test]
    fn reset_view_restarts_the_idle_sway() {
        let mut s = framed_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        let mut now = t0 + Duration::from_secs(1);
        s.tick(now);
        drag_and_settle(&mut s, &mut now);
        assert!(!s.machines.orbit.drifting());
        s.reset_view();
        assert!(s.machines.orbit.drifting());
        now += Duration::from_millis(16);
        s.tick(now);
        let framed = s.machines.orbit.theta;
        assert_eq!(s.tick(now + Duration::from_secs(1)), Motion::Ambient);
        assert!(s.machines.orbit.theta != framed, "the reset view did not sway");
    }

    /// The sway's azimuth `at` seconds after a Reset view, stepped at `fps` ticks a second.
    fn sway_after(fps: u32, at: f64) -> f32 {
        let mut s = framed_fleet_stage();
        let t0 = Instant::now();
        let ticks = (at * f64::from(fps)).round() as u32;
        for k in 0..=ticks {
            s.tick(t0 + Duration::from_secs_f64(f64::from(k) / f64::from(fps)));
        }
        s.machines.orbit.theta
    }

    /// Steps the stage until its flight rests, checking the sway is off while an ENTER sequence is open.
    fn settle_flight(s: &mut Stage, now: &mut Instant) {
        for _ in 0..10_000 {
            *now += Duration::from_millis(16);
            let motion = s.tick(*now);
            if s.entry.is_some() {
                assert!(!s.machines.orbit.drifting(), "the sway runs during the ENTER sequence");
            }
            if motion != Motion::Active {
                return;
            }
        }
        panic!("the flight never settled");
    }

    #[test]
    fn back_restarts_the_idle_sway_once_the_reverse_flight_is_done() {
        let mut s = framed_fleet_stage();
        let t0 = Instant::now();
        s.tick(t0);
        let mut now = t0;
        s.enter("n0".to_string());
        settle_flight(&mut s, &mut now);
        assert!(!s.machines.orbit.drifting());
        s.leave();
        settle_flight(&mut s, &mut now);
        assert!(s.entry.is_none(), "the reverse flight did not finish");
        assert!(matches!(s.view, View::Machines));
        assert!(s.machines.orbit.drifting(), "Back did not restart the sway");
        now += Duration::from_millis(16);
        assert_eq!(s.tick(now), Motion::Ambient);
    }

    #[test]
    fn the_sway_angle_at_a_time_does_not_depend_on_the_frame_rate() {
        for at in [0.5, 2.0] {
            let (fast, slow) = (sway_after(60, at), sway_after(12, at));
            assert!((fast - slow).abs() < 1e-6, "at {at} s: 60 fps {fast}, 12 fps {slow}");
        }
    }
}
