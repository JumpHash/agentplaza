//! The floor itself (`floor.ts`): districts by role around the orchestrator's dais, a robot per
//! agent, wandering and socialising, packets when work changes hands, labels and the agent card.
//! `layout` builds the world and keeps it in step with the agent list, `motion` moves it each
//! frame, `view` is the iced side (the shader widget, pointer input, labels, card, legend).

pub mod layout;
pub mod motion;
pub mod view;

use std::collections::HashMap;
use std::sync::Arc;

use glam::Quat;

use super::controls::Orbit;
use super::props::{Anim, Box2};
use super::robot::{Robot, RobotPalette, RobotState};
use super::scene::{self, Color, Draw, Filter, Frame, Geo, Mat, Obj, PointLightDraw, World};

/// What the floor needs of an agent (from `GET /v1/agents`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// "idle" | "working" | "offline".
    pub status: String,
    /// The model id, for the card.
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct District {
    pub role: String,
    /// Centre, grid units.
    pub cx: f32,
    pub cy: f32,
    /// Half-size (square) or radius (round).
    pub r: f32,
    pub hue: f32,
    pub round: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Go,
    Talk,
    Stroll,
}

/// A robot's visit, chat or stroll (`Social`).
#[derive(Debug, Clone)]
pub struct Social {
    pub phase: Phase,
    /// The partner's cid.
    pub partner: Option<u64>,
    pub gx: f32,
    pub gy: f32,
    /// When it ends, ms; infinity until the walk is done.
    pub until: f64,
}

/// One agent's robot on the floor (`Bot`).
pub struct Bot {
    pub agent: AgentInfo,
    /// Index into `Floor::districts`.
    pub district: usize,
    /// The console slot, grid units.
    pub home: (f32, f32),
    pub x: f32,
    pub y: f32,
    pub tx: f32,
    pub ty: f32,
    pub heading: f32,
    pub moving: bool,
    pub py: f32,
    pub vy: f32,
    pub state: Option<RobotState>,
    pub next_wander: f64,
    pub next_social: f64,
    pub social: Option<Social>,
    /// What the agent is doing now (label and card).
    pub activity: String,
    pub robot: Robot,
    pub robot_key: String,
    pub tilt: Quat,
    pub pad: Obj,
    pub pad_mat: Mat,
    pub token: Obj,
    pub highlighted: bool,
}

/// A message flying between two robots.
pub struct Packet {
    pub from: u64,
    pub to: u64,
    pub start: f64,
    pub core: Obj,
    pub halo: Obj,
}

/// A district's monitors and whether one of its robots works now.
pub struct DistrictLight {
    pub screens: Vec<Mat>,
    pub lit: bool,
}

/// The scene's cartoon grade of the console palette (`C` in floor.ts).
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub sheet: Color,
    pub rule: Color,
    pub ink3: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
    pub paper: Color,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            sheet: Color::hex(0xfff4de),
            rule: Color::hex(0xe2b97f),
            ink3: Color::hex(0x667892),
            signal: Color::hex(0xe8590c),
            ok: Color::hex(0x2e6a40),
            info: Color::hex(0x2d5f8a),
            paper: Color::hex(0xe9ece5),
        }
    }
}

/// Geometry and materials every robot, packet and pad shares.
pub struct Shared {
    pub pad_geo: Geo,
    pub packet_geo: Geo,
    pub halo_geo: Geo,
    pub token_geo: Geo,
    pub accent: Mat,
    pub ok: Mat,
    pub accent_halo: Mat,
    pub ok_halo: Mat,
    pub token: Mat,
}

/// The whole floor.
pub struct Floor {
    pub root: Obj,
    pub district_group: Obj,
    pub path_group: Obj,
    pub plaza: Obj,
    pub plaza_anim: Anim,
    pub clouds: Obj,
    pub dust: Obj,
    /// Per mote: x, y, z, rise speed.
    pub dust_seeds: Vec<[f32; 4]>,
    pub colors: Colors,
    pub robot_palette: RobotPalette,
    pub shared: Shared,
    pub districts: Vec<District>,
    pub district_sig: String,
    pub bots: Vec<Bot>,
    pub packets: Vec<Packet>,
    pub prop_boxes: Vec<Box2>,
    pub lit_by_role: HashMap<String, DistrictLight>,
    pub prop_anim: Vec<Anim>,
    pub orbit: Orbit,
    pub world: World,
    pub hovered: Option<u64>,
    pub selected: Option<u64>,
    /// The view has been fitted to the districts once.
    pub fitted: bool,
    /// Seconds since the floor opened, and the wall clock (ms) of the last step.
    pub time: f32,
    pub now_ms: f64,
    /// Set whenever static parts of the scene change (the world was rebuilt): re-bake.
    pub statics_dirty: bool,
    statics: Arc<Vec<Draw>>,
    statics_version: u64,
    static_lights: Vec<PointLightDraw>,
}

impl Floor {
    /// An empty floor with its sky, ground, clouds, plaza and dust (`layout::base`).
    pub fn new() -> Floor {
        let colors = Colors::default();
        let robot_palette = RobotPalette {
            ink: Color::hex(0x151916),
            sheet: Color::hex(0xf4f6f1),
            paper: colors.paper,
            rule: Color::hex(0xc7cfc3),
            signal: colors.signal,
            ok: colors.ok,
            info: colors.info,
            ink_muted: Color::hex(0x7d867e),
        };
        let shared = layout::shared(&colors);
        let mut floor = Floor {
            root: Obj::group(),
            district_group: Obj::group(),
            path_group: Obj::group(),
            plaza: Obj::group(),
            plaza_anim: Box::new(|_| {}),
            clouds: Obj::group(),
            dust: Obj::group(),
            dust_seeds: Vec::new(),
            colors,
            robot_palette,
            shared,
            districts: Vec::new(),
            district_sig: String::new(),
            bots: Vec::new(),
            packets: Vec::new(),
            prop_boxes: Vec::new(),
            lit_by_role: HashMap::new(),
            prop_anim: Vec::new(),
            orbit: Orbit::default(),
            world: World::default(),
            hovered: None,
            selected: None,
            fitted: false,
            time: 0.0,
            now_ms: 0.0,
            statics_dirty: true,
            statics: Arc::new(Vec::new()),
            statics_version: 0,
            static_lights: Vec::new(),
        };
        layout::base(&mut floor);
        floor
    }

    /// This frame for the renderer: the baked statics (re-baked when dirty) and the dynamic draws.
    pub fn frame(&mut self, aspect: f32) -> Frame {
        if self.statics_dirty {
            self.statics = Arc::new(scene::bake(&self.root));
            self.static_lights = scene::flatten(&self.root, Filter::Static).1;
            self.statics_version += 1;
            self.statics_dirty = false;
        }
        let (draws, mut lights) = scene::flatten(&self.root, Filter::Dynamic);
        lights.extend(self.static_lights.iter().copied());
        Frame {
            camera: self.orbit.camera(aspect),
            world: self.world,
            statics: Arc::clone(&self.statics),
            statics_version: self.statics_version,
            draws,
            lights,
            time: self.time,
        }
    }

    pub fn bot(&self, cid: u64) -> Option<&Bot> {
        self.bots.iter().find(|b| b.agent.cid == cid)
    }
}

impl Default for Floor {
    fn default() -> Self {
        Floor::new()
    }
}
