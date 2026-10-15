//! The robot lab (`APZ_ROBOT_LAB`), as the web's `#/robot-lab`: a line-up of species (or one) under the
//! floor's lighting, each turned to the camera. Env, all optional except the first:
//!   APZ_ROBOT_LAB      a species id, or `all` (the whole line-up from index.json)
//!   APZ_ROBOT_STATE    idle | working | walking | offline (default idle)
//!   APZ_ROBOT_CLIP     a shared or own clip id (empty: the state's clip)
//!   APZ_ROBOT_POSE     wave | proud | think | cheer (a still pose, after the clip)
//!   APZ_ROBOT_PROP     a station prop id, or all (the line-up of every prop): a robot at each of its spots, facing
//!                      it and playing the spot's clip (APZ_ROBOT_CLIP wins when set); APZ_ROBOT_LAB names the species
//!                      (all: the species in turn). A prop screen replaces the species line-up.
//!   APZ_ROBOT_TOOL     a tool id, held in the grip
//!   APZ_ROBOT_CARRY    an item id, carried
//!   APZ_ROBOT_FX       sparks | zzz | dots | hearts | keys, spawned every 0.7 s
//!   APZ_ROBOT_HUE      0-360 (default 200): the shell and the panel tint
//!   APZ_ROBOT_ORCH     1: the rover as the orchestrator (in a line-up, only when the rover is first)
//!   APZ_ROBOT_CAM      front (default) | iso | side | q34 | top
//!   APZ_ROBOT_T        seconds: a frozen clock (each robot takes its pose at that time, then stops)
//!   APZ_ROBOT_BG       dark | light (the scene background, see `Lab::background`; unset: the engine's)

use super::super::robot::{Robot, RobotPalette, RobotPose, RobotState, robot_palette};
use super::super::scene::Color;
use super::super::world::Floor;
use super::files::registry;
use super::props::{build_prop, prop_registry};

/// The distance between two robots of the line-up, along x (the web's 2.2).
const SPACING: f32 = 2.2;
const FX_EVERY: f32 = 0.7;
/// The web's default hue (the lab's pale blue).
const DEFAULT_HUE: f32 = 200.0;
/// The prop line-up's spacing along x and z, and its columns (web: 2.6, 2.4 and 6).
const PROP_SPACING_X: f32 = 2.6;
const PROP_SPACING_Z: f32 = 2.4;
const PROP_COLS: usize = 6;

/// The robots of a line-up, and their clock.
pub struct Lab {
    pub robots: Vec<Robot>,
    /// The props on screen (0 on the species screen).
    pub props: usize,
    t: f32,
    fx: Option<String>,
    fx_timer: f32,
    /// A frozen clock: the robots took their pose once, at build, and nothing moves after.
    frozen: bool,
    bg: Option<Color>,
}

/// A camera preset, named as the web's `cam` parameter names it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cam {
    Front,
    Iso,
    Side,
    Q34,
    Top,
}

impl Cam {
    /// The preset named `s` (front | iso | side | q34 | top), or None.
    pub fn parse(s: &str) -> Option<Cam> {
        Some(match s {
            "front" => Cam::Front,
            "iso" => Cam::Iso,
            "side" => Cam::Side,
            "q34" => Cam::Q34,
            "top" => Cam::Top,
            _ => return None,
        })
    }

    /// The eye and the target of the preset, for a line-up or one robot: the web's CAM_POS and the
    /// target of its frame() (0.75 high for a line-up, 0.7 for one robot).
    pub fn view(self, lineup: bool) -> ([f32; 3], [f32; 3]) {
        let eye = match (self, lineup) {
            (Cam::Front, true) => [0.0, 1.5, 9.8],
            (Cam::Front, false) => [0.0, 1.1, 4.4],
            (Cam::Iso, true) => [5.0, 3.0, 8.0],
            (Cam::Iso, false) => [2.6, 2.0, 3.6],
            (Cam::Side, true) => [0.0, 1.3, 11.0],
            (Cam::Side, false) => [4.6, 1.1, 0.0],
            // Three-quarter: 35 degrees round to the front-left, 15 degrees up, 4.4 from the centre.
            (Cam::Q34, true) => [-5.4, 4.4, 8.2],
            (Cam::Q34, false) => [-2.44, 1.84, 3.48],
            // Looking down from above the centre.
            (Cam::Top, true) => [0.0, 11.0, 1.8],
            (Cam::Top, false) => [0.0, 5.0, 0.8],
        };
        let y = if lineup { 0.75 } else { 0.7 };
        (eye, [0.0, y, 0.0])
    }

