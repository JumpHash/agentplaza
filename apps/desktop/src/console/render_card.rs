//! The floor's renderer in small scenes: a machine's exterior in a card (`exterior`), and an agent's robot portrait
//! (`robot_portrait`). Both draw through the floor's own renderer (`engine::render`), each on its own slot. The cards
//! and the portraits are two pools of slots: iced keeps one pipeline per type, and two primitives on one pipeline
//! overwrite each other's targets within a frame, so a card and a portrait never share a slot.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use iced::widget::shader::{self, Viewport};
use iced::{Element, Length, Rectangle, mouse, wgpu, window};

use crate::console::engine::hardware::{self, Built, Model, Params, Status};
use crate::console::engine::render;
use crate::console::engine::robot::{Robot, RobotPose, RobotState, robot_palette};
use crate::console::engine::scene::{
    self, Camera, Draw, DrawKind, Filter, Frame, Geometry, Material, Obj, PointLightDraw, Shading,
    Texture, World,
};

/// The machine cards' slots (`slot` wraps at this count).
pub(crate) const CARD_SLOTS: usize = 8;

/// The portraits' slots, a pool of their own. A slot past the pool gets no portrait (the caller's letter tile shows),
/// so two portraits never share one.
pub(crate) const PORTRAIT_SLOTS: usize = 32;

/// Each built model's statics and each portrait get their own version, so a slot reused by another model never shows
/// stale statics.
static BUILDS: AtomicU64 = AtomicU64::new(1);

/// A card redraws about 30 times a second, for its LEDs; the exterior itself holds still (no turn).
const CARD_FRAME: Duration = Duration::from_millis(33);

/// One slot's renderer: the floor's own pipeline, held apart per slot and per pool (iced keeps one pipeline per type).
pub(crate) struct CardPipeline<const PORTRAIT: bool, const N: usize>(render::Pipeline);

impl<const PORTRAIT: bool, const N: usize> shader::Pipeline for CardPipeline<PORTRAIT, N> {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut p = render::Pipeline::new(device, queue, format);
        // A card or a portrait has no page-sized backdrop: its clear is transparent, so the page shows behind it.
        p.clear_alpha = 0.0;
        // A card's corner is empty, so it is the probe's sample (`APZ_CARD_PROBE`); a portrait's corner is its backdrop.
        p.probe_corner = !PORTRAIT;
        CardPipeline(p)
    }
}

/// A card's or a portrait's frame for the GPU: the floor's own primitive, drawn with its slot's pipeline.
#[derive(Debug)]
pub(crate) struct CardPrimitive<const PORTRAIT: bool, const N: usize> {
    frame: Option<Arc<Frame>>,
}

impl<const PORTRAIT: bool, const N: usize> shader::Primitive for CardPrimitive<PORTRAIT, N> {
    type Pipeline = CardPipeline<PORTRAIT, N>;

    fn prepare(
        &self,
        p: &mut CardPipeline<PORTRAIT, N>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let Some(frame) = &self.frame else { return };
        let inner = render::Primitive {
            frame: Arc::clone(frame),
        };
        let rect = render::Rect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        };
        inner.prepare(&mut p.0, device, queue, &rect, viewport.scale_factor());
    }

    fn draw(&self, p: &CardPipeline<PORTRAIT, N>, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(frame) = &self.frame else {
            return true;
        };
        let inner = render::Primitive {
            frame: Arc::clone(frame),
        };
        inner.draw(&p.0, pass)
    }
}

// ---- A machine's exterior: the floor's 3D model in a card -------------------------------------

/// A card's state: its built model with the statics baked from it, the time of its last redraw, and the frame the
/// GPU draws next. The LEDs blink in `update`, once per redraw; `draw` only hands the frame over.
#[derive(Default)]
pub(crate) struct CardState {
    built: Option<Built>,
    statics: Option<(u64, Arc<Vec<Draw>>, Vec<PointLightDraw>)>,
    last: Option<Instant>,
    /// The card's ambient clock in seconds (the pulses and blinks read it); the card redraws continuously, so it integrates dt.
    clock: f32,
    frame: Option<Arc<Frame>>,
}

/// A machine's exterior in a card, on card slot `N`.
pub(crate) struct Card<const N: usize> {
    model: &'static Model,
    gpus: usize,
    active: bool,
}

impl<const N: usize, M> shader::Program<M> for Card<N> {
    type State = CardState;
    type Primitive = CardPrimitive<false, N>;

