//! The ENTER sequence on the desktop: the Machines view flies into a machine, its case opens, the camera dives
//! through the opening onto the machine's board, and the rooms build in. Back (or Esc) reverses it from where it
//! stands. The timeline is `entry.rs` (pure keyframes, parity-checked with the web); this module samples it each
//! frame and applies the sample to the scene.
//!
//! Ported from `apps/web/src/views/floor3d/entry.ts` (the `EntrySequence` class) and the ENTER hooks in
//! `apps/web/src/views/floor.ts`. Web step -> desktop:
//! - `beginEntry` with the Machines camera and fog -> `Handoff` (the screen enters with it, `screens/floor.rs`).
//! - the fleet moves into the plaza's scene for the sequence and returns to the Machines' scene after it -> `prepare`,
//!   `teardown`.
//! - the other machines fold (`SINK * dim`, after the fleet's own update) and fade -> `step`, the machine roots.
//! - the cables fade with the machines -> the fleet group's other children.
//! - `anchors.open` -> `FleetScene::set_open`, which the fleet's own update applies to the machine's open part.
//! - the light spill -> a point light at the exterior atom, in the plaza's root.
//! - `fly` (an arc to `anchors.camera`) and `dive` (a quadratic through the opening to the default view) -> the
//!   plaza's `Orbit::look`. The orbit ignores input while the sequence drives it.
//! - the board's match cut (its atom on the exterior's atom, scaled to its width, easing to its own place) -> the
//!   board's group and the rooms' group (they move together).
//! - the board's fade and the atom's glow -> `Fade` (per-mesh opacity over owned copies of the materials), the atom
//!   skipped by the board's fade.
//! - the rooms' rise -> per room: scale.y, position.y and a fade, as the web.
//! - the robots' pop -> `robots_popped` decides which are visible; the scale is `ease_out_back` over the pop.
//! - the name tags -> `Floor::labels` skips a robot that is not visible (it has not popped in).
//! - `look(k)` -> the backdrop shows at night, the ground swaps from the night sheet to the dark board ground at the
//!   dive, and the fog interpolates from the Machines' fog to the plaza's.
//! - `finish` and `onLeft` -> `Phase::Finished` and `Phase::Left`; the screen hands the view back.
//!
//! Renderer note: the scene bakes its static nodes. The sequence marks the nodes it moves dynamic (they are walked
//! each frame and left out of the bake) and re-bakes once when it ends. The fleet's group is moved into the plaza's
//! scene while the sequence runs.
//!
//! Env hook: `APZ_ENTRY_T=<seconds>` pins the timeline at that time: it neither advances nor finishes, and Back does
//! not move it, so the snapshot strip can capture fixed frames (`APZ_SNAPSHOT=<path>`, see `console/mod.rs`). An ENTER
//! (or a direct load) starts pinned at that time; the sequence's camera is held there while the env is set.

use std::f32::consts::PI;

use glam::{Quat, Vec3};

use super::controls::Orbit;
use super::entry::{self, Stage, KEY, ROOM_DROP, SINK, WAVE};
use super::fleet::{FleetResponse, FleetScene};
use super::hardware::{self, Model};
use super::scene::{Camera, Color, Kind, Mat, Obj};
use super::warm::BuildCache;
use super::world::Floor;

/// The built boards ENTER reuses, by `board_key`: a machine entered again takes its board from here, not a rebuild.
pub type BoardCache = BuildCache<hardware::Built>;

/// A machine's board key: the machine, its hardware model and the params its build reads.
pub fn board_key(machine: &str, model: &str, params: &hardware::Params) -> String {
    format!("{machine}|{model}|{:?}", params.values)
}

/// The machine's built board: the cached one when its key is held (taken out of the cache for use), else `build`.
pub fn take_board(cache: &mut BoardCache, key: &str, build: impl FnOnce() -> hardware::Built) -> hardware::Built {
    cache.take(key).unwrap_or_else(build)
}

/// Returns a board once its scene no longer shows it (the caller takes it out of the scene first). The cache keeps the
/// newest boards and drops the ones it pushes out.
pub fn release_board(cache: &mut BoardCache, key: &str, built: hardware::Built) {
    cache.release(key.to_string(), built);
}

