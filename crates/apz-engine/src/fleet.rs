//! The Machines view's scene: every machine of the fleet as its kit model (`kit`), standing on a plinth in a grid of
//! zones (the hub, compute, GPU, desktops and devices, workstations). Each plinth carries the machine's CPU and memory
//! bars; each model's status light and fans run on the GPU from the fleet's tables ([`FleetInstances`]). The camera is
//! orthographic: the zones read as a plan. `build_fleet` builds the scene; `FleetScene` is its query API (`update`,
//! `instances`, `pick`, `hover`, `tags`, `frame`, `label_of`).

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::sync::Arc;

use glam::Vec3;
use serde::{Deserialize, Deserializer};

use super::geometry::{self, edges::feature_edges};
use super::kit;
use super::scene::{Color, Geo, Geometry, Mat, Material, Obj, Shading};
use super::style::{Lamp, Palette};

// ---- Data: GET /v1/fleet ----------------------------------------------------------------------

/// A null field reads as its default. The hub sends null for what a machine has not reported yet (the hub's own node
/// before it starts, as on a Mac), and one null must not refuse the whole fleet: that left the 3D view empty.
fn nullable<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// `GET /v1/fleet`: the machines and the links between them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FleetResponse {
    #[serde(default, deserialize_with = "nullable")]
    pub machines: Vec<Machine>,
    #[serde(default, deserialize_with = "nullable")]
    pub links: Vec<Link>,
}

/// One machine. `role` is `hub` or `node`; `status` is `online`, `draining`, `offline` or `revoked`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Machine {
    #[serde(default, deserialize_with = "nullable")]
    pub id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub role: String,
    #[serde(default, deserialize_with = "nullable")]
    pub kind: String,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub apple: Option<Apple>,
    #[serde(default, deserialize_with = "nullable")]
    pub status: String,
    #[serde(default, deserialize_with = "nullable")]
    pub arch: String,
    #[serde(default, deserialize_with = "nullable")]
    pub os: String,
    #[serde(default, deserialize_with = "nullable")]
    pub backend: String,
    #[serde(default, deserialize_with = "nullable")]
    pub cpu: Cpu,
    #[serde(default, deserialize_with = "nullable")]
    pub memory_mib: u64,
    #[serde(default, deserialize_with = "nullable")]
    pub gpus: Vec<Gpu>,
    #[serde(default, deserialize_with = "nullable")]
    pub storage_gib: u64,
    /// The cids of the agents running here.
    #[serde(default, deserialize_with = "nullable")]
    pub agents: Vec<u64>,
    /// Of those agents, how many are working.
    #[serde(default, deserialize_with = "nullable")]
    pub working: u32,
    #[serde(default)]
    pub stats: Option<Stats>,
    #[serde(default, deserialize_with = "nullable")]
    pub last_seen_ms: u64,
    /// A workstation's facts (the fleet's `station` field); `None` for the hub and the nodes.
    #[serde(default)]
    pub station: Option<Station>,
}

/// A workstation on the Floor: its OS family and form pick its model, and its active leases light its screen.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Station {
    /// `macos`, `windows` or `linux`.
    #[serde(default, deserialize_with = "nullable")]
    pub os: String,
    /// `laptop`, `desktop`, `mini`, `server` or `vm`.
    #[serde(default, deserialize_with = "nullable")]
    pub form: String,
    /// Every lease, queued ones included.
    #[serde(default, deserialize_with = "nullable")]
    pub leases: u32,
    /// The leases that are active (the agent works on the machine now).
    #[serde(default, deserialize_with = "nullable")]
    pub active: u32,
}

/// A Mac's chip and core counts (only on Macs).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Apple {
    #[serde(default, deserialize_with = "nullable")]
    pub chip: String,
    #[serde(default, deserialize_with = "nullable")]
    pub p_cores: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub e_cores: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub gpu_cores: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub neural_cores: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub unified_memory_gib: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Cpu {
    #[serde(default, deserialize_with = "nullable")]
    pub model: String,
    #[serde(default, deserialize_with = "nullable")]
    pub cores: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub threads: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Gpu {
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub vendor: String,
    /// Null when the hub cannot read the GPU's memory.
    #[serde(default)]
    pub memory_mib: Option<u64>,
}

/// Live load; remote nodes send it with every heartbeat.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Stats {
    #[serde(default, deserialize_with = "nullable")]
    pub cpu_pct: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub mem_used_mib: u64,
}

/// A cable between two machines, by id.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Link {
    #[serde(default, deserialize_with = "nullable")]
    pub a: String,
    #[serde(default, deserialize_with = "nullable")]
    pub b: String,
    #[serde(default, deserialize_with = "nullable")]
    pub online: bool,
}

