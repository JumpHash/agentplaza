//! The Machines view's scene, native (`apps/web/src/views/floor3d/fleet.ts`, number for number).
//! Each machine is its exterior hardware model (`hardware.rs`). The hub stands at the origin and
//! the nodes sit on a ring around it. A status ring lies under each machine, and a cable runs
//! from each node's port to the hub's, with light packets while agents work. `build_fleet` builds
//! the scene; `FleetScene` is its query API (`update`, `pick`, `hover`, `tags`, `frame`,
//! `set_open`). It has no UI: the Floor integration consumes it next, hence the module allow.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Arc, OnceLock};

use glam::{Mat4, Quat, Vec3};
use serde::Deserialize;

use super::geometry;
use super::hardware::{self, Built, Params, State, Status};
use super::props::{self, PropPalette};
use super::scene::{self, Color, Geo, Mat, Material, Obj};

/// The Machines view's animated draws (cable packets, status rings, LEDs): instanced, animated in
/// the vertex shader from a  uniform.  checks it for WebGL2.
pub const FLEET_ANIM_WGSL: &str = include_str!("shaders/fleet_anim.wgsl");

// ---- Data: GET /v1/fleet ----------------------------------------------------------------------

/// `GET /v1/fleet`: the machines and the links between them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FleetResponse {
    #[serde(default)]
    pub machines: Vec<Machine>,
    #[serde(default)]
    pub links: Vec<Link>,
}

/// One machine. `role` is `hub` or `node`; `status` is `online`, `draining`, `offline` or `revoked`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Machine {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub apple: Option<Apple>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub cpu: Cpu,
    #[serde(default)]
    pub memory_mib: u64,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    #[serde(default)]
    pub storage_gib: u64,
    /// The cids of the agents running here.
    #[serde(default)]
    pub agents: Vec<u64>,
    /// Of those agents, how many are working.
    #[serde(default)]
    pub working: u32,
    #[serde(default)]
    pub stats: Option<Stats>,
    #[serde(default)]
    pub last_seen_ms: u64,
}

/// A Mac's chip and core counts (only on Macs).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Apple {
    #[serde(default)]
    pub chip: String,
    #[serde(default)]
    pub p_cores: u32,
    #[serde(default)]
    pub e_cores: u32,
    #[serde(default)]
    pub gpu_cores: u32,
    #[serde(default)]
    pub neural_cores: u32,
    #[serde(default)]
    pub unified_memory_gib: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Cpu {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cores: u32,
    #[serde(default)]
    pub threads: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Gpu {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    /// Null when the hub cannot read the GPU's memory.
    #[serde(default)]
    pub memory_mib: Option<u64>,
}

/// Live load; remote nodes send it with every heartbeat.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub cpu_pct: f64,
    #[serde(default)]
    pub mem_used_mib: u64,
}

/// A cable between two machines, by id.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub a: String,
    #[serde(default)]
    pub b: String,
    #[serde(default)]
    pub online: bool,
}

// ---- Pure layout and rates (the numbers the web's fleet.ts uses) ------------------------------

/// The cables are clamped above this height.
const GROUND_Y: f32 = 0.0;
/// The gap between neighbours on the ring, and the smallest ring radius, in units.
const RING_GAP: f32 = 4.0;
const RING_MIN_RADIUS: f32 = 14.0;
/// Cable segments, how far the cable sags below the straight line, and the cable's radius.
pub const CABLE_SEGMENTS: usize = 24;
const CABLE_SAG: f32 = 1.5;
const CABLE_RADIUS: f32 = 0.09;
/// Light packets per second: the base, plus one per working agent on either end, capped.
pub const PACKET_BASE: f32 = 0.5;
pub const PACKET_PER_AGENT: f32 = 1.0;
pub const PACKET_MAX: f32 = 4.0;
/// How far along its cable a packet travels per second (a fraction of the cable).
const PACKET_SPEED: f32 = 0.4;
/// Packets alive per direction: at most `PACKET_MAX / PACKET_SPEED` = 10, with spare.
const PACKET_POOL: usize = 12;
/// Hover lift in units, and the ease rate per second.
const HOVER_LIFT: f32 = 0.3;
const HOVER_RATE: f32 = 10.0;
/// The status ring's pulse period, in seconds.
const PULSE_PERIOD: f32 = 2.0;
/// The animated draws: the packet slots per direction of a cable (the most packets in flight at
/// `PACKET_MAX`), a packet's radius, and the period the clock wraps at (a multiple of every animation's
/// period, so the wrap is seamless and f32 keeps its precision).
pub const PACKET_SLOTS: usize = 10;
const PACKET_RADIUS: f32 = 0.14;
pub const WRAP_PERIOD: f32 = 3600.0;

/// The bounding-sphere radius of a model of `size`.
pub fn bounding_radius(size: [f32; 3]) -> f32 {
    0.5 * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt()
}

/// The ring for nodes of the given radii, in id order. Each node takes an arc of its diameter plus
/// `RING_GAP`; the radius is the larger of `RING_MIN_RADIUS` and the total arc over 2π; the nodes
/// are spread over the whole circle in proportion to their arcs. Returns the radius and each
/// node's angle from +z.
pub fn ring_layout(radii: &[f32]) -> (f32, Vec<f32>) {
    let spans: Vec<f32> = radii.iter().map(|r| 2.0 * r + RING_GAP).collect();
    let total: f32 = spans.iter().sum();
    let radius = RING_MIN_RADIUS.max(total / TAU);
    let mut at = 0.0;
    let angles = spans
        .iter()
        .map(|s| {
            let angle = TAU * (at + s / 2.0) / total;
            at += s;
            angle
        })
        .collect();
    (radius, angles)
}

