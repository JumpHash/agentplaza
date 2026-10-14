//! The hardware lab (`APZ_HW_LAB`), as the web's `#/hardware-lab` on the engine: one hardware model on the machines'
//! ground under the floor's light, its state set by the env. Env, all optional except the first:
//!   APZ_HW_LAB      a model id, e.g. board/workstation or exterior/workstation
//!   APZ_HW_GPUS     the GPUs, 0..2 (default 0)
//!   APZ_HW_STATUS   online | draining | offline | revoked (default online)
//!   APZ_HW_WORKING  how many agents are busy, 0..6 (default 0): the activity is min(1, working / 4)
//!   APZ_HW_OPEN     how far it is opened, 0..1 (default 0)
//!   APZ_HW_TRACES   1 shows the copper traces and their pulses (default 0)

use glam::Vec3;

use super::hardware::{self, Params, State, Status};
use super::world::Floor;

/// The model on the machines' ground: its state, and its traces and pulses when shown.
pub struct Lab {
    built: hardware::Built,
    pulses: Option<hardware::Pulses>,
    state: State,
}

fn var(name: &str) -> Option<String> {
    crate::env_var(name).filter(|v| !v.is_empty())
}

/// A finite number from the env, clamped to `lo..=hi`; the fallback when unset or unreadable.
fn number(name: &str, lo: f32, hi: f32, fallback: f32) -> f32 {
    var(name).and_then(|s| s.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).map_or(fallback, |v| v.clamp(lo, hi))
}

fn status(s: Option<String>) -> Status {
    match s.as_deref() {
        Some("draining") => Status::Draining,
        Some("offline") => Status::Offline,
        Some("revoked") => Status::Revoked,
        _ => Status::Online,
    }
}

/// The model's box in its own frame: the union of its solid parts' boxes, or its size when it has none.
fn bounds(model: &hardware::Model, params: &Params) -> (Vec3, Vec3) {
    let boxes = hardware::part_boxes(model, params);
    if boxes.is_empty() {
        let s = Vec3::from(model.size);
        return (Vec3::new(-s.x / 2.0, 0.0, -s.z / 2.0), Vec3::new(s.x / 2.0, s.y, s.z / 2.0));
    }
    boxes.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), b| {
        (lo.min(Vec3::from(b.lo)), hi.max(Vec3::from(b.hi)))
    })
}

/// The model, when `APZ_HW_LAB` names one: it joins the floor's root at the origin, and the camera frames it. None
/// otherwise (or for an unknown model), so the floor is then the machines' own view.
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let model = hardware::model(&var("APZ_HW_LAB")?)?;
    let params = Params::default().with("gpus", f64::from(number("APZ_HW_GPUS", 0.0, 2.0, 0.0).round()));
    let state = State {
        status: status(var("APZ_HW_STATUS")),
        activity: (number("APZ_HW_WORKING", 0.0, 6.0, 0.0) / 4.0).min(1.0),
        open: number("APZ_HW_OPEN", 0.0, 1.0, 0.0),
    };
    night_stage(floor);
    let built = hardware::build(model, &params);
    floor.root.add(&built.root);
    let pulses = if var("APZ_HW_TRACES").is_some_and(|t| t == "1") {
        let traces = hardware::traces(model);
        floor.root.add(&traces);
        let pulses = hardware::Pulses::new(model);
        floor.root.add(&pulses.obj);
        Some(pulses)
    } else {
        None
    };
    floor.statics_dirty = true; // the model's static parts are baked
    let (lo, hi) = bounds(model, &params);
    floor.fit_sphere(((lo + hi) / 2.0).to_array(), (hi - lo).length() / 2.0 + 0.5, 1.5);
    let mut lab = Lab { built, pulses, state };
    lab.step(0.0, 0.0);
    Some(lab)
}

/// The web hardware lab's stage (hardware-lab.ts): its night sky (the floor's dome), no plaza scenery, a flat ground of
/// 0x1d2140 under a 120-unit grid (three's GridHelper: the centre lines 0x3b4566, the rest 0x242a48), its lights.
fn night_stage(floor: &mut Floor) {
    use crate::scene::{Color, Mat, Obj};
    use std::f32::consts::FRAC_PI_2;
    floor.stars.set_visible(false);
    floor.planet.set_visible(false);
    floor.dust.set_visible(false);
    floor.backdrop.set_visible(false);
    floor.ground.set_visible(false);
    let ground = Obj::mesh(&crate::geometry::plane(800.0, 800.0, 1, 1), &crate::toon::toon_mat(crate::scene::Material { color: Color::hex(0x1d2140), ..Default::default() }));
    ground.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, -0.01, 0.0).set_shadows(false, true);
    floor.root.add(&ground);
    let (centre, line) = (Mat::basic(Color::hex(0x3b4566)), Mat::basic(Color::hex(0x242a48)));
    let grid = Obj::group();
    for k in 0..=120 {
        let at = k as f32 - 60.0;
        let mat = if k == 60 { &centre } else { &line };
        let along_x = Obj::mesh(&crate::geometry::plane(120.0, 0.05, 1, 1), mat);
        along_x.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, 0.002, at);
        let along_z = Obj::mesh(&crate::geometry::plane(0.05, 120.0, 1, 1), mat);
        along_z.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(at, 0.002, 0.0);
        grid.add(&along_x);
        grid.add(&along_z);
    }
    floor.root.add(&grid);
    floor.statics_dirty = true;
    let world = &mut floor.world;
    world.fog_near = 1.0e6;
    world.fog_far = 2.0e6;
    world.hemi_sky = Color::hex(0x9fb0ff);
    world.hemi_ground = Color::hex(0x2a1d52);
    world.hemi_intensity = 1.15;
    world.sun_color = Color::hex(0xd2ddff);
    world.sun_intensity = 2.0;
}

impl Lab {
    /// Advances the model by `dt` seconds (the fans turn by it) at the ambient clock `clock` in seconds (the LEDs and
    /// the pulses read it).
    pub fn step(&mut self, dt: f32, clock: f32) {
        self.built.update(dt, clock, &self.state);
        if let Some(pulses) = self.pulses.as_mut() {
            pulses.update(dt, self.state.activity);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stage::Stage;
    use std::collections::BTreeMap;

    #[test]
    fn the_stage_builds_the_lab_when_its_model_is_named() {
        let mut vars = BTreeMap::new();
        vars.insert("APZ_HW_LAB".to_string(), "board/workstation".to_string());
        vars.insert("APZ_HW_GPUS".to_string(), "1".to_string());
        crate::set_env_overrides(vars);
        let stage = Stage::default();
        crate::set_env_overrides(BTreeMap::new());
        assert!(stage.hardware_lab.is_some());
    }
}
