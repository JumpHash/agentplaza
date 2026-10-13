//! The machine stage: one machine's exterior on the studio stage, the view the web's Machines list morphs into when a
//! row expands (`apps/web/src/views/floor3d/machine-stage.ts`). The model is the floor's exterior for the machine's
//! look, configured from the machine: its GPU cards (count and vendor tint), its memory sticks and, where the model
//! has them, its storage drives. The camera orbits the machine: it turns by itself until the first input, then the
//! user drives it. The look and param rules are the web's, so a morph starts from the same picture.
//!
//! TODO(thumbnail): `Engine::thumbnail(id, w, h) -> Vec<u8>` (the stage pose rendered offscreen, read back as RGBA) is
//! not built. It needs an offscreen colour target and a buffer readback on the host's device.
//! TODO(tint): `Picture::tints` holds each shown card's vendor tint and is tested, but the tint is not yet applied to
//! the model's shroud material. That needs a palette override in `hardware::build` (the web's `retint` step).
//! TODO(shadow): the web's shadow catcher (a transparent plane under the machine) is not drawn. The sun's shadow map
//! is the only ground shadow.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use glam::Vec3;
use web_time::Instant;

use crate::controls::Orbit;
use crate::fleet::{status_of, Machine};
use crate::gpus::{self, GpuInput, Match};
use crate::hardware::{self, Built, Model, Params, State};
use crate::scene::{self, Camera, Color, Draw, Filter, Frame, Obj, PointLightDraw, World};
use crate::stage::{Motion, Stage};

/// The stage camera's vertical field of view, degrees (the web's stage: FOV 30).
pub const FOV_DEG: f32 = 30.0;
/// The share of the view the machine fills, by height or width.
pub const FILL: f32 = 0.7;
/// The auto-rotate speed, radians per second: one turn in 30 s (three's autoRotateSpeed 2.0).
pub const SPIN_RATE: f32 = std::f32::consts::TAU / 30.0;
/// How far the orbit pose looks down onto the machine (the web's `high` vector).
const ELEVATION: f32 = 0.12;

/// The picture a machine is drawn as (the web's `Look`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Workstation,
    Server,
    Laptop,
    Edge,
    Phone,
    Macbook,
    Mac,
}

impl Look {
    /// The model file for the look (the web's `EXTERIOR`).
    pub fn exterior(self) -> &'static str {
        match self {
            Look::Macbook => "exterior/apple-laptop",
            Look::Mac => "exterior/apple-desktop",
            Look::Laptop => "exterior/laptop",
            Look::Edge => "exterior/edge",
            Look::Phone => "exterior/phone",
            Look::Server => "exterior/server",
            Look::Workstation => "exterior/workstation",
        }
    }

    /// The GPU cards the look draws at most: the workstation's two, the server's four 2x2 cards, the laptop's one chip.
    fn card_cap(self) -> usize {
        match self {
            Look::Workstation => 2,
            Look::Server => 4,
            Look::Laptop => 1,
            _ => 0,
        }
    }

    /// The DIMM slots the look's model has (its `dimms` param is the stick count).
    fn memory_slots(self) -> u32 {
        match self {
            Look::Workstation | Look::Laptop => 4,
            Look::Server => 16,
            _ => 0,
        }
    }
}

/// The look of a machine of `kind` made by `vendor`. Apple's laptops are MacBooks, its other machines Mac minis and
/// Studios. Unknown kinds draw as a workstation, as `hardware::models_for` does.
pub fn look_of(kind: &str, vendor: Option<&str>) -> Look {
    let apple = vendor == Some("apple");
    match (kind, apple) {
        ("phone", _) => Look::Phone,
        ("laptop", true) => Look::Macbook,
        (_, true) => Look::Mac,
        ("laptop", false) => Look::Laptop,
        ("edge", false) => Look::Edge,
        ("server", false) => Look::Server,
        _ => Look::Workstation,
    }
}

/// The GPU shroud colour per vendor (the web's `CARD_TINT`). Other vendors keep the palette's shroud.
fn card_tint(vendor: &str) -> Option<u32> {
    match vendor.to_ascii_lowercase().as_str() {
        "nvidia" => Some(0x76b900),
        "amd" => Some(0xc8102e),
        "intel" => Some(0x0068b5),
        _ => None,
    }
}

/// The sticks for `memory_mib` of RAM over `slots`: a stick is a power of two of at least 8 GiB, sized to fill the
/// slots evenly (the web's `sticksFor`).
pub fn sticks_for(memory_mib: u64, slots: u32) -> u32 {
    let gib = memory_mib as f64 / 1024.0;
    let per_slot = (gib / slots as f64).ceil();
    let mut p: u64 = 1;
    while (p as f64) < per_slot {
        p *= 2;
    }
    let stick = p.max(8) as f64;
    ((gib / stick).round() as u32).clamp(1, slots)
}