#[cfg(test)]
mod board_cache_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn entering_the_same_machine_again_reuses_its_board() {
        let model = hardware::model("exterior/workstation").expect("the fallback exterior is embedded");
        let params = hardware::Params::default().with("gpus", 1.0);
        let key = board_key("m-1", "exterior/workstation", &params);
        let builds = Cell::new(0);
        let build = || {
            builds.set(builds.get() + 1);
            hardware::build(model, &params)
        };
        let mut cache = BoardCache::new(3);
        let first = take_board(&mut cache, &key, &build);
        release_board(&mut cache, &key, first);
        let again = take_board(&mut cache, &key, &build);
        assert_eq!(builds.get(), 1, "the second entry takes the board from the cache");
        release_board(&mut cache, &key, again);
    }
}

/// A camera pose: where the eye sits and what it looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub eye: Vec3,
    pub target: Vec3,
}

/// The Machines view at an ENTER: the camera and fog to fly from, the machine that opens, and the Machines' orbit to
/// restore when a Back has played out.
pub struct Handoff {
    pub machine: String,
    pub from: Pose,
    pub fog: (f32, f32),
    pub back: Orbit,
}

/// What a step reports to the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Still playing, or reversing.
    Playing,
    /// The forward sequence has just completed: the plaza shows the board.
    Finished,
    /// A reverse has just reached its start: the screen hands the view back to the Machines.
    Left,
    /// Nothing to do: the sequence has finished and is not reversing.
    Done,
}

/// The scene parts a step drives.
pub struct Parts<'a> {
    /// The plaza: the board, its rooms, the robots, the camera and the look.
    pub floor: &'a mut Floor,
    /// The Machines view: the fleet's home until the handoff takes it.
    pub machines: &'a mut Floor,
    /// The fleet (the exterior of the entered machine and the other machines), when there is one.
    pub fleet: Option<&'a mut FleetScene>,
    /// The fleet's latest snapshot, for the fleet's own update.
    pub data: Option<&'a FleetResponse>,
    /// The stage's width over height.
    pub aspect: f32,
    /// The ambient clock in seconds, and its real step (0 with reduced motion): the fleet's ambient visuals read them.
    pub ambient: f32,
    pub ambient_dt: f32,
}

/// One subtree's materials, each mesh given its own copy so that its fade does not touch the meshes that share its
/// original (a room's, a machine's). The copies follow the original each frame, so a material's own animation still
/// shows.
struct Fade {
    root: Obj,
    /// (the node, its original material, the copy it now carries).
    meshes: Vec<(Obj, Mat, Mat)>,
}

impl Fade {
    /// Owns the meshes under `root`, leaving out the subtree of `skip` (the atom, which fades on its own).
    fn own(root: &Obj, skip: Option<&Obj>) -> Fade {
        let mut skipped = Vec::new();
        if let Some(node) = skip {
            node.traverse(&mut |n: &Obj| skipped.push(n.clone()));
        }
        let mut meshes = Vec::new();
        root.traverse(&mut |node: &Obj| {
            if skipped.contains(node) {
                return;
            }
            if let Some(orig) = node.material() {
                let copy = Mat::new(orig.snapshot());
                node.set_material(&copy);
                meshes.push((node.clone(), orig, copy));
            }
        });
        Fade { root: root.clone(), meshes }
    }

    /// Sets every owned mesh to its original at opacity `k` (0..1). Below 1 the copies draw transparent, without depth.
    fn apply(&self, k: f32) {
        for (_, orig, copy) in &self.meshes {
            let mut m = orig.snapshot();
            if k < 1.0 {
                m.opacity *= k;
                m.transparent = true;
                m.depth_write = false;
            }
            *copy.get() = m;
        }
    }

    /// Puts the original materials back.
    fn restore(&self) {
        for (node, orig, _) in &self.meshes {
            node.set_material(orig);
        }
    }
}

/// The exterior's view, in world space: its camera anchor, its atom, and its turn.
struct Exterior {
    eye: Vec3,
    target: Vec3,
    atom: Vec3,
    rot_y: f32,
    size: [f32; 3],
}