    /// The eye and the target of the prop screen's preset (the web's PROP_CAM): a line-up of every prop is wider than
    /// a species line-up, and its target is lower.
    pub fn view_prop(self, lineup: bool) -> ([f32; 3], [f32; 3]) {
        let eye = match (self, lineup) {
            (Cam::Front, true) => [0.0, 8.0, 17.0],
            (Cam::Front, false) => [0.0, 1.7, 5.4],
            (Cam::Iso, true) => [11.0, 9.0, 13.0],
            (Cam::Iso, false) => [3.4, 2.7, 4.8],
            (Cam::Side, true) => [17.0, 5.0, 0.0],
            (Cam::Side, false) => [5.6, 1.5, 0.0],
            (Cam::Q34, true) => [-8.9, 7.8, 12.8],
            (Cam::Q34, false) => [-2.5, 2.6, 3.56],
            (Cam::Top, true) => [0.0, 18.0, 0.5],
            (Cam::Top, false) => [0.0, 5.6, 0.9],
        };
        let y = if lineup { 0.5 } else { 0.6 };
        (eye, [0.0, y, 0.0])
    }
}

fn var(name: &str) -> Option<String> {
    crate::env_var(name).filter(|v| !v.is_empty())
}

/// The hue as the web reads it: a number, wrapped into 0–360; anything else is the default 200.
pub fn hue_of(v: Option<&str>) -> f32 {
    match v
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|h| h.is_finite())
    {
        Some(h) => h.rem_euclid(360.0),
        None => DEFAULT_HUE,
    }
}

/// The frozen clock: a finite time in seconds, not below zero; None for the live clock.
pub fn frozen_of(v: Option<&str>) -> Option<f32> {
    v.and_then(|s| s.parse::<f32>().ok())
        .filter(|t| t.is_finite())
        .map(|t| t.max(0.0))
}

/// The scene background of the web's `bg` (dark or light); None: the engine's own.
pub fn background_of(v: Option<&str>) -> Option<Color> {
    match v? {
        "dark" => Some(Color::hex(0x1b1a17)),
        "light" => Some(Color::hex(0xf3efe6)),
        _ => None,
    }
}

/// The web lab's default scene background (`DEFAULT_BG` in `robot-lab.ts`): the pale blue.
pub const DEFAULT_BACKGROUND: u32 = 0xe9edf3;

/// The lab's scene background: the web's `bg`, or the default pale blue when `bg` is unset.
pub fn background_or_default(v: Option<&str>) -> Color {
    background_of(v).unwrap_or(Color::hex(DEFAULT_BACKGROUND))
}

/// The labs' own stage, as the web's robot and room labs (robot-lab.ts, room-lab.ts): no plaza scenery (the stars,
/// the planet, the dust, the skyline and the machines' ground), no fog, their lights, and a flat `bg` background.
pub(crate) fn pale_stage(floor: &mut Floor, bg: Color) {
    floor.stars.set_visible(false);
    floor.planet.set_visible(false);
    floor.dust.set_visible(false);
    floor.backdrop.set_visible(false);
    floor.ground.set_visible(false);
    floor.statics_dirty = true;
    let world = &mut floor.world;
    world.background = bg;
    world.flat_background = true;
    world.fog_near = 1.0e6;
    world.fog_far = 2.0e6; // past fog_near, or every fragment is fully fogged
    world.hemi_sky = Color::hex(0xeaf4ff);
    world.hemi_ground = Color::hex(0x8a7a5a);
    world.hemi_intensity = 1.3;
    world.sun_color = Color::hex(0xfff1d6);
    world.sun_intensity = 2.6;
    // The labs' key light at (2.5, 4, 3), with three's default shadow camera (5 each way, near 0.5, far 500).
    world.sun_position = [2.5, 4.0, 3.0].into();
    world.shadow_extent = 5.0;
    world.shadow_near = 0.5;
    world.shadow_far = 500.0;
    // The robot and room labs rendered with three's default shadow map: redrawn every frame.
    world.shadow_every = 1;
}

/// The labs' ground: a `ShadowMaterial` at 0.2 on `geo` (laid flat at the origin), so only the robots' and the rooms'
/// shadows show on the flat background.
pub(crate) fn shadow_ground(floor: &mut Floor, geo: &crate::scene::Geo) {
    use crate::scene::{Mat, Material, Obj, Shading};
    let mat = Mat::new(Material {
        shading: Shading::Shadow,
        color: Color::BLACK,
        opacity: 0.2,
        transparent: true,
        depth_write: false,
        fog: false,
        ..Material::default()
    });
    let ground = Obj::mesh(geo, &mat);
    ground
        .set_rotation(-std::f32::consts::FRAC_PI_2, 0.0, 0.0)
        .set_shadows(false, true);
    floor.root.add(&ground);
}

/// The web's orchestrator rule: the rover alone, or in a line-up only the first robot, when it is the rover.
fn is_orchestrator(orch: bool, id: &str, index: usize, lineup: bool) -> bool {
    orch && id == "rover" && (!lineup || index == 0)
}

