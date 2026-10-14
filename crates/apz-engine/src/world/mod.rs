//! The floor itself (`floor.ts`): a machine's board (`Board`: its rooms on the components, the
//! copper, the atom), a robot per agent, wandering, walking the copper and socialising, packets when
//! work changes hands, labels and the agent card. `layout` builds the scene and keeps it in step
//! with the agent list, `board_layout` places the rooms, `motion` moves it each frame, `view` is the
//! iced side (the shader widget, pointer input, labels, card, legend).

pub mod behaviour;
pub mod board_layout;
pub mod labels;
pub mod layout;
pub mod lift_mesh;
pub mod motion;
pub mod nav;
pub mod spots;
pub mod walkway_mesh;
pub mod view;
pub mod work;

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};

use super::controls::{Orbit, FOV_DEG};
use super::entry_scene::BoardCache;
use super::hardware::{Built, Model, Params, Pulses, Status};
use super::props::{Anim, Box2, PropPalette};
use board_layout::{BoardLayout, TraceGraph};
use layout::{AZIMUTH, POLAR};
use super::robot::{robot_palette, Robot, RobotPalette, RobotState};
use super::scene::{self, Color, Draw, Filter, Frame, Geo, Lod, Mat, Obj, PointLightDraw, World};

/// The name the floor shows (`floorName` in roles.ts): the agent's own name, or "agent <cid>" while
/// it has not chosen one (the hub returns an empty name until then).
pub fn display_name(cid: u64, name: &str) -> String {
    if name.trim().is_empty() {
        format!("agent {cid}")
    } else {
        name.to_string()
    }
}

/// What the floor needs of an agent (from `GET /v1/agents`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// "idle" | "working" | "offline".
    pub status: String,
    /// "orchestrator" | "worker": the orchestrator gets the dais and the bigger robot.
    pub tier: String,
    /// The model id, for the card.
    pub model: String,
    /// The robot species the hub sent (None: the cid's deal, `robot::species_of`).
    pub species: Option<String>,
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
    /// The walkway the robot follows to `route_goal` (its next waypoint first), across the copper.
    pub route: Vec<[f32; 2]>,
    pub route_goal: Option<[f32; 2]>,
}

