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

use super::super::robot::{robot_palette, Robot, RobotPalette, RobotPose, RobotState};
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
    match v.and_then(|s| s.parse::<f32>().ok()).filter(|h| h.is_finite()) {
        Some(h) => h.rem_euclid(360.0),
        None => DEFAULT_HUE,
    }
}

/// The frozen clock: a finite time in seconds, not below zero; None for the live clock.
pub fn frozen_of(v: Option<&str>) -> Option<f32> {
    v.and_then(|s| s.parse::<f32>().ok()).filter(|t| t.is_finite()).map(|t| t.max(0.0))
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
}

/// The labs' ground: a `ShadowMaterial` at 0.2 on `geo` (laid flat at the origin), so only the robots' and the rooms'
/// shadows show on the flat background.
pub(crate) fn shadow_ground(floor: &mut Floor, geo: &crate::scene::Geo) {
    use crate::scene::{Mat, Material, Obj, Shading};
    let mat = Mat::new(Material { shading: Shading::Shadow, color: Color::BLACK, opacity: 0.2, transparent: true, depth_write: false, fog: false, ..Material::default() });
    let ground = Obj::mesh(geo, &mat);
    ground.set_rotation(-std::f32::consts::FRAC_PI_2, 0.0, 0.0).set_shadows(false, true);
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
/// turn, one to each spot; a named species is every robot.
fn build_props(floor: &mut Floor, prop: &str, species: &str, look: &Look, hue: f32, palette: &RobotPalette, robots: &mut Vec<Robot>) {
    let reg = prop_registry();
    let lineup = prop == "all";
    let ids: Vec<String> = if lineup { reg.order.clone() } else { vec![prop.to_string()] };
    let cols = if lineup { PROP_COLS } else { 1 };
    let rows = ids.len().div_ceil(cols);
    let species_ids = &registry().index.species;
    let mut dealt = 0usize;
    for (i, pid) in ids.iter().enumerate() {
        let Some(p) = reg.props.get(pid) else { continue };
        let (col, row) = ((i % cols) as f32, (i / cols) as f32);
        let ox = if lineup { (col - (cols as f32 - 1.0) / 2.0) * PROP_SPACING_X } else { 0.0 };
        let oz = if lineup { ((rows as f32 - 1.0) / 2.0 - row) * PROP_SPACING_Z } else { 0.0 };
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
            robot.group.set_position(ox + s.at[0], s.at[1], oz + s.at[2]).set_rotation(0.0, s.face.to_radians(), 0.0);
            dress(floor, &mut robot, look, look.clip.as_deref().or(Some(s.clip.as_str())));
            robots.push(robot);
        }
    }
}

