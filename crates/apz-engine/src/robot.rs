//! `floor3d/robot.ts`: a plaza robot, built from its species' JSON (`assets/robots/species/<id>.json`)
//! and animated by the shared and species clips (`assets/robots/clips.json`). The species decides the
//! parts, bones, eyes and motion; the clips decide what it does. The scene places it; the robot owns its
//! own animation. The data, the rig and the effects are in `robots/`.

use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use glam::{Mat4, Quat, Vec3};

use super::robots::clips::{eval_clip, rest_of};
use super::robots::files::{registry, warn_once};
use super::robots::fx::{seed, Fx};
use super::robots::rig::{add_bone, bake_species, bake_tool, instantiate, robot_mats, BoneGeo, Rig, RobotMats, SpeciesGeo};
use super::robots::types::{Clip, Species, Tool};
use super::scene::{Color, Obj};

/// What a robot is doing: the state picks its clip and its eyes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotState {
    Idle,
    Working,
    Walking,
    Offline,
}

/// A still pose for a portrait: the shared clips of the same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotPose {
    Wave,
    Proud,
    Think,
    Cheer,
}

/// The body layer's mode (gravity and heights; `bodyClip` in floor.ts): its clip overrides the hold, the one-shot and the
/// state's clip while it is set. `Ground` is none. `Fall` is the web's `fall`, `glide` and `out` (all `fall.air`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyMode {
    Ground,
    Crouch,
    Air,
    Fall,
    Land,
}

impl BodyMode {
    /// The clip the mode plays (`bodyClip` in floor.ts), or `None` on the ground.
    pub fn clip_name(self) -> Option<&'static str> {
        match self {
            BodyMode::Ground => None,
            BodyMode::Crouch => Some("jump.crouch"),
            BodyMode::Air => Some("jump.air"),
            BodyMode::Fall => Some("fall.air"),
            BodyMode::Land => Some("land"),
        }
    }

    /// The feet's height offset while the mode's clip plays (`AIR_FEET` in robot.ts): tucked in the air, hanging in the fall.
    fn feet_tuck(self) -> Option<f32> {
        match self {
            BodyMode::Air => Some(0.06),
            BodyMode::Fall => Some(-0.03),
            _ => None,
        }
    }
}

/// A cid's species when the hub's record names none: the id from `index.json`'s order without the oracle (reserved for
/// model agents, so the hub never deals it to a robot; the web's `robotVariant` skips it too), `cid % len` (neighbouring
/// cids get different ones).
pub fn robot_variant(cid: u64) -> &'static str {
    let list = &registry().index.species;
    let dealt = |id: &&String| id.as_str() != "oracle";
    let n = list.iter().filter(dealt).count();
    if n == 0 {
        return "rover";
    }
    list.iter().filter(dealt).nth((cid % n as u64) as usize).map_or("rover", String::as_str)
}

/// A robot's species: the hub's `species` on the agent record (an unknown id is the rover, as `Robot::with_variant` draws
/// it), else `robot_variant(cid)` when the record has none.
pub fn species_of(hub: Option<&str>, cid: u64) -> &'static str {
    let Some(id) = hub.map(str::trim).filter(|s| !s.is_empty()) else { return robot_variant(cid) };
    registry().index.species.iter().map(String::as_str).find(|s| *s == id).unwrap_or("rover")
}

/// The console palette the robot is painted in (`Palette` in robot.ts): read from `palette.json`.
#[derive(Debug, Clone, Copy)]
pub struct RobotPalette {
    pub ink: Color,
    pub sheet: Color,
    pub paper: Color,
    pub rule: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
    pub ink_muted: Color,
}

/// The robots' own paint, the same in the light and the dark theme (`ROBOT_PALETTE` in robot.ts). A
/// colour that does not parse is magenta, so a bad palette file shows.
pub fn robot_palette() -> RobotPalette {
    let p = &registry().palette;
    let c = |hex: &str| Color::css(hex).unwrap_or(Color::hex(0xff00ff));
    RobotPalette {
        ink: c(&p.ink),
        sheet: c(&p.sheet),
        paper: c(&p.paper),
        rule: c(&p.rule),
        signal: c(&p.signal),
        ok: c(&p.ok),
        info: c(&p.info),
        ink_muted: c(&p.ink_muted),
    }
}

