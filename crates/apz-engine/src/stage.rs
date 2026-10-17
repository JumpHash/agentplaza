//! The stage: what the Machines screen shows and how it moves, platform-agnostic. The host (the desktop's screen, the
//! browser's page) owns the window and the fleet's transport; this owns the Machines view's scene, the one-machine
//! stage (`machine_stage`), the labs, the view state and the capture hooks.

use std::sync::Arc;

use web_time::Instant;

use crate::controls::FOV_DEG;
use crate::fleet::{self, FleetResponse, FleetScene};
use crate::hardware_lab as hw_lab;
use crate::robots::lab;
use crate::scene::Frame;
use crate::style::{Palette, Style};
use crate::world::tags::machine_overlays;
use crate::world::{AgentInfo, Floor};

/// How much of the scene still moves after a tick. `Rest < Ambient < Active`, so `max` combines the sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Motion {
    /// Nothing animates: the host may stop drawing until input or data arrives.
    Rest,
    /// Only clock-driven decoration turns (stars, planet, dust, pulses, fans, packets): a pure function of the clock, so
    /// it may be drawn at a low rate.
    Ambient,
    /// Something is in motion (damping, tweens): draw at full rate.
    Active,
}

pub struct Stage {
    /// The agents from `GET /v1/agents`, for the hosts' machine popups.
    pub agents: Vec<AgentInfo>,
    /// The Machines view's scene: the sky and ground, with the fleet's group in its root.
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
    /// The machine whose popup is open.
    pub popup: Option<String>,
    /// The machine under the pointer.
    pub hovered_machine: Option<String>,
    /// The frame the shader draws; rebuilt on every animation tick.
    pub frame: Arc<Frame>,
    /// The stage's size in pixels, as the shader reports it.
    pub size: (f32, f32),
    pub error: Option<String>,
    /// The previous animation tick, for the step's dt.
    pub last: Option<Instant>,
    /// Device pixels per logical pixel, for the labels' textures (2 for crisp text: the window's scale factor is not
    /// known to the widget).
    pub scale: f32,
    /// The user has orbited, panned or zoomed: a resize no longer refits the view.
    pub moved: bool,
    /// `APZ_ROBOT_LAB`: a line-up of robots on the machines' ground.
    pub lab: Option<lab::Lab>,
    /// `APZ_HW_LAB`: the machine model kit on the machines' ground (see `hardware_lab`).
    pub hardware_lab: Option<hw_lab::Lab>,
    /// Reduced motion: the ambient visuals hold their t = 0 pose, and `tick` never reports `Motion::Ambient`.
    pub reduced_motion: bool,
    /// One machine on the studio stage (`Cmd::ShowMachine`), when it is shown; it takes over the view.
    pub machine: Option<crate::machine_stage::MachineStage>,
    /// The machines' style, and whether the console's theme is dark (the palette follows both).
    pub style: Style,
    pub dark: bool,
}

impl Default for Stage {
    fn default() -> Stage {
        let mut machines = Floor::machines();
        let lab = lab::build(&mut machines);
        let hardware_lab = hw_lab::build(&mut machines);
        let lab_mode = lab.is_some() || hardware_lab.is_some();
        // The Machines view reads as a plan: an orthographic camera at the isometric angle (the labs keep perspective).
        machines.orbit.ortho = !lab_mode;
        if !lab_mode {
            machines.apply_palette(&Palette::of(Style::default(), true));
        }
        let popup = crate::env_var("APZ_FLOOR_POPUP").filter(|id| !id.is_empty());
        let frame = Arc::new(machines.frame(1.0));
        Stage {
            agents: Vec::new(),
            machines,
            lab,
            hardware_lab,
            fleet: None,
            fleet_data: None,
            fleet_error: None,
            fleet_asking: false,
            fleet_started: false,
            fleet_seen: false,
            popup,
            hovered_machine: None,
            frame,
            size: (0.0, 0.0),
            error: None,
            last: None,
            scale: 2.0,
            // A lab keeps its own framing: the fleet does not refit the camera.
            moved: lab_mode,
            machine: None,
            reduced_motion: false,
            style: Style::default(),
            dark: true,
        }
    }
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("error", &self.error)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Whether `APZ_PERF` is set: the frame and prebuild timing lines are printed.
pub fn perf_log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_var("APZ_PERF").is_some())
}