/// The line-up, when `APZ_ROBOT_LAB` asks for one: its robots join the floor's root, and the camera
/// frames them. None otherwise (the floor is then the plaza). APZ_ROBOT_PROP puts the prop screen in its place.
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let want = var("APZ_ROBOT_LAB")?;
    let lineup = want == "all";
    let ids: Vec<String> = if lineup { registry().index.species.clone() } else { vec![want.clone()] };
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
    let cam = var("APZ_ROBOT_CAM").as_deref().and_then(Cam::parse).unwrap_or(Cam::Front);
    let bg = background_or_default(var("APZ_ROBOT_BG").as_deref());
    pale_stage(floor, bg);
    // robot-lab.ts's floor: a disc of radius 4.5 that shows only the shadows.
    shadow_ground(floor, &crate::geometry::circle(4.5, 48, 0.0, std::f32::consts::TAU));

    let look = Look { state, tool, carry, clip, pose, frozen };
    let mut robots = Vec::new();
    let (eye, target) = match var("APZ_ROBOT_PROP") {
        Some(prop) => {
            let prop_lineup = prop == "all";
            build_props(floor, &prop, &want, &look, hue, &palette, &mut robots);
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
    // The web's camera was 30 degrees and the orbit's is FOV_DEG: the eye comes in by tan 15 / tan(FOV_DEG / 2), so
    // the robots fill the frame as they did.
    let field = 15f32.to_radians().tan() / (crate::controls::FOV_DEG / 2.0).to_radians().tan();
    let eye = std::array::from_fn::<f32, 3, _>(|i| target[i] + (eye[i] - target[i]) * field);
    floor.orbit.look(eye.into(), target.into());
    // robot-lab.ts's OrbitControls kept three's defaults: no polar or distance limits (the front view is near level).
    floor.orbit.unlimited(0.0, f32::INFINITY);
    Some(Lab {
        robots,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's CSS-style hsl to sRGB (hue in degrees): the reference for three's setHSL.
    fn hsl_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
        let f = |n: f32| {
            let k = (n + h / 30.0).rem_euclid(12.0);
            l - s * l.min(1.0 - l) * (k - 3.0).min(9.0 - k).min(1.0).max(-1.0)
        };
        (f(0.0), f(8.0), f(4.0))
    }

    #[test]
    fn cam_presets_are_the_webs_numbers() {
        // The web's CAM_POS for a line-up and for one robot, and the frame() target (0.75 and 0.7).
        let cases = [
            (Cam::Front, [0.0, 1.5, 9.8], [0.0, 1.1, 4.4]),
            (Cam::Iso, [5.0, 3.0, 8.0], [2.6, 2.0, 3.6]),
            (Cam::Side, [0.0, 1.3, 11.0], [4.6, 1.1, 0.0]),
            (Cam::Q34, [-5.4, 4.4, 8.2], [-2.44, 1.84, 3.48]),
            (Cam::Top, [0.0, 11.0, 1.8], [0.0, 5.0, 0.8]),
        ];
        for (cam, lineup, one) in cases {
            assert_eq!(cam.view(true), (lineup, [0.0, 0.75, 0.0]), "{cam:?} line-up");
            assert_eq!(cam.view(false), (one, [0.0, 0.7, 0.0]), "{cam:?} one");
        }
        assert_eq!(Cam::parse("q34"), Some(Cam::Q34));
        assert_eq!(Cam::parse("behind"), None);
    }

    #[test]
    fn hue_tint_for_200_is_the_webs_hsl() {
        let hue = hue_of(Some("200"));
        assert_eq!(hue, 200.0);
        // The panel: setHSL(hue / 360, 0.55, 0.78). For 200 the sample is (0.659, 0.820, 0.901) sRGB.
        let (r, g, b) = hsl_rgb(hue, 0.55, 0.78);
        assert!((r - 0.659).abs() < 1e-3 && (g - 0.8203).abs() < 1e-3 && (b - 0.901).abs() < 1e-3);
        let got = Color::hsl(hue / 360.0, 0.55, 0.78);
        let want = Color::srgb(r, g, b);
        assert!((got.r - want.r).abs() < 1e-5, "r {} vs {}", got.r, want.r);
        assert!((got.g - want.g).abs() < 1e-5, "g {} vs {}", got.g, want.g);
        assert!((got.b - want.b).abs() < 1e-5, "b {} vs {}", got.b, want.b);
    }

    #[test]
    fn hue_wraps_and_defaults() {
        assert_eq!(hue_of(None), 200.0);
        assert_eq!(hue_of(Some("bogus")), 200.0);
        assert_eq!(hue_of(Some("-160")), 200.0);
        assert_eq!(hue_of(Some("560")), 200.0);
        assert_eq!(hue_of(Some("0")), 0.0);
    }

    #[test]
    fn frozen_clock_and_background() {
        assert_eq!(frozen_of(None), None);
        assert_eq!(frozen_of(Some("")), None);
        assert_eq!(frozen_of(Some("1.5")), Some(1.5));
        assert_eq!(frozen_of(Some("-2")), Some(0.0));
        assert_eq!(background_of(Some("dark")), Some(Color::hex(0x1b1a17)));
        assert_eq!(background_of(Some("light")), Some(Color::hex(0xf3efe6)));
        assert_eq!(background_of(Some("blue")), None);
        assert_eq!(background_of(None), None);
    }

    #[test]
    fn the_default_background_is_the_webs_pale_blue() {
        assert_eq!(background_or_default(None), Color::hex(0xe9edf3));
        assert_eq!(background_or_default(Some("dark")), Color::hex(0x1b1a17));
        assert_eq!(background_or_default(Some("light")), Color::hex(0xf3efe6));
        assert_eq!(background_or_default(Some("blue")), Color::hex(0xe9edf3));
    }

    #[test]
    fn orchestrator_is_the_rover_alone_or_first_in_a_line_up() {
        assert!(is_orchestrator(true, "rover", 0, false));
        assert!(is_orchestrator(true, "rover", 0, true));
        assert!(!is_orchestrator(true, "rover", 1, true));
        assert!(!is_orchestrator(true, "crab", 0, true));
        assert!(!is_orchestrator(true, "crab", 0, false));
        assert!(!is_orchestrator(false, "rover", 0, false));
    }

    #[test]
    fn one_robot_keeps_its_close_level_view_through_the_orbits_update() {
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("APZ_ROBOT_LAB".to_string(), "duck".to_string());
        crate::set_env_overrides(vars);
        let mut floor = Floor::machines();
        let lab = build(&mut floor);
        crate::set_env_overrides(Default::default());
        assert!(lab.is_some());
        let before = floor.orbit.camera(1.0).eye;
        floor.orbit.update();
        let after = floor.orbit.camera(1.0).eye;
        assert!(before.distance(after) < 1e-4, "the update moved the eye from {before:?} to {after:?}");
        assert!(floor.orbit.radius < 4.0, "radius {}", floor.orbit.radius);
    }

    #[test]
    fn prop_camera_presets_are_the_webs_numbers() {
        assert_eq!(Cam::Front.view_prop(true), ([0.0, 8.0, 17.0], [0.0, 0.5, 0.0]));
        assert_eq!(Cam::Top.view_prop(false), ([0.0, 5.6, 0.9], [0.0, 0.6, 0.0]));
        assert_eq!(Cam::Side.view_prop(true), ([17.0, 5.0, 0.0], [0.0, 0.5, 0.0]));
    }

    /// Builds the lab with the given options, on the test thread's own overrides.
    fn build_with(options: &[(&str, &str)]) -> Option<Lab> {
        let vars = options.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        crate::set_env_overrides(vars);
        let mut floor = Floor::new();
        let lab = build(&mut floor);
        crate::set_env_overrides(Default::default());
        lab
    }

    #[test]
    fn a_prop_puts_a_robot_at_each_of_its_spots() {
        let reg = prop_registry();
        let prop = reg.order.first().expect("the index lists a prop").clone();
        let spots = reg.props[&prop].spots.len();
        let lab = build_with(&[("APZ_ROBOT_LAB", "rover"), ("APZ_ROBOT_PROP", prop.as_str())]).expect("the lab builds");
        assert_eq!(lab.robots.len(), spots, "{prop}: one robot per spot");
    }

    #[test]
    fn the_prop_line_up_deals_every_spot_of_every_prop() {
        let reg = prop_registry();
        let all: usize = reg.order.iter().map(|p| reg.props[p].spots.len()).sum();
        let lab = build_with(&[("APZ_ROBOT_LAB", "all"), ("APZ_ROBOT_PROP", "all")]).expect("the lab builds");
        assert_eq!(lab.robots.len(), all);
        assert!(all > 0, "the index lists props with spots");
    }

    #[test]
    fn an_effect_option_is_kept_for_the_lab_to_spawn() {
        let lab = build_with(&[("APZ_ROBOT_LAB", "rover"), ("APZ_ROBOT_FX", "hearts")]).expect("the lab builds");
        assert_eq!(lab.fx.as_deref(), Some("hearts"));
        let mut lab = lab;
        // The first step's timer is spent at once: the effect spawns on every robot, and the timer rearms.
        lab.step(1.0);
        assert_eq!(lab.fx_timer, FX_EVERY);
    }
}
