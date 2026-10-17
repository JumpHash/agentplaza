//! The hardware lab (`APZ_HW_LAB`): the machine model kit on the machines' ground, one model or the whole line-up, in a
//! style, at a level of detail and with its status light set by the env. Env, all optional except the first:
//!   APZ_HW_LAB      a kit model id (`hub`, `server`, `rack`, `gpu`, `tower`, `edge`, `mini`, `studio`, `laptop`,
//!                   `phone`), or `all` for every model in a row
//!   APZ_HW_LOD      the level of detail, 0 (full) to 2 (block); default 0
//!   APZ_HW_STYLE    ink | blueprint | night (default ink)
//!   APZ_HW_THEME    light | dark (default dark)
//!   APZ_HW_STATUS   working | idle | strained | offline (default working)
//!   APZ_HW_CAM      iso (default) | front | side | back | top
//!   APZ_HW_ORTHO    1 draws with the orthographic camera (the 2D views); default perspective
//! The readout (`Lab::info`) names the models shown, with their sizes, the level and the style.

use glam::Vec3;
use serde_json::{Value, json};

use super::controls::FOV_DEG;
use super::fleet::{FleetInstances, MACHINE_SLOTS, rgba};
use super::kit::{self, KitModel};
use super::style::{Lamp, Palette, Style};
use super::world::Floor;

/// The gap between two models of the line-up, in kit units.
const GAP: f32 = 0.8;
/// The fans' turn rate, radians per second (the fleet's idle rate).
const FAN_RATE: f32 = std::f32::consts::TAU * 2.0;

/// The models on the machines' ground, each bound to its own row of the GPU tables, with their lamp.
pub struct Lab {
    count: usize,
    lamp: Lamp,
    palette: Palette,
    fan_angle: f32,
    info: Value,
}

fn var(name: &str) -> Option<String> {
    crate::env_var(name).filter(|v| !v.is_empty())
}

/// The lamp a status name sets (working when unset or unknown).
fn lamp_of(s: Option<&str>) -> Lamp {
    match s {
        Some("idle") => Lamp::Idle,
        Some("strained") => Lamp::Strained,
        Some("offline") => Lamp::Offline,
        _ => Lamp::Working,
    }
}

/// The camera's direction from the line-up's centre (iso is the default).
fn direction(cam: &str) -> Vec3 {
    match cam {
        "top" => Vec3::new(0.0, 1.0, 0.0001),
        "front" => Vec3::Z,
        "back" => Vec3::NEG_Z,
        "side" => Vec3::X,
        _ => Vec3::new(0.8, 0.6, 1.0),
    }
}

/// The models `APZ_HW_LAB` names: one by id, or every model for `all`. Empty for an unknown id.
fn chosen(name: &str) -> Vec<&'static KitModel> {
    if name == "all" {
        kit::models().iter().collect()
    } else {
        kit::model(name).into_iter().collect()
    }
}