/// The storage drives a workstation shows: one when the count is unknown, at most two (the web's `drivesFor`).
fn drives_for(drives: Option<u32>) -> u32 {
    drives.map_or(1, |d| d.min(2))
}

/// The fields of a machine's picture that change its model's params.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub look: Look,
    /// The vendor tint of each card shown, in card order (None: the palette's shroud).
    pub tints: Vec<Option<u32>>,
    pub sticks: u32,
    pub drives: u32,
}

/// A machine's picture: its look, its shown cards and their tints, its sticks and its drives.
pub fn picture(machine: &Machine, drives: Option<u32>) -> Picture {
    let look = look_of(&machine.kind, machine.vendor.as_deref());
    let tints = machine.gpus.iter().take(look.card_cap()).map(|g| card_tint(&g.vendor)).collect();
    let slots = look.memory_slots();
    Picture {
        look,
        tints,
        sticks: if slots > 0 { sticks_for(machine.memory_mib, slots) } else { 0 },
        drives: if look == Look::Workstation { drives_for(drives) } else { 0 },
    }
}

/// The hardware params of a picture (the web's `assemble`): `gpus` when the model has cards, `dimms` when it has
/// slots, `drives` for a workstation. Other params keep the model's defaults.
pub fn params_of(pic: &Picture) -> Params {
    let mut params = Params::default();
    if pic.look.card_cap() > 0 {
        params = params.with("gpus", pic.tints.len() as f64);
    }
    if pic.look.memory_slots() > 0 {
        params = params.with("dimms", pic.sticks as f64);
    }
    if pic.look == Look::Workstation {
        params = params.with("drives", pic.drives as f64);
    }
    params
}

/// Each detected GPU matched to its card (`gpus::match_card`), in the machine's GPU order.
pub fn cards_of(machine: &Machine) -> Vec<Option<Match>> {
    machine
        .gpus
        .iter()
        .map(|g| {
            let input = GpuInput { vendor: g.vendor.clone(), name: g.name.clone(), memory_mib: g.memory_mib.map(|m| m as f64) };
            gpus::match_card(&input, Some(&machine.kind))
        })
        .collect()
}

/// The studio lights (the web's `makeScene`): a warm hemisphere light and a warm key light from above and to the front.
fn studio_world() -> World {
    World {
        hemi_sky: Color::hex(0xfff8ec),
        hemi_ground: Color::hex(0x9c8f75),
        hemi_intensity: 1.4,
        sun_color: Color::hex(0xfff1d6),
        sun_intensity: 2.4,
        sun_position: Vec3::new(2.5, 4.0, 3.0),
        fog_near: 1.0e6,
        fog_far: 1.0e6 + 1.0,
        ..World::default()
    }
}

/// Each stage's statics get their own version, so the renderer never reuses one machine's baked draws for another.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

/// One machine on the stage: its built exterior, its baked statics and its orbit camera.
pub struct MachineStage {
    pub id: String,
    pub picture: Picture,
    /// The GPU card each detected GPU matched (None: no rule matched).
    pub cards: Vec<Option<Match>>,
    /// The model's size, in model units.
    pub size: [f32; 3],
    pub orbit: Orbit,
    center: Vec3,
    dir: Vec3,
    aspect: f32,
    /// Auto-rotate runs until the first input.
    auto: bool,
    /// The user has orbited or zoomed: the view is no longer refitted on resize.
    touched: bool,
    time: f32,
    statics: Arc<Vec<Draw>>,
    statics_version: u64,
    lights: Vec<PointLightDraw>,
    _built: Built,
}

impl MachineStage {
    /// The stage for `machine`, built from its look's model with its params. Null only when the model is missing.
    pub fn new(machine: &Machine, drives: Option<u32>) -> Option<MachineStage> {
        let pic = picture(machine, drives);
        let model: &'static Model = hardware::model(pic.look.exterior()).or_else(|| hardware::model("exterior/workstation"))?;
        let mut built = hardware::build(model, &params_of(&pic));
        built.freeze_cutaways(); // no fades on the stage
        built.update(0.0, 0.0, &State { status: status_of(&machine.status), activity: 0.0, open: 0.0 });

        let root = Obj::group();
        root.add(&built.root);
        let statics = Arc::new(scene::bake(&root));
        let lights = scene::flatten(&root, Filter::Static).1;

        let size = model.size;
        let center = Vec3::new(0.0, size[1] / 2.0, 0.0);
        let dir = model
            .anchors
            .camera
            .map(|c| (Vec3::from(c.at) - Vec3::from(c.look)).normalize_or_zero())
            .filter(|d| *d != Vec3::ZERO)
            .unwrap_or(Vec3::Z);
        let mut stage = MachineStage {
            id: machine.id.clone(),
            picture: pic,
            cards: cards_of(machine),
            size,
            orbit: Orbit::default(),
            center,
            dir,
            aspect: 1.0,
            auto: true,
            touched: false,
            time: 0.0,
            statics,
            statics_version: NEXT_VERSION.fetch_add(1, Ordering::Relaxed),
            lights,
            _built: built,
        };
        stage.fit(1.0);
        Some(stage)
    }