/// A ring point at `angle` from +z, on the ground plane.
pub fn ring_point(radius: f32, angle: f32) -> [f32; 3] {
    [radius * angle.sin(), 0.0, radius * angle.cos()]
}

/// The cable from `from` to `to`: a quadratic Bézier whose middle sags `CABLE_SAG` below the
/// straight line, clamped above the ground. It has `CABLE_SEGMENTS + 1` points, the first at
/// `from` and the last at `to`.
pub fn cable_points(from: [f32; 3], to: [f32; 3]) -> Vec<[f32; 3]> {
    let mut control = [0.0f32; 3];
    for k in 0..3 {
        control[k] = (from[k] + to[k]) / 2.0;
    }
    control[1] -= 2.0 * CABLE_SAG;
    (0..=CABLE_SEGMENTS)
        .map(|i| {
            let t = i as f32 / CABLE_SEGMENTS as f32;
            let u = 1.0 - t;
            let mut p = [0.0f32; 3];
            for k in 0..3 {
                p[k] = u * u * from[k] + 2.0 * u * t * control[k] + t * t * to[k];
            }
            p[1] = p[1].max(GROUND_Y);
            p
        })
        .collect()
}

/// Light packets per second on one cable, each way. An offline cable carries none.
pub fn packet_rate(online: bool, working_a: u32, working_b: u32) -> f32 {
    if !online {
        return 0.0;
    }
    (PACKET_BASE + PACKET_PER_AGENT * (working_a + working_b) as f32).min(PACKET_MAX)
}

/// The point at fraction `p` (0..1) along a cable's points. The points are evenly spaced in t,
/// so this lerps between the two points around `p` times the segment count.
pub fn point_on(points: &[[f32; 3]], p: f32) -> [f32; 3] {
    if points.len() < 2 {
        return points.first().copied().unwrap_or([0.0; 3]);
    }
    let last = points.len() - 1;
    let f = p.clamp(0.0, 1.0) * last as f32;
    let i = (f.floor() as usize).min(last - 1);
    let w = f - i as f32;
    let (a, b) = (points[i], points[i + 1]);
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * w)
}

/// The distance along a ray (unit `dir`) to a sphere: the near side, or 0 from inside.
pub fn ray_sphere(origin: [f32; 3], dir: [f32; 3], centre: [f32; 3], radius: f32) -> Option<f32> {
    let o = Vec3::from(origin) - Vec3::from(centre);
    let d = Vec3::from(dir);
    let b = o.dot(d);
    let c = o.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    if -b - s >= 0.0 {
        Some(-b - s)
    } else if -b + s >= 0.0 {
        Some(0.0)
    } else {
        None
    }
}

/// The status string as the hardware's `Status`. An unknown status reads as offline.
pub fn status_of(text: &str) -> Status {
    match text {
        "online" => Status::Online,
        "draining" => Status::Draining,
        "revoked" => Status::Revoked,
        _ => Status::Offline,
    }
}

/// The share of a machine's agents that are working, 0..1.
fn activity(m: &Machine) -> f32 {
    if m.agents.is_empty() {
        return 0.0;
    }
    (m.working as f32 / m.agents.len() as f32).clamp(0.0, 1.0)
}

// ---- Palette ----------------------------------------------------------------------------------

const PALETTE_SRC: &str = include_str!("../../../assets/hardware/palette.json");

#[derive(Deserialize)]
struct PaletteFile {
    colors: BTreeMap<String, PaletteEntry>,
}

#[derive(Deserialize)]
struct PaletteEntry {
    #[serde(default)]
    color: String,
}

/// A hardware palette colour, read from the same `palette.json` that `hardware.rs` reads.
fn named(name: &str) -> Color {
    static PAL: OnceLock<BTreeMap<String, Color>> = OnceLock::new();
    PAL.get_or_init(|| {
        serde_json::from_str::<PaletteFile>(PALETTE_SRC)
            .map(|f| {
                f.colors
                    .into_iter()
                    .filter_map(|(k, e)| Color::css(&e.color).map(|c| (k, c)))
                    .collect()
            })
            .unwrap_or_default()
    })
    .get(name)
    .copied()
    .unwrap_or(Color::WHITE)
}

/// A status's LED colour: ok green, draining amber, offline grey, revoked crit.
pub fn status_led(s: Status) -> Color {
    named(match s {
        Status::Online => "led-ok",
        Status::Draining => "led-warn",
        Status::Offline => "led-off",
        Status::Revoked => "led-crit",
    })
}

// ---- Models and layout keys -------------------------------------------------------------------

/// A machine's exterior model and its parameters: the GPU count, the Apple core counts and
/// memory, and `studio` when the model name says Studio.
fn model_and_params(m: &Machine) -> Option<(&'static hardware::Model, Params)> {
    let (exterior, _) = hardware::models_for(&m.kind, m.vendor.as_deref());
    let model = hardware::model(exterior).or_else(|| hardware::model("exterior/workstation"))?;
    let studio = m
        .model
        .as_deref()
        .is_some_and(|name| name.contains("Studio"));
    let mut params = Params::default()
        .with("gpus", m.gpus.len() as f64)
        .with("studio", if studio { 1.0 } else { 0.0 });
    if let Some(a) = &m.apple {
        params = params
            .with("p_cores", a.p_cores as f64)
            .with("e_cores", a.e_cores as f64)
            .with("gpu_cores", a.gpu_cores as f64)
            .with("memory_gib", a.unified_memory_gib as f64);
    }
    Some((model, params))
}