/// What every robot of the lab shares: its state, its tool, its carried item, its clip (APZ_ROBOT_CLIP), a still pose
/// after the clip, and the frozen clock.
struct Look {
    state: RobotState,
    tool: Option<String>,
    carry: Option<String>,
    clip: Option<String>,
    pose: Option<RobotPose>,
    frozen: Option<f32>,
}

/// Sets one robot up as the lab's look says, `clip` (the state's when None) played, and adds it to the floor's root.
fn dress(floor: &Floor, robot: &mut Robot, look: &Look, clip: Option<&str>) {
    robot.set_state(look.state);
    robot.hold(look.tool.as_deref());
    robot.carry(look.carry.as_deref());
    robot.play(clip, None);
    if let Some(p) = look.pose {
        robot.pose(p);
    }
    if let Some(t) = look.frozen {
        // A blend of 1 snaps each joint to its target at time t (the web's frozen clock).
        robot.update(1.0, t);
    }
    floor.root.add(&robot.group);
}

/// The prop screen (the web's buildProps): one prop, or every prop in a line-up. Each prop stands on the floor, and
/// each spot gets a robot facing it, playing the spot's clip. `species` is APZ_ROBOT_LAB: `all` deals the species in
/// turn, one to each spot; a named species is every robot. Returns the number of props built.
fn build_props(
    floor: &mut Floor,
    prop: &str,
    species: &str,
    look: &Look,
    hue: f32,
    palette: &RobotPalette,
    robots: &mut Vec<Robot>,
) -> usize {
    let reg = prop_registry();
    let lineup = prop == "all";
    let ids: Vec<String> = if lineup {
        reg.order.clone()
    } else {
        vec![prop.to_string()]
    };
    let cols = if lineup { PROP_COLS } else { 1 };
    let rows = ids.len().div_ceil(cols);
    let species_ids = &registry().index.species;
    let mut dealt = 0usize;
    let mut built_props = 0usize;
    for (i, pid) in ids.iter().enumerate() {
        let Some(p) = reg.props.get(pid) else {
            continue;
        };
        built_props += 1;
        let (col, row) = ((i % cols) as f32, (i / cols) as f32);
        let ox = if lineup {
            (col - (cols as f32 - 1.0) / 2.0) * PROP_SPACING_X
        } else {
            0.0
        };
        let oz = if lineup {
            ((rows as f32 - 1.0) / 2.0 - row) * PROP_SPACING_Z
        } else {
            0.0
        };
        let built = build_prop(p, palette, hue);
        built.group.set_position(ox, 0.0, oz);
        floor.root.add(&built.group);
        for s in &built.spots {
            let id = if species == "all" && !species_ids.is_empty() {
                species_ids[dealt % species_ids.len()].as_str()
            } else {
                species
            };
            dealt += 1;
            let mut robot = Robot::with_variant(hue, false, palette, hue, id);
            robot
                .group
                .set_position(ox + s.at[0], s.at[1], oz + s.at[2])
                .set_rotation(0.0, s.face.to_radians(), 0.0);
            dress(
                floor,
                &mut robot,
                look,
                look.clip.as_deref().or(Some(s.clip.as_str())),
            );
            robots.push(robot);
        }
    }
    built_props
}