/// Where the sequence stands. A forward run starts at the Machines' pose (a handoff) or at the build wave (a direct
/// load); a reverse runs back to the same start.
pub struct Entry {
    /// Seconds on the timeline (`entry.rs`).
    t: f64,
    /// +1 forward, -1 reverse.
    dir: f64,
    /// A direct load: no exterior, the board's build wave only.
    direct: bool,
    handoff: Option<Handoff>,
    /// The plaza's own fog: the end of the fog's interpolation.
    board_fog: (f32, f32),
    /// `APZ_ENTRY_T`: the timeline stays at this time.
    pinned: Option<f64>,
    finished: bool,
    /// The scene's parts are set for the sequence (`prepare` to `unprepare`).
    prepared: bool,
    /// The plaza's default view, once the board exists and the stage has a size.
    default: Option<Pose>,
    /// The spill light: a point light at the exterior's atom.
    light: Obj,
    fades: Vec<Fade>,
    /// Each room's rest position and scale, captured the first time the sequence touches it.
    rest: Vec<(Obj, Vec3, Vec3)>,
    /// The board atom's rest scale.
    atom_rest: Option<Vec3>,
}

impl Entry {
    fn new(handoff: Option<Handoff>, t: f64, direct: bool, board_fog: (f32, f32)) -> Entry {
        let light = Obj::point_light(Color::hex(0xffb070), 0.0, 40.0, 1.6);
        light.set_dynamic(true);
        Entry {
            t,
            dir: 1.0,
            direct,
            handoff,
            board_fog,
            pinned: pinned_from_env(),
            finished: false,
            prepared: false,
            default: None,
            light,
            fades: Vec::new(),
            rest: Vec::new(),
            atom_rest: None,
        }
    }

    /// An ENTER from the Machines view: the sequence flies from `h.from` to the machine.
    pub fn handoff(h: Handoff, board_fog: (f32, f32)) -> Entry {
        Entry::new(Some(h), 0.0, false, board_fog)
    }

    /// A direct load: the board's build wave, from its start.
    pub fn direct(board_fog: (f32, f32)) -> Entry {
        Entry::new(None, entry::DIRECT_START, true, board_fog)
    }

    /// Seconds since the sequence began on its timeline: a handoff starts at 0, a direct load at the wave's start.
    pub fn elapsed(&self) -> f64 {
        self.t - if self.direct { entry::DIRECT_START } else { 0.0 }
    }

    /// Whether the sequence drives the camera (the orbit ignores input meanwhile).
    pub fn driving(&self) -> bool {
        !self.finished
    }

    /// Back or Esc: reverses from where the sequence stands, also after it has finished. A reverse already under way
    /// does nothing more.
    pub fn back(&mut self) {
        if self.dir < 0.0 {
            return;
        }
        self.dir = -1.0;
        self.finished = false;
    }

    /// The Machines' orbit to return to, once the sequence has left (a handoff only).
    pub fn into_return(self) -> Option<Orbit> {
        self.handoff.map(|h| h.back)
    }

    /// Advances the sequence by `dt` seconds and applies its state to the scene.
    pub fn step(&mut self, dt: f64, p: &mut Parts) -> Phase {
        if self.finished && self.dir > 0.0 {
            return Phase::Done;
        }
        self.prepare(p);
        let rooms = p.floor.district_group.get().children.len();
        let robots = p.floor.bots.len();
        let end = entry::end_at(rooms, robots);
        let min = if self.direct { KEY.direct[0] } else { 0.0 };
        if self.default.is_none() && p.floor.board.is_some() && p.aspect > 0.0 {
            self.default = Some(default_pose(p.floor, p.aspect));
        }
        let before = self.t;
        let mut t = match self.pinned {
            Some(pinned) => pinned,
            None => self.t + self.dir * dt.min(0.05),
        };
        // The dive needs the board and its default view: the sequence holds at the dive's start until both exist.
        if self.dir > 0.0 && self.default.is_none() && t >= KEY.dive[0] {
            t = before.max(KEY.dive[0]);
        }
        let t = t.clamp(min, end);
        self.t = t;

        let st = entry::stage_at(t, self.direct);
        self.apply(&st, t, dt, rooms, robots, p);

        if self.pinned.is_none() {
            if self.dir > 0.0 && t >= end {
                return self.finish(p);
            }
            if self.dir < 0.0 && t <= min {
                return self.leave(p);
            }
        }
        Phase::Playing
    }