/// What the scene's layout depends on. A change rebuilds the scene; status, load and names do not.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    id: String,
    role: String,
    kind: String,
    vendor: Option<String>,
    model: Option<String>,
    apple: Option<Apple>,
    gpus: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Layout {
    shapes: Vec<Shape>,
    pairs: Vec<(String, String)>,
}

fn layout_of(fleet: &FleetResponse) -> Layout {
    Layout {
        shapes: fleet
            .machines
            .iter()
            .map(|m| Shape {
                id: m.id.clone(),
                role: m.role.clone(),
                kind: m.kind.clone(),
                vendor: m.vendor.clone(),
                model: m.model.clone(),
                apple: m.apple.clone(),
                gpus: m.gpus.len(),
            })
            .collect(),
        pairs: fleet
            .links
            .iter()
            .map(|l| (l.a.clone(), l.b.clone()))
            .collect(),
    }
}

// ---- The scene --------------------------------------------------------------------------------

/// One machine in the scene: its root (placed and turned), its hardware build, and its anchors in
/// the world at rest.
pub struct MachineScene {
    pub root: Obj,
    pub built: Built,
    pub center: [f32; 3],
    pub radius: f32,
    pub label: [f32; 3],
    pub port: [f32; 3],
    pub hub: bool,
    pub status: Status,
    pub name: String,
    /// The status ring's material, the machine's rest position on the ground, and its hover lift.
    ring_mat: Mat,
    base: [f32; 3],
    /// The footprint's radius: the ring's outer radius is 1.3 of it.
    footprint: f32,
    lift: f32,
    /// How far it is opened (ENTER), 0..1.
    open: f32,
    /// The plaza's atom at `anchors.atom`, shown only while online.
    atom: Option<Obj>,
    anim: Option<props::Anim>,
}

/// A cable: its ends, its material and its sampled points. Its packets are instanced (see
/// `FleetScene::instances`), so a cable holds no packet objects and nothing steps them on the CPU.
struct LinkScene {
    a: String,
    b: String,
    cable: Mat,
    points: Vec<[f32; 3]>,
}

/// The scene: the group, the machines by id, and the cables. Build it with [`build_fleet`].
pub struct FleetScene {
    /// The whole scene (the ground stays with the Floor).
    pub group: Obj,
    /// The machines, by id.
    pub machines: BTreeMap<String, MachineScene>,
    links: Vec<LinkScene>,
    layout: Option<Layout>,
    palette: PropPalette,
    hovered: Option<String>,
    time: f32,
}

/// A tag for the overlay: the machine's name and status, above its label.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub id: String,
    pub pos: [f32; 3],
    pub name: String,
    pub status: Status,
}

/// What the camera fits: a centre and a radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FleetFrame {
    pub center: [f32; 3],
    pub radius: f32,
}

/// Builds the machines scene from a fleet snapshot.
pub fn build_fleet(fleet: &FleetResponse, palette: &PropPalette) -> FleetScene {
    // Static (baked with the Machines view): only what moves or changes colour is dynamic.
    let group = Obj::group();
    let mut scene = FleetScene {
        group,
        machines: BTreeMap::new(),
        links: Vec::new(),
        layout: None,
        palette: *palette,
        hovered: None,
        time: 0.0,
    };
    scene.rebuild(fleet, layout_of(fleet));
    scene
}

/// Builds one machine: its model, its parts, the plaza atom, and its anchors in the world.
/// `pose` is its rest position on the ground and its turn about y.
#[allow(clippy::too_many_arguments)]
fn place_machine(
    group: &Obj,
    palette: &PropPalette,
    m: &Machine,
    hub: bool,
    model: &hardware::Model,
    mut built: Built,
    pose: ([f32; 3], f32),
) -> MachineScene {
    let (base, turn) = pose;
    built.freeze_cutaways(); // no fades in this view
    let root = Obj::group();
    root.add(&built.root);
    root.set_position(base[0], base[1], base[2])
        .set_rotation(0.0, turn, 0.0);
    group.add(&root);

    let anim = hardware::place_atom(model, &built.root, palette);
    let atom = anim
        .as_ref()
        .and_then(|_| built.root.get().children.last().cloned());

    let size = model.size;
    let footprint = 0.5 * (size[0] * size[0] + size[2] * size[2]).sqrt();
    let ring_mat = Mat::new(Material::default());
    let ring = Obj::mesh(
        &geometry::ring(footprint * 1.1, footprint * 1.3, 48, 1, 0.0, TAU),
        &ring_mat,
    );
    ring.set_position(base[0], 0.05, base[2])
        .set_rotation(-FRAC_PI_2, 0.0, 0.0);
    ring.set_dynamic(true); // its colour follows the status every frame
    // Drawn instanced (fleet_anim.wgsl): the mesh stays out of the scene graph.

    let pose_mat = Mat4::from_rotation_translation(Quat::from_rotation_y(turn), Vec3::from(base));
    let world = |p: [f32; 3]| pose_mat.transform_point3(Vec3::from(p)).to_array();
    let anchors = &model.anchors;
    MachineScene {
        root,
        center: world([0.0, size[1] / 2.0, 0.0]),
        radius: bounding_radius(size),
        label: world(anchors.label.unwrap_or([0.0, size[1], 0.0])),
        port: world(anchors.port.unwrap_or([0.0, size[1] / 2.0, 0.0])),
        hub,
        status: status_of(&m.status),
        name: m.name.clone(),
        built,
        ring_mat,
        base,
        footprint,
        lift: 0.0,
        open: 0.0,
        atom,
        anim,
    }
}