    /// Frames the machine so it fills [`FILL`] of the view, from the camera's pose (the web's `frameOf`, orbit pose).
    fn fit(&mut self, aspect: f32) {
        self.aspect = aspect;
        let tan = (FOV_DEG.to_radians() / 2.0).tan();
        let [sx, sy, sz] = self.size;
        let by_height = sy / 2.0 / (FILL * tan);
        let by_width = sx / 2.0 / (FILL * tan * aspect.max(1e-3));
        let dist = by_height.max(by_width) + sz / 2.0;
        let high = (self.dir + Vec3::Y * ELEVATION).normalize();
        self.orbit.look(self.center + high * dist, self.center);
        self.orbit.min_distance = dist * 0.5;
        self.orbit.max_distance = dist * 1.6;
    }

    /// Steps the camera by `dt` seconds and returns the motion it needs: [`Motion::Active`] while the user's drag is
    /// still damping, [`Motion::Ambient`] while it auto-rotates, [`Motion::Rest`] otherwise. `reduced` (the platform's
    /// reduced-motion preference) holds the spin.
    pub fn step(&mut self, dt: f32, aspect: f32, reduced: bool) -> Motion {
        if !self.touched && (aspect - self.aspect).abs() > 1e-4 {
            self.fit(aspect);
        }
        self.time += dt;
        let spinning = self.auto && !reduced;
        if spinning {
            self.orbit.theta += SPIN_RATE * dt;
        }
        if self.orbit.update() {
            Motion::Active
        } else if spinning {
            Motion::Ambient
        } else {
            Motion::Rest
        }
    }

    /// A drag or zoom: the spin stops for good and the user drives the camera.
    pub fn rotate(&mut self, dx: f32, dy: f32, height: f32) {
        self.touch();
        self.orbit.rotate(dx, dy, height);
    }

    pub fn dolly(&mut self, steps: f32) {
        self.touch();
        self.orbit.dolly(steps);
    }

    fn touch(&mut self) {
        self.auto = false;
        self.touched = true;
    }

    /// The stage camera (FOV 30, the orbit's pose).
    pub fn camera(&self) -> Camera {
        let mut camera = self.orbit.camera(self.aspect);
        camera.fov = FOV_DEG;
        camera
    }

    /// The frame for the renderer: the baked statics and the studio lights, with no dynamic draws.
    pub fn frame(&self) -> Frame {
        Frame {
            camera: self.camera(),
            world: studio_world(),
            statics: Arc::clone(&self.statics),
            statics_version: self.statics_version,
            draws: Vec::new(),
            lights: self.lights.clone(),
            time: self.time,
            overlays: Vec::new(),
            lod: None,
            fades: Arc::new(Vec::new()), // no cutaways on the stage
            focus: None,
        }
    }
}

impl Stage {
    /// Shows one machine of the current feed on the stage (`Cmd::ShowMachine`). False when the feed has no such machine.
    pub fn show_machine(&mut self, id: &str) -> bool {
        let machine = self.fleet_data.as_ref().and_then(|f| f.machines.iter().find(|m| m.id == id));
        match machine.and_then(|m| MachineStage::new(m, None)) {
            Some(stage) => {
                self.machine = Some(stage);
                true
            }
            None => false,
        }
    }

    /// Returns from the machine stage to the view underneath (`Cmd::LeaveMachine`).
    pub fn leave_machine(&mut self) {
        self.machine = None;
    }

    /// One tick of the machine stage: steps its camera and builds its frame.
    pub(crate) fn tick_machine(&mut self, now: Instant) -> Motion {
        let dt = self.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32()).min(0.05);
        self.last = Some(now);
        let (w, h) = self.size;
        let aspect = if h > 0.0 { w / h } else { 1.0 };
        let reduced = self.reduced_motion;
        let Some(m) = self.machine.as_mut() else { return Motion::Rest };
        let motion = m.step(dt, aspect, reduced);
        self.frame = Arc::new(m.frame());
        motion
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::FleetResponse;
    use serde_json::json;

    fn fixture() -> FleetResponse {
        serde_json::from_str(include_str!("../testdata/floor-15.json")).expect("the fixture parses")
    }