/// One trace's pulses, and the slots at its two ends (their activity lights the pulses).
pub struct TracePulses {
    pub pulses: Pulses,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// A machine's board, shown in the Machine view: its model built at the origin, the copper and its
/// pulses, the atom over the centre, the rooms on its slots (`sync`), and what the frames drive.
pub struct Board {
    /// The model, params and status the board was built with: a change rebuilds it.
    pub key: String,
    /// The key the built parts are cached under (`board_key`): the board goes back to `Floor::boards` by it.
    pub cache_key: String,
    pub model: &'static Model,
    pub params: Params,
    pub status: Status,
    /// The board's group in the root: the built parts, the copper and the pulses.
    pub obj: Obj,
    pub built: Built,
    /// The atom's animation, when the board has an atom anchor.
    pub atom: Option<Anim>,
    /// The atom's wrap node (the group `place_atom` adds), for the ENTER's scale and fade (`entry_scene`).
    pub atom_node: Option<Obj>,
    pub pulses: Vec<TracePulses>,
    /// Each slot's glowing materials with their rest intensity.
    pub glow: HashMap<String, Vec<(Mat, f32)>>,
    /// Each slot's activity (0..1, eased toward its rooms' work).
    pub level: HashMap<String, f32>,
    pub layout: BoardLayout,
    pub graph: TraceGraph,
    /// The layout's signature: the rooms are rebuilt when it changes.
    pub sig: String,
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
            sheet: Color::hex(0x2c3554),
            rule: Color::hex(0x56638a),
            ink3: Color::hex(0x7d8bb5),
            signal: Color::hex(0xff7a2e),
            ok: Color::hex(0x3ee08f),
            info: Color::hex(0x4cc9ff),
            paper: Color::hex(0x0a0e19),
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
    /// The Machines view's ground, and the dark ground under a machine's board.
    pub ground: Obj,
    pub board_ground: Obj,
    /// The skyline behind the Machines' ground: shown at night, hidden on the board (`entry_scene`).
    pub backdrop: Obj,
    /// The machine's board, once the Machine view has one.
    pub board: Option<Board>,
    /// The machine whose board this is, as the web's route names it (`''` on the fleet): it seeds the rooms.
    pub machine_id: String,
    /// The built boards no longer shown, by `board_key`: a machine shown again takes its parts from here.
    pub boards: BoardCache,
    /// The starfield (it turns slowly) and its ringed planet (it spins).
    pub stars: Obj,
    pub planet: Obj,
    pub dust: Obj,
    /// Per mote: x, y, z, rise speed.
    pub dust_seeds: Vec<[f32; 4]>,
    pub colors: Colors,
    pub robot_palette: RobotPalette,
    pub shared: Shared,
    pub districts: Vec<District>,
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
    /// The board's rooms still to build, a district per step (`layout.rs`'s `step_rooms`).
    rooms_job: Option<layout::RoomsJob>,
    /// The last board layout, with the key it is for (the board and the roster): `begin_sync` reuses it.
    layout_cache: Option<(String, board_layout::BoardLayout)>,
    /// The board's navigation (nav.rs) when its layout is the shared one: the robots walk its grid.
    pub nav: Option<nav::BoardNav>,
    statics: Arc<Vec<Draw>>,
    statics_version: u64,
    /// The GPU-animated batches of the statics (`bake_split`), versioned with them.
    anim_batches: Arc<Vec<scene::AnimBatch>>,
    static_lights: Vec<PointLightDraw>,
    /// The dynamic subtrees inside the root's static children (found when baking).
    deep_dynamic: Vec<scene::Rig>,
}

impl Floor {
    /// An empty floor with its sky, stars, ground, plaza and dust (`layout::base`).
    pub fn new() -> Floor {
        let colors = Colors::default();
        let robot_palette = robot_palette();
        let shared = layout::shared(&colors);
        let mut floor = Floor {
            root: Obj::group(),
            district_group: Obj::group(),
            ground: Obj::group(),
            board_ground: Obj::group(),
            backdrop: Obj::group(),
            board: None,
            machine_id: String::new(),
            boards: BoardCache::new(3),
            stars: Obj::group(),
            planet: Obj::group(),
            dust: Obj::group(),
            dust_seeds: Vec::new(),
            colors,
            robot_palette,
            shared,
            districts: Vec::new(),
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
            anim_batches: Arc::new(Vec::new()),
            static_lights: Vec::new(),
            rooms_job: None,
            layout_cache: None,
            nav: None,
            deep_dynamic: Vec::new(),
        };
        layout::base(&mut floor);
        floor
    }

    /// This frame for the renderer: the baked statics (re-baked when dirty) and the dynamic draws.
    pub fn frame(&mut self, aspect: f32) -> Frame {
        if self.statics_dirty {
            let baked = scene::bake_split(&self.root, scene::GPU_HW_ANIM);
            self.statics = Arc::new(baked.statics);
            self.anim_batches = Arc::new(baked.animated);
            self.static_lights = scene::flatten(&self.root, Filter::Static).1;
            let root_world = self.root.local_matrix();
            let children = self.root.get().children.clone();
            self.deep_dynamic = children
                .iter()
                .filter(|c| !c.get().dynamic)
                .flat_map(|c| scene::rigs(c, root_world))
                .collect();
            self.statics_version += 1;
            self.statics_dirty = false;
        }
        // Only the moving parts are walked: the root's dynamic children (robots and packets come
        // and go) and the dynamic subtrees found inside the baked ones.
        let root_world = self.root.local_matrix();
        let roots: Vec<(Obj, glam::Mat4)> =
            self.root.get().children.iter().filter(|c| c.get().dynamic).map(|c| (c.clone(), root_world)).collect();
        let (mut draws, mut lights) = scene::flatten_roots(&roots);
        let (rig_draws, rig_lights) = scene::flatten_rigs_instanced(&self.deep_dynamic);
        draws.extend(rig_draws);
        lights.extend(rig_lights);
        lights.extend(self.static_lights.iter().copied());
        Frame {
            animated: None,
            camera: self.orbit.camera(aspect),
            world: self.world,
            statics: Arc::clone(&self.statics),
            statics_version: self.statics_version,
            draws,
            lights,
            time: self.time,
            overlays: Vec::new(),
            lod: self.board.as_ref().map(|b| Lod {
                center: Vec3::new(0.0, b.model.size[1] * 0.5, 0.0),
                radius: 0.5 * Vec3::from(b.model.size).length(),
            }),
            fades: Arc::new(self.board.as_ref().map(|b| b.built.fades.clone()).unwrap_or_default()),
            plaza: self.plaza_area(),
            anim_batches: Arc::clone(&self.anim_batches),
            focus: self.board.as_ref().and_then(|b| b.model.anchors.atom).map(Vec3::from),
        }
    }