/// Ids compared as `localeCompare(…, { numeric: true })` does, a run of digits by its value (n2
/// before n10), the rest ignoring case.
pub fn by_id(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for ((da, ra), (db, rb)) in ca.iter().zip(cb.iter()) {
        let ord = if *da && *db {
            let (ta, tb) = (ra.trim_start_matches('0'), rb.trim_start_matches('0'));
            ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb))
        } else {
            ra.to_lowercase().cmp(&rb.to_lowercase())
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len()).then_with(|| a.cmp(b))
}

// ---- Layout -------------------------------------------------------------------------------------

/// The distance between two cells' centres, in kit units.
pub const PITCH: f32 = 5.2;
/// The plinth a machine stands on: its side and its height.
pub const PLINTH: f32 = 4.4;
pub const PLINTH_H: f32 = 0.25;
/// The gap between two zones, beyond the cells' own spacing.
const ZONE_GAP: f32 = 2.2;
/// How far the zone plate reaches past its outer plinths.
const ZONE_MARGIN: f32 = 0.5;
/// The models stand on their plinth at this scale, a little back from its front edge (the bars are in front).
const MODEL_SCALE: f32 = 0.9;
const MODEL_BACK: f32 = 0.35;
/// The load bars: their length, depth and the z of each (CPU, then memory) on the plinth.
const BAR_W: f32 = 3.6;
const BAR_D: f32 = 0.18;
const BAR_Z: [f32; 2] = [1.55, 1.85];

const HOVER_LIFT: f32 = 0.3;
const HOVER_RATE: f32 = 10.0;

/// The fleet's tables (fan angles, machine rows, status colours): one entry per machine slot.
pub const MACHINE_SLOTS: usize = 128;
/// The clock the GPU animations read wraps at this many seconds.
pub const WRAP_PERIOD: f32 = 3600.0;

/// A machine's connection state, from the fleet's `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Online,
    Draining,
    Offline,
    Revoked,
}

/// The status string as a [`Status`]. An unknown status reads as offline.
pub fn status_of(text: &str) -> Status {
    match text {
        "online" => Status::Online,
        "draining" => Status::Draining,
        "revoked" => Status::Revoked,
        _ => Status::Offline,
    }
}

/// What a machine's status light shows: working (online with an agent at work), idle, strained (draining) or offline.
pub fn lamp_of(m: &Machine) -> Lamp {
    lamp_for(status_of(&m.status), m)
}

/// The status light for `m` once its status is parsed as `status`, so the frame's update parses it once.
fn lamp_for(status: Status, m: &Machine) -> Lamp {
    match status {
        Status::Online if m.working > 0 => Lamp::Working,
        Status::Online => Lamp::Idle,
        Status::Draining => Lamp::Strained,
        Status::Offline | Status::Revoked => Lamp::Offline,
    }
}

/// The share of a machine's agents that are working, 0..1.
fn activity(m: &Machine) -> f32 {
    if m.agents.is_empty() {
        return 0.0;
    }
    (m.working as f32 / m.agents.len() as f32).clamp(0.0, 1.0)
}

/// A machine's CPU and memory load, 0..1 each (0 without live stats).
pub fn load_of(m: &Machine) -> (f32, f32) {
    let Some(stats) = &m.stats else {
        return (0.0, 0.0);
    };
    let cpu = (stats.cpu_pct / 100.0).clamp(0.0, 1.0) as f32;
    let mem = if m.memory_mib > 0 {
        (stats.mem_used_mib as f64 / m.memory_mib as f64).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    (cpu, mem)
}

/// The zones of the grid, in the order they stand from left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Zone {
    Hub,
    Compute,
    Gpu,
    Desktops,
    Workstations,
}

impl Zone {
    /// The zone's name as its label shows it.
    pub fn label(self) -> &'static str {
        match self {
            Zone::Hub => "Hub",
            Zone::Compute => "Compute",
            Zone::Gpu => "GPU",
            Zone::Desktops => "Desktops and devices",
            Zone::Workstations => "Workstations",
        }
    }
}

/// The zone a machine stands in: the hub, the workstations (stations agents operate), the machines with GPUs, the
/// servers and edge boxes, and everything else.
pub fn zone_of(m: &Machine) -> Zone {
    if m.role == "hub" {
        return Zone::Hub;
    }
    if m.role == "station" || m.station.is_some() {
        return Zone::Workstations;
    }
    let apple = m.vendor.as_deref() == Some("apple");
    match m.kind.as_str() {
        "server" | "workstation" if !apple && !m.gpus.is_empty() => Zone::Gpu,
        "server" | "edge" => Zone::Compute,
        _ => Zone::Desktops,
    }
}

/// The kit's level of detail for a fleet of `count` machines: full detail up to 32 machines, the simple level up to 96,
/// and the block beyond, so a large fleet's grid stays inside the frame budget (`tests/machines_budget.rs`).
pub fn lod_for(count: usize) -> usize {
    match count {
        0..=32 => 0,
        33..=96 => 1,
        _ => 2,
    }
}

