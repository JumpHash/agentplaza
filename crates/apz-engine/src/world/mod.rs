//! The scene the Machines view stands in: the night sky with its stars and planet, the ground, the drifting dust,
//! the camera's orbit and the world's light, baked into the frames the renderer draws. The fleet (`fleet`) goes into
//! its root; the labs (`hardware_lab`, `robots::lab`) build their own stage in it. `tags` paints the machines' name
//! tags.

pub mod tags;

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};
use std::sync::Arc;

use super::controls::{FOV_DEG, Orbit};
use super::geometry;
use super::props::PropPalette;
use super::robot::{RobotPalette, robot_palette};
use super::scene::{
    self, Color, Draw, Filter, Frame, Mat, Material, Obj, PointLightDraw, Shading, World,
};
use super::textures::{self, Rng};
use super::toon;
use glam::Vec3;

/// The camera's angle from vertical, about 55 degrees: isometric-ish.
pub const POLAR: f32 = 0.95;
/// The camera's turn about y.
pub const AZIMUTH: f32 = FRAC_PI_4;
/// Dust motes drifting over the ground.
const DUST_N: usize = 90;

/// The name an agent shows: its own name, or "agent <cid>" while it has not chosen one (the hub returns an empty name
/// until then).
pub fn display_name(cid: u64, name: &str) -> String {
    if name.trim().is_empty() {
        format!("agent {cid}")
    } else {
        name.to_string()
    }
}

/// What the hosts' machine popups show of an agent (from `GET /v1/agents`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// "idle" | "working" | "offline".
    pub status: String,
    /// "orchestrator" | "worker".
    pub tier: String,
    /// The model id.
    pub model: String,
    /// The robot species the hub sent (None: the cid's deal, `robot::species_of`), for the agent's portrait.
    pub species: Option<String>,
}

/// The scene's colours: the console's dark sheet, rules and status inks.
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

/// The scene: its root and the parts the labs hide, the orbit, the light and the baked statics.
pub struct Floor {
    pub root: Obj,
    pub ground: Obj,
    /// The starfield (it turns slowly) and its ringed planet (it spins).
    pub stars: Obj,
    pub planet: Obj,
    pub dust: Obj,
    /// Per mote: x, y, z, rise speed.
    pub dust_seeds: Vec<[f32; 4]>,
    pub colors: Colors,
    pub robot_palette: RobotPalette,
    pub orbit: Orbit,
    pub world: World,
    /// Seconds since the scene opened, and the wall clock (ms) of the last step.
    pub time: f32,
    pub now_ms: f64,
    /// Set whenever static parts of the scene change: re-bake.
    pub statics_dirty: bool,
    statics: Arc<Vec<Draw>>,
    statics_version: u64,
    /// The GPU-animated batches of the statics (`bake_split`), versioned with them.
    anim_batches: Arc<Vec<scene::AnimBatch>>,
    static_lights: Vec<PointLightDraw>,
    /// The dynamic subtrees inside the root's static children (found when baking).
    deep_dynamic: Vec<scene::Rig>,
}

impl Floor {
    /// An empty scene with its sky, stars, ground and dust.
    pub fn new() -> Floor {
        let mut floor = Floor {
            root: Obj::group(),
            ground: Obj::group(),
            stars: Obj::group(),
            planet: Obj::group(),
            dust: Obj::group(),
            dust_seeds: Vec::new(),
            colors: Colors::default(),
            robot_palette: robot_palette(),
            orbit: Orbit::default(),
            world: World::default(),
            time: 0.0,
            now_ms: 0.0,
            statics_dirty: true,
            statics: Arc::new(Vec::new()),
            statics_version: 0,
            anim_batches: Arc::new(Vec::new()),
            static_lights: Vec::new(),
            deep_dynamic: Vec::new(),
        };
        floor.base();
        floor
    }

    /// The Machines view's scene. Its shadows are drawn when they are due (the first frame, a change), never on a
    /// cadence.
    pub fn machines() -> Floor {
        let mut floor = Floor::new();
        floor.world.shadow_every = 0;
        floor
    }