    fn machine(json: serde_json::Value) -> Machine {
        serde_json::from_value(json).expect("a machine")
    }

    fn gpus(n: usize, vendor: &str) -> Vec<serde_json::Value> {
        (0..n).map(|_| json!({ "vendor": vendor, "name": "GPU" })).collect()
    }

    #[test]
    fn every_machine_in_the_fixture_builds_a_stage() {
        let fleet = fixture();
        assert!(!fleet.machines.is_empty());
        for m in &fleet.machines {
            assert!(MachineStage::new(m, None).is_some(), "no stage for {}", m.id);
        }
    }

    #[test]
    fn a_four_gpu_workstation_shows_two_tinted_cards_and_four_sticks() {
        let m = machine(json!({
            "id": "w", "name": "w", "kind": "workstation", "vendor": "dell",
            "memory_mib": 65536, "gpus": gpus(4, "NVIDIA")
        }));
        let pic = picture(&m, None);
        assert_eq!(pic.look, Look::Workstation);
        assert_eq!(pic.tints, vec![Some(0x76b900), Some(0x76b900)]);
        let params = params_of(&pic);
        assert_eq!(params.values.get("gpus"), Some(&2.0));
        assert_eq!(params.values.get("dimms"), Some(&4.0));
        assert_eq!(params.values.get("drives"), Some(&1.0));
        assert_eq!(cards_of(&m).len(), 4);
        assert!(MachineStage::new(&m, None).is_some());
    }

    #[test]
    fn a_laptop_has_one_chip_and_its_sticks_and_no_drives() {
        let m = machine(json!({ "id": "l", "name": "l", "kind": "laptop", "vendor": "dell", "memory_mib": 16384 }));
        let params = params_of(&picture(&m, None));
        assert_eq!(params.values.get("gpus"), Some(&0.0));
        assert_eq!(params.values.get("dimms"), Some(&2.0));
        assert!(!params.values.contains_key("drives"));
    }

    #[test]
    fn an_apple_laptop_is_a_macbook_with_no_card_or_stick_params() {
        let m = machine(json!({ "id": "a", "name": "a", "kind": "laptop", "vendor": "apple", "memory_mib": 16384 }));
        let pic = picture(&m, None);
        assert_eq!(pic.look, Look::Macbook);
        assert!(params_of(&pic).values.is_empty());
        assert!(MachineStage::new(&m, None).is_some());
    }

    #[test]
    fn an_apple_desktop_is_a_mac_with_no_card_or_stick_params() {
        let m = machine(json!({ "id": "s", "name": "s", "kind": "workstation", "vendor": "apple", "memory_mib": 32768, "gpus": gpus(1, "Apple") }));
        let pic = picture(&m, None);
        assert_eq!(pic.look, Look::Mac);
        assert!(params_of(&pic).values.is_empty());
    }

    #[test]
    fn the_stick_count_fills_the_slots_with_power_of_two_sticks() {
        assert_eq!(sticks_for(65536, 4), 4); // 64 GiB: four 16 GiB sticks
        assert_eq!(sticks_for(16384, 4), 2); // 16 GiB: two 8 GiB sticks
        assert_eq!(sticks_for(262144, 16), 16); // 256 GiB over 16 slots: 16 x 16 GiB
    }

    #[test]
    fn the_stage_spins_as_ambient_until_the_first_input_then_the_user_drives() {
        let m = machine(json!({ "id": "l", "name": "l", "kind": "laptop", "vendor": "dell", "memory_mib": 16384 }));
        let mut stage = MachineStage::new(&m, None).expect("a stage");
        let theta = stage.orbit.theta;
        assert_eq!(stage.step(0.016, 1.5, false), Motion::Ambient);
        assert!(stage.orbit.theta > theta, "auto-rotate turns the camera");

        stage.rotate(40.0, 0.0, 400.0);
        assert_eq!(stage.step(0.016, 1.5, false), Motion::Active, "a drag is active while it damps");
        for _ in 0..400 {
            stage.step(0.05, 1.5, false);
        }
        let theta = stage.orbit.theta;
        assert_eq!(stage.step(0.5, 1.5, false), Motion::Rest, "a settled user view rests");
        assert_eq!(stage.orbit.theta, theta, "the spin stays off after the first input");
    }

    #[test]
    fn reduced_motion_holds_the_spin() {
        let m = machine(json!({ "id": "l", "name": "l", "kind": "laptop", "vendor": "dell", "memory_mib": 16384 }));
        let mut stage = MachineStage::new(&m, None).expect("a stage");
        assert_eq!(stage.step(0.5, 1.5, true), Motion::Rest);
    }
}