    /// The plaza area the cutaways fade over (the web's `plazaArea`): the points the default view frames,
    /// grown by a third of their size on each side in x and z (the shrouds beside the plaza stand in view).
    /// None without a board.
    pub fn plaza_area(&self) -> Option<(Vec3, Vec3)> {
        if self.board.is_none() {
            return None;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in self.fit_points() {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let grow = (hi - lo) / 3.0;
        lo.x -= grow.x;
        hi.x += grow.x;
        lo.z -= grow.z;
        hi.z += grow.z;
        Some((lo, hi))
    }

    pub fn bot(&self, cid: u64) -> Option<&Bot> {
        self.bots.iter().find(|b| b.agent.cid == cid)
    }

    /// The Machines view's scene: the Floor's sky, stars, ground and outskirts, with no plaza, no
    /// districts and no robots. The fleet's group goes into its root.
    pub fn machines() -> Floor {
        let floor = Floor::new();
        floor.root.remove(&floor.district_group);
        floor
    }

    /// The palette the fleet scene is built with: the floor's colours.
    pub fn fleet_palette(&self) -> PropPalette {
        let c = &self.colors;
        PropPalette { sheet: c.sheet, rule: c.rule, ink3: c.ink3, signal: c.signal, ok: c.ok, info: c.info }
    }

    /// Frames a sphere (`centre`, `radius`) in an `aspect` stage with the floor's view angles, at
    /// the distance from [`fit_distance`].
    pub fn fit_sphere(&mut self, centre: [f32; 3], radius: f32, aspect: f32) {
        let dir = Vec3::new(POLAR.sin() * AZIMUTH.sin(), POLAR.cos(), POLAR.sin() * AZIMUTH.cos());
        let target = Vec3::from(centre);
        let dist = fit_distance(radius, aspect).max(self.orbit.min_distance);
        self.orbit.look(target + dir * dist, target);
    }
}

/// The share of the tighter half-extent that a framed sphere fills.
const MACHINES_FILL: f32 = 0.8;

/// The camera distance at which a sphere of `radius` fills `MACHINES_FILL` of the tighter
/// half-extent of an `aspect` (width / height) stage.
pub fn fit_distance(radius: f32, aspect: f32) -> f32 {
    let tan_v = (FOV_DEG / 2.0).to_radians().tan();
    let tan_h = tan_v * aspect.max(1e-3);
    radius / (MACHINES_FILL * tan_v.min(tan_h))
}

impl Default for Floor {
    fn default() -> Self {
        Floor::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{display_name, fit_distance, FOV_DEG};

    #[test]
    fn fit_distance_holds_the_sphere_in_the_tighter_axis() {
        let tan_v = (FOV_DEG / 2.0).to_radians().tan();
        // A square stage: the sphere fills 80% of both half-extents.
        assert!((fit_distance(10.0, 1.0) - 10.0 / (0.8 * tan_v)).abs() < 1e-3);
        // A wide stage: the height is the tighter axis, so the distance does not change.
        assert!((fit_distance(10.0, 2.0) - fit_distance(10.0, 1.0)).abs() < 1e-3);
        // A tall stage: the width is the tighter axis, so the sphere stands farther off.
        let tall = fit_distance(10.0, 0.5);
        assert!((tall - 10.0 / (0.8 * tan_v * 0.5)).abs() < 1e-3);
        assert!(tall > fit_distance(10.0, 1.0));
    }

    #[test]
    fn floor_name_falls_back_to_the_cid_when_empty_or_blank() {
        assert_eq!(display_name(7, ""), "agent 7");
        assert_eq!(display_name(7, "   "), "agent 7");
        assert_eq!(display_name(7, "\t\n"), "agent 7");
    }

    #[test]
    fn floor_name_keeps_a_chosen_name() {
        assert_eq!(display_name(7, "Ada"), "Ada");
        assert_eq!(display_name(7, " Ada "), " Ada ");
    }
}