    fn finish(&mut self, p: &mut Parts) -> Phase {
        self.finished = true;
        // Inside the machine only its board shows: the other machines and the cables leave the view (a reverse sets
        // their visibility again frame by frame as they come back).
        if let (Some(_), Some(fleet)) = (self.handoff.as_ref(), p.fleet.as_deref_mut()) {
            for m in fleet.machines.values() {
                m.root.set_visible(false);
            }
            let kids = fleet.group.get().children.clone();
            for child in kids {
                child.set_visible(false);
            }
            fleet.group.set_visible(false);
        }
        self.unprepare(p);
        // The robots are whole from here on: one that has not yet popped in gets no pop left to run.
        for bot in &p.floor.bots {
            bot.robot.group.set_visible(true).set_scalar(1.0);
            bot.pad.set_visible(true);
        }
        self.look(1.0, p.floor);
        Phase::Finished
    }

    fn leave(&mut self, p: &mut Parts) -> Phase {
        self.finished = true;
        self.teardown(p);
        Phase::Left
    }

    /// Sets the scene for the sequence: the nodes it moves become dynamic, the fleet joins the plaza, the light joins
    /// the plaza. Once per run.
    fn prepare(&mut self, p: &mut Parts) {
        if self.prepared {
            return;
        }
        self.prepared = true;
        p.floor.statics_dirty = true;
        p.machines.statics_dirty = true;
        set_dynamic_all(p.floor, true);
        if let Some(fleet) = p.fleet.as_deref_mut() {
            if self.handoff.is_some() {
                // The fleet joins the plaza for the sequence (a reverse after a finish joins it again: `attach` keeps one).
                attach(&p.machines.root, &fleet.group, false);
                attach(&p.floor.root, &fleet.group, true);
                fleet.group.set_dynamic(true);
            }
        }
        attach(&p.floor.root, &self.light, self.handoff.is_some());
    }

    /// Takes the sequence's parts back out of the plaza's bake: the fades end, the nodes are static again.
    fn unprepare(&mut self, p: &mut Parts) {
        if !self.prepared {
            return;
        }
        self.prepared = false;
        for f in self.fades.drain(..) {
            f.restore();
        }
        p.floor.statics_dirty = true;
        set_dynamic_all(p.floor, false);
        attach(&p.floor.root, &self.light, false);
        if let Some(fleet) = p.fleet.as_deref_mut() {
            fleet.group.set_dynamic(false);
        }
    }

    /// The end of a reverse: the Machines' scene gets the fleet back, and the plaza rests as it was before the ENTER.
    fn teardown(&mut self, p: &mut Parts) {
        self.unprepare(p);
        if let (Some(h), Some(fleet)) = (self.handoff.as_ref(), p.fleet.as_deref_mut()) {
            attach(&p.floor.root, &fleet.group, false);
            attach(&p.machines.root, &fleet.group, true);
            p.machines.statics_dirty = true;
            fleet.set_open(&h.machine, 0.0);
            for m in fleet.machines.values() {
                m.root.set_visible(true);
            }
            let kids = fleet.group.get().children.clone();
            for child in kids {
                child.set_visible(true);
            }
        }
        if let Some(b) = p.floor.board.as_ref() {
            b.obj.set_position(0.0, 0.0, 0.0).set_rotation(0.0, 0.0, 0.0).set_scalar(1.0).set_visible(true);
            if let (Some(atom), Some(s)) = (b.atom_node.as_ref(), self.atom_rest) {
                atom.set_scale(s.x, s.y, s.z).set_visible(true);
            }
        }
        p.floor.district_group.set_position(0.0, 0.0, 0.0).set_rotation(0.0, 0.0, 0.0).set_scalar(1.0);
        for (room, pos, scale) in &self.rest {
            room.set_visible(true).set_position(pos.x, pos.y, pos.z).set_scale(scale.x, scale.y, scale.z);
        }
        for bot in &p.floor.bots {
            bot.robot.group.set_visible(true).set_scalar(1.0);
            bot.pad.set_visible(true);
        }
        p.floor.backdrop.set_visible(false);
        attach(&p.floor.root, &p.floor.ground, false);
        attach(&p.floor.root, &p.floor.board_ground, true);
        p.floor.world.fog_near = self.board_fog.0;
        p.floor.world.fog_far = self.board_fog.1;
        self.rest.clear();
    }