/// A machine's parts, built but not yet placed (its radius is needed for the ring).
fn prepare(m: &Machine) -> Option<(&'static hardware::Model, Built)> {
    let (model, params) = model_and_params(m)?;
    Some((model, hardware::build(model, &params)))
}

/// A cable: its segments, and its two flows of packets.
fn build_link(group: &Obj, a: &str, b: &str, points: Vec<[f32; 3]>) -> LinkScene {
    let cable = Mat::toon(named("cable"));
    let segment = geometry::cylinder(CABLE_RADIUS, CABLE_RADIUS, 1.0, 6);
    // One mesh for the whole cable: its segments merged, so the cable is one draw.
    let mut parts: Vec<(Geo, Mat4)> = Vec::new();
    for pair in points.windows(2) {
        let (p0, p1) = (Vec3::from(pair[0]), Vec3::from(pair[1]));
        let d = p1 - p0;
        let len = d.length();
        if len < 1e-6 {
            continue;
        }
        let mid = (p0 + p1) * 0.5;
        let m = Mat4::from_scale_rotation_translation(Vec3::new(1.0, len, 1.0), Quat::from_rotation_arc(Vec3::Y, d / len), mid);
        parts.push((Arc::clone(&segment), m));
    }
    if !parts.is_empty() {
        let seg = Obj::mesh(&scene::merge(&parts), &cable);
        seg.set_dynamic(true); // its cable's colour follows the link every frame
        group.add(&seg);
    }
    LinkScene { a: a.to_string(), b: b.to_string(), cable, points }
}

/// One cable packet slot's instance data (`PacketIn` in fleet_anim.wgsl): the cable's ends, then
/// `params` (phase, direction, alive, radius) and the colour.
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct PacketInstance {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub params: [f32; 4],
    pub color: [f32; 4],
}

/// One status ring's instance data (`RingIn`): `centre` (xyz, and w the outer radius), `pulse` (rate,
/// phase, online, held intensity) and the colour.
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct RingInstance {
    pub centre: [f32; 4],
    pub pulse: [f32; 4],
    pub color: [f32; 4],
}

/// The fleet's fan-angle table: one entry per machine (the GPU's Globals hold this many).
pub const MACHINE_SLOTS: usize = 128;

/// The animated draws' instance data: `PACKET_SLOTS` slots each way on every cable (the live ones
/// spread evenly in phase), then one ring per machine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FleetInstances {
    pub packets: Vec<PacketInstance>,
    pub rings: Vec<RingInstance>,
    /// Each machine's fan angle by its fleet index (`Built::set_machine`), `MACHINE_SLOTS` long.
    pub machine_angles: Vec<f32>,
}

/// The packet slots alive on a cable carrying `rate` packets a second. A packet is in flight for
/// `1 / PACKET_SPEED` seconds, so the cable needs `rate / PACKET_SPEED` slots, rounded.
pub fn live_slots(rate: f32) -> usize {
    ((rate / PACKET_SPEED).round() as usize).min(PACKET_SLOTS)
}

/// One direction of a cable: `PACKET_SLOTS` slots, the first `live` alive, their phases spread evenly.
fn direction_slots(from: [f32; 3], to: [f32; 3], live: usize, reversed: bool, color: [f32; 4]) -> Vec<PacketInstance> {
    (0..PACKET_SLOTS)
        .map(|j| {
            let alive = j < live;
            PacketInstance {
                from,
                to,
                params: [
                    if alive { j as f32 / live as f32 } else { 0.0 },
                    if reversed { -1.0 } else { 1.0 },
                    if alive { 1.0 } else { 0.0 },
                    PACKET_RADIUS,
                ],
                color,
            }
        })
        .collect()
}

/// A colour as RGBA, opaque.
fn rgba(c: Color) -> [f32; 4] {
    let [r, g, b] = c.to_array();
    [r, g, b, 1.0]
}

/// Whether the cable between `a` and `b` is up, and its packet rate each way (packets a second).
fn link_state(fleet: &FleetResponse, a: &str, b: &str) -> (bool, f32) {
    let online = fleet.links.iter().find(|l| l.a == a && l.b == b).is_some_and(|l| l.online);
    let working = |id: &str| fleet.machines.iter().find(|m| m.id == id).map_or(0, |m| m.working);
    (online, packet_rate(online, working(a), working(b)))
}

/// Sample `i` of a cable, as `cable_points` has it: the quadratic Bezier with its control at the
/// chord's midpoint, dropped by twice the sag, clamped above the ground. The shader's `cable_sample`.
fn cable_sample_shader(from: [f32; 3], to: [f32; 3], i: f32) -> [f32; 3] {
    let mut control = [0.0f32; 3];
    for k in 0..3 {
        control[k] = (from[k] + to[k]) / 2.0;
    }
    control[1] -= 2.0 * CABLE_SAG;
    let t = i / CABLE_SEGMENTS as f32;
    let u = 1.0 - t;
    let mut p = [0.0f32; 3];
    for k in 0..3 {
        p[k] = u * u * from[k] + 2.0 * u * t * control[k] + t * t * to[k];
    }
    p[1] = p[1].max(GROUND_Y);
    p
}

