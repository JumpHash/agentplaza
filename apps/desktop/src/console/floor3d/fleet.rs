//! The Machines view's scene, native (`apps/web/src/views/floor3d/fleet.ts`, number for number).
//! Each machine is its exterior hardware model (`hardware.rs`). The hub stands at the origin and
//! the nodes sit on a ring around it. A status ring lies under each machine, and a cable runs
//! from each node's port to the hub's, with light packets while agents work. `build_fleet` builds
//! the scene; `FleetScene` is its query API (`update`, `pick`, `hover`, `tags`, `frame`,
//! `set_open`). It has no UI: the Floor integration consumes it next, hence the module allow.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::OnceLock;

use glam::{Mat4, Quat, Vec3};
use serde::Deserialize;

use super::geometry;
use super::hardware::{self, Built, Params, State, Status};
use super::props::{self, PropPalette};
use super::scene::{Color, Geo, Mat, Material, Obj, Shading};

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
    #[serde(default)]
    pub memory_mib: u64,
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

const PALETTE_SRC: &str = include_str!("../../../../../assets/hardware/palette.json");

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

fn status_led(s: Status) -> Color {
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
    lift: f32,
    /// How far it is opened (ENTER), 0..1.
    open: f32,
    /// The plaza's atom at `anchors.atom`, shown only while online.
    atom: Option<Obj>,
    anim: Option<props::Anim>,
}

struct Packet {
    obj: Obj,
    /// Progress along the cable, 0..1, or `None` when the packet is free.
    p: Option<f32>,
}

/// The packets going one way along a cable.
struct Flow {
    acc: f32,
    reversed: bool,
    packets: Vec<Packet>,
}

struct LinkScene {
    a: String,
    b: String,
    cable: Mat,
    points: Vec<[f32; 3]>,
    forward: Flow,
    backward: Flow,
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
    let group = Obj::group();
    group.set_dynamic(true);
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
    built: Built,
    pose: ([f32; 3], f32),
) -> MachineScene {
    let (base, turn) = pose;
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
    group.add(&ring);

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

fn flow(group: &Obj, geo: &Geo, color: Color, reversed: bool) -> Flow {
    let mat = Mat::new(Material {
        shading: Shading::Basic,
        color,
        emissive: color,
        emissive_intensity: 2.0,
        additive: true,
        transparent: true,
        depth_write: false,
        ..Material::default()
    });
    let packets = (0..PACKET_POOL)
        .map(|_| {
            let obj = Obj::mesh(geo, &mat);
            obj.set_visible(false);
            group.add(&obj);
            Packet { obj, p: None }
        })
        .collect();
    Flow {
        acc: 0.0,
        reversed,
        packets,
    }
}

/// A cable: its segments, and its two flows of packets.
fn build_link(group: &Obj, a: &str, b: &str, points: Vec<[f32; 3]>) -> LinkScene {
    let cable = Mat::toon(named("cable"));
    let segment = geometry::cylinder(CABLE_RADIUS, CABLE_RADIUS, 1.0, 6);
    for pair in points.windows(2) {
        let (p0, p1) = (Vec3::from(pair[0]), Vec3::from(pair[1]));
        let d = p1 - p0;
        let len = d.length();
        if len < 1e-6 {
            continue;
        }
        let mid = (p0 + p1) * 0.5;
        let seg = Obj::mesh(&segment, &cable);
        seg.set_position(mid.x, mid.y, mid.z)
            .set_quaternion(Quat::from_rotation_arc(Vec3::Y, d / len))
            .set_scale(1.0, len, 1.0);
        group.add(&seg);
    }
    let packet = geometry::sphere(0.14, 8, 6);
    LinkScene {
        a: a.to_string(),
        b: b.to_string(),
        cable,
        forward: flow(group, &packet, named("copper-live"), false),
        backward: flow(group, &packet, named("led-info"), true),
        points,
    }
}

/// Moves one flow's packets: spawns at its rate, advances them, and hides the ones that arrive.
fn step_flow(flow: &mut Flow, dt: f32, rate: f32, points: &[[f32; 3]]) {
    if rate <= 0.0 {
        flow.acc = 0.0;
        for pk in flow.packets.iter_mut() {
            pk.p = None;
            pk.obj.set_visible(false);
        }
        return;
    }
    flow.acc += rate * dt;
    while flow.acc >= 1.0 {
        flow.acc -= 1.0;
        if let Some(pk) = flow.packets.iter_mut().find(|pk| pk.p.is_none()) {
            pk.p = Some(0.0);
        }
    }
    for pk in flow.packets.iter_mut() {
        let Some(p) = pk.p else {
            pk.obj.set_visible(false);
            continue;
        };
        let p = p + PACKET_SPEED * dt;
        if p >= 1.0 {
            pk.p = None;
            pk.obj.set_visible(false);
            continue;
        }
        pk.p = Some(p);
        let at = if flow.reversed { 1.0 - p } else { p };
        let q = point_on(points, at);
        pk.obj.set_position(q[0], q[1], q[2]).set_visible(true);
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

        if let Some((h, (model, built))) = hub_prepared {
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
        for ((m, (model, built)), angle) in node_prepared.into_iter().zip(angles) {
            let base = ring_point(radius, angle);
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
    /// the status rings, the hover lift, the atoms and the packets.
    pub fn update(&mut self, dt: f32, fleet: &FleetResponse) {
        let layout = layout_of(fleet);
        if self.layout.as_ref() != Some(&layout) {
            self.rebuild(fleet, layout);
        }
        self.time += dt;
        let time = self.time;
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
                dt,
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
            node.lift += (target - node.lift) * (1.0 - (-dt * HOVER_RATE).exp());
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
            let online = fleet
                .links
                .iter()
                .find(|l| l.a == link.a && l.b == link.b)
                .is_some_and(|l| l.online);
            let working = |id: &str| {
                fleet
                    .machines
                    .iter()
                    .find(|m| m.id == id)
                    .map_or(0, |m| m.working)
            };
            let rate = packet_rate(online, working(&link.a), working(&link.b));
            let base = named("cable");
            link.cable.get().color = if online { base } else { base.scale(0.5) };
            step_flow(&mut link.forward, dt, rate, &link.points);
            step_flow(&mut link.backward, dt, rate, &link.points);
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
        scene.update(0.016, &fleet);
        assert_eq!(scene.group.get().children.len(), children);

        // A new machine: the scene rebuilds with four.
        let mut grown = fleet.clone();
        grown.machines.push(Machine {
            id: "n9".into(),
            role: "node".into(),
            kind: "laptop".into(),
            ..Machine::default()
        });
        scene.update(0.016, &grown);
        assert_eq!(scene.machines.len(), 4);
        assert!(scene.frame().radius > 0.0);
    }
}