    fn update(
        &self,
        st: &mut CardState,
        event: &iced::Event,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<shader::Action<M>> {
        let iced::Event::Window(window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        let dt = st
            .last
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        st.last = Some(*now);
        if st.built.is_none() {
            let built = hardware::build(
                self.model,
                &Params::default().with("gpus", self.gpus as f64),
            );
            let version = BUILDS.fetch_add(1, AtomicOrdering::Relaxed);
            let statics = Arc::new(scene::bake(&built.root));
            let lights = scene::flatten(&built.root, Filter::Static).1;
            st.statics = Some((version, statics, lights));
            st.built = Some(built);
        }
        let (Some(built), Some((version, statics, lights))) =
            (st.built.as_mut(), st.statics.as_ref())
        else {
            return None;
        };
        let status = hardware::State {
            status: Status::Online,
            activity: if self.active { 1.0 } else { 0.0 },
            open: 1.0,
        };
        st.clock += dt;
        built.update(dt, st.clock, &status);
        let aspect = (bounds.width / bounds.height.max(1.0)).max(0.1);
        st.frame = Some(Arc::new(card_frame(
            built,
            self.model,
            (*version, statics, lights),
            aspect,
        )));
        Some(shader::Action::request_redraw_at(*now + CARD_FRAME))
    }

    fn draw(
        &self,
        st: &CardState,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> CardPrimitive<false, N> {
        CardPrimitive {
            frame: st.frame.clone(),
        }
    }
}

/// The frame of an exterior: its statics and its dynamic parts (the LEDs), under a camera that frames the whole model
/// (`fit_camera`). The paper colour is the world's background, but the card's clear is transparent, so only the meshes
/// show; no fog, so the card sits on the page.
fn card_frame(
    built: &Built,
    model: &Model,
    statics: (u64, &Arc<Vec<Draw>>, &[PointLightDraw]),
    aspect: f32,
) -> Frame {
    let (version, statics, static_lights) = statics;
    let root = &built.root;
    let root_world = root.local_matrix();
    let children = root.get().children.clone();
    let mut roots: Vec<(Obj, Mat4)> = Vec::new();
    for c in &children {
        if c.get().dynamic {
            roots.push((c.clone(), root_world));
        } else {
            roots.extend(scene::dynamic_roots(c, root_world));
        }
    }
    let (draws, mut lights) = scene::flatten_roots(&roots);
    lights.extend_from_slice(static_lights);
    let camera = fit_camera(model, aspect, &draws, statics.as_slice());
    let world = World {
        background: scene::Color::srgb(
            crate::theme::palette().paper.r,
            crate::theme::palette().paper.g,
            crate::theme::palette().paper.b,
        ),
        fog_near: 1.0e6,
        fog_far: 1.0e6 + 1.0,
        ..Default::default()
    };
    Frame {
        animated: None,
        camera,
        world,
        statics: Arc::clone(statics),
        statics_version: version,
        draws,
        lights,
        time: 0.0,
        overlays: Vec::new(),
        fades: Arc::new(built.fades.clone()),
        plaza: None,
        anim_batches: Arc::new(Vec::new()),
        focus: None,
    }
}

/// The camera for a model: it looks along the model's own camera anchor's direction, at the centre of the bounding sphere
/// of the whole posed model (the opened lid included, from its world-space meshes; the authored size is the floor), from
/// far enough back that the sphere fits the card's height and width (a 40 degree vertical field) at any aspect.
fn fit_camera(model: &Model, aspect: f32, draws: &[Draw], statics: &[Draw]) -> Camera {
    const FOV: f32 = 40.0;
    const MARGIN: f32 = 1.12;
    let anchor = model.anchors.camera.as_ref();
    let look = Vec3::from(anchor.map_or([0.0, model.size[1] * 0.5, 0.0], |c| c.look));
    let dir = anchor
        .and_then(|c| (Vec3::from(c.at) - look).try_normalize())
        .unwrap_or(Vec3::Z);
    let (mut lo, mut hi) = (Vec3::ZERO, Vec3::from(model.size));
    for d in draws.iter().chain(statics) {
        if matches!(d.kind, DrawKind::Mesh | DrawKind::Outline) {
            let (c, r) = scene::world_sphere(&d.geo, d.model);
            lo = lo.min(c - Vec3::splat(r));
            hi = hi.max(c + Vec3::splat(r));
        }
    }
    let centre = (lo + hi) * 0.5;
    let radius = ((hi - lo) * 0.5).length().max(0.05);
    let t = (FOV * 0.5).to_radians().tan();
    let t_min = t.min(t * aspect.max(0.1));
    // A sphere of angular radius asin(r / d) fits the narrower half field when d >= r * sqrt(1 + t^2) / t.
    let dist = radius * (1.0 + t_min * t_min).sqrt() / t_min * MARGIN;
    Camera {
        eye: centre + dir * dist,
        target: centre,
        fov: FOV,
        near: 0.05,
        far: 1000.0,
        aspect,
    }
}

/// A machine's exterior drawn in a card on card slot `slot` (wraps). Returns None when the model is missing.
pub(crate) fn exterior<'a, M: 'a>(
    slot: usize,
    kind: &str,
    vendor: &str,
    gpus: usize,
    active: bool,
) -> Option<Element<'a, M>> {
    let name = hardware::models_for(kind, Some(vendor)).0;
    let model = hardware::model(name).or_else(|| hardware::model("exterior/workstation"))?;
    macro_rules! card {
        ($($n:literal)*) => {
            match slot % CARD_SLOTS {
                $($n => shader::Shader::<M, _>::new(Card::<$n> { model, gpus, active }).width(Length::Fixed(160.0)).height(Length::Fixed(120.0)).into(),)*
                _ => return None,
            }
        };
    }
    Some(card!(0 1 2 3 4 5 6 7))
}

// ---- An agent's portrait: the web's studio photograph ----------------------------------------

/// An agent's portrait (the web draws the same, through the wasm engine): its own robot, its species in its hues, posed and drawn once,
/// on the sunburst of its own hue. The orchestrator is drawn at 0.85 of its size (`portrait_frame`).
#[derive(Clone, Copy)]
pub(crate) struct PortraitSpec {
    pub cid: u64,
    pub orchestrator: bool,
    /// The district (role) hue: the robot's front panel.
    pub role_hue: f32,
    /// The agent's own hue: its shell, and the sunburst behind it.
    pub body_hue: f32,
    pub offline: bool,
    /// The robot's species (`robot::species_of`: the hub's, else the cid's deal).
    pub species: &'static str,
}

/// A portrait's cache: the frame drawn for this agent, species, hues and size. The web's portrait is a still (no turn, no
/// blink), so the frame is drawn once and reused until the key changes.
#[derive(Default)]
pub(crate) struct PortraitState {
    key: Option<(u64, bool, u32, u32, bool, u32, &'static str)>,
    frame: Option<Arc<Frame>>,
}

/// A portrait on portrait slot `N`.
pub(crate) struct Portrait<const N: usize>(PortraitSpec);

impl<const N: usize, M> shader::Program<M> for Portrait<N> {
    type State = PortraitState;
    type Primitive = CardPrimitive<true, N>;

    fn update(
        &self,
        st: &mut PortraitState,
        event: &iced::Event,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<shader::Action<M>> {
        let iced::Event::Window(window::Event::RedrawRequested(_)) = event else {
            return None;
        };
        let spec = self.0;
        let aspect = (bounds.width / bounds.height.max(1.0)).max(0.1);
        let key = (
            spec.cid,
            spec.orchestrator,
            spec.role_hue.to_bits(),
            spec.body_hue.to_bits(),
            spec.offline,
            aspect.to_bits(),
            spec.species,
        );
        if st.key == Some(key) && st.frame.is_some() {
            return None;
        }
        let palette = robot_palette();
        let mut robot = Robot::with_variant(
            spec.role_hue,
            spec.orchestrator,
            &palette,
            spec.body_hue,
            spec.species,
        );
        robot.set_state(if spec.offline {
            RobotState::Offline
        } else {
            RobotState::Idle
        });
        let poses = [
            RobotPose::Wave,
            RobotPose::Proud,
            RobotPose::Think,
            RobotPose::Cheer,
        ];
        robot.pose(poses[(spec.cid % 4) as usize]);
        robot.update(0.0, 0.0);
        let version = BUILDS.fetch_add(1, AtomicOrdering::Relaxed);
        st.frame = Some(Arc::new(portrait_frame(&robot, spec, version, aspect)));
        st.key = Some(key);
        None
    }

    fn draw(
        &self,
        st: &PortraitState,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> CardPrimitive<true, N> {
        CardPrimitive {
            frame: st.frame.clone(),
        }
    }
}

/// The portrait's frame, the web's studio (`portrait.ts`): the robot turned by -0.35 rad (the orchestrator at 0.85 of its
/// size), photographed by the camera at (1, 1.25, 3.4) aimed at (0, 0.72, 0) with a 30 degree field; the sunburst card
/// behind it, the key light from (2.5, 4, 3) casting its shadow on a disc under the robot, and the hemisphere light.
fn portrait_frame(robot: &Robot, spec: PortraitSpec, version: u64, aspect: f32) -> Frame {
    let scale = if spec.orchestrator { 0.85 } else { 1.0 };
    let pose = Mat4::from_rotation_y(-0.35) * Mat4::from_scale(Vec3::splat(scale));
    let (mut draws, lights) = scene::flatten_roots(&[(robot.group.clone(), pose)]);
    draws.push(backdrop(spec.body_hue));
    draws.push(shadow_disc());
    // The web's offline portrait is `grayscale(0.85) opacity(0.75)` on the whole image (profile.css `.avatar.offline img`):
    // the robot's colours go grey, and the whole card, its backdrop and the shadow disc included, fades to 75 per cent.
    if spec.offline {
        for d in draws.iter_mut() {
            d.material.color = greyed(d.material.color);
            d.material.emissive = greyed(d.material.emissive);
            d.material.opacity *= 0.75;
            d.material.transparent = true;
        }
    }
    let world = World {
        hemi_sky: scene::Color::hex(0xeaf4ff),
        hemi_ground: scene::Color::hex(0x8a7a5a),
        hemi_intensity: 1.3,
        sun_color: scene::Color::hex(0xfff1d6),
        sun_intensity: 2.6,
        sun_position: Vec3::new(2.5, 4.0, 3.0),
        shadow_extent: 5.0,
        shadow_near: 0.5,
        shadow_far: 60.0,
        fog_near: 1.0e6,
        fog_far: 1.0e6 + 1.0,
        ..Default::default()
    };
    Frame {
        animated: None,
        camera: Camera {
            eye: Vec3::new(1.0, 1.25, 3.4),
            target: Vec3::new(0.0, 0.72, 0.0),
            fov: 30.0,
            near: 0.1,
            far: 50.0,
            aspect,
        },
        world,
        statics: Arc::new(Vec::new()),
        statics_version: version,
        draws,
        lights,
        time: 0.0,
        overlays: Vec::new(),
        fades: Arc::new(Vec::new()),
        plaza: None,
        anim_batches: Arc::new(Vec::new()),
        focus: None,
    }
}

/// A colour pulled 85 per cent toward its own grey (CSS `grayscale(0.85)`, on luma).
fn greyed(c: scene::Color) -> scene::Color {
    let luma = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    c.lerp(
        scene::Color {
            r: luma,
            g: luma,
            b: luma,
        },
        0.85,
    )
}

/// The web's 9 x 9 card at z -2.2 (`portrait.ts`'s backdrop, a basic material): the sunburst of the agent's hue. Its
/// texture's top row is the top of the card; drawn first, so the robot stands in front of it.
fn backdrop(hue: f32) -> Draw {
    let geo = Geometry::new(
        vec![
            [-4.5, 5.7, -2.2],
            [-4.5, -3.3, -2.2],
            [4.5, -3.3, -2.2],
            [4.5, 5.7, -2.2],
        ],
        vec![[0.0, 0.0, 1.0]; 4],
        vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        vec![0, 1, 2, 0, 2, 3],
    );
    let material = Material {
        shading: Shading::Basic,
        map: Some(Arc::new(sunburst(hue))),
        fog: false,
        ..Material::default()
    };
    Draw {
        kind: DrawKind::Mesh,
        geo,
        mat_id: scene::next_id(),
        material,
        model: Mat4::IDENTITY,
        cast_shadow: false,
        receive_shadow: false,
        render_order: -1000,
        lod: 0,
        fade: 0,
        lift: 0,
        owner: 0,
    }
}

/// The web's floor disc (radius 1.4, `ShadowMaterial` at opacity 0.25): it shows only the robot's shadow.
fn shadow_disc() -> Draw {
    const SEGMENTS: u32 = 48;
    const RADIUS: f32 = 1.4;
    let mut positions = vec![[0.0, 0.0, 0.0]];
    for i in 0..SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * TAU;
        positions.push([a.cos() * RADIUS, 0.0, a.sin() * RADIUS]);
    }
    // Each fan triangle faces up (the ring runs backwards from the centre's view).
    let mut indices = Vec::with_capacity(SEGMENTS as usize * 3);
    for i in 0..SEGMENTS {
        indices.extend([0, 1 + (i + 1) % SEGMENTS, 1 + i]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    let geo = Geometry::new(positions, normals, Vec::new(), indices);
    let material = Material {
        shading: Shading::Shadow,
        color: scene::Color::BLACK,
        opacity: 0.25,
        transparent: true,
        depth_write: false,
        fog: false,
        ..Material::default()
    };
    Draw {
        kind: DrawKind::Mesh,
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

/// CSS `hsl(h s l)` in sRGB, each 0 to 1 (`h` in degrees).
fn css_hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

/// The web's starburst (`portrait.ts` `sunburst`), painted on the CPU: 512 px square, the night colour of `hue`, eighteen
/// rays from (256, 300), a glow there and a sprinkle of stars. Its first row is the top, as the canvas's is.
fn sunburst(hue: f32) -> Texture {
    const SIDE: usize = 512;
    const CX: f32 = 256.0;
    const CY: f32 = 300.0;
    let h = hue.round();
    let night = css_hsl(h, 0.45, 0.16);
    let ray = css_hsl(h, 0.55, 0.22);
    let glow = css_hsl(h, 0.90, 0.65);
    let sector = PI / 18.0;
    let mut px = vec![0u8; SIDE * SIDE * 4];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (dx, dy) = (x as f32 + 0.5 - CX, y as f32 + 0.5 - CY);
            // The rays: wedge i covers the sectors 2i of the eighteen half-sectors (the canvas's y runs down, as atan2's does).
            let mut angle = dy.atan2(dx);
            if angle < 0.0 {
                angle += TAU;
            }
            let mut c = if ((angle / sector) as u32).is_multiple_of(2) {
                ray
            } else {
                night
            };
            // The glow: 0.75 at the centre, fading to nothing 260 px out.
            let r = (dx * dx + dy * dy).sqrt();
            let a = if r <= 10.0 {
                0.75
            } else if r >= 260.0 {
                0.0
            } else {
                0.75 * (1.0 - (r - 10.0) / 250.0)
            };
            for k in 0..3 {
                c[k] = glow[k] * a + c[k] * (1.0 - a);
            }
            let at = (y * SIDE + x) * 4;
            for k in 0..3 {
                px[at + k] = (c[k] * 255.0).round() as u8;
            }
            px[at + 3] = 255;
        }
    }
    // A sprinkle of stars: rgba(230, 238, 255, 0.8), squares of 2 or 3 px.
    let star = [230.0 / 255.0, 238.0 / 255.0, 1.0];
    let hi = h as usize;
    for i in 0..40usize {
        let (x0, y0) = ((i * 197 + hi * 7) % 512, (i * 131 + hi * 3) % 260);
        let size = if i % 5 == 0 { 3 } else { 2 };
        for y in y0..(y0 + size).min(SIDE) {
            for x in x0..(x0 + size).min(SIDE) {
                let at = (y * SIDE + x) * 4;
                for k in 0..3 {
                    let c = star[k] * 0.8 + f32::from(px[at + k]) / 255.0 * 0.2;
                    px[at + k] = (c * 255.0).round() as u8;
                }
            }
        }
    }
    Texture::new(SIDE as u32, SIDE as u32, px)
}

/// An agent's robot portrait, `size` square, on portrait slot `slot`. Past the pool (`PORTRAIT_SLOTS`) there is none, and
/// None means the caller's letter tile shows alone. The caller stacks its letter tile under it, which shows where the GPU
/// cannot draw.
pub(crate) fn robot_portrait<'a, M: 'a>(
    slot: usize,
    spec: PortraitSpec,
    size: f32,
) -> Option<Element<'a, M>> {
    if slot >= PORTRAIT_SLOTS {
        return None;
    }
    macro_rules! card {
        ($($n:literal)*) => {
            match slot {
                $($n => shader::Shader::<M, _>::new(Portrait::<$n>(spec)).width(Length::Fixed(size)).height(Length::Fixed(size)).into(),)*
                _ => return None,
            }
        };
    }
    Some(
        card!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31),
    )
}