    /// Night: a deep blue zenith banded down to a violet horizon glow, stars and a ringed planet, one smooth ground
    /// sheet out to the horizon, and dust motes in the air.
    fn base(&mut self) {
        self.root.add(&toon::sky(
            Color::hex(0x05071a),
            Color::hex(0x3a2275),
            Color::hex(0x090c18),
        ));
        let sky = toon::starfield(900, 330.0, &mut Rng::new(31));
        self.stars = sky.group;
        self.planet = sky.planet;
        self.stars.set_dynamic(true); // turns every frame
        self.root.add(&self.stars);

        let ground_mat = toon::toon_mat(Material {
            map: Some(textures::ground()),
            ..Material::default()
        });
        let ground = Obj::mesh(&geometry::plane(500.0, 500.0, 1, 1), &ground_mat);
        ground
            .set_rotation(-FRAC_PI_2, 0.0, 0.0)
            .set_position(0.0, -0.01, 0.0)
            .set_shadows(false, true);
        self.root.add(&ground);
        self.ground = ground;

        let mut rnd = Rng::new(17);
        self.dust_seeds = (0..DUST_N)
            .map(|_| {
                let a = rnd.next() * TAU;
                let r = rnd.next().sqrt() * 9.0;
                let y = rnd.next() * 4.2;
                let rise = 0.04 + rnd.next() * 0.05;
                [a.cos() * r, y, a.sin() * r, rise]
            })
            .collect();
        let dust_mat = Mat::new(Material {
            shading: Shading::Points { size: 0.08 },
            color: Color::hex(0x7fe3ff),
            transparent: true,
            opacity: 0.8,
            additive: true,
            depth_write: false,
            ..Material::default()
        });
        self.dust = Obj::points(vec![[0.0; 3]; DUST_N], &dust_mat);
        self.dust.set_dynamic(true);
        self.root.add(&self.dust);
    }

    /// One frame: `dt` seconds since the last (capped at 50 ms), `now_ms` the wall clock, and `ambient` the ambient
    /// clock in seconds. The stars, the planet and the dust read `ambient`, so they look the same at any tick rate.
    pub fn step(&mut self, dt: f32, now_ms: f64, ambient: f32) {
        self.time += dt.min(0.05);
        self.now_ms = now_ms;
        let points: Vec<[f32; 3]> = self
            .dust_seeds
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let fi = i as f32;
                [
                    d[0] + (ambient * 0.3 + fi).sin() * 0.25,
                    0.3 + ((d[1] + ambient * d[3]) % 4.2),
                    d[2] + (ambient * 0.25 + fi).cos() * 0.25,
                ]
            })
            .collect();
        self.dust.set_points(points);
        self.stars.set_rotation(0.0, ambient * 0.004, 0.0);
        self.planet.get().rotation.y = ambient * 0.05;
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
        // Only the moving parts are walked: the root's dynamic children and the dynamic subtrees inside the baked ones.
        let root_world = self.root.local_matrix();
        let roots: Vec<(Obj, glam::Mat4)> = self
            .root
            .get()
            .children
            .iter()
            .filter(|c| c.get().dynamic)
            .map(|c| (c.clone(), root_world))
            .collect();
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
            fades: Arc::new(Vec::new()),
            plaza: None,
            anim_batches: Arc::clone(&self.anim_batches),
            focus: None,
        }
    }

    /// The palette the fleet scene is built with: the scene's colours.
    pub fn fleet_palette(&self) -> PropPalette {
        let c = &self.colors;
        PropPalette {
            sheet: c.sheet,
            rule: c.rule,
            ink3: c.ink3,
            signal: c.signal,
            ok: c.ok,
            info: c.info,
        }
    }

    /// Frames a sphere (`centre`, `radius`) in an `aspect` stage with the scene's view angles, at the distance from
    /// [`fit_distance`].
    pub fn fit_sphere(&mut self, centre: [f32; 3], radius: f32, aspect: f32) {
        let dir = Vec3::new(
            POLAR.sin() * AZIMUTH.sin(),
            POLAR.cos(),
            POLAR.sin() * AZIMUTH.cos(),
        );
        let target = Vec3::from(centre);
        let dist = fit_distance(radius, aspect).max(self.orbit.min_distance);
        self.orbit.look(target + dir * dist, target);
    }
}

impl Default for Floor {
    fn default() -> Self {
        Floor::new()
    }
}

/// The share of the tighter half-extent that a framed sphere fills.
const MACHINES_FILL: f32 = 0.8;

/// The camera distance at which a sphere of `radius` fills `MACHINES_FILL` of the tighter half-extent of an `aspect`
/// (width / height) stage.
pub fn fit_distance(radius: f32, aspect: f32) -> f32 {
    let tan_v = (FOV_DEG / 2.0).to_radians().tan();
    let tan_h = tan_v * aspect.max(1e-3);
    // The sphere's angular radius a has tan a = fill * tan(half-field), and sin a = radius / distance, so the distance
    // is radius * sqrt(1 + 1 / (fill * tan)^2).
    let t = MACHINES_FILL * tan_v.min(tan_h);
    radius * (1.0 + 1.0 / (t * t)).sqrt()
}