/// The lab, when `APZ_HW_LAB` names a kit model or `all`: the models stand in a row along x on the floor's ground, in
/// the palette of `APZ_HW_STYLE` and `APZ_HW_THEME`, and the camera frames the row. None otherwise (or for an unknown
/// model), so the floor is then the machines' own view.
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let name = var("APZ_HW_LAB")?;
    let models = chosen(&name);
    if models.is_empty() {
        return None;
    }
    let lod = var("APZ_HW_LOD")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(2);
    let style = var("APZ_HW_STYLE")
        .as_deref()
        .and_then(Style::parse)
        .unwrap_or_default();
    let dark = var("APZ_HW_THEME").as_deref() != Some("light");
    let palette = Palette::of(style, dark);
    floor.apply_palette(&palette);

    let width: f32 = models.iter().map(|m| m.size.x).sum::<f32>() + GAP * (models.len() - 1) as f32;
    let mut x = -width / 2.0;
    let mut height: f32 = 0.0;
    let mut depth: f32 = 0.0;
    let mut shown = Vec::new();
    for (slot, model) in models.iter().enumerate().take(MACHINE_SLOTS) {
        let inst = model.instantiate(lod, &palette);
        kit::set_machine(&inst.root, slot);
        inst.root.set_position(x + model.size.x / 2.0, 0.0, 0.0);
        floor.root.add(&inst.root);
        x += model.size.x + GAP;
        height = height.max(model.size.y);
        depth = depth.max(model.size.z);
        shown.push(json!({ "id": model.id, "kind": model.kind, "size": model.size.to_array() }));
    }
    floor.statics_dirty = true;

    let cam = var("APZ_HW_CAM").unwrap_or_else(|| "iso".to_string());
    let centre = Vec3::new(0.0, height / 2.0, 0.0);
    let radius = Vec3::new(width, height, depth).length() / 2.0;
    let dist = radius.max(1.0) / (FOV_DEG / 2.0).to_radians().sin() * 0.9;
    floor
        .orbit
        .look(centre + direction(&cam).normalize() * dist, centre);
    floor.orbit.unlimited(0.5, 1000.0);
    floor.orbit.ortho = var("APZ_HW_ORTHO").as_deref() == Some("1");

    let info = json!({
        "kind": "hardware",
        "models": shown,
        "lod": lod,
        "style": style.name(),
        "dark": dark,
        "summary": format!("{name}: {} model(s), lod{lod}, {}", models.len(), style.name()),
    });
    Some(Lab {
        count: models.len().min(MACHINE_SLOTS),
        lamp: lamp_of(var("APZ_HW_STATUS").as_deref()),
        palette,
        fan_angle: 0.0,
        info,
    })
}

impl Lab {
    /// The readout the page shows: the models, the level and the style, as JSON.
    pub fn info(&self) -> &Value {
        &self.info
    }

    /// Advances the fans by `dt` seconds.
    pub fn step(&mut self, dt: f32, _clock: f32) {
        self.fan_angle = (self.fan_angle + FAN_RATE * dt) % std::f32::consts::TAU;
    }

    /// The GPU tables for the models shown: each one's row has the fan angle, the busy flag and the lamp's colour.
    pub fn instances(&self) -> FleetInstances {
        let mut out = FleetInstances {
            machine_angles: vec![0.0; MACHINE_SLOTS],
            machine_states: vec![[0.0; 4]; MACHINE_SLOTS],
            machine_statuses: vec![[0.0; 4]; MACHINE_SLOTS],
        };
        let busy = if self.lamp == Lamp::Working { 1.0 } else { 0.0 };
        for slot in 0..self.count {
            out.machine_angles[slot] = self.fan_angle;
            out.machine_states[slot] = [busy, busy, 0.0, 0.0];
            out.machine_statuses[slot] = rgba(self.palette.lamp(self.lamp));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lab_with(vars: &[(&str, &str)]) -> Option<Lab> {
        let mut floor = Floor::machines();
        crate::set_env_overrides(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        let lab = build(&mut floor);
        crate::set_env_overrides(std::collections::BTreeMap::new());
        lab
    }

    #[test]
    fn the_lab_shows_one_model_or_the_whole_kit() {
        let one = lab_with(&[("APZ_HW_LAB", "server"), ("APZ_HW_LOD", "1")]).expect("the server");
        assert_eq!(one.info()["models"].as_array().unwrap().len(), 1);
        assert_eq!(one.info()["lod"], 1);
        let all =
            lab_with(&[("APZ_HW_LAB", "all"), ("APZ_HW_STYLE", "blueprint")]).expect("the kit");
        assert_eq!(
            all.info()["models"].as_array().unwrap().len(),
            kit::models().len()
        );
        assert_eq!(all.info()["style"], "blueprint");
        assert!(lab_with(&[("APZ_HW_LAB", "toaster")]).is_none());
    }

    #[test]
    fn each_model_lights_its_own_row_in_the_lamp_colour() {
        let lab =
            lab_with(&[("APZ_HW_LAB", "all"), ("APZ_HW_STATUS", "offline")]).expect("the kit");
        let inst = lab.instances();
        let offline = rgba(Palette::of(Style::default(), true).lamp(Lamp::Offline));
        for slot in 0..kit::models().len() {
            assert_eq!(inst.machine_statuses[slot], offline);
            assert_eq!(
                inst.machine_states[slot][0], 0.0,
                "an offline machine is not busy"
            );
        }
        assert_eq!(inst.machine_statuses[kit::models().len()], [0.0; 4]);
    }
}
