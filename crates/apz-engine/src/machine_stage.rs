//! The machine stage: one machine's kit model (`crate::kit`, `fleet::kit_model_of`) on a plinth, in the current
//! style's palette, the view a machine opens into from the Machines grid. Its status light and fans run on the GPU from
//! the same tables the fleet uses, in one row of its own. The camera orbits the machine: it turns by itself until the
//! first input, then the user drives it.
//!
//! The thumbnail is `Renderer::thumbnail` (native) and `WebEngine::thumbnail` (the browser): this pose rendered
//! offscreen, with a shadow catcher under the machine.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use web_time::Instant;

use crate::controls::Orbit;
use crate::fleet::{FleetInstances, Machine, kit_model_of, lamp_of, rgba};
use crate::geometry;
use crate::kit;
use crate::scene::{self, Camera, Color, Draw, Filter, Frame, Mat, Obj, PointLightDraw, World};
use crate::stage::{Motion, Stage};
use crate::style::{Lamp, Palette};

/// The stage camera's vertical field of view, degrees (the web's stage: FOV 30).
pub const FOV_DEG: f32 = 30.0;
/// The share of the view the machine fills, by height or width.
pub const FILL: f32 = 0.7;
/// The share of a thumbnail's shorter side the machine fills (a card, so more than the stage's `FILL`).
pub const THUMB_FILL: f32 = 0.85;
/// The auto-rotate speed, radians per second: one turn in 30 s (three's autoRotateSpeed 2.0).
pub const SPIN_RATE: f32 = std::f32::consts::TAU / 30.0;
/// How far the orbit pose looks down onto the machine (the web's `high` vector).
const ELEVATION: f32 = 0.12;
/// The stage's machine binds its GPU animations to this row of the tables it hands the renderer.
const SLOT: usize = 0;
/// The fans' turn rate, radians per second (the fleet's idle rate).
const FAN_RATE: f32 = std::f32::consts::TAU * 2.0;
/// The plinth under the machine: this much wider and deeper than the model, this tall.
const PLINTH_MARGIN: f32 = 0.5;
const PLINTH_H: f32 = 0.18;

/// A drawing of the machine: the stage's three-quarter view, or one of the flat views of a technical drawing (an
/// orthographic camera square to a face: no perspective, so the parts keep their proportions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Iso,
    Front,
    Side,
    Back,
    Top,
}

impl View {
    /// The flat views, in the order a drawing sheet shows them.
    pub const FLAT: [View; 4] = [View::Front, View::Side, View::Back, View::Top];

    /// The view a host names: `iso`, `front`, `side`, `back` or `top`, any case.
    pub fn parse(name: &str) -> Option<View> {
        match name.trim().to_ascii_lowercase().as_str() {
            "iso" => Some(View::Iso),
            "front" => Some(View::Front),
            "side" => Some(View::Side),
            "back" => Some(View::Back),
            "top" => Some(View::Top),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Iso => "iso",
            View::Front => "front",
            View::Side => "side",
            View::Back => "back",
            View::Top => "top",
        }
    }

    /// From the machine's centre towards the eye of a flat view (the front faces +z; the side is the right-hand one;
    /// the top looks down with the front at the bottom of the picture). None for the three-quarter view.
    fn eye_dir(self) -> Option<Vec3> {
        match self {
            View::Iso => None,
            View::Front => Some(Vec3::Z),
            View::Side => Some(Vec3::X),
            View::Back => Some(Vec3::NEG_Z),
            View::Top => Some(Vec3::new(0.0, 1.0, 1.0e-4).normalize()),
        }
    }
}