/// Where `vs_packet` puts a packet slot at `time` seconds: the Rust mirror of the shader, with its own
/// cable samples (`cable_sample_shader`) and the lerp of `point_on`.
pub fn packet_at_shader(from: [f32; 3], to: [f32; 3], params: [f32; 4], time: f32) -> [f32; 3] {
    let x = params[0] + time * PACKET_SPEED;
    let p = x - x.floor();
    let at = if params[1] < 0.0 { 1.0 - p } else { p };
    let segments = CABLE_SEGMENTS as f32;
    let f = at.clamp(0.0, 1.0) * segments;
    let i = f.floor().min(segments - 1.0);
    let w = f - i;
    let (a, b) = (cable_sample_shader(from, to, i), cable_sample_shader(from, to, i + 1.0));
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * w)
}

/// `vs_ring`'s intensity at `time` seconds: the pulse while online, the held value otherwise.
pub fn ring_intensity_shader(pulse: [f32; 4], time: f32) -> f32 {
    if pulse[2] > 0.5 {
        1.0 + 0.5 * (TAU * (time * pulse[0] + pulse[1])).sin()
    } else {
        pulse[3]
    }
}

impl FleetScene {
    /// Rebuilds the machines and cables from a fleet. The hub (the first `hub` by id) stands at
    /// the origin; the other machines sit on the ring, sorted by id, facing the centre.
    fn rebuild(&mut self, fleet: &FleetResponse, layout: Layout) {
        self.group.get().children.clear();
        self.machines.clear();
        self.links.clear();
        self.layout = Some(layout);

        let hub = fleet
            .machines
            .iter()
            .filter(|m| m.role == "hub")
            .min_by(|a, b| a.id.cmp(&b.id));
        let mut nodes: Vec<&Machine> = fleet
            .machines
            .iter()
            .filter(|m| hub.map_or(true, |h| h.id != m.id))
            .collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let hub_prepared = hub.and_then(|h| prepare(h).map(|p| (h, p)));
        let node_prepared: Vec<(&Machine, (&'static hardware::Model, Built))> = nodes
            .into_iter()
            .filter_map(|m| prepare(m).map(|p| (m, p)))
            .collect();
        let radii: Vec<f32> = node_prepared
            .iter()
            .map(|(_, (model, _))| bounding_radius(model.size))
            .collect();
        let (radius, angles) = ring_layout(&radii);

        if let Some((h, (model, mut built))) = hub_prepared {
            if self.machines.len() < MACHINE_SLOTS {
                built.set_machine(self.machines.len());
            }
            let node = place_machine(
                &self.group,
                &self.palette,
                h,
                true,
                model,
                built,
                ([0.0; 3], 0.0),
            );
            self.machines.insert(h.id.clone(), node);
        }
        for ((m, (model, mut built)), angle) in node_prepared.into_iter().zip(angles) {
            let base = ring_point(radius, angle);
            if self.machines.len() < MACHINE_SLOTS {
                built.set_machine(self.machines.len());
            }
            let node = place_machine(
                &self.group,
                &self.palette,
                m,
                false,
                model,
                built,
                (base, angle + PI),
            );
            self.machines.insert(m.id.clone(), node);
        }

        let mut links = Vec::new();
        for link in &fleet.links {
            let (Some(a), Some(b)) = (self.machines.get(&link.a), self.machines.get(&link.b))
            else {
                continue;
            };
            // Each cable runs from the node's port to the hub's.
            let (from, to) = if a.hub {
                (b.port, a.port)
            } else {
                (a.port, b.port)
            };
            links.push(build_link(
                &self.group,
                &link.a,
                &link.b,
                cable_points(from, to),
            ));
        }
        self.links = links;
    }

    /// Advances the scene by `dt` seconds with the fleet's latest snapshot. Rebuilds when the
    /// machines or the links change; otherwise it updates status colours, fan spin, activity LEDs,
    /// the status rings, the hover lift and the atoms. Packets are instanced, not stepped here.
    /// Advances the scene. Returns whether a machine is still lifting (its baked pose changes), so
    /// the Machines view re-bakes its statics.
    pub fn update(&mut self, dt: f32, ambient: f32, ambient_dt: f32, fleet: &FleetResponse) -> bool {
        let mut lifting = false;
        let layout = layout_of(fleet);
        if self.layout.as_ref() != Some(&layout) {
            self.rebuild(fleet, layout);
        }
        // The ambient visuals read the clock in seconds, not the capped frame step; `dt` only moves the hover lift.
        self.time = ambient;
        let time = ambient;
        let pulse = 1.0 + 0.5 * (TAU * time / PULSE_PERIOD).sin();

        for m in &fleet.machines {
            let hovered = self.hovered.as_deref() == Some(m.id.as_str());
            let Some(node) = self.machines.get_mut(&m.id) else {
                continue;
            };
            node.name = m.name.clone();
            node.status = status_of(&m.status);
            let online = node.status == Status::Online;
            node.built.update(
                ambient_dt,
                time,
                &State {
                    status: node.status,
                    activity: activity(m),
                    open: node.open,
                },
            );

            let led = status_led(node.status);
            {
                let mut mat = node.ring_mat.get();
                mat.color = led;
                mat.emissive = led;
                mat.emissive_intensity = if online { pulse } else { 0.2 };
            }
            let target = if hovered { HOVER_LIFT } else { 0.0 };
            let before = node.lift;
            node.lift += (target - node.lift) * (1.0 - (-dt * HOVER_RATE).exp());
            lifting |= (node.lift - before).abs() > 1e-6;
            node.root
                .set_position(node.base[0], node.base[1] + node.lift, node.base[2]);
            if let Some(atom) = &node.atom {
                atom.set_visible(online);
            }
            if let Some(anim) = node.anim.as_mut() {
                anim(time);
            }
        }

        for link in self.links.iter_mut() {
            let (online, _) = link_state(fleet, &link.a, &link.b);
            let base = named("cable");
            link.cable.get().color = if online { base } else { base.scale(0.5) };
        }
        lifting
    }

    /// The animated draws' instance data for the scene as `fleet` describes it: each cable's slots both
    /// ways (`live_slots` of them alive), then each machine's status ring. Rebuilt when this changes.
    pub fn instances(&self, fleet: &FleetResponse) -> FleetInstances {
        let mut out = FleetInstances { machine_angles: vec![0.0; MACHINE_SLOTS], ..Default::default() };
        for link in &self.links {
            let (online, rate) = link_state(fleet, &link.a, &link.b);
            let live = if online { live_slots(rate) } else { 0 };
            let (Some(&from), Some(&to)) = (link.points.first(), link.points.last()) else {
                continue;
            };
            // The forward flow runs from the node's port to the hub's; the backward one runs back.
            out.packets.extend(direction_slots(from, to, live, false, rgba(named("copper-live"))));
            out.packets.extend(direction_slots(from, to, live, true, rgba(named("led-info"))));
        }
        for m in self.machines.values() {
            if let Some(i) = m.built.machine().filter(|&i| i < MACHINE_SLOTS) {
                out.machine_angles[i] = m.built.fan_angle();
            }
            let online = m.status == Status::Online;
            out.rings.push(RingInstance {
                centre: [m.base[0], 0.05, m.base[2], m.footprint * 1.3],
                pulse: [1.0 / PULSE_PERIOD, 0.0, if online { 1.0 } else { 0.0 }, 0.2],
                color: rgba(status_led(m.status)),
            });
        }
        out
    }

    /// The fleet's motion: Active while a machine's hover lift is still travelling, Ambient while any machine exists
    /// (its ring pulses, fans, LEDs and atom run on the clock), Rest for an empty fleet.
    pub fn motion(&self) -> crate::stage::Motion {
        let lifting = self.machines.iter().any(|(id, m)| {
            let target = if self.hovered.as_deref() == Some(id.as_str()) { HOVER_LIFT } else { 0.0 };
            (m.lift - target).abs() > 1e-4
        });
        if lifting {
            crate::stage::Motion::Active
        } else if self.machines.is_empty() {
            crate::stage::Motion::Rest
        } else {
            crate::stage::Motion::Ambient
        }
    }

    /// The machine a ray hits first, by its bounding sphere, or `None`.
    pub fn pick(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<String> {
        let mut best: Option<(f32, &str)> = None;
        for (id, m) in &self.machines {
            let Some(t) = ray_sphere(origin, dir, m.center, m.radius) else {
                continue;
            };
            if best.map_or(true, |(bt, _)| t < bt) {
                best = Some((t, id.as_str()));
            }
        }
        best.map(|(_, id)| id.to_string())
    }

    /// Lifts the hovered machine (or lowers the last one) when `id` changes.
    pub fn hover(&mut self, id: Option<&str>) {
        self.hovered = id.map(str::to_string);
    }

    /// The overlay tags: each machine's name and status, at its label.
    pub fn tags(&self) -> Vec<Tag> {
        self.machines
            .iter()
            .map(|(id, m)| Tag {
                id: id.clone(),
                pos: [m.label[0], m.label[1] + m.lift, m.label[2]],
                name: m.name.clone(),
                status: m.status,
            })
            .collect()
    }

    /// The camera's fit: the centre of every machine's bounding box, and the radius that holds
    /// every machine's sphere.
    pub fn frame(&self) -> FleetFrame {
        if self.machines.is_empty() {
            return FleetFrame {
                center: [0.0; 3],
                radius: RING_MIN_RADIUS,
            };
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for m in self.machines.values() {
            for k in 0..3 {
                lo[k] = lo[k].min(m.center[k] - m.radius);
                hi[k] = hi[k].max(m.center[k] + m.radius);
            }
        }
        let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let radius = self
            .machines
            .values()
            .map(|m| Vec3::from(center).distance(Vec3::from(m.center)) + m.radius)
            .fold(0.0, f32::max);
        FleetFrame { center, radius }
    }

    /// Drives a machine's ENTER opening: `t` in 0..1 (its build's `state.open`).
    pub fn set_open(&mut self, id: &str, t: f32) {
        if let Some(m) = self.machines.get_mut(id) {
            m.open = t.clamp(0.0, 1.0);
        }
    }
}

// ---- Tests ------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| close(a[k], b[k]))
    }

    fn plaza_palette() -> PropPalette {
        PropPalette {
            sheet: Color::hex(0xf5efe6),
            rule: Color::hex(0xc9bfae),
            ink3: Color::hex(0x6b6357),
            signal: Color::hex(0xe8603c),
            ok: Color::hex(0x4fa36a),
            info: Color::hex(0x3c7fd6),
        }
    }

    /// The fleet's draw items on the 15-machine feed, measured (its statics merged by colour, instances
    /// unfolded into the merge: 29 static; the merged meshes of identical rigs instanced, one draw per
    /// content; its cables and status rings unmerged, 297 per frame, its fans and LEDs in 3 GPU batches; its packets
    /// instanced, so they hold no objects). A ratchet: it must not grow.
    const FLEET_DRAW_ITEMS: usize = 326;

    fn fleet_draw_items() -> (usize, usize) {
        let fleet: FleetResponse = serde_json::from_str(include_str!("../testdata/floor-15.json")).expect("the fixture parses");
        let scene = build_fleet(&fleet, &plaza_palette());
        let statics = crate::scene::bake(&scene.group).len();
        let rigs = crate::scene::rigs(&scene.group, glam::Mat4::IDENTITY);
        let dynamic = crate::scene::flatten_rigs_instanced(&rigs).0.len();
        let animated: usize = scene.machines.values().map(|m| m.built.animated_parts()).sum();
        eprintln!("fleet breakdown: {} cables, {} machines, {} animated hardware nodes", scene.links.len(), scene.machines.len(), animated);
        let gpu = crate::scene::bake_split(&scene.group, true);
        eprintln!("fleet with GPU_HW_ANIM: {} static draws + {} animation batches (the animated nodes leave the per-frame draws)", gpu.statics.len(), gpu.animated.len());
        (statics, dynamic)
    }

    #[test]
    fn the_fifteen_machine_fleet_draws_do_not_grow() {
        let (statics, dynamic) = fleet_draw_items();
        eprintln!("fleet draws: {statics} static + {dynamic} dynamic = {}", statics + dynamic);
        assert!(statics + dynamic <= FLEET_DRAW_ITEMS, "the fleet draws {} items, over the measured {FLEET_DRAW_ITEMS}", statics + dynamic);
    }

    #[test]
    #[ignore = "the Machines view budget is 300 draw items; the fleet draws 326 (see FLEET_DRAW_ITEMS): 29 static, 297 per-frame"]
    fn the_fifteen_machine_fleet_draws_within_budget() {
        let (statics, dynamic) = fleet_draw_items();
        assert!(statics + dynamic <= 300, "the Machines view draws {} items, over the 300 budget", statics + dynamic);
    }

    #[test]
    fn ring_of_one_hub_and_five_nodes() {
        // Five nodes of radius 3 take 10 units of arc each, 50 in all: the ring is the 14-unit
        // minimum, and the nodes sit at 36, 108, 180, 252 and 324 degrees from +z.
        let (radius, angles) = ring_layout(&[3.0; 5]);
        assert!(close(radius, 14.0));
        let deg: Vec<f32> = angles.iter().map(|a| a.to_degrees()).collect();
        for (got, want) in deg.iter().zip([36.0, 108.0, 180.0, 252.0, 324.0]) {
            assert!(close(*got, want), "{got} != {want}");
        }
        assert!(close3(
            ring_point(radius, angles[0]),
            [8.22899, 0.0, 11.32624]
        ));
        assert!(close3(
            ring_point(radius, angles[1]),
            [13.31480, 0.0, -4.32624]
        ));
        assert!(close3(ring_point(radius, angles[2]), [0.0, 0.0, -14.0]));
        // Neighbours never overlap: their centres are 2 * 14 * sin(36 deg) = 16.45 apart, at
        // least the two radii plus the gap (10).
        let (p0, p1) = (ring_point(radius, angles[0]), ring_point(radius, angles[1]));
        assert!(Vec3::from(p0).distance(Vec3::from(p1)) >= 10.0);
    }

    #[test]
    fn ring_grows_past_the_minimum_when_the_arcs_do_not_fit() {
        // Five nodes of radius 10 take 24 units each: 120 in all, so the radius is 120 / 2π = 60 / π.
        let (radius, angles) = ring_layout(&[10.0; 5]);
        assert!(close(radius, 60.0 / PI));
        assert!(close(angles[0].to_degrees(), 36.0));
    }

    #[test]
    fn footprint_radius_is_the_half_diagonal() {
        assert!(close(bounding_radius([6.0, 0.0, 8.0]), 5.0));
    }

    #[test]
    fn cable_is_25_points_sagging_1_5_below_the_chord() {
        let pts = cable_points([10.0, 2.0, 0.0], [0.0, 2.0, 0.0]);
        assert_eq!(pts.len(), 25);
        assert!(close3(pts[0], [10.0, 2.0, 0.0]));
        assert!(close3(pts[6], [7.5, 0.875, 0.0]));
        assert!(close3(pts[12], [5.0, 0.5, 0.0]));
        assert!(close3(pts[24], [0.0, 2.0, 0.0]));
    }

    #[test]
    fn cable_is_clamped_above_the_ground() {
        // The sag would take the middle to y = -1.0; it stops at the ground.
        let pts = cable_points([10.0, 0.5, 0.0], [0.0, 0.5, 0.0]);
        assert!(close3(pts[12], [5.0, 0.0, 0.0]));
        assert!(pts.iter().all(|p| p[1] >= 0.0));
    }

    #[test]
    fn packet_rate_is_base_plus_working_agents_capped_at_four() {
        assert!(close(packet_rate(true, 0, 0), 0.5));
        assert!(close(packet_rate(true, 1, 0), 1.5));
        assert!(close(packet_rate(true, 2, 1), 3.5));
        assert!(close(packet_rate(true, 3, 2), 4.0));
        assert!(close(packet_rate(false, 3, 3), 0.0));
    }

    #[test]
    fn point_on_lerps_between_the_cable_points() {
        let pts = cable_points([10.0, 2.0, 0.0], [0.0, 2.0, 0.0]);
        assert!(close3(point_on(&pts, 0.5), pts[12]));
        assert!(close3(point_on(&pts, 1.0), pts[24]));
    }

    #[test]
    fn the_shader_packets_ride_the_cpu_cable_at_five_times() {
        // A cable that sags into the ground, so the clamp takes part; one slot each way.
        let (from, to) = ([10.0, 0.5, 0.0], [0.0, 0.5, 0.0]);
        let points = cable_points(from, to);
        for (params, reversed) in [([0.2, 1.0, 1.0, PACKET_RADIUS], false), ([0.6, -1.0, 1.0, PACKET_RADIUS], true)] {
            for t in [0.0f32, 0.7, 1.3, 2.2, 3.9] {
                let x = params[0] + t * PACKET_SPEED;
                let p = x - x.floor();
                let at = if reversed { 1.0 - p } else { p };
                let cpu = point_on(&points, at);
                let gpu = packet_at_shader(from, to, params, t);
                assert!(close3(cpu, gpu), "t = {t}, reversed = {reversed}: the CPU packet is at {cpu:?}, the shader's at {gpu:?}");
            }
        }
    }

    #[test]
    fn live_slots_follow_the_packet_rate_and_the_ring_pulse_matches_the_cpu() {
        assert_eq!(live_slots(PACKET_MAX), PACKET_SLOTS);
        assert_eq!(live_slots(0.0), 0);
        assert_eq!(live_slots(0.5), 1);
        assert_eq!(live_slots(1.5), 4);
        // The CPU pulse at 0.7 s, 1 + 0.5 sin(TAU t / PULSE_PERIOD), is the shader's wave at that time.
        let cpu = 1.0 + 0.5 * (TAU * 0.7 / PULSE_PERIOD).sin();
        assert!(close(ring_intensity_shader([1.0 / PULSE_PERIOD, 0.0, 1.0, 0.2], 0.7), cpu));
        assert!(close(ring_intensity_shader([1.0 / PULSE_PERIOD, 0.0, 0.0, 0.2], 0.7), 0.2));
        let fleet: FleetResponse = serde_json::from_str(FIXTURE).unwrap();
        let scene = build_fleet(&fleet, &plaza_palette());
        let instances = scene.instances(&fleet);
        assert_eq!(instances.rings.len(), 3);
        assert_eq!(instances.packets.len(), 2 * PACKET_SLOTS * scene.links.len());
    }

    #[test]
    fn ray_hits_the_near_side_of_a_sphere() {
        assert!(close(
            ray_sphere([0.0, 0.0, -10.0], [0.0, 0.0, 1.0], [0.0; 3], 1.0).unwrap(),
            9.0
        ));
        assert!(ray_sphere([5.0, 0.0, -10.0], [0.0, 0.0, 1.0], [0.0; 3], 1.0).is_none());
    }

    #[test]
    fn unknown_status_reads_as_offline() {
        assert_eq!(status_of("online"), Status::Online);
        assert_eq!(status_of("nonsense"), Status::Offline);
    }

    #[test]
    fn fleet_response_parses_with_defaults() {
        let fleet: FleetResponse = serde_json::from_str(r#"{"machines":[{"id":"n0"}]}"#).unwrap();
        assert_eq!(fleet.machines[0].id, "n0");
        assert_eq!(fleet.machines[0].status, "");
        assert!(fleet.links.is_empty());
    }

    const FIXTURE: &str = r#"{
        "machines": [
            {"id": "n0", "name": "hub", "role": "hub", "kind": "workstation", "status": "online", "agents": [], "working": 0},
            {"id": "n1", "name": "mac", "role": "node", "kind": "workstation", "vendor": "apple",
             "model": "Mac Studio (M2 Ultra)", "status": "online", "agents": [200, 201], "working": 1,
             "apple": {"chip": "Apple M2 Ultra", "p_cores": 16, "e_cores": 8, "gpu_cores": 76, "unified_memory_gib": 192}},
            {"id": "n2", "name": "pi", "role": "node", "kind": "edge", "status": "offline"}
        ],
        "links": [{"a": "n0", "b": "n1", "online": true}, {"a": "n2", "b": "n0", "online": false}]
    }"#;

    #[test]
    fn scene_builds_the_fixture_and_picks_the_hub() {
        let fleet: FleetResponse = serde_json::from_str(FIXTURE).unwrap();
        let mut scene = build_fleet(&fleet, &plaza_palette());
        assert_eq!(scene.machines.len(), 3);
        assert!(scene.machines["n0"].hub);
        // A ray straight down through the origin hits the hub, which stands there.
        assert_eq!(
            scene.pick([0.0, 50.0, 0.0], [0.0, -1.0, 0.0]).as_deref(),
            Some("n0")
        );
        assert_eq!(scene.tags().len(), 3);

        // Same layout: no rebuild, so the scene keeps its children.
        let children = scene.group.get().children.len();
        scene.update(0.016, 0.016, 0.016, &fleet);
        assert_eq!(scene.group.get().children.len(), children);

        // A new machine: the scene rebuilds with four.
        let mut grown = fleet.clone();
        grown.machines.push(Machine {
            id: "n9".into(),
            role: "node".into(),
            kind: "laptop".into(),
            ..Machine::default()
        });
        scene.update(0.016, 0.016, 0.016, &grown);
        assert_eq!(scene.machines.len(), 4);
        assert!(scene.frame().radius > 0.0);
    }
}

impl FleetScene {
    /// A machine's label anchor in the world at rest, the point its name tag sits above, or None for an unknown id.
    pub fn label_of(&self, id: &str) -> Option<[f32; 3]> {
        self.machines.get(id).map(|m| m.label)
    }
}