/// The line-up, when `APZ_ROBOT_LAB` asks for one: its robots join the floor's root, and the camera
/// frames them. None otherwise (the floor is then the plaza). APZ_ROBOT_PROP puts the prop screen in its place.
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let want = var("APZ_ROBOT_LAB")?;
    let lineup = want == "all";
    let ids: Vec<String> = if lineup {
        registry().index.species.clone()
    } else {
        vec![want.clone()]
    };
    let palette = robot_palette();
    let state = match var("APZ_ROBOT_STATE").as_deref() {
        Some("working") => RobotState::Working,
        Some("walking") => RobotState::Walking,
        Some("offline") => RobotState::Offline,
        _ => RobotState::Idle,
    };
    let clip = var("APZ_ROBOT_CLIP");
    let pose = var("APZ_ROBOT_POSE").and_then(|p| match p.as_str() {
        "wave" => Some(RobotPose::Wave),
        "proud" => Some(RobotPose::Proud),
        "think" => Some(RobotPose::Think),
        "cheer" => Some(RobotPose::Cheer),
        _ => None,
    });
    let tool = var("APZ_ROBOT_TOOL");
    let carry = var("APZ_ROBOT_CARRY");
    let hue = hue_of(var("APZ_ROBOT_HUE").as_deref());
    let orch = var("APZ_ROBOT_ORCH").as_deref() == Some("1");
    let frozen = frozen_of(var("APZ_ROBOT_T").as_deref());
    let cam = var("APZ_ROBOT_CAM")
        .as_deref()
        .and_then(Cam::parse)
        .unwrap_or(Cam::Front);
    let bg = background_or_default(var("APZ_ROBOT_BG").as_deref());
    pale_stage(floor, bg);
    // robot-lab.ts's floor: a disc of radius 4.5 that shows only the shadows.
    shadow_ground(
        floor,
        &crate::geometry::circle(4.5, 48, 0.0, std::f32::consts::TAU),
    );

    let look = Look {
        state,
        tool,
        carry,
        clip,
        pose,
        frozen,
    };
    let mut robots = Vec::new();
    let mut props = 0;
    let (eye, target) = match var("APZ_ROBOT_PROP") {
        Some(prop) => {
            let prop_lineup = prop == "all";
            props = build_props(floor, &prop, &want, &look, hue, &palette, &mut robots);
            cam.view_prop(prop_lineup)
        }
        None => {
            let n = ids.len();
            for (i, id) in ids.iter().enumerate() {
                let as_orch = is_orchestrator(orch, id, i, lineup);
                let mut robot = Robot::with_variant(hue, as_orch, &palette, hue, id);
                let x = (i as f32 - (n as f32 - 1.0) / 2.0) * SPACING;
                robot.group.set_position(x, 0.0, 0.0);
                dress(floor, &mut robot, &look, look.clip.as_deref());
                robots.push(robot);
            }
            // The camera: the web's preset for a line-up or one robot.
            cam.view(lineup)
        }
    };
    // The readout's tally: robot i's draws count toward owner i + 1 (see `Node::tally`); the readout turns counting on.
    for (i, robot) in robots.iter().enumerate() {
        robot.group.set_tally(i as u32 + 1);
    }
    crate::render::set_tally(true);
    // The web's camera was 30 degrees and the orbit's is FOV_DEG: the eye comes in by tan 15 / tan(FOV_DEG / 2), so
    // the robots fill the frame as they did.
    let field = 15f32.to_radians().tan() / (crate::controls::FOV_DEG / 2.0).to_radians().tan();
    let eye = std::array::from_fn::<f32, 3, _>(|i| target[i] + (eye[i] - target[i]) * field);
    floor.orbit.look(eye.into(), target.into());
    // robot-lab.ts's OrbitControls kept three's defaults: no polar or distance limits (the front view is near level).
    floor.orbit.unlimited(0.0, f32::INFINITY);
    Some(Lab {
        robots,
        props,
        t: frozen.unwrap_or(0.0),
        fx: var("APZ_ROBOT_FX"),
        fx_timer: 0.0,
        frozen: frozen.is_some(),
        bg: Some(bg),
    })
}

impl Lab {
    /// Advances the line-up by `dt` seconds (each frame), and spawns the chosen effect on its interval.
    /// A frozen clock does nothing: the pose was taken at build.
    pub fn step(&mut self, dt: f32) {
        if self.frozen {
            return;
        }
        self.t += dt;
        let t = self.t;
        for r in &mut self.robots {
            r.update(dt, t);
        }
        if let Some(kind) = &self.fx {
            self.fx_timer -= dt;
            if self.fx_timer <= 0.0 {
                self.fx_timer = FX_EVERY;
                for r in &mut self.robots {
                    r.fx(kind);
                }
            }
        }
    }

    /// The scene background the lab asks for: `APZ_ROBOT_BG`, or the default pale blue.
    pub fn background(&self) -> Option<Color> {
        self.bg
    }

    /// The readout's draw counts for one frame's main pass: `owned` is the main pass's draw calls by owner (index n is
    /// tally n, robot n - 1; see `Renderer::owned_draws`) and `calls` its whole total.
    pub fn draw_counts(&self, owned: &[u32], calls: u32) -> DrawCounts {
        let robots = self
            .robots
            .iter()
            .enumerate()
            .map(|(i, r)| RobotCalls {
                species: r.species().id.clone(),
                calls: owned.get(i + 1).copied().unwrap_or(0),
            })
            .collect();
        DrawCounts {
            props: self.props,
            robots,
            calls,
        }
    }
}

/// The robot lab's draw calls of one frame's main pass, as its readout shows them (`Engine::lab_draws`): the props on
/// screen, each robot's draws by species (in the lab's order), and the whole main pass. A robot's draws are those whose
/// node is under its group. An instanced draw shared by several robots counts toward none of them (it is in `calls`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DrawCounts {
    /// The props on screen: 0 on the species screen.
    pub props: usize,
    /// Each robot's species id and its draw calls, in the lab's order.
    pub robots: Vec<RobotCalls>,
    /// The main pass's draw calls, the whole frame.
    pub calls: u32,
}

/// One robot's draw calls in the main pass.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RobotCalls {
    pub species: String,
    pub calls: u32,
}