/// The studio lights (the web's `makeScene`): a warm hemisphere light and a warm key light from above and to the front,
/// over the palette's flat background.
fn studio_world(palette: &Palette) -> World {
    World {
        background: palette.background,
        flat_background: true,
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

/// The web's shadow catcher (`makeScene`): a transparent 14 by 14 floor just above the ground at opacity 0.2 that shows
/// only the machine's shadow.
fn shadow_catcher() -> Draw {
    const HALF: f32 = 7.0;
    const LIFT: f32 = 0.002;
    let positions = vec![
        [-HALF, LIFT, -HALF],
        [HALF, LIFT, -HALF],
        [HALF, LIFT, HALF],
        [-HALF, LIFT, HALF],
    ];
    let normals = vec![[0.0, 1.0, 0.0]; 4];
    let geo = scene::Geometry::new(positions, normals, Vec::new(), vec![0, 2, 1, 0, 3, 2]);
    let material = scene::Material {
        shading: scene::Shading::Shadow,
        color: Color::BLACK,
        opacity: 0.2,
        transparent: true,
        depth_write: false,
        fog: false,
        ..scene::Material::default()
    };
    Draw {
        kind: scene::DrawKind::Mesh,
        geo,
        mat_id: scene::next_id(),
        material,
        model: Mat4::IDENTITY,
        cast_shadow: false,
        receive_shadow: true,
        render_order: 0,
        lod: 0,
        fade: 0,
        lift: 0,
        owner: 0,
    }
}

/// One machine on the stage: its kit model on a plinth in the style's palette, its baked statics and its orbit camera.
pub struct MachineStage {
    pub id: String,
    /// The kit model the machine is drawn as (`fleet::kit_model_of`).
    pub model: &'static str,
    /// The kit model's own size (width, height, depth, kit units of about 10 cm), without the plinth.
    pub model_size: [f32; 3],
    /// The machine and its plinth: width, height and depth.
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
    fan_angle: f32,
    lamp: Lamp,
    palette: Palette,
    statics: Arc<Vec<Draw>>,
    statics_version: u64,
    anim_batches: Arc<Vec<scene::AnimBatch>>,
    lights: Vec<PointLightDraw>,
}

impl MachineStage {
    /// The stage for `machine` in `palette`: its kit model's full detail on a plinth, its status light and fans driven
    /// like the fleet's. None only when the kit has no such model.
    pub fn new(machine: &Machine, palette: &Palette) -> Option<MachineStage> {
        let model_id = kit_model_of(machine);
        let model = kit::model(model_id)?;
        let inst = model.instantiate(0, palette);
        kit::set_machine(&inst.root, SLOT);

        let root = Obj::group();
        let (w, d) = (model.size.x + PLINTH_MARGIN, model.size.z + PLINTH_MARGIN);
        let plinth_mat = Mat::new(scene::Material {
            color: palette.plinth,
            shading: if palette.flat {
                scene::Shading::Basic
            } else {
                scene::Shading::Toon
            },
            ..scene::Material::default()
        });
        let plinth = Obj::mesh(&geometry::rounded_box(w, PLINTH_H, d, 2, 0.08), &plinth_mat);
        plinth
            .set_position(0.0, PLINTH_H / 2.0, 0.0)
            .set_shadows(false, true);
        root.add(&plinth);
        inst.root.set_position(0.0, PLINTH_H, 0.0);
        root.add(&inst.root);

        let baked = scene::bake_split(&root, scene::GPU_HW_ANIM);
        let mut statics = baked.statics;
        statics.push(shadow_catcher());
        let lights = scene::flatten(&root, Filter::Static).1;

        let size = [w, model.size.y + PLINTH_H, d];
        let center = Vec3::new(0.0, size[1] / 2.0, 0.0);
        // A three-quarter view from the front right.
        let dir = Vec3::new(0.55, 0.0, 1.0).normalize();
        let mut stage = MachineStage {
            id: machine.id.clone(),
            model: model_id,
            model_size: model.size.to_array(),
            size,
            orbit: Orbit::default(),
            center,
            dir,
            aspect: 1.0,
            auto: true,
            touched: false,
            time: 0.0,
            fan_angle: 0.0,
            lamp: lamp_of(machine),
            palette: palette.clone(),
            statics: Arc::new(statics),
            statics_version: scene::next_id(), // process-unique: the renderer never reuses another scene's bake
            anim_batches: Arc::new(baked.animated),
            lights,
        };
        stage.fit(1.0);
        Some(stage)
    }

    /// The machine's status light follows the feed: a new snapshot of the same machine.
    pub fn update(&mut self, machine: &Machine) {
        self.lamp = lamp_of(machine);
    }

    /// The GPU tables for the stage's one machine: its fan angle, its busy flag and its lamp colour in row [`SLOT`].
    fn instances(&self) -> FleetInstances {
        let mut out = FleetInstances::ZERO;
        let working = self.lamp == Lamp::Working;
        out.machine_angles[SLOT] = self.fan_angle;
        out.machine_states[SLOT] = [
            f32::from(u8::from(working)),
            f32::from(u8::from(working)),
            0.0,
            0.0,
        ];
        out.machine_statuses[SLOT] = rgba(self.palette.lamp(self.lamp));
        out
    }

    /// Frames the machine so it fills [`FILL`] of the view, from the camera's pose (the web's `frameOf`, orbit pose).
    fn fit(&mut self, aspect: f32) {
        self.aspect = aspect;
        let mut orbit = std::mem::take(&mut self.orbit);
        self.framed(&mut orbit, aspect);
        self.orbit = orbit;
    }

    /// Sets `orbit` to the framing of the machine at `aspect`.
    fn framed(&self, orbit: &mut Orbit, aspect: f32) {
        self.framed_at(orbit, aspect, FILL);
    }

    /// Frames the bounds to `fill` of the view's shorter side: the stage's `FILL`, or a thumbnail's `THUMB_FILL`.
    fn framed_at(&self, orbit: &mut Orbit, aspect: f32, fill: f32) {
        let tan = (FOV_DEG.to_radians() / 2.0).tan();
        let [sx, sy, sz] = self.size;
        let by_height = sy / 2.0 / (fill * tan);
        let by_width = sx / 2.0 / (fill * tan * aspect.max(1e-3));
        let dist = by_height.max(by_width) + sz / 2.0;
        let high = (self.dir + Vec3::Y * ELEVATION).normalize();
        orbit.look(self.center + high * dist, self.center);
        orbit.min_distance = dist * 0.5;
        orbit.max_distance = dist * 1.6;
    }

    /// Steps the camera by `dt` seconds and returns the motion it needs: [`Motion::Active`] while the user's drag is
    /// still damping, [`Motion::Ambient`] while it auto-rotates, [`Motion::Rest`] otherwise. `reduced` (the platform's
    /// reduced-motion preference) holds the spin.
    pub fn step(&mut self, dt: f32, aspect: f32, reduced: bool) -> Motion {
        if !self.touched && (aspect - self.aspect).abs() > 1e-4 {
            self.fit(aspect);
        }
        self.time += dt;
        if !reduced {
            self.fan_angle = (self.fan_angle + FAN_RATE * dt) % std::f32::consts::TAU;
        }
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
        self.camera_of(&self.orbit, self.aspect)
    }

    fn camera_of(&self, orbit: &Orbit, aspect: f32) -> Camera {
        let mut camera = orbit.camera(aspect);
        camera.fov = FOV_DEG;
        camera
    }

    /// The frame for the renderer: the baked statics and the studio lights, with no dynamic draws.
    pub fn frame(&self) -> Frame {
        self.frame_with(self.camera())
    }

    /// The thumbnail's frame at `aspect`: the start pose framed for that aspect (no spin, no user drive).
    pub fn thumbnail_frame(&self, aspect: f32) -> Frame {
        let mut orbit = Orbit::default();
        self.framed_at(&mut orbit, aspect, THUMB_FILL);
        self.frame_with(self.camera_of(&orbit, aspect))
    }

    /// The frame of `view` at `aspect`: the thumbnail's pose for [`View::Iso`]; for a flat view, an orthographic camera
    /// square to that face, the machine and its plinth filling [`THUMB_FILL`] of the picture by its tighter side.
    pub fn view_frame(&self, view: View, aspect: f32) -> Frame {
        let Some(dir) = view.eye_dir() else {
            return self.thumbnail_frame(aspect);
        };
        let right = Vec3::Y.cross(dir).normalize();
        let up = dir.cross(right);
        let half = Vec3::from(self.size) / 2.0;
        let (mut u, mut v) = (0.0f32, 0.0f32);
        for k in 0..8 {
            let corner = Vec3::new(
                if k & 1 == 0 { -half.x } else { half.x },
                if k & 2 == 0 { -half.y } else { half.y },
                if k & 4 == 0 { -half.z } else { half.z },
            );
            u = u.max(corner.dot(right).abs());
            v = v.max(corner.dot(up).abs());
        }
        let half_h = v.max(u / aspect.max(1e-3)) / THUMB_FILL;
        let dist = half_h / (FOV_DEG.to_radians() / 2.0).tan();
        let camera = Camera {
            eye: self.center + dir * dist,
            target: self.center,
            fov: FOV_DEG,
            near: 0.05,
            far: 1000.0,
            aspect,
            ortho: true,
        };
        self.frame_with(camera)
    }

    fn frame_with(&self, camera: Camera) -> Frame {
        Frame {
            animated: Some(Arc::new(self.instances())),
            camera,
            world: studio_world(&self.palette),
            statics: Arc::clone(&self.statics),
            statics_version: self.statics_version,
            draws: Vec::new(),
            lights: self.lights.clone(),
            time: self.time,
            overlays: Vec::new(),
            fades: Arc::new(Vec::new()), // no cutaways on the stage
            plaza: None,
            anim_batches: Arc::clone(&self.anim_batches),
            focus: None,
        }
    }
}

impl Stage {
    /// One machine of the current feed as a fresh stage at its start pose, or None when the feed has no such machine.
    pub fn machine_stage_for(&self, id: &str) -> Option<MachineStage> {
        let machine = self
            .fleet_data
            .as_ref()
            .and_then(|f| f.machines.iter().find(|m| m.id == id))?;
        MachineStage::new(machine, &self.palette())
    }

    /// The kit model one machine of the current feed draws as, and that model's size (width, height, depth in kit units
    /// of about 10 cm), or None when the feed has no such machine.
    pub fn machine_model(&self, id: &str) -> Option<(&'static str, [f32; 3])> {
        let machine = self
            .fleet_data
            .as_ref()
            .and_then(|f| f.machines.iter().find(|m| m.id == id))?;
        let model = kit::model(kit_model_of(machine))?;
        Some((kit_model_of(machine), model.size.to_array()))
    }

    /// Each GPU of one machine of the current feed with the reference card it matched (`gpus::card_summary`), in the
    /// machine's GPU order, or None when the feed has no such machine.
    pub fn machine_cards(&self, id: &str) -> Option<Vec<serde_json::Value>> {
        let machine = self
            .fleet_data
            .as_ref()
            .and_then(|f| f.machines.iter().find(|m| m.id == id))?;
        Some(
            machine
                .gpus
                .iter()
                .map(|g| {
                    let input = crate::gpus::GpuInput {
                        vendor: g.vendor.clone(),
                        name: g.name.clone(),
                        memory_mib: g.memory_mib.map(|m| m as f64),
                    };
                    crate::gpus::card_summary(&input, Some(&machine.kind))
                })
                .collect(),
        )
    }

    /// Shows one machine of the current feed on the stage (`Cmd::ShowMachine`). False when the feed has no such machine.
    pub fn show_machine(&mut self, id: &str) -> bool {
        match self.machine_stage_for(id) {
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
        let dt = self
            .last
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32())
            .min(0.05);
        self.last = Some(now);
        let (w, h) = self.size;
        let aspect = if h > 0.0 { w / h } else { 1.0 };
        let reduced = self.reduced_motion;
        let Some(m) = self.machine.as_mut() else {
            return Motion::Rest;
        };
        let motion = m.step(dt, aspect, reduced);
        self.frame = Arc::new(m.frame());
        motion
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Renderer, block_ready};
    use crate::fleet::FleetResponse;
    use crate::style::Style;
    use serde_json::json;

    fn fixture() -> FleetResponse {
        serde_json::from_str(include_str!("../testdata/floor-15.json")).expect("the fixture parses")
    }

    fn machine(json: serde_json::Value) -> Machine {
        serde_json::from_value(json).expect("a machine")
    }

    fn ink() -> Palette {
        Palette::of(Style::Ink, true)
    }

    #[test]
    fn every_fixture_machine_gets_its_kit_model_in_every_style() {
        for m in &fixture().machines {
            for style in [Style::Ink, Style::Blueprint, Style::Night] {
                let stage = MachineStage::new(m, &Palette::of(style, true)).expect("a stage");
                assert_eq!(stage.model, kit_model_of(m), "{}", m.id);
                let model = kit::model(stage.model).expect("the kit model");
                assert!(
                    stage.size[1] > model.size.y,
                    "{}: the plinth raises the model",
                    m.id
                );
                let frame = stage.frame();
                assert!(!frame.statics.is_empty(), "{}: the stage draws", m.id);
                assert!(
                    !frame.anim_batches.is_empty(),
                    "{}: its status light runs on the GPU",
                    m.id
                );
            }
        }
    }

    #[test]
    fn the_status_light_follows_the_machine() {
        let mut m = machine(
            json!({ "id": "s", "name": "s", "kind": "server", "status": "online", "working": 1 }),
        );
        let mut stage = MachineStage::new(&m, &ink()).expect("a stage");
        let working = rgba(ink().lamp(Lamp::Working));
        let row = |stage: &MachineStage| {
            stage.frame().animated.expect("the tables").machine_statuses[SLOT]
        };
        assert_eq!(row(&stage), working);
        m.status = "offline".into();
        stage.update(&m);
        assert_eq!(row(&stage), rgba(ink().lamp(Lamp::Offline)));
    }

    #[test]
    fn the_stage_spins_as_ambient_until_the_first_input_then_the_user_drives() {
        let m = machine(
            json!({ "id": "l", "name": "l", "kind": "laptop", "vendor": "dell", "memory_mib": 16384 }),
        );
        let mut stage = MachineStage::new(&m, &ink()).expect("a stage");
        let theta = stage.orbit.theta;
        assert_eq!(stage.step(0.016, 1.5, false), Motion::Ambient);
        assert!(stage.orbit.theta > theta, "auto-rotate turns the camera");

        stage.rotate(40.0, 0.0, 400.0);
        assert_eq!(
            stage.step(0.016, 1.5, false),
            Motion::Active,
            "a drag is active while it damps"
        );
        for _ in 0..400 {
            stage.step(0.05, 1.5, false);
        }
        let theta = stage.orbit.theta;
        assert_eq!(
            stage.step(0.5, 1.5, false),
            Motion::Rest,
            "a settled user view rests"
        );
        assert_eq!(
            stage.orbit.theta, theta,
            "the spin stays off after the first input"
        );
    }

    /// A device for the GPU test, or None on a machine with no adapter (the test is then skipped).
    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            block_ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        block_ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    /// The RGBA bytes of one fixture machine's 64 by 64 thumbnail, on the test device.
    fn thumbnail_of(device: &wgpu::Device, queue: &wgpu::Queue, id: &str) -> Vec<u8> {
        let fleet = fixture();
        let m = fleet
            .machines
            .iter()
            .find(|m| m.id == id)
            .expect("the fixture machine");
        let stage = MachineStage::new(m, &ink()).expect("a stage");
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm);
        renderer
            .thumbnail(&stage, View::Iso, 64, 64)
            .expect("the readback")
    }

    /// One pixel of a 64 by 64 RGBA thumbnail.
    fn pixel(px: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * 64 + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn a_thumbnail_is_a_transparent_card_with_the_machine_lit_and_framed() {
        let Some((device, queue)) = test_device() else {
            eprintln!("no wgpu adapter here: the thumbnail test is skipped");
            return;
        };
        const N: usize = 64;
        // n1 is a server and n2 a laptop in floor-15.json.
        let mut server = Vec::new();
        for id in ["n1", "n2"] {
            let px = thumbnail_of(&device, &queue, id);
            assert_eq!(px.len(), N * N * 4);
            // The top corners are transparent. The bottom corners hold no sky or backdrop, only the machine's own cast
            // shadow: black, at most the shadow catcher's 0.2 opacity (alpha 51). A laptop's shadow reaches a bottom corner.
            for (x0, y0, below) in [
                (0, 0, false),
                (N - 4, 0, false),
                (0, N - 4, true),
                (N - 4, N - 4, true),
            ] {
                for y in y0..y0 + 4 {
                    for x in x0..x0 + 4 {
                        let p = pixel(&px, x, y);
                        if below {
                            assert!(
                                p[..3] == [0, 0, 0] && p[3] <= 51,
                                "{id}: the corner ({x}, {y}) is not shadow: rgba {p:?}"
                            );
                        } else {
                            assert_eq!(p[3], 0, "{id}: the corner ({x}, {y}) is transparent");
                        }
                    }
                }
            }
            // The opaque pixels (alpha 128 and up) span the machine, by the longer of their two sides.
            let (mut lo, mut hi) = ((N, N), (0, 0));
            for y in 0..N {
                for x in 0..N {
                    if pixel(&px, x, y)[3] >= 128 {
                        lo = (lo.0.min(x), lo.1.min(y));
                        hi = (hi.0.max(x), hi.1.max(y));
                    }
                }
            }
            let span = (hi.0 - lo.0 + 1).max(hi.1 - lo.1 + 1);
            assert!(
                span as f32 >= 0.6 * N as f32,
                "{id}: the machine spans {span} of {N} px"
            );
            if id == "n1" {
                server = px;
            }
        }
        // The server's case is dark grey by design (as in the web's studio shot), so its centre says little. What shows
        // that it is lit, not a black silhouette, is its highlights: the brightest twentieth of its opaque pixels.
        let mut lumas: Vec<f32> = (0..N * N)
            .map(|i| pixel(&server, i % N, i / N))
            .filter(|c| c[3] >= 128)
            .map(|c| 0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2]))
            .collect();
        lumas.sort_by(f32::total_cmp);
        let bright = lumas[lumas.len() * 19 / 20];
        assert!(
            bright >= 60.0,
            "the server is lit: its brightest twentieth reaches only {bright:.0} of 255"
        );
    }

    #[test]
    fn the_flat_views_are_orthographic_and_square_to_their_faces() {
        let m = machine(json!({ "id": "s", "name": "s", "kind": "server" }));
        let stage = MachineStage::new(&m, &ink()).expect("a stage");
        for (view, dir) in [
            (View::Front, Vec3::Z),
            (View::Side, Vec3::X),
            (View::Back, Vec3::NEG_Z),
            (View::Top, Vec3::Y),
        ] {
            let camera = stage.view_frame(view, 1.0).camera;
            assert!(camera.ortho, "{view:?} is flat");
            let towards_eye = (camera.eye - camera.target).normalize();
            assert!(
                towards_eye.dot(dir) > 0.999,
                "{view:?} looks along {towards_eye}"
            );
            assert_eq!(View::parse(view.name()), Some(view));
        }
        assert!(
            !stage.view_frame(View::Iso, 1.0).camera.ortho,
            "the three-quarter view keeps its perspective"
        );
        assert_eq!(View::parse("Elevation"), None);
    }

    #[test]
    fn each_flat_view_draws_the_machine_large_on_a_clear_card() {
        let Some((device, queue)) = test_device() else {
            eprintln!("no wgpu adapter here: the view test is skipped");
            return;
        };
        const N: usize = 64;
        let m = machine(json!({ "id": "s", "name": "s", "kind": "server" }));
        let stage = MachineStage::new(&m, &ink()).expect("a stage");
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for view in View::FLAT {
            let px = renderer
                .thumbnail(&stage, view, 64, 64)
                .expect("the readback");
            // The corners hold nothing but, from above, the faint cast shadow (the catcher's 0.2 opacity: alpha 51).
            for (x, y) in [(0, 0), (N - 1, 0), (0, N - 1), (N - 1, N - 1)] {
                assert!(
                    pixel(&px, x, y)[3] <= 51,
                    "{view:?}: the corner ({x}, {y}) is {:?}",
                    pixel(&px, x, y)
                );
            }
            let (mut lo, mut hi) = ((N, N), (0, 0));
            for y in 0..N {
                for x in 0..N {
                    if pixel(&px, x, y)[3] >= 128 {
                        lo = (lo.0.min(x), lo.1.min(y));
                        hi = (hi.0.max(x), hi.1.max(y));
                    }
                }
            }
            let span = (hi.0.saturating_sub(lo.0) + 1).max(hi.1.saturating_sub(lo.1) + 1);
            assert!(
                span as f32 >= 0.75 * N as f32,
                "{view:?}: the machine spans {span} of {N} px"
            );
        }
    }

    #[test]
    fn reduced_motion_holds_the_spin() {
        let m = machine(
            json!({ "id": "l", "name": "l", "kind": "laptop", "vendor": "dell", "memory_mib": 16384 }),
        );
        let mut stage = MachineStage::new(&m, &ink()).expect("a stage");
        assert_eq!(stage.step(0.5, 1.5, true), Motion::Rest);
    }
}