/// The kit model a machine draws as (see `assets/hardware/kit`).
pub fn kit_model_of(m: &Machine) -> &'static str {
    if m.role == "hub" {
        return "hub";
    }
    if let Some(st) = &m.station {
        return match (st.os.as_str(), st.form.as_str()) {
            (_, "vm") => "rack",
            (_, "laptop") => "laptop",
            (_, "server") => "server",
            ("macos", _) => "mini",
            _ => "tower",
        };
    }
    if m.vendor.as_deref() == Some("apple") {
        return match m.kind.as_str() {
            "laptop" => "laptop",
            _ if m.model.as_deref().is_some_and(|n| n.contains("Studio")) => "studio",
            _ => "mini",
        };
    }
    match m.kind.as_str() {
        "server" if !m.gpus.is_empty() => "gpu",
        "server" => "server",
        "workstation" if !m.gpus.is_empty() => "gpu",
        "edge" => "edge",
        "phone" => "phone",
        "laptop" => "laptop",
        _ => "tower",
    }
}

/// One zone of the grid: which, and its plate's extent on the ground (x and z, min and max).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneRect {
    pub zone: Zone,
    pub min: [f32; 2],
    pub max: [f32; 2],
}

/// The grid: each machine's cell centre on the ground (by id), and the zones' plates. Each zone is a block of cells
/// filled row by row, its machines sorted by id, roughly square. The zones are shelved in [`Zone`] order: left to right,
/// wrapping to a new row of zones in front of the last once a row is about as wide as the whole grid should be (a wide
/// rectangle, like the view it fills). The whole grid is centred on the origin.
pub fn grid_layout(machines: &[&Machine]) -> (Vec<(String, Zone, [f32; 3])>, Vec<ZoneRect>) {
    let mut by_zone: BTreeMap<Zone, Vec<&Machine>> = BTreeMap::new();
    for m in machines {
        by_zone.entry(zone_of(m)).or_default().push(m);
    }
    // Each zone's columns and rows, then the width a row of zones may reach: the grid's area at 3:1 on the ground, which
    // the plan view's tilt foreshortens to about 16:9 on screen.
    let blocks: Vec<(Zone, Vec<&Machine>, usize, usize)> = by_zone
        .into_iter()
        .map(|(zone, mut list)| {
            list.sort_by(|a, b| by_id(&a.id, &b.id));
            let cols = ((list.len() as f32).sqrt().ceil() as usize).max(1);
            let rows = list.len().div_ceil(cols);
            (zone, list, cols, rows)
        })
        .collect();
    let width_of = |cols: usize| cols as f32 * PITCH + ZONE_GAP;
    let depth_of = |rows: usize| rows as f32 * PITCH + ZONE_GAP;
    let area: f32 = blocks.iter().map(|b| width_of(b.2) * depth_of(b.3)).sum();
    let widest = blocks.iter().map(|b| width_of(b.2)).fold(0.0, f32::max);
    let shelf = (area * 3.0).sqrt().max(widest);

    let mut cells = Vec::new();
    let mut rects = Vec::new();
    let (mut x0, mut z0, mut row_depth) = (0.0f32, 0.0f32, 0.0f32);
    for (zone, list, cols, rows) in blocks {
        if x0 > 0.0 && x0 + width_of(cols) > shelf + PITCH / 2.0 {
            x0 = 0.0;
            z0 += row_depth;
            row_depth = 0.0;
        }
        for (i, m) in list.iter().enumerate() {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            cells.push((m.id.clone(), zone, [x0 + c * PITCH, 0.0, z0 + r * PITCH]));
        }
        let half = PLINTH / 2.0 + ZONE_MARGIN;
        rects.push(ZoneRect {
            zone,
            min: [x0 - half, z0 - half],
            max: [
                x0 + (cols - 1) as f32 * PITCH + half,
                z0 + (rows - 1) as f32 * PITCH + half,
            ],
        });
        x0 += width_of(cols);
        row_depth = row_depth.max(depth_of(rows));
    }
    // Centre the grid on the origin.
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for r in &rects {
        lo = [lo[0].min(r.min[0]), lo[1].min(r.min[1])];
        hi = [hi[0].max(r.max[0]), hi[1].max(r.max[1])];
    }
    if rects.is_empty() {
        return (cells, rects);
    }
    let (cx, cz) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    for (_, _, p) in cells.iter_mut() {
        p[0] -= cx;
        p[2] -= cz;
    }
    for r in rects.iter_mut() {
        r.min = [r.min[0] - cx, r.min[1] - cz];
        r.max = [r.max[0] - cx, r.max[1] - cz];
    }
    (cells, rects)
}

