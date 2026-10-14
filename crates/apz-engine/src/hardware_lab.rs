//! The hardware lab (`APZ_HW_LAB`), as the web's `#/hardware-lab` on the engine: one hardware model on the machines'
//! ground under the floor's light, its state set by the env. Env, all optional except the first:
//!   APZ_HW_LAB      a model id, e.g. board/workstation or exterior/workstation
//!   APZ_HW_GPUS     the GPUs, 0..2 (default 0)
//!   APZ_HW_STATUS   online | draining | offline | revoked (default online)
//!   APZ_HW_WORKING  how many agents are busy, 0..6 (default 0): the activity is min(1, working / 4)
//!   APZ_HW_OPEN     how far it is opened, 0..1 (default 0)
//!   APZ_HW_TRACES   1 shows the copper traces and their pulses (default 0)
//!   APZ_HW_CAM      iso (default) | top | front | side | anchor (the model's own anchor view, when it has one)
//!   APZ_HW_PARAMS   the model's params, `name=value` pairs joined by commas (any numeric key the model reads; gpus
//!                   is APZ_HW_GPUS)
//! The readout (`Lab::info`) is the model's validation: its errors, or "<model>: no errors".

use glam::Vec3;
use serde_json::{json, Value};

use super::controls::FOV_DEG;
use super::hardware::{self, Params, State, Status};
use super::props::Anim;
use super::scene::Obj;
use super::world::Floor;

/// The model on the machines' ground: its state, its traces and pulses when shown, and its atom when it has an anchor.
pub struct Lab {
    built: hardware::Built,
    pulses: Option<hardware::Pulses>,
    atom: Option<Atom>,
    exterior: bool,
    state: State,
    info: Value,
}

/// The plaza's atom under the model's root: its node (for the exterior's glow) and its animation, called each frame.
struct Atom {
    node: Obj,
    anim: Anim,
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

/// The model's params: the GPUs (APZ_HW_GPUS), then each `name=value` of APZ_HW_PARAMS. A pair that does not read as a
/// finite number is left out, and `gpus` is the GPUs' own.
fn params_of(gpus: f32, extra: Option<&str>) -> Params {
    let mut params = Params::default().with("gpus", f64::from(gpus));
    for pair in extra.unwrap_or("").split(',') {
        let Some((name, value)) = pair.split_once('=') else { continue };
        let name = name.trim();
        if name.is_empty() || name == "gpus" {
            continue;
        }
        if let Some(v) = value.trim().parse::<f64>().ok().filter(|v| v.is_finite()) {
            params = params.with(name, v);
        }
    }
    params
}

/// The camera's direction from the model's centre, as the web's frameSphere names it (iso is the default).
fn direction(cam: &str) -> Vec3 {
    match cam {
        "top" => Vec3::new(0.0, 1.0, 0.0001),
        "front" => Vec3::Z,
        "side" => Vec3::X,
        _ => Vec3::new(0.8, 0.6, 1.0),
    }
}

/// The camera's eye and target for `cam`: the model's anchor view when `anchor` names one, else the preset framed on the
/// model's size (its half-diagonal, 40 degrees of field, 0.9 of the frame).
fn view(model: &hardware::Model, cam: &str) -> ([f32; 3], [f32; 3]) {
    if cam == "anchor" {
        if let Some(c) = model.anchors.camera {
            return (c.at, c.look);
        }
    }
    let [w, h, d] = model.size;
    let centre = Vec3::new(0.0, h / 2.0, 0.0);
    let radius = Vec3::new(w, h, d).length() / 2.0;
    let dist = radius.max(1.0) / (FOV_DEG / 2.0).to_radians().sin() * 0.9;
    let eye = centre + direction(cam).normalize() * dist;
    (eye.to_array(), centre.to_array())
}

/// The model, when `APZ_HW_LAB` names one: it joins the floor's root at the origin, and the camera frames it. None
/// otherwise (or for an unknown model), so the floor is then the machines' own view.
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let name = var("APZ_HW_LAB")?;
    let model = hardware::model(&name)?;
    let params = params_of(number("APZ_HW_GPUS", 0.0, 2.0, 0.0).round(), var("APZ_HW_PARAMS").as_deref());
    let state = State {
        status: status(var("APZ_HW_STATUS")),
        activity: (number("APZ_HW_WORKING", 0.0, 6.0, 0.0) / 4.0).min(1.0),
        open: number("APZ_HW_OPEN", 0.0, 1.0, 0.0),
    };
    night_stage(floor);
    let built = hardware::build(model, &params);
    // hardware-lab.ts placed the atom under the model's group with the Floor's own colours (ATOM_PALETTE): the same here,
    // as the layout's boards do. Its node is the child place_atom appends (layout.rs reads it the same way).
    let before = built.root.get().children.len();
    let anim = hardware::place_atom(model, &built.root, &floor.fleet_palette());
    let node = built.root.get().children.get(before).cloned();
    let atom = match (anim, node) {
        (Some(anim), Some(node)) => Some(Atom { node, anim }),
        _ => None,
    };
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
    let (eye, target) = view(model, &var("APZ_HW_CAM").unwrap_or_else(|| "iso".to_string()));
    floor.orbit.look(eye.into(), target.into());
    // hardware-lab.ts's OrbitControls: any polar angle, a distance from 0.5 to 1000.
    floor.orbit.unlimited(0.5, 1000.0);
    let errors = hardware::validate(model, &name);
    let summary = if errors.is_empty() { format!("{name}: no errors") } else { errors.join("\n") };
    let info = json!({ "kind": "hardware", "model": name, "errors": errors, "summary": summary });
    let exterior = model.view == hardware::View::Exterior;
    let mut lab = Lab { built, pulses, atom, exterior, state, info };
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
        // three's GridHelper is lines: they neither cast nor take a shadow.
        along_x.set_shadows(false, false);
        along_z.set_shadows(false, false);
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
    // hardware-lab.ts's sun shadow camera: 60 each way, near 1, far 200.
    world.shadow_extent = 60.0;
    world.shadow_near = 1.0;
    world.shadow_far = 200.0;
    // hardware-lab.ts drew through the perf meter: the shadow map every third frame.
    world.shadow_every = 3;
}