    /// Applies the sample at `t` to the scene: the fleet, the camera, the board, the rooms, the robots and the look.
    fn apply(&mut self, st: &Stage, t: f64, dt: f64, rooms: usize, robots: usize, p: &mut Parts) {
        let dive = st.dive as f32;
        let from = self.handoff.as_ref().map(|h| (h.machine.clone(), h.from));
        let board = p.floor.board.as_ref().map(|b| (b.obj.clone(), b.model, b.atom_node.clone()));

        // The exterior: the fleet's update, the open part, the other machines folding and fading, the cables.
        let mut ext: Option<Exterior> = None;
        if let (Some((machine, _)), Some(fleet)) = (from.as_ref(), p.fleet.as_deref_mut()) {
            if let Some(data) = p.data {
                fleet.update(dt as f32, p.ambient, p.ambient_dt, data);
            }
            fleet.set_open(machine, st.open as f32);
            fleet.group.set_visible(true);
            let roots: Vec<Obj> = fleet.machines.values().map(|m| m.root.clone()).collect();
            for (id, m) in fleet.machines.iter() {
                let mine = id == machine;
                let k = (if mine { st.ext } else { st.others }) as f32;
                if !mine {
                    // After the fleet's update, which sets the lift each frame: the fold sinks the root.
                    let mut pos = m.root.position();
                    pos.y -= SINK as f32 * st.dim as f32;
                    m.root.set_position(pos.x, pos.y, pos.z);
                }
                m.root.set_visible(k > 0.002);
                self.fade(&m.root, k, None);
            }
            let kids = fleet.group.get().children.clone();
            for child in kids {
                if roots.contains(&child) {
                    continue;
                }
                child.set_visible(st.cables > 0.002);
                self.fade(&child, st.cables as f32, None);
            }
            if let Some(model) = exterior_model(p.data, machine) {
                ext = exterior_of(fleet, machine, model);
            }
        }

        // The light spills out of the opening.
        if let Some(e) = ext.as_ref() {
            self.light.set_position(e.atom.x, e.atom.y, e.atom.z);
            let mut node = self.light.get();
            if let Kind::PointLight { intensity, .. } = &mut node.kind {
                *intensity = 10.0 * st.light as f32;
            }
        }

        // The camera: a fly-in on an arc, then the dive through the opening to the board's default view.
        let cam: Option<(Vec3, Vec3)> = match (ext.as_ref(), from.as_ref(), self.default) {
            (Some(e), Some(_), Some(pose)) if dive > 0.0 => {
                let control = e.eye.lerp(e.atom, 0.5);
                let d = dive;
                let a = (1.0 - d) * (1.0 - d);
                let b = 2.0 * (1.0 - d) * d;
                let c = d * d;
                let eye = e.eye * a + control * b + pose.eye * c;
                Some((eye, e.target.lerp(pose.target, d)))
            }
            (Some(e), Some((_, from_pose)), _) => {
                let f = st.fly as f32;
                let mut eye = from_pose.eye.lerp(e.eye, f);
                eye.y += (PI * f).sin() * 0.12 * from_pose.eye.distance(e.eye);
                Some((eye, from_pose.target.lerp(e.target, f)))
            }
            _ => self.default.map(|pose| (pose.eye, pose.target)),
        };
        if let Some((eye, target)) = cam {
            p.floor.orbit.look(eye, target);
        }

        // The board: a match cut at the exterior's place, easing to its own place; the rooms move with it.
        if let Some((obj, model, atom)) = board.as_ref() {
            let (pos, rot_y, scale) = match (ext.as_ref(), *model) {
                (Some(e), bm) => {
                    let (pos, scale) = match_of(e, bm);
                    (pos.lerp(Vec3::ZERO, dive), e.rot_y * (1.0 - dive), scale + (1.0 - scale) * dive)
                }
                (None, _) => (Vec3::ZERO, 0.0, 1.0),
            };
            obj.set_position(pos.x, pos.y, pos.z).set_rotation(0.0, rot_y, 0.0).set_scalar(scale);
            p.floor.district_group.set_position(pos.x, pos.y, pos.z).set_rotation(0.0, rot_y, 0.0).set_scalar(scale);
            obj.set_visible(st.board > 0.002);
            let skip = atom.as_ref();
            self.fade(obj, st.board as f32, skip);
            if let Some(atom) = atom.as_ref() {
                let base = match self.atom_rest {
                    Some(base) => base,
                    None => {
                        let s = atom.get().scale;
                        self.atom_rest = Some(s);
                        s
                    }
                };
                let g = (0.2 + 0.8 * st.atom) as f32;
                atom.set_scale(base.x * g, base.y * g, base.z * g).set_visible(st.atom > 0.002);
                self.fade(atom, st.atom as f32, None);
            }
        }

        // The rooms rise from their slots in a staggered wave; the robots pop in after the last one.
        let room_nodes: Vec<Obj> = p.floor.district_group.get().children.clone();
        for (i, room) in room_nodes.iter().enumerate() {
            let (a, b) = entry::rise_span(i, rooms);
            let e = entry::span(t, a, b, entry::ease_out_cubic) as f32;
            let (pos, scale) = self.rest_of(room);
            room.set_visible(e > 0.002);
            if e <= 0.002 {
                continue;
            }
            room.set_position(pos.x, pos.y - (1.0 - e) * ROOM_DROP as f32, pos.z);
            room.set_scale(scale.x, scale.y * e.max(0.02), scale.z);
            self.fade(room, e, None);
        }
        let popped = entry::robots_popped(t, rooms, robots);
        let r0 = entry::robots_at(rooms);
        let rg = entry::robot_gap(robots);
        for (j, bot) in p.floor.bots.iter().enumerate() {
            let a = r0 + j as f64 * rg;
            let s = entry::span(t, a, a + WAVE.pop, entry::ease_out_back) as f32;
            let on = j < popped;
            bot.robot.group.set_visible(on).set_scalar(s.max(0.001));
            bot.pad.set_visible(on);
        }

        self.look(st.dive, p.floor);
    }