/// What decides the scene's shape: each machine's id, model and zone. A change rebuilds the scene; anything else
/// (names, status, load) updates it in place.
#[derive(Debug, Clone, PartialEq)]
struct Layout(Vec<(String, &'static str, Zone)>);

fn layout_of(fleet: &FleetResponse) -> Layout {
    let mut v: Vec<(String, &'static str, Zone)> = fleet
        .machines
        .iter()
        .map(|m| (m.id.clone(), kit_model_of(m), zone_of(m)))
        .collect();
    v.sort_by(|a, b| by_id(&a.0, &b.0));
    Layout(v)
}

// ---- The scene ----------------------------------------------------------------------------------

/// One machine in the scene.
pub struct MachineScene {
    pub root: Obj,
    /// Its cell centre on the ground, and its kit model's size (scaled as it stands).
    pub base: [f32; 3],
    pub size: Vec3,
    /// Its label anchor in the world.
    pub label: [f32; 3],
    pub zone: Zone,
    pub model: &'static str,
    pub status: Status,
    pub lamp: Lamp,
    pub name: String,
    /// Its slot in the fleet's tables (None past `MACHINE_SLOTS`).
    pub slot: Option<usize>,
    lift: f32,
    fan_angle: f32,
    activity: f32,
    /// Its load bars' fills (CPU, memory), dynamic, and the loads they show.
    fills: [Obj; 2],
    load: [f32; 2],
}

/// The scene: its group, the machines by id, and the zones. Build it with [`build_fleet`].
pub struct FleetScene {
    pub group: Obj,
    pub machines: BTreeMap<String, MachineScene>,
    pub zones: Vec<ZoneRect>,
    layout: Option<Layout>,
    palette: Palette,
    hovered: Option<String>,
    time: f32,
    /// The fleet's machine indices in id order, kept between frames so the layout check needs no allocation.
    order: Vec<usize>,
}

/// A tag for the overlay: the machine's name and its lamp, above its label.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub id: String,
    pub pos: [f32; 3],
    pub name: String,
    pub lamp: Lamp,
}

/// What the perspective camera fits: a centre and a radius (the plan view fits [`FleetScene::outline`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FleetFrame {
    pub center: [f32; 3],
    pub radius: f32,
}

/// The fleet's GPU tables: each machine's fan angle, its row (busy, activity, hover lift, 0) and its status colour,
/// by its slot. Each table is an inline array `MACHINE_SLOTS` long, laid out as the Globals block's tables hold them,
/// so a frame builds them with no allocation of their own.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetInstances {
    pub machine_angles: [f32; MACHINE_SLOTS],
    pub machine_states: [[f32; 4]; MACHINE_SLOTS],
    pub machine_statuses: [[f32; 4]; MACHINE_SLOTS],
}

impl FleetInstances {
    /// Every table zero: no machine, no fan turning.
    pub const ZERO: FleetInstances = FleetInstances {
        machine_angles: [0.0; MACHINE_SLOTS],
        machine_states: [[0.0; 4]; MACHINE_SLOTS],
        machine_statuses: [[0.0; 4]; MACHINE_SLOTS],
    };
}

impl Default for FleetInstances {
    fn default() -> Self {
        Self::ZERO
    }
}

/// A colour as RGBA, opaque.
pub(crate) fn rgba(c: Color) -> [f32; 4] {
    let [r, g, b] = c.to_array();
    [r, g, b, 1.0]
}

/// The activity LEDs' on and off colours (hw_anim.wgsl's `leds`).
pub fn led_colours() -> [[f32; 4]; 2] {
    [rgba(Color::hex(0x4fa36a)), rgba(Color::hex(0x4a4f5c))]
}

/// Builds the Machines view from a fleet snapshot, in `palette`'s look.
pub fn build_fleet(fleet: &FleetResponse, palette: &Palette) -> FleetScene {
    let mut scene = FleetScene {
        group: Obj::group(),
        machines: BTreeMap::new(),
        zones: Vec::new(),
        layout: None,
        palette: palette.clone(),
        hovered: None,
        time: 0.0,
        order: Vec::new(),
    };
    scene.rebuild(fleet, layout_of(fleet));
    scene
}

/// A mesh node in `mat`, with its feature edges as a child in `edges` when the palette draws them.
fn with_edges(geo: &Geo, mat: &Mat, edges: Option<&Mat>) -> Obj {
    let node = Obj::mesh(geo, mat);
    if let Some(edge_mat) = edges {
        let pairs = feature_edges(&geo.positions, &geo.indices, kit::EDGE_ANGLE);
        if !pairs.is_empty() {
            let lines = Geometry::new(geo.positions.clone(), Vec::new(), Vec::new(), pairs);
            node.add(&Obj::mesh(&lines, edge_mat));
        }
    }
    node
}

/// The plain material of `color` in `palette`'s shading.
fn surface(palette: &Palette, color: Color) -> Mat {
    Mat::new(Material {
        shading: if palette.flat {
            Shading::Basic
        } else {
            Shading::Toon
        },
        color,
        ..Material::default()
    })
}

/// The shared parts of every plinth: its geometry, the bar geometry and their materials.
struct Parts {
    plinth: Geo,
    bar: Geo,
    plinth_mat: Mat,
    track_mat: Mat,
    fill_mats: [Mat; 2],
    plate_mat: Mat,
    edge_mat: Option<Mat>,
}