/// Seconds from the Floor's origin (its first `perf_t` call, made when the Floor state is built) to `now`.
pub fn perf_t(now: Instant) -> f64 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    now.saturating_duration_since(*ORIGIN.get_or_init(|| now))
        .as_secs_f64()
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
    at.saturating_duration_since(*EPOCH.get_or_init(|| at))
        .as_secs_f64()
        * 1000.0
}

impl Stage {
    /// The fleet's detail parts show by the camera's eye; a part that switched re-bakes the statics.
    fn detail_parts(&mut self, eye: glam::Vec3) {
        let top = self.machines.root.clone();
        let mut changed = false;
        if let Some(fleet) = self.fleet.as_mut() {
            changed |= fleet.frame_detail(eye, &top);
        }
        if changed {
            self.machines.statics_dirty = true;
        }
    }

    /// The lab's readout as JSON (the hardware lab's or the robot lab's; `{}` when no lab runs): what the page shows of
    /// the lab's options (the model's validation, the robots and the camera).
    pub fn lab_info(&self) -> String {
        if let Some(lab) = self.hardware_lab.as_ref() {
            return lab.info().to_string();
        }
        if let Some(lab) = self.lab.as_ref() {
            let o = &self.machines.orbit;
            let eye = o.camera(1.0).eye;
            return serde_json::json!({
                "robots": lab.robots.len(),
                "eye": [eye.x, eye.y, eye.z],
                "target": [o.target.x, o.target.y, o.target.z],
            })
            .to_string();
        }
        "{}".to_string()
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
            if self.machines.orbit.ortho {
                // The plan view has no haze: every machine reads at full contrast.
                self.machines.fit_points(&fleet.outline(), w / h);
                self.machines.world.fog_near = 1.0e6;
                self.machines.world.fog_far = 2.0e6;
            } else {
                self.machines.fit_sphere(f.center, f.radius, w / h);
                // The haze keeps its look at the fitted distance (near 0.8, far 3 times it), whatever the fleet's size.
                let dist = crate::world::fit_distance(f.radius, w / h);
                self.machines.world.fog_near = dist * 0.8;
                self.machines.world.fog_far = dist * 3.0;
            }
        }
    }

    /// Opens the popup of machine `id` (a click), or closes it with None.
    pub fn open_popup(&mut self, id: Option<String>) {
        self.popup = id;
    }

    /// `Msg::Resized`: the stage's new size; an unmoved view refits to it.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.size = (w, h);
        if !self.moved {
            self.fit_machines();
        }
    }

    /// Orbits the shown view by a drag.
    pub fn rotate(&mut self, dx: f32, dy: f32, height: f32) {
        if let Some(m) = self.machine.as_mut() {
            m.rotate(dx, dy, height);
            return;
        }
        self.moved = true;
        self.machines.orbit.rotate(dx, dy, height);
    }

    /// Pans the shown view by a drag.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        self.moved = true;
        self.machines.orbit.pan(dx, dy, height, FOV_DEG);
    }

    /// Zooms the shown view by a wheel step count.
    pub fn dolly(&mut self, steps: f32) {
        if let Some(m) = self.machine.as_mut() {
            m.dolly(steps);
            return;
        }
        self.moved = true;
        self.machines.orbit.dolly(steps);
    }

    /// Frames the view afresh (the Reset view control).
    pub fn reset_view(&mut self) {
        self.moved = false;
        self.fit_machines();
    }

    /// The pointer hovers a machine; `None` leaves it.
    pub fn hover(&mut self, at: Option<(f32, f32)>) {
        let hit = at.and_then(|p| machine_at(self, p));
        if let Some(fleet) = self.fleet.as_mut() {
            fleet.hover(hit.as_deref());
        }
        self.hovered_machine = hit;
    }

    /// A click: opens the popup of the machine under the pointer, or closes it.
    pub fn select(&mut self, at: Option<(f32, f32)>) {
        let id = at.and_then(|p| machine_at(self, p));
        self.open_popup(id);
    }

    /// One animation tick: the scene steps by `dt` (the time since the last tick, at most 50 ms) and the frame it draws
    /// is built. Returns how much of the scene still moves after this tick: `Active` while damping or tweens run,
    /// `Ambient` while only the clock-driven decoration turns, `Rest` when nothing animates.
    pub fn tick(&mut self, now: Instant) -> Motion {
        if self.machine.is_some() {
            return self.tick_machine(now);
        }
        let elapsed = self
            .last
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        let dt = elapsed.min(0.05);
        self.last = Some(now);
        // The ambient visuals run on the clock in seconds (`now_ms`), not on the capped step, so a throttled tick draws
        // the pose a full-rate one would. Reduced motion holds them at t = 0.
        let (ambient, ambient_dt) = if self.reduced_motion {
            (0.0, 0.0)
        } else {
            ((now_ms(now) / 1000.0) as f32, elapsed.min(0.5))
        };
        let (w, h) = self.size;
        // The stars, the planet and the dust turn on the clock.
        let mut motion = Motion::Ambient;
        if self.machines.orbit.update() {
            motion = Motion::Active;
        }
        self.machines.step(dt, now_ms(now), ambient);
        if let Some(lab) = self.lab.as_mut() {
            lab.step(dt);
            motion = Motion::Active; // the lab's robots animate their clips
        }
        if let Some(lab) = self.hardware_lab.as_mut() {
            lab.step(ambient_dt, ambient);
        }
        if let (Some(fleet), Some(data)) = (self.fleet.as_mut(), self.fleet_data.as_ref()) {
            fleet.update(dt, ambient, ambient_dt, data);
        }
        if let Some(fleet) = &self.fleet {
            motion = motion.max(fleet.motion());
        }
        if w > 0.0 && h > 0.0 {
            let camera = self.machines.orbit.camera(w / h);
            self.detail_parts(camera.eye);
            let mut frame = self.machines.frame(w / h);
            // The robot lab's `bg` is the Machines view's sky too (the lab's line-up is on the machines' ground).
            if let Some(bg) = self.lab.as_ref().and_then(lab::Lab::background) {
                frame.world.background = bg;
                frame.world.flat_background = true;
            }
            frame.animated = match &self.hardware_lab {
                Some(lab) => Some(Arc::new(lab.instances())),
                None => self
                    .fleet
                    .as_ref()
                    .zip(self.fleet_data.as_ref())
                    .map(|(f, d)| Arc::new(f.instances(d))),
            };
            if let Some(fleet) = &self.fleet {
                frame.overlays = machine_overlays(
                    fleet,
                    &camera,
                    w,
                    h,
                    self.scale,
                    self.hovered_machine.as_deref(),
                );
            }
            self.frame = Arc::new(frame);
        }
        if self.reduced_motion && motion == Motion::Ambient {
            Motion::Rest
        } else {
            motion
        }
    }

    /// The agents from `GET /v1/agents`, parsed by the host: the machine popups name them.
    pub fn apply_agents(&mut self, list: Vec<AgentInfo>) {
        self.error = None;
        self.agents = list;
    }

    /// The fleet snapshot from `GET /v1/fleet`, parsed by the host: the scene is built once, and the view follows it.
    pub fn apply_fleet(&mut self, data: FleetResponse) {
        self.fleet_error = None;
        self.fleet_seen = true;
        // A lab shows its own subject alone: no machines join it.
        if self.fleet.is_none() && self.lab.is_none() && self.hardware_lab.is_none() {
            let fleet = fleet::build_fleet(&data, &self.palette());
            self.machines.root.add(&fleet.group);
            self.machines.statics_dirty = true; // the fleet's static parts are baked
            self.fleet = Some(fleet);
        }
        self.fleet_data = Some(data);
        if !self.moved {
            self.fit_machines();
        }
    }

    /// The palette of the stage's style and theme.
    pub fn palette(&self) -> Palette {
        Palette::of(self.style, self.dark)
    }

    /// Draws the machines in `style`, for a dark or light console theme: the scene is rebuilt in the new palette (the
    /// camera stays where it is). The labs keep their own look.
    pub fn set_style(&mut self, style: Style, dark: bool) {
        if (self.style, self.dark) == (style, dark) {
            return;
        }
        self.style = style;
        self.dark = dark;
        if self.lab.is_some() || self.hardware_lab.is_some() {
            return;
        }
        let palette = self.palette();
        self.machines.apply_palette(&palette);
        if let Some(old) = self.fleet.take() {
            self.machines.root.remove(&old.group);
        }
        if let Some(data) = self.fleet_data.as_ref() {
            let fleet = fleet::build_fleet(data, &palette);
            self.machines.root.add(&fleet.group);
            self.fleet = Some(fleet);
        }
        self.machines.statics_dirty = true;
    }

    /// A fleet request that failed (or could not be read): the error is kept for the host to show.
    pub fn fleet_failed(&mut self, error: String) {
        self.fleet_error = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The default stage in the Machines view, with two online machines and their fleet read.
    fn online_fleet_stage() -> Stage {
        let mut s = Stage {
            size: (800.0, 600.0),
            ..Default::default()
        };
        let data: FleetResponse = serde_json::from_str(
            r#"{"machines":[{"id":"n0","status":"online"},{"id":"n1","status":"online"}]}"#,
        )
        .unwrap();
        s.apply_fleet(data);
        s
    }
    #[test]
    fn an_online_fleet_at_rest_settles_to_ambient_and_reduced_motion_reports_rest() {
        // An online fleet at rest is ambient once 300 ms pass.
        {
            let mut s = online_fleet_stage();
            let t0 = Instant::now();
            s.tick(t0);
            assert_eq!(s.tick(t0 + Duration::from_millis(300)), Motion::Ambient);
        }
        // Reduced motion holds the ambient visuals and reports rest.
        {
            let mut s = online_fleet_stage();
            let t0 = Instant::now();
            s.tick(t0);
            s.reduced_motion = true;
            assert_eq!(s.tick(t0 + Duration::from_millis(600)), Motion::Rest);
        }
    }
    /// The online fleet, framed by a Reset view.
    fn framed_fleet_stage() -> Stage {
        let mut s = online_fleet_stage();
        s.reset_view();
        s
    }

    /// Drags the view, then steps until its damping settles.
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
    fn the_grid_holds_still_in_its_plan_view_and_reset_view_refits_it() {
        let mut s = framed_fleet_stage();
        assert!(
            s.machines.orbit.ortho,
            "the Machines view is drawn as a plan"
        );
        let t0 = Instant::now();
        s.tick(t0);
        let framed = (s.machines.orbit.theta, s.machines.orbit.radius);
        s.tick(t0 + Duration::from_secs(1));
        assert_eq!(s.machines.orbit.theta, framed.0, "the camera holds still");
        let mut now = t0 + Duration::from_secs(1);
        drag_and_settle(&mut s, &mut now);
        assert_ne!(s.machines.orbit.theta, framed.0, "a drag turns the view");
        s.reset_view();
        for _ in 0..400 {
            now += Duration::from_millis(16);
            s.tick(now);
        }
        assert!(
            (s.machines.orbit.radius - framed.1).abs() < 1e-3,
            "reset view refits the fleet"
        );
    }

    #[test]
    fn a_click_on_nothing_closes_the_popup_and_a_failed_fleet_keeps_its_error() {
        let mut s = online_fleet_stage();
        s.open_popup(Some("n1".to_string()));
        assert_eq!(s.popup.as_deref(), Some("n1"));
        s.select(Some((0.0, 0.0)));
        assert_eq!(s.popup, None);
        s.fleet_failed("hub unreachable".to_string());
        assert_eq!(s.fleet_error.as_deref(), Some("hub unreachable"));
        assert!(s.fleet.is_some(), "the scene read before stays");
    }
}