impl Lab {
    /// The readout the page shows: the model's validation (its errors, or "no errors"), as JSON.
    pub fn info(&self) -> &Value {
        &self.info
    }

    /// Advances the model by `dt` seconds (the fans turn by it) at the ambient clock `clock` in seconds (the LEDs and
    /// the pulses read it).
    pub fn step(&mut self, dt: f32, clock: f32) {
        self.built.update(dt, clock, &self.state);
        if let Some(pulses) = self.pulses.as_mut() {
            pulses.update(dt, self.state.activity);
        }
        if let Some(atom) = self.atom.as_mut() {
            (atom.anim)(clock);
            // An exterior's atom glows through its window only while the machine is online (hardware-lab.ts).
            atom.node.set_visible(!self.exterior || self.state.status == Status::Online);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn the_params_read_as_name_value_pairs_and_the_gpus_are_their_own() {
        let p = params_of(1.0, Some("amd=1,studio=0.5,gpus=2,bad,x=NaN, pcb_top = 0.336"));
        assert_eq!(p.values.get("gpus"), Some(&1.0));
        assert_eq!(p.values.get("amd"), Some(&1.0));
        assert_eq!(p.values.get("studio"), Some(&0.5));
        assert_eq!(p.values.get("pcb_top"), Some(&0.336));
        assert!(!p.values.contains_key("x") && !p.values.contains_key("bad"));
    }

    #[test]
    fn the_camera_presets_look_at_the_model_from_their_side() {
        let model = hardware::model("exterior/workstation").expect("the model is embedded");
        let (front, target) = view(model, "front");
        assert!(front[2] > 0.0 && front[0].abs() < 1e-4, "front looks from +z: {front:?}");
        assert!((target[1] - model.size[1] / 2.0).abs() < 1e-4);
        let (iso, _) = view(model, "iso");
        assert!(iso[0] > 0.0 && iso[1] > target[1] && iso[2] > 0.0, "iso is up and to the front-right: {iso:?}");
        let (top, _) = view(model, "top");
        assert!(top[1] > front[1], "top is above the front view");
    }

    #[test]
    fn the_readout_is_the_models_validation() {
        let name = "board/workstation";
        let mut floor = Floor::new();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("APZ_HW_LAB".to_string(), name.to_string());
        vars.insert("APZ_HW_CAM".to_string(), "anchor".to_string());
        vars.insert("APZ_HW_PARAMS".to_string(), "amd=1,studio=0".to_string());
        crate::set_env_overrides(vars);
        let lab = build(&mut floor);
        crate::set_env_overrides(std::collections::BTreeMap::new());
        let lab = lab.expect("the lab builds");
        let want = hardware::validate(hardware::model(name).expect("embedded"), name);
        assert_eq!(lab.info()["errors"], json!(want));
        assert_eq!(lab.info()["kind"], "hardware");
    }

    #[test]
    fn the_lab_places_the_plaza_atom_at_the_models_anchor() {
        let name = "board/workstation";
        assert!(hardware::model(name).expect("embedded").anchors.atom.is_some(), "the board has an atom anchor");
        let mut floor = Floor::new();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("APZ_HW_LAB".to_string(), name.to_string());
        crate::set_env_overrides(vars);
        let lab = build(&mut floor);
        crate::set_env_overrides(std::collections::BTreeMap::new());
        let lab = lab.expect("the lab builds");
        let node = lab.atom.as_ref().expect("the atom is placed at the anchor").node.clone();
        let mut under_root = false;
        floor.root.traverse(&mut |o: &Obj| under_root |= *o == node);
        assert!(under_root, "the atom's node is under the lab's scene root");
    }
}