impl Parts {
    fn new(palette: &Palette) -> Parts {
        let track = palette.ground.lerp(palette.plinth, 0.5).scale(0.8);
        Parts {
            plinth: geometry::rounded_box(PLINTH, PLINTH_H, PLINTH, 2, 0.12),
            bar: geometry::box_geo(1.0, 0.04, BAR_D),
            plinth_mat: surface(palette, palette.plinth),
            track_mat: surface(palette, track),
            fill_mats: [
                Mat::new(Material {
                    shading: Shading::Basic,
                    color: palette.cpu,
                    ..Material::default()
                }),
                Mat::new(Material {
                    shading: Shading::Basic,
                    color: palette.memory,
                    ..Material::default()
                }),
            ],
            plate_mat: surface(palette, palette.ground.lerp(palette.plinth, 0.35)),
            edge_mat: palette.edge_material().map(Mat::new),
        }
    }
}

/// Sets a load bar's fill to `load` (0..1): it grows from the bar's left end.
fn set_fill(fill: &Obj, load: f32) {
    let w = (BAR_W * load).max(0.001);
    let z = fill.get().position.z;
    fill.set_scale(w, 1.0, 1.0)
        .set_position(-BAR_W / 2.0 + w / 2.0, PLINTH_H + 0.03, z)
        .set_visible(load > 0.002);
}

impl FleetScene {
    /// Rebuilds the scene: the zones' plates, then each machine on its plinth, in its cell.
    fn rebuild(&mut self, fleet: &FleetResponse, layout: Layout) {
        self.group.get().children.clear();
        self.machines.clear();
        self.layout = Some(layout);
        let parts = Parts::new(&self.palette);
        let ordered: Vec<&Machine> = {
            let mut v: Vec<&Machine> = fleet.machines.iter().collect();
            v.sort_by(|a, b| by_id(&a.id, &b.id));
            v
        };
        let (cells, zones) = grid_layout(&ordered);
        let lod = lod_for(ordered.len());
        for z in &zones {
            let (w, d) = (z.max[0] - z.min[0], z.max[1] - z.min[1]);
            let plate = with_edges(
                &geometry::rounded_box(w, 0.08, d, 2, 0.25),
                &parts.plate_mat,
                parts.edge_mat.as_ref(),
            );
            plate
                .set_position(
                    (z.min[0] + z.max[0]) / 2.0,
                    -0.04,
                    (z.min[1] + z.max[1]) / 2.0,
                )
                .set_shadows(false, true);
            self.group.add(&plate);
        }
        self.zones = zones;
        let by_id: BTreeMap<&str, &Machine> = ordered.iter().map(|m| (m.id.as_str(), *m)).collect();
        for (slot, (id, zone, base)) in cells.into_iter().enumerate() {
            let Some(m) = by_id.get(id.as_str()) else {
                continue;
            };
            let model_id = kit_model_of(m);
            let Some(model) = kit::model(model_id) else {
                continue;
            };
            let root = Obj::group();
            root.set_position(base[0], base[1], base[2]);
            let slot = (slot < MACHINE_SLOTS).then_some(slot);
            if let Some(i) = slot {
                root.set_lift(i); // its hover lift rises in the shaders, by its slot's row
            }
            let plinth = with_edges(&parts.plinth, &parts.plinth_mat, parts.edge_mat.as_ref());
            plinth
                .set_position(0.0, PLINTH_H / 2.0, 0.0)
                .set_shadows(false, true);
            root.add(&plinth);
            let mut fills = Vec::new();
            for (k, z) in BAR_Z.iter().enumerate() {
                let track = Obj::mesh(&parts.bar, &parts.track_mat);
                track
                    .set_scale(BAR_W, 1.0, 1.0)
                    .set_position(0.0, PLINTH_H + 0.02, *z)
                    .set_shadows(false, false);
                root.add(&track);
                let fill = Obj::mesh(&parts.bar, &parts.fill_mats[k]);
                fill.set_position(0.0, PLINTH_H + 0.03, *z)
                    .set_shadows(false, false);
                fill.set_dynamic(true); // the load changes with every snapshot
                root.add(&fill);
                fills.push(fill);
            }
            let inst = model.instantiate(lod, &self.palette);
            if let Some(i) = slot {
                kit::set_machine(&inst.root, i);
            }
            inst.root
                .set_position(0.0, PLINTH_H, -MODEL_BACK)
                .set_scale(MODEL_SCALE, MODEL_SCALE, MODEL_SCALE);
            root.add(&inst.root);
            self.group.add(&root);
            let model_at = |p: Vec3| {
                [
                    base[0] + p.x * MODEL_SCALE,
                    base[1] + PLINTH_H + p.y * MODEL_SCALE,
                    base[2] - MODEL_BACK + p.z * MODEL_SCALE,
                ]
            };
            let fills: [Obj; 2] = [fills[0].clone(), fills[1].clone()];
            let mut scene = MachineScene {
                root,
                base,
                size: model.size * MODEL_SCALE,
                label: model_at(inst.label),
                zone,
                model: model_id,
                status: status_of(&m.status),
                lamp: lamp_of(m),
                name: m.name.clone(),
                slot,
                lift: 0.0,
                fan_angle: 0.0,
                activity: activity(m),
                fills,
                load: [-1.0, -1.0],
            };
            scene.set_load(load_of(m));
            self.machines.insert(id, scene);
        }
    }