    /// `look(k)`: the backdrop at night, the night sheet or the dark board ground, and the fog from the Machines'
    /// fog to the plaza's.
    fn look(&self, k: f64, floor: &mut Floor) {
        let night = k < 0.5;
        floor.backdrop.set_visible(night);
        attach(&floor.root, &floor.ground, night);
        attach(&floor.root, &floor.board_ground, !night);
        if let Some(h) = self.handoff.as_ref() {
            let k = k as f32;
            floor.world.fog_near = h.fog.0 + (self.board_fog.0 - h.fog.0) * k;
            floor.world.fog_far = h.fog.1 + (self.board_fog.1 - h.fog.1) * k;
        }
    }

    /// Multiplies the opacity under `root` by `k`, over owned copies of its materials (made on first use).
    fn fade(&mut self, root: &Obj, k: f32, skip: Option<&Obj>) {
        let i = match self.fades.iter().position(|f| f.root == *root) {
            Some(i) => i,
            None => {
                self.fades.push(Fade::own(root, skip));
                self.fades.len() - 1
            }
        };
        self.fades[i].apply(k);
    }

    /// A room's rest position and scale, captured the first time.
    fn rest_of(&mut self, room: &Obj) -> (Vec3, Vec3) {
        if let Some((_, pos, scale)) = self.rest.iter().find(|(o, _, _)| o == room) {
            return (*pos, *scale);
        }
        let (pos, scale) = {
            let node = room.get();
            (node.position, node.scale)
        };
        self.rest.push((room.clone(), pos, scale));
        (pos, scale)
    }
}

/// The nodes the sequence moves: the plaza's sheets, the rooms and the board are dynamic while it runs.
fn set_dynamic_all(floor: &Floor, on: bool) {
    floor.ground.set_dynamic(on);
    floor.board_ground.set_dynamic(on);
    floor.backdrop.set_dynamic(on);
    floor.district_group.set_dynamic(on);
    if let Some(b) = floor.board.as_ref() {
        b.obj.set_dynamic(on);
    }
}

/// Adds `node` to `root` when `on`, takes it out when not (no duplicates).
fn attach(root: &Obj, node: &Obj, on: bool) {
    let has = root.get().children.iter().any(|c| c == node);
    if on && !has {
        root.add(node);
    } else if !on && has {
        root.remove(node);
    }
}