const SCALE: f32 = 1.3; // a touch bigger than the furniture, so the robots read from afar
const ORCH_SCALE: f32 = 1.25 * SCALE;
const BLEND: f32 = 8.0; // the rate each joint moves toward its target (k = min(1, dt·8))
const WALK_FEET_RATE: f32 = 9.0; // rad/s of the feet's step while walking
const FLOAT_OFFLINE: f32 = -0.07; // a hover robot sets down when offline
/// A bone's joints: rotation x, y, z, offset x, y, z, and scale. The eye lids follow them all.
const PROPS: usize = 7;
const PROP_NAMES: [&str; PROPS] = ["rot.x", "rot.y", "rot.z", "pos.x", "pos.y", "pos.z", "scale"];

static SPECIES_GEO: Mutex<BTreeMap<(String, bool), Arc<SpeciesGeo>>> = Mutex::new(BTreeMap::new());
static TOOL_GEO: Mutex<BTreeMap<String, Arc<BoneGeo>>> = Mutex::new(BTreeMap::new());
static ROBOTS: AtomicU64 = AtomicU64::new(1);

fn species_geo(sp: &Species, palette: &RobotPalette, orchestrator: bool) -> Arc<SpeciesGeo> {
    let mut cache = SPECIES_GEO.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(cache.entry((sp.id.clone(), orchestrator)).or_insert_with(|| Arc::new(bake_species(sp, palette, orchestrator))))
}

fn tool_geo(id: &str, tool: &Tool, palette: &RobotPalette) -> Arc<BoneGeo> {
    let mut cache = TOOL_GEO.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(cache.entry(id.to_string()).or_insert_with(|| Arc::new(bake_tool(tool, palette))))
}