    /// Whether `fleet` has another layout than the scene's: other machines, models or zones. It checks the machines in
    /// id order, as [`layout_of`] sorts them, without building a layout: `order` keeps its capacity between frames.
    fn layout_changed(&mut self, fleet: &FleetResponse) -> bool {
        let Some(layout) = &self.layout else {
            return true;
        };
        let machines = &fleet.machines;
        if layout.0.len() != machines.len() {
            return true;
        }
        self.order.clear();
        self.order.extend(0..machines.len());
        // Equal ids keep their input order, as the stable sort in `layout_of` keeps them.
        self.order
            .sort_unstable_by(|&a, &b| by_id(&machines[a].id, &machines[b].id).then(a.cmp(&b)));
        layout.0.iter().zip(&self.order).any(|(entry, &i)| {
            let m = &machines[i];
            entry.0 != m.id || entry.1 != kit_model_of(m) || entry.2 != zone_of(m)
        })
    }

    /// Advances the scene by `dt` seconds with the fleet's latest snapshot: rebuilds when the machines, their models or
    /// zones change; otherwise updates each machine's name, status, fans, load bars and hover lift. `ambient` is the
    /// ambient clock in seconds and `ambient_dt` its step (the fans turn by it).
    pub fn update(&mut self, dt: f32, ambient: f32, ambient_dt: f32, fleet: &FleetResponse) {
        if self.layout_changed(fleet) {
            self.rebuild(fleet, layout_of(fleet));
        }
        self.time = ambient;
        for m in &fleet.machines {
            let hovered = self.hovered.as_deref() == Some(m.id.as_str());
            let Some(node) = self.machines.get_mut(&m.id) else {
                continue;
            };
            // A name changes only on a rename, so the string's buffer is reused while it stays the same.
            if node.name != m.name {
                node.name.clone_from(&m.name);
            }
            let status = status_of(&m.status);
            node.status = status;
            node.lamp = lamp_for(status, m);
            node.activity = activity(m);
            if node.status == Status::Online {
                // Two turns a second at rest, three times that with every agent at work.
                node.fan_angle =
                    (node.fan_angle + ambient_dt * TAU * 2.0 * (1.0 + 2.0 * node.activity)) % TAU;
            }
            node.set_load(load_of(m));
            let target = if hovered { HOVER_LIFT } else { 0.0 };
            node.lift += (target - node.lift) * (1.0 - (-dt * HOVER_RATE).exp());
            if !hovered && node.lift.abs() < 1e-4 {
                node.lift = 0.0;
            }
            node.root.set_lift_y(node.lift);
        }
    }

    /// The fleet's GPU tables for this frame.
    pub fn instances(&self, _fleet: &FleetResponse) -> FleetInstances {
        let mut out = FleetInstances::ZERO;
        for m in self.machines.values() {
            let Some(i) = m.slot else {
                continue;
            };
            out.machine_angles[i] = m.fan_angle;
            out.machine_states[i] = [
                if m.lamp == Lamp::Working { 1.0 } else { 0.0 },
                m.activity,
                m.lift,
                0.0,
            ];
            out.machine_statuses[i] = rgba(self.palette.lamp(m.lamp));
        }
        out
    }