/// The plaza's default view: the view the Floor's reset frames, read without moving the orbit.
fn default_pose(floor: &mut Floor, aspect: f32) -> Pose {
    let saved = floor.orbit.clone();
    floor.reset_view(aspect);
    let cam: Camera = floor.orbit.camera(aspect);
    floor.orbit = saved;
    Pose { eye: cam.eye, target: cam.target }
}

/// The exterior model of a machine: its kind's exterior, else the workstation's (as the web falls back).
fn exterior_model(data: Option<&FleetResponse>, machine: &str) -> Option<&'static Model> {
    let m = data?.machines.iter().find(|x| x.id == machine)?;
    let name = hardware::models_for(&m.kind, m.vendor.as_deref()).0;
    hardware::model(name).or_else(|| hardware::model("exterior/workstation"))
}

/// The exterior's camera anchor and atom, in world space, from the machine's current transform.
fn exterior_of(fleet: &FleetScene, machine: &str, model: &Model) -> Option<Exterior> {
    let m = fleet.machines.get(machine)?;
    let world = fleet.group.local_matrix() * m.root.local_matrix();
    let cam = model.anchors.camera.unwrap_or(hardware::Camera { at: [0.0; 3], look: [0.0; 3] });
    let atom = model.anchors.atom.unwrap_or([0.0; 3]);
    let rot_y = m.root.get().rotation.y;
    Some(Exterior {
        eye: world.transform_point3(Vec3::from(cam.at)),
        target: world.transform_point3(Vec3::from(cam.look)),
        atom: world.transform_point3(Vec3::from(atom)),
        rot_y,
        size: model.size,
    })
}

/// The board's position and scale at the match: its atom on the exterior's atom, scaled to the exterior's width.
fn match_of(e: &Exterior, board: &Model) -> (Vec3, f32) {
    let scale = e.size[0] / board.size[0].max(1e-6);
    let b_atom = Vec3::from(board.anchors.atom.unwrap_or([0.0; 3])) * scale;
    let off = Quat::from_rotation_y(e.rot_y) * b_atom;
    (e.atom - off, scale)
}