/// A species' clip by name: its own (an override first), else the shared one.
fn clip_named(sp: &'static Species, name: &str) -> Option<&'static Clip> {
    if let Some(own) = sp.overrides.get(name).and_then(|o| sp.clips.get(o)) {
        return Some(own);
    }
    sp.clips.get(name).or_else(|| registry().clips.get(name))
}

/// The walk clip of the species' locomotion; the treads walk when it has none (with one warning).
fn walk_clip(sp: &'static Species) -> Option<&'static Clip> {
    let walk = format!("walk.{}", sp.locomotion);
    clip_named(sp, &walk).or_else(|| {
        warn_once(&format!("no clip \"{walk}\" for locomotion \"{}\" (the treads walk is used)", sp.locomotion));
        clip_named(sp, "walk.treads")
    })
}

/// A bone's rest position in the robot's root: the sum of its `at`s up its parents.
fn rest_position(sp: &Species, name: &str) -> Vec3 {
    let mut p = Vec3::ZERO;
    let mut cur = name;
    for _ in 0..=sp.bones.len() {
        let Some(def) = sp.bones.get(cur) else { break };
        p += Vec3::from(def.at);
        cur = def.parent.as_str();
    }
    p
}

fn axis(a: &str) -> Option<usize> {
    match a {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    }
}

/// A held tool or carried item: its meshes under a new holder at `bone`.
fn attach(bone: &Obj, geo: &BoneGeo, mats: &RobotMats) -> Obj {
    let holder = Obj::group();
    bone.add(&holder);
    add_bone(&holder, geo, mats, &mut Vec::new());
    holder
}

/// A clip that started at `start` and plays until it is replaced (a pose, a held clip).
#[derive(Clone, Copy)]
struct Timed {
    clip: &'static Clip,
    start: f32,
}

/// A one-shot: plays for `dur` seconds, then the state's clip returns.
#[derive(Clone, Copy)]
struct Once {
    clip: &'static Clip,
    start: f32,
    dur: f32,
}

/// The body layer: its mode, the clip it plays, and the time it was entered (the clip plays from there).
#[derive(Clone, Copy)]
struct Body {
    mode: BodyMode,
    clip: &'static Clip,
    start: f32,
}

/// The state's clips (`None` when the species and the shared file have none).
#[derive(Clone, Copy)]
struct StateClips {
    idle: Option<&'static Clip>,
    working: Option<&'static Clip>,
    offline: Option<&'static Clip>,
    walking: Option<&'static Clip>,
}

/// One robot. `group` goes into the scene (marked dynamic); position and turn it from outside.
pub struct Robot {
    pub group: Obj,
    sp: &'static Species,
    palette: RobotPalette,
    orchestrator: bool,
    mats: RobotMats,
    rig: Rig,
    clips: StateClips,
    /// The offset from gripR to gripL at rest: a two-handed tool aims along it.
    to_grip_l: Vec3,
    state: RobotState,
    eye_intensity: f32,
    last_t: f32,
    held_clip: Option<Timed>,
    once_clip: Option<Once>,
    /// The body layer (see `BodyMode`): held until `Ground` or another mode.
    body: Option<Body>,
    /// The held tool: its bone, and its holder group under it.
    held_tool: Option<(Obj, Obj)>,
    /// The carried item: the carry bone, and its holder group under it.
    carried: Option<(Obj, Obj)>,
    /// The blended joints (see `PROPS`), then the eye lids.
    cur: Vec<f32>,
    /// The angles accumulated by rate channels and spins: each bone's rot x, y, z.
    acc: Vec<f32>,
    /// This frame's clip targets, by joint: the value, and whether it snaps.
    targets: Vec<Option<(f32, bool)>>,
    /// Each bone's offset this frame: the feet's step and the float's bob.
    extras: Vec<Vec3>,
    fx: Fx,
    /// The clip whose effects are timed (its address), and each effect's timer.
    fx_clip: Option<usize>,
    fx_timer: Vec<f32>,
}

impl Robot {
    /// `makeRobot(hue, orchestrator, palette, bodyHue)` with the rover design; hues in degrees.
    pub fn new(hue: f32, orchestrator: bool, palette: &RobotPalette, body_hue: f32) -> Robot {
        Robot::with_variant(hue, orchestrator, palette, body_hue, "rover")
    }

    /// `makeRobot(hue, orchestrator, palette, bodyHue, variant)`: `variant` is a species id. The
    /// orchestrator is always the rover, larger and with its antenna light; an unknown species is the rover.
    pub fn with_variant(hue: f32, orchestrator: bool, palette: &RobotPalette, body_hue: f32, variant: &str) -> Robot {
        let reg = registry();
        let wanted = if orchestrator { "rover" } else { variant };
        let sp: &'static Species = match reg.species.get(wanted) {
            Some(sp) => sp,
            None => {
                warn_once(&format!("species \"{wanted}\" is not valid (see registry().errors); the rover is used"));
                reg.species.get("rover").expect("the rover species is valid")
            }
        };
        let palette = *palette;
        let root = Obj::group();
        root.set_name(if orchestrator { "robot-orchestrator" } else { "robot" });
        root.set_scalar(if orchestrator { ORCH_SCALE } else { SCALE });
        root.set_dynamic(true);

        let mats = robot_mats(&palette, hue, body_hue);
        let geo = species_geo(sp, &palette, orchestrator);
        let rig = instantiate(sp, &geo, &mats, &root);
        let bones = rig.bones.len();
        let joints = bones * PROPS + 1;
        let rest_joint = |j: usize| if j == bones * PROPS { 1.0 } else { rest_of(PROP_NAMES[j % PROPS]) };
        let mut robot = Robot {
            group: root,
            sp,
            palette,
            orchestrator,
            mats,
            rig,
            clips: StateClips {
                idle: clip_named(sp, "idle"),
                working: clip_named(sp, "type"),
                offline: clip_named(sp, "offline"),
                walking: walk_clip(sp),
            },
            to_grip_l: rest_position(sp, "gripL") - rest_position(sp, "gripR"),
            state: RobotState::Idle,
            eye_intensity: 1.3,
            last_t: 0.0,
            held_clip: None,
            once_clip: None,
            body: None,
            held_tool: None,
            carried: None,
            cur: (0..joints).map(rest_joint).collect(),
            acc: vec![0.0; bones * 3],
            targets: vec![None; joints],
            extras: vec![Vec3::ZERO; bones],
            fx: Fx::new(seed(ROBOTS.fetch_add(1, Ordering::Relaxed))),
            fx_clip: None,
            fx_timer: Vec::new(),
        };
        robot.apply_state(RobotState::Idle);
        robot.apply();
        robot
    }

    /// The world point at the species' label height above the root (the status label's anchor).
    pub fn label_anchor(&self) -> Vec3 {
        let to_world = self.group.world_matrix_of(&self.group).unwrap_or(Mat4::IDENTITY);
        to_world.transform_point3(Vec3::new(0.0, self.sp.label_top, 0.0))
    }

    pub fn set_state(&mut self, state: RobotState) {
        if state != self.state {
            self.apply_state(state);
        }
    }

    fn eye_colour(&self, state: RobotState) -> Color {
        match state {
            RobotState::Offline => self.palette.ink_muted,
            RobotState::Working => self.palette.info,
            _ => self.palette.ok,
        }
    }

    fn apply_state(&mut self, next: RobotState) {
        self.state = next;
        let colour = self.eye_colour(next);
        {
            let mut eye = self.mats.eye.get();
            eye.color = colour;
            eye.emissive = colour;
        }
        self.eye_intensity = match next {
            RobotState::Offline => 0.2,
            RobotState::Working => 2.0,
            _ => 1.3,
        };
        self.mats.thruster.get().color = if next == RobotState::Offline { self.palette.ink_muted } else { self.palette.info };
    }

    /// The state's clip: idle, type (working), the walk for the locomotion, or offline.
    fn state_clip(&self) -> Option<&'static Clip> {
        match self.state {
            RobotState::Idle => self.clips.idle,
            RobotState::Working => self.clips.working,
            RobotState::Walking => self.clips.walking,
            RobotState::Offline => self.clips.offline,
        }
    }

    /// A bone's group by name.
    fn bone_obj(&self, name: &str) -> Option<Obj> {
        self.rig.index_of(name).map(|b| self.rig.bones[b].clone())
    }

    /// The joint index of a channel's bone and prop (`lid` has no bone of its own).
    fn joint_of(&self, bone: &str, prop: &str) -> Option<usize> {
        if prop == "lid" {
            return Some(self.lid());
        }
        let p = PROP_NAMES.iter().position(|p| *p == prop)?;
        Some(self.rig.index_of(bone)? * PROPS + p)
    }

    /// The accumulator index of a rotation prop ("rot.x") of a bone.
    fn acc_of(&self, bone: &str, prop: &str) -> Option<usize> {
        Some(self.rig.index_of(bone)? * 3 + axis(prop.strip_prefix("rot.")?)?)
    }

    fn lid(&self) -> usize {
        self.rig.bones.len() * PROPS
    }

    /// The rest value of a joint.
    fn rest(&self, j: usize) -> f32 {
        if j == self.lid() {
            1.0
        } else {
            rest_of(PROP_NAMES[j % PROPS])
        }
    }

    /// The clip playing now and its elapsed time: the body layer over a one-shot over a held clip over the state's clip.
    fn active(&mut self, t: f32) -> Option<(&'static Clip, f64)> {
        if let Some(b) = self.body {
            return Some((b.clip, f64::from(t - b.start)));
        }
        if let Some(o) = self.once_clip {
            if t - o.start >= o.dur {
                self.once_clip = None;
            } else {
                return Some((o.clip, f64::from(t - o.start)));
            }
        }
        if let Some(h) = self.held_clip {
            let since = f64::from(t - h.start);
            let elapsed = match h.clip.dur {
                Some(d) => since % f64::from(d),
                None => since,
            };
            return Some((h.clip, elapsed));
        }
        self.state_clip().map(|c| (c, 0.0))
    }

    /// The clip's effects, spawned on their interval while it plays.
    fn clip_fx(&mut self, clip: &'static Clip, dt: f32) {
        let key = clip as *const Clip as usize;
        if self.fx_clip != Some(key) {
            self.fx_clip = Some(key);
            self.fx_timer.clear();
        }
        for (i, f) in clip.fx.iter().enumerate() {
            let every = f.every.unwrap_or(0.6);
            let next = self.fx_timer.get(i).copied().unwrap_or(every) - dt;
            let value = if next <= 0.0 {
                self.spawn_at(&f.kind, &f.at);
                every
            } else {
                next
            };
            if self.fx_timer.len() <= i {
                self.fx_timer.resize(i + 1, every);
            }
            self.fx_timer[i] = value;
        }
    }

    /// The bone an effect is anchored to, and the point on it: `head` is 0.3 above its pivot.
    fn anchor(&self, name: &str) -> (Obj, Vec3) {
        if name != "head" {
            if let Some(obj) = self.bone_obj(name) {
                return (obj, Vec3::ZERO);
            }
        }
        (self.bone_obj("head").unwrap_or_else(|| self.group.clone()), Vec3::new(0.0, 0.3, 0.0))
    }

    fn spawn_at(&mut self, kind: &str, anchor: &str) {
        let (obj, at) = self.anchor(anchor);
        self.fx.spawn(kind, &obj, at);
    }

    /// Sets the joints to a clip's targets at once (a pose: no blend).
    fn snap_to(&mut self, clip: &'static Clip, t: f32) {
        let sp = self.sp;
        let mut targets = std::mem::take(&mut self.targets);
        targets.iter_mut().for_each(|x| *x = None);
        eval_clip(clip, f64::from(t), &sp.gait, 0.0, |ch, value| {
            if let Some(j) = self.joint_of(&ch.bone, &ch.prop) {
                targets[j] = Some((value as f32, false));
            }
        });
        for j in 0..self.cur.len() {
            let v = targets[j].map_or_else(|| self.rest(j), |(v, _)| v);
            self.cur[j] = v;
        }
        self.targets = targets;
        self.apply();
    }

    /// Writes the joints to the bones: the rest pose, the joints, the offsets, then the eye lids.
    fn apply(&mut self) {
        for (b, obj) in self.rig.bones.iter().enumerate() {
            let c = b * PROPS;
            let a = b * 3;
            let rest = self.rig.rest[b];
            let e = self.extras[b];
            obj.set_rotation(self.cur[c] + self.acc[a], self.cur[c + 1] + self.acc[a + 1], self.cur[c + 2] + self.acc[a + 2]);
            obj.set_position(rest[0] + self.cur[c + 3] + e.x, rest[1] + self.cur[c + 4] + e.y, rest[2] + self.cur[c + 5] + e.z);
            obj.set_scalar(self.cur[c + 6]);
        }
        let lid = self.cur[self.lid()];
        for p in &self.rig.pupils {
            p.set_scale(1.0, lid, 1.0);
        }
    }

    /// Advances the animation: `dt` seconds since the last frame, `t` seconds since start.
    pub fn update(&mut self, dt: f32, t: f32) {
        self.last_t = t;
        let k = (dt * BLEND).min(1.0);
        let sp = self.sp;
        let active = self.active(t);
        let mut targets = std::mem::take(&mut self.targets);
        targets.iter_mut().for_each(|x| *x = None);
        if let Some((clip, elapsed)) = active {
            eval_clip(clip, f64::from(t), &sp.gait, elapsed, |ch, value| {
                if let Some(j) = self.joint_of(&ch.bone, &ch.prop) {
                    targets[j] = Some((value as f32, ch.snap == Some(true)));
                }
            });
            for ch in &clip.channels {
                if let Some(rate) = ch.rate {
                    if let Some(a) = self.acc_of(&ch.bone, &ch.prop) {
                        self.acc[a] += rate * dt;
                    }
                }
            }
            self.clip_fx(clip, dt);
        }
        self.targets = targets;

        // Blend every joint toward its target; a joint no clip names eases back to rest.
        for j in 0..self.cur.len() {
            let (goal, snap) = match self.targets[j] {
                Some((v, s)) => (v, s),
                None => (self.rest(j), false),
            };
            let v = self.cur[j];
            self.cur[j] = if snap { goal } else { v + (goal - v) * k };
        }

        // Spins: the species' rates by state (a rotor, a ball wheel).
        for s in &sp.spin {
            let rate = match self.state {
                RobotState::Walking => s.walk,
                RobotState::Offline => s.offline.unwrap_or(0.0),
                _ => s.idle,
            };
            if let (Some(b), Some(ax)) = (self.rig.index_of(&s.bone), axis(&s.axis)) {
                self.acc[b * 3 + ax] += rate * dt;
            }
        }

        // Feet step alternately while walking; floating robots bob, and sit down when offline.
        for e in &mut self.extras {
            *e = Vec3::ZERO;
        }
        for (i, bone) in sp.feet.iter().enumerate() {
            let p = if self.state == RobotState::Walking { (WALK_FEET_RATE * t + i as f32 * PI).sin() } else { 0.0 };
            if let Some(b) = self.rig.index_of(bone) {
                self.extras[b].y += 0.035 * p.max(0.0);
                self.extras[b].z += 0.06 * p;
            }
        }
        if let Some(tuck) = self.body.and_then(|b| b.mode.feet_tuck()) {
            for bone in &sp.feet {
                if let Some(b) = self.rig.index_of(bone) {
                    self.extras[b].y += tuck;
                }
            }
        }
        if sp.floats {
            let lift = if self.state == RobotState::Offline { FLOAT_OFFLINE } else { 0.035 * (2.2 * t).sin() };
            for name in ["base", "body"] {
                if let Some(b) = self.rig.index_of(name) {
                    self.extras[b].y += lift;
                }
            }
        }

        self.apply();
        self.fx.update(dt);
        self.mats.eye.get().emissive_intensity = if self.state == RobotState::Working { self.eye_intensity + 0.3 * (5.0 * t).sin() } else { self.eye_intensity };
        if self.orchestrator {
            self.mats.light.get().emissive_intensity = 1.1 + 0.5 * (3.0 * t).sin();
        }
    }

    /// The highlight: the matte takes the signal glow (not the glass, which never glows).
    pub fn set_highlighted(&mut self, on: bool) {
        let mut matte = self.mats.matte.get();
        matte.emissive = if on { self.palette.signal } else { Color::BLACK };
        matte.emissive_intensity = if on { 0.35 } else { 0.0 };
    }

    /// Strikes a still pose (for a portrait), held until the next play or pose.
    pub fn pose(&mut self, kind: RobotPose) {
        let name = match kind {
            RobotPose::Wave => "wave",
            RobotPose::Proud => "proud",
            RobotPose::Think => "think",
            RobotPose::Cheer => "cheer",
        };
        let Some(clip) = clip_named(self.sp, name) else {
            return warn_once(&format!("no pose clip \"{name}\""));
        };
        self.held_clip = Some(Timed { clip, start: self.last_t });
        self.once_clip = None;
        let now = self.last_t;
        self.snap_to(clip, now);
    }

    /// Plays a clip: held when it loops (or with `looped`), else a one-shot that returns to the state's clip.
    /// `None` ends a held clip.
    pub fn play(&mut self, clip_id: Option<&str>, looped: Option<bool>) {
        let Some(id) = clip_id else {
            self.held_clip = None;
            return;
        };
        let Some(clip) = clip_named(self.sp, id) else {
            return warn_once(&format!("no clip \"{id}\" for species \"{}\"", self.sp.id));
        };
        if looped.unwrap_or(clip.looped != Some(false)) {
            self.held_clip = Some(Timed { clip, start: self.last_t });
            self.once_clip = None;
        } else {
            self.once_clip = Some(Once { clip, start: self.last_t, dur: clip.dur.unwrap_or(1.6) });
        }
    }

    /// The body layer (`setBody` in robot.ts): the mode's clip overrides the hold, the one-shot and the state's clip while
    /// it is set, and plays from `now`. The same mode again keeps its entry time; `Ground` ends the layer.
    pub fn set_body(&mut self, mode: BodyMode, now: f32) {
        let Some(name) = mode.clip_name() else {
            self.body = None;
            return;
        };
        if self.body.is_some_and(|b| b.mode == mode) {
            return;
        }
        let Some(clip) = clip_named(self.sp, name) else {
            return warn_once(&format!("no body clip \"{name}\" for species \"{}\"", self.sp.id));
        };
        self.body = Some(Body { mode, clip, start: now });
    }

    /// The body layer's mode (`Ground` when none is set).
    pub fn body_mode(&self) -> BodyMode {
        self.body.map_or(BodyMode::Ground, |b| b.mode)
    }

    /// Holds a tool in the grip: one hand, or both (a two-handed tool aims at the other grip). `None` puts it down.
    pub fn hold(&mut self, tool_id: Option<&str>) {
        if let Some((bone, holder)) = self.held_tool.take() {
            bone.remove(&holder);
        }
        let Some(id) = tool_id else { return };
        let Some(tool) = registry().tools.get(id) else {
            return warn_once(&format!("no tool \"{id}\""));
        };
        let bone_name = if tool.grip == "carry" { "carry" } else { "gripR" };
        let Some(bone) = self.bone_obj(bone_name) else { return };
        let holder = attach(&bone, &tool_geo(id, tool, &self.palette), &self.mats);
        if tool.grip == "two" {
            holder.set_quaternion(Quat::from_rotation_arc(Vec3::X, self.to_grip_l.normalize_or_zero()));
        } else if tool.grip == "one" {
            // The parts hang along −y; a fist turns them ahead (−y → +z), or stands them up.
            let angle = match tool.aim.as_deref() {
                Some("down") => 0.0,
                Some("up") => std::f32::consts::PI,
                _ => -std::f32::consts::FRAC_PI_2,
            };
            holder.set_quaternion(Quat::from_rotation_x(angle));
        }
        self.held_tool = Some((bone, holder));
    }

    /// Carries an item at the carry bone (a crate). `None` puts it down.
    pub fn carry(&mut self, item_id: Option<&str>) {
        if let Some((bone, holder)) = self.carried.take() {
            bone.remove(&holder);
        }
        let Some(id) = item_id else { return };
        let Some(item) = registry().tools.get(id) else {
            return warn_once(&format!("no item \"{id}\""));
        };
        let Some(bone) = self.bone_obj("carry") else { return };
        let holder = attach(&bone, &tool_geo(id, item, &self.palette), &self.mats);
        self.carried = Some((bone, holder));
    }

    /// Spawns one effect at the head (sparks, zzz, dots, hearts, keys).
    pub fn fx(&mut self, kind: &str) {
        self.spawn_at(kind, "head");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robot_variant_cycles_through_the_species() {
        let list: Vec<&str> = registry().index.species.iter().map(String::as_str).filter(|s| *s != "oracle").collect();
        assert_eq!(robot_variant(0), list[0]);
        assert_eq!(robot_variant(list.len() as u64), list[0]);
        assert_eq!(robot_variant(1), list[1]);
    }

    #[test]
    fn fallback_never_deals_the_oracle() {
        assert!(registry().index.species.iter().any(|s| s == "oracle"), "the oracle is in the index");
        for cid in 0..10_000u64 {
            assert_ne!(robot_variant(cid), "oracle", "cid {cid}");
            assert_ne!(species_of(None, cid), "oracle", "cid {cid}");
            assert_ne!(species_of(Some(""), cid), "oracle", "cid {cid}");
        }
    }

    #[test]
    fn a_record_species_still_wins_when_it_is_the_oracle() {
        for cid in [0u64, 1, 7, 9_999] {
            assert_eq!(species_of(Some("oracle"), cid), "oracle", "the record's own species, cid {cid}");
        }
    }

    #[test]
    fn species_comes_from_the_agent_record() {
        let named = registry().index.species.last().expect("a species").as_str();
        assert_eq!(species_of(Some(named), 0), named, "the record's species wins over the cid");
        assert_eq!(species_of(Some(named), 1), named, "the record's species wins over the cid");
    }

    #[test]
    fn species_falls_back_to_the_cid_when_the_record_has_none() {
        assert_eq!(species_of(None, 3), robot_variant(3));
        assert_eq!(species_of(Some("  "), 3), robot_variant(3));
    }

    // The web's body layer, mirrored from floor.ts (`bodyClip`) and robot.ts (`update`, BLEND, AIR_FEET). A body clip's
    // channels hold still (a base, no amp, freq or rate), so each base is a goal; every joint eases toward its goal by
    // k = min(1, dt·8) a frame, and a joint no clip names eases to rest. The feet take AIR_FEET's tuck.
    const WEB_BLEND: f32 = 8.0;

    /// floor.ts `bodyClip`: the web's clip id per mode.
    fn web_body_clip(mode: BodyMode) -> Option<&'static str> {
        match mode {
            BodyMode::Ground => None,
            BodyMode::Crouch => Some("jump.crouch"),
            BodyMode::Air => Some("jump.air"),
            BodyMode::Fall => Some("fall.air"),
            BodyMode::Land => Some("land"),
        }
    }

    /// The joints at rest (`restOf`): a scale and the eye lids at 1, the rest at 0.
    fn web_rest(robot: &Robot) -> Vec<f32> {
        (0..robot.cur.len()).map(|j| if j == robot.lid() || j % PROPS == 6 { 1.0 } else { 0.0 }).collect()
    }

    /// The goals of a body clip: its bases over the rest.
    fn web_goals(robot: &Robot, clip_id: &str, rest: &[f32]) -> Vec<f32> {
        use crate::robots::types::Param;
        let clip = registry().clips.get(clip_id).expect("the shared body clip");
        let mut goals = rest.to_vec();
        for ch in &clip.channels {
            assert!(ch.amp.is_none() && ch.freq.is_none() && ch.rate.is_none(), "{clip_id} holds still");
            let Some(Param::Num(base)) = &ch.base else { panic!("{clip_id} has a numeric base") };
            goals[robot.joint_of(&ch.bone, &ch.prop).expect("a joint")] = *base;
        }
        goals
    }

    /// The web's joints after `frames` frames of `dt` in `mode`, entered from rest.
    fn web_joints(robot: &Robot, mode: BodyMode, frames: usize, dt: f32) -> Vec<f32> {
        let rest = web_rest(robot);
        let goals = match web_body_clip(mode) {
            Some(id) => web_goals(robot, id, &rest),
            None => rest.clone(),
        };
        let k = (dt * WEB_BLEND).min(1.0);
        let mut cur = rest;
        for _ in 0..frames {
            for (c, g) in cur.iter_mut().zip(&goals) {
                *c += (g - *c) * k;
            }
        }
        cur
    }

    #[test]
    fn body_modes_play_the_web_clip_and_pose() {
        let palette = robot_palette();
        let dt = 1.0 / 12.0; // the Ambient tier's rate
        let samples = [1usize, 3, 12]; // 1/12 s, 1/4 s and 1 s into the mode
        for mode in [BodyMode::Crouch, BodyMode::Air, BodyMode::Fall, BodyMode::Land] {
            // The clip depends only on the time since entry, so an entry at 7.5 s is the same as one at 0.
            for entered in [0.0f32, 7.5] {
                let mut robot = Robot::new(200.0, false, &palette, 120.0);
                robot.set_body(mode, entered);
                assert_eq!(robot.body_mode(), mode);
                assert_eq!(mode.clip_name(), web_body_clip(mode));
                let mut frame = 0usize;
                for &n in &samples {
                    while frame < n {
                        frame += 1;
                        robot.update(dt, entered + frame as f32 * dt);
                    }
                    let want = web_joints(&robot, mode, n, dt);
                    for (j, (got, exp)) in robot.cur.iter().zip(&want).enumerate() {
                        assert!((got - exp).abs() < 1e-5, "{mode:?} entered {entered}, {n} frames, joint {j}: {got} vs {exp}");
                    }
                }
            }
        }
    }

    #[test]
    fn air_lifts_the_feet_by_air_feet() {
        let palette = robot_palette();
        let cases = [
            (BodyMode::Ground, 0.0f32),
            (BodyMode::Crouch, 0.0),
            (BodyMode::Air, 0.06),
            (BodyMode::Fall, -0.03),
            (BodyMode::Land, 0.0),
        ];
        let mut checked = 0;
        for id in registry().index.species.iter().map(String::as_str) {
            let mut robot = Robot::with_variant(200.0, false, &palette, 120.0, id);
            let feet = robot.sp.feet.clone();
            if feet.is_empty() {
                continue;
            }
            checked += 1;
            for (mode, want) in cases {
                robot.set_body(mode, 0.0);
                robot.update(1.0 / 12.0, 1.0 / 12.0);
                for bone in &feet {
                    let b = robot.rig.index_of(bone).expect("a foot bone");
                    assert!((robot.extras[b].y - want).abs() < 1e-6, "{id} {mode:?}: {bone} lifts {want}, got {}", robot.extras[b].y);
                }
            }
        }
        assert!(checked > 0, "some species have feet to lift");
    }
}