    /// The fleet's motion: Active while a hover lift is travelling, Ambient while any machine exists (fans and lights
    /// run on the clock), Rest for an empty fleet.
    pub fn motion(&self) -> crate::stage::Motion {
        let lifting = self.machines.iter().any(|(id, m)| {
            let target = if self.hovered.as_deref() == Some(id.as_str()) {
                HOVER_LIFT
            } else {
                0.0
            };
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

    /// The machine a ray hits first: its plinth and model's box (the plinth's footprint, up to the model's top).
    pub fn pick(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<String> {
        let mut best: Option<(f32, &str)> = None;
        for (id, m) in &self.machines {
            let half = PLINTH / 2.0;
            let lo = [m.base[0] - half, 0.0, m.base[2] - half];
            let hi = [
                m.base[0] + half,
                PLINTH_H + m.size.y + m.lift,
                m.base[2] + half,
            ];
            let Some(t) = ray_box(origin, dir, lo, hi) else {
                continue;
            };
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, id.as_str()));
            }
        }
        best.map(|(_, id)| id.to_string())
    }

    /// Lifts the hovered machine (and lowers the last one) from now on.
    pub fn hover(&mut self, id: Option<&str>) {
        self.hovered = id.map(str::to_string);
    }

    /// The overlay tags: each machine's name and lamp, at its label.
    pub fn tags(&self) -> Vec<Tag> {
        self.machines
            .iter()
            .map(|(id, m)| Tag {
                id: id.clone(),
                pos: [m.label[0], m.label[1] + m.lift, m.label[2]],
                name: m.name.clone(),
                lamp: m.lamp,
            })
            .collect()
    }

    /// The level of detail is fixed (full) for now: nothing switches per frame.
    pub fn frame_detail(&mut self, _eye: Vec3, _top: &crate::scene::Obj) -> bool {
        false
    }

    /// The camera's fit: the centre of the grid and the radius of its bounding sphere.
    pub fn frame(&self) -> FleetFrame {
        if self.zones.is_empty() {
            return FleetFrame {
                center: [0.0; 3],
                radius: PITCH * 2.0,
            };
        }
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for z in &self.zones {
            lo = [lo[0].min(z.min[0]), lo[1].min(z.min[1])];
            hi = [hi[0].max(z.max[0]), hi[1].max(z.max[1])];
        }
        let top = self
            .machines
            .values()
            .map(|m| PLINTH_H + m.size.y)
            .fold(1.0, f32::max);
        let center = [(lo[0] + hi[0]) / 2.0, top / 3.0, (lo[1] + hi[1]) / 2.0];
        let radius = 0.5 * ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + top * top).sqrt();
        FleetFrame { center, radius }
    }

    /// The points the plan view frames: each machine's plinth corners on the ground and its model's top corners. Fitting
    /// these, rather than the grid's box, keeps a part-filled row of zones from leaving an empty corner in the view.
    pub fn outline(&self) -> Vec<[f32; 3]> {
        let half = PLINTH / 2.0;
        let mut points = Vec::with_capacity(self.machines.len() * 8);
        for m in self.machines.values() {
            let [x, _, z] = m.base;
            let top = PLINTH_H + m.size.y;
            for (dx, dz) in [(-half, -half), (half, -half), (-half, half), (half, half)] {
                points.push([x + dx, 0.0, z + dz]);
                points.push([x + dx, top, z + dz]);
            }
        }
        points
    }

    /// The palette the scene is drawn in.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The label anchor of machine `id` in the world (with its hover lift).
    pub fn label_of(&self, id: &str) -> Option<[f32; 3]> {
        let m = self.machines.get(id)?;
        Some([m.label[0], m.label[1] + m.lift, m.label[2]])
    }
}

impl MachineScene {
    /// Shows `load` (CPU, memory; 0..1) on the plinth's bars, when it moved.
    fn set_load(&mut self, load: (f32, f32)) {
        for (k, v) in [load.0, load.1].into_iter().enumerate() {
            if (self.load[k] - v).abs() > 0.002 {
                self.load[k] = v;
                set_fill(&self.fills[k], v);
            }
        }
    }
}