/// `APZ_ENTRY_T`: the timeline's pinned time, seconds.
fn pinned_from_env() -> Option<f64> {
    crate::env_var("APZ_ENTRY_T").and_then(|s| s.trim().parse::<f64>().ok()).filter(|t| *t >= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::build_fleet;
    use crate::hardware::{self, Params, Status};

    /// Steps the sequence at 60 frames a second until it reports `until` (or `Done`), within a bound.
    fn play(e: &mut Entry, floor: &mut Floor, machines: &mut Floor, until: Phase) -> Phase {
        let mut phase = Phase::Playing;
        for _ in 0..5000 {
            phase = e.step(1.0 / 60.0, &mut Parts { floor: &mut *floor, machines: &mut *machines, fleet: None, data: None, aspect: 1.0, ambient: 0.0, ambient_dt: 0.0 });
            if phase == until || phase == Phase::Done {
                break;
            }
        }
        phase
    }

    /// The plaza rests on its dark board ground, without the night sheet.
    fn on_board(floor: &Floor) -> bool {
        let kids = floor.root.get().children.clone();
        kids.iter().any(|c| c == &floor.board_ground) && !kids.iter().any(|c| c == &floor.ground)
    }

    fn workstation() -> Floor {
        let model = hardware::model("board/workstation").expect("the default workstation board");
        let mut floor = Floor::new();
        floor.show_board("test".to_string(), model, Params::default(), Status::Online);
        floor.sync(&[]);
        floor
    }

    #[test]
    fn a_direct_load_builds_up_and_back_plays_it_out_and_rests_the_board() {
        let mut floor = workstation();
        let mut machines = Floor::machines();
        let mut e = Entry::direct((700.0, 3000.0));

        assert_eq!(play(&mut e, &mut floor, &mut machines, Phase::Finished), Phase::Finished);
        assert!(floor.board.as_ref().is_some_and(|b| b.obj.get().visible));
        assert!(floor.district_group.get().children.iter().all(|r| r.get().visible));
        assert!(on_board(&floor) && !floor.backdrop.get().visible);
        assert!(!floor.district_group.get().dynamic, "the finished scene is baked again");

        e.back();
        assert_eq!(play(&mut e, &mut floor, &mut machines, Phase::Left), Phase::Left);
        let obj = floor.board.as_ref().expect("the board stays prebuilt").obj.clone();
        assert_eq!(obj.get().position, Vec3::ZERO);
        assert_eq!(obj.get().scale, Vec3::ONE);
        assert!(on_board(&floor));
        assert!(e.into_return().is_none(), "a direct load returns no camera: the Machines view refits");
    }

    #[test]
    fn back_mid_run_reverses_from_where_the_sequence_stands() {
        let mut floor = workstation();
        let mut machines = Floor::machines();
        let mut e = Entry::direct((700.0, 3000.0));
        for _ in 0..20 {
            e.step(1.0 / 60.0, &mut Parts { floor: &mut floor, machines: &mut machines, fleet: None, data: None, aspect: 1.0, ambient: 0.0, ambient_dt: 0.0 });
        }
        let at = e.t;
        e.back();
        assert_eq!(play(&mut e, &mut floor, &mut machines, Phase::Left), Phase::Left);
        assert!(at > entry::DIRECT_START, "the back came after the wave had started");
        assert!(on_board(&floor));
    }

    /// Steps a handoff with its fleet until `until`.
    fn play_fleet(e: &mut Entry, floor: &mut Floor, machines: &mut Floor, fleet: &mut Option<FleetScene>, data: &FleetResponse, until: Phase) -> Phase {
        let mut phase = Phase::Playing;
        for _ in 0..5000 {
            phase = e.step(1.0 / 60.0, &mut Parts { floor: &mut *floor, machines: &mut *machines, fleet: fleet.as_mut(), data: Some(data), aspect: 1.0, ambient: 0.0, ambient_dt: 0.0 });
            if phase == until || phase == Phase::Done {
                break;
            }
        }
        phase
    }

    #[test]
    fn a_handoff_moves_the_fleet_into_the_plaza_and_back_and_returns_the_orbit() {
        let data: FleetResponse = serde_json::from_str(
            r#"{"machines":[
                {"id":"n0","name":"hub","role":"hub","kind":"workstation","status":"online","agents":[],"working":0},
                {"id":"n1","name":"pi","role":"node","kind":"edge","status":"offline"}],
              "links":[{"a":"n0","b":"n1","online":false}]}"#,
        )
        .expect("a fleet");
        let mut floor = workstation();
        let mut machines = Floor::machines();
        let mut fleet = Some(build_fleet(&data, &machines.fleet_palette()));
        let group = fleet.as_ref().expect("the fleet").group.clone();
        machines.root.add(&group);

        let handoff = Handoff {
            machine: "n0".to_string(),
            from: Pose { eye: Vec3::new(0.0, 40.0, 60.0), target: Vec3::ZERO },
            fog: (100.0, 300.0),
            back: machines.orbit.clone(),
        };
        let mut e = Entry::handoff(handoff, (700.0, 3000.0));
        assert_eq!(play_fleet(&mut e, &mut floor, &mut machines, &mut fleet, &data, Phase::Finished), Phase::Finished);
        // The fleet is the plaza's now; the entered machine is gone from view (its board is there instead).
        assert!(floor.root.get().children.iter().any(|c| c == &group));
        assert!(!machines.root.get().children.iter().any(|c| c == &group));
        let hub = &fleet.as_ref().expect("the fleet").machines["n0"];
        assert!(!hub.root.get().visible);
        // Inside the machine the other machines are out of view too (the owner's report, 2026-10-14).
        assert!(fleet.as_ref().expect("the fleet").machines.values().all(|m| !m.root.get().visible), "another machine shows inside");
        assert!(fleet.as_ref().expect("the fleet").group.get().children.iter().all(|c| !c.get().visible), "a cable shows inside");

        e.back();
        assert_eq!(play_fleet(&mut e, &mut floor, &mut machines, &mut fleet, &data, Phase::Left), Phase::Left);
        assert!(machines.root.get().children.iter().any(|c| c == &group));
        assert!(!floor.root.get().children.iter().any(|c| c == &group));
        assert!(fleet.as_ref().expect("the fleet").machines.values().all(|m| m.root.get().visible), "the machines are back");
        assert!(e.into_return().is_some(), "a handoff returns the Machines' orbit");
    }
}