/// The distance along a ray (unit `dir`) to an axis-aligned box `lo`..`hi` (the slab test): the near side, or 0 from
/// inside.
pub fn ray_box(origin: [f32; 3], dir: [f32; 3], lo: [f32; 3], hi: [f32; 3]) -> Option<f32> {
    let (mut near, mut far) = (f32::MIN, f32::MAX);
    for k in 0..3 {
        if dir[k].abs() < 1e-9 {
            if origin[k] < lo[k] || origin[k] > hi[k] {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((lo[k] - origin[k]) / dir[k], (hi[k] - origin[k]) / dir[k]);
        near = near.max(t1.min(t2));
        far = far.min(t1.max(t2));
    }
    (near <= far && far >= 0.0).then_some(near.max(0.0))
}

/// The fleet's tables as one shared value per frame.
pub fn shared(instances: FleetInstances) -> Arc<FleetInstances> {
    Arc::new(instances)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> FleetResponse {
        serde_json::from_str(include_str!("../testdata/floor-15.json")).expect("the fixture parses")
    }

    fn ink() -> Palette {
        Palette::of(crate::style::Style::Ink, true)
    }

    #[test]
    fn a_machine_that_has_not_reported_reads_with_nulls_as_empty() {
        // The hub's own machine on a Mac before its node starts: GET /v1/fleet as the hub sends it.
        let json = r#"{"machines":[{"id":"n0","name":"hub","role":"hub","kind":null,"vendor":null,"model":null,
            "apple":null,"status":"offline","arch":null,"os":null,"backend":null,"cpu":null,"memory_mib":null,
            "gpus":null,"storage_gib":null,"agents":[],"working":0,"stats":null,"last_seen_ms":1792359170409}],
            "links":null}"#;
        let fleet: FleetResponse = serde_json::from_str(json).expect("nulls read as empty");
        let hub = &fleet.machines[0];
        assert_eq!(
            (hub.kind.as_str(), hub.arch.as_str(), hub.memory_mib),
            ("", "", 0)
        );
        assert_eq!(hub.cpu, Cpu::default());
        assert!(hub.gpus.is_empty() && fleet.links.is_empty());
        let scene = build_fleet(&fleet, &ink());
        assert_eq!(scene.machines.len(), 1);
    }

    #[test]
    fn the_fixture_builds_every_machine_on_a_cell_in_its_zone() {
        let fleet = fixture();
        let scene = build_fleet(&fleet, &ink());
        assert_eq!(scene.machines.len(), fleet.machines.len());
        let hub = fleet
            .machines
            .iter()
            .find(|m| m.role == "hub")
            .expect("a hub");
        assert_eq!(scene.machines[&hub.id].zone, Zone::Hub);
        assert_eq!(scene.machines[&hub.id].model, "hub");
        // No two machines share a cell, and every cell is inside its zone's plate.
        let mut seen = std::collections::HashSet::new();
        for m in scene.machines.values() {
            assert!(
                seen.insert(((m.base[0] * 10.0) as i32, (m.base[2] * 10.0) as i32)),
                "two machines on one cell"
            );
            let z = scene
                .zones
                .iter()
                .find(|z| z.zone == m.zone)
                .expect("its zone has a plate");
            assert!(
                m.base[0] > z.min[0]
                    && m.base[0] < z.max[0]
                    && m.base[2] > z.min[1]
                    && m.base[2] < z.max[1]
            );
        }
    }

    #[test]
    fn the_layout_is_the_same_for_the_same_fleet_and_zones_do_not_overlap() {
        let fleet = fixture();
        let ordered: Vec<&Machine> = fleet.machines.iter().collect();
        let (a, zones) = grid_layout(&ordered);
        let mut reversed: Vec<&Machine> = fleet.machines.iter().rev().collect();
        reversed.sort_by(|a, b| by_id(&a.id, &b.id));
        let (b, _) = grid_layout(&reversed);
        let sort = |mut v: Vec<(String, Zone, [f32; 3])>| {
            v.sort_by(|x, y| x.0.cmp(&y.0));
            v
        };
        assert_eq!(sort(a), sort(b));
        for (i, z) in zones.iter().enumerate() {
            for w in zones.iter().skip(i + 1) {
                // Zones in one row are apart in x, zones in different rows apart in z.
                assert!(
                    z.max[0] < w.min[0]
                        || w.max[0] < z.min[0]
                        || z.max[1] < w.min[1]
                        || w.max[1] < z.min[1],
                    "{:?} and {:?} overlap",
                    z.zone,
                    w.zone
                );
            }
        }
    }

    #[test]
    fn a_ray_down_onto_a_cell_picks_its_machine_and_a_ray_between_cells_picks_none() {
        let fleet = fixture();
        let scene = build_fleet(&fleet, &ink());
        for (id, m) in &scene.machines {
            let hit = scene.pick([m.base[0], 50.0, m.base[2]], [0.0, -1.0, 0.0]);
            assert_eq!(hit.as_deref(), Some(id.as_str()));
        }
        let far = scene
            .zones
            .iter()
            .map(|z| z.max[0])
            .fold(f32::MIN, f32::max)
            + 3.0;
        assert_eq!(scene.pick([far, 50.0, 0.0], [0.0, -1.0, 0.0]), None);
    }

    #[test]
    fn lamps_follow_status_and_work_and_the_tables_carry_them() {
        let mut fleet = fixture();
        fleet.machines.truncate(3);
        fleet.machines[0].status = "online".into();
        fleet.machines[0].working = 1;
        fleet.machines[0].agents = vec![1];
        fleet.machines[1].status = "draining".into();
        fleet.machines[2].status = "offline".into();
        let scene = build_fleet(&fleet, &ink());
        let lamps: Vec<Lamp> = fleet.machines.iter().map(lamp_of).collect();
        assert_eq!(lamps, [Lamp::Working, Lamp::Strained, Lamp::Offline]);
        let inst = scene.instances(&fleet);
        let m = &scene.machines[&fleet.machines[0].id];
        assert_eq!(
            inst.machine_statuses[m.slot.unwrap()],
            rgba(ink().lamp(Lamp::Working))
        );
        assert_eq!(inst.machine_states[m.slot.unwrap()][0], 1.0);
    }

    #[test]
    fn a_ray_box_hits_from_outside_and_inside_and_misses_beside() {
        assert_eq!(
            ray_box([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], [-1.0; 3], [1.0; 3]),
            Some(4.0)
        );
        assert_eq!(
            ray_box([0.0, 0.0, 0.0], [0.0, -1.0, 0.0], [-1.0; 3], [1.0; 3]),
            Some(0.0)
        );
        assert_eq!(
            ray_box([3.0, 5.0, 0.0], [0.0, -1.0, 0.0], [-1.0; 3], [1.0; 3]),
            None
        );
    }
}
