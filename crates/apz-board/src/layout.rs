//! The board layout (docs/design/apz-board.md, "Board layout"): where each role's room sits on a board's slots, the
//! plates that carry them and their columns, each room's footprint and door, the homes of the agents, and the walk
//! surfaces the geometry produces (docs/design/walk-surfaces.md). Ported from apps/web/src/views/floor3d/board-layout.ts.
//!
//! Two passes. This pass is the geometry: it is deterministic, reads only the board (drawn tops and parts), and
//! emits no links. The second pass adds what needs the board-only nav grid: the aisle exits (exitOf, gangwayOf,
//! reachMain, landingOf, perimeterOpen, mainComponent), the spur gaps and the end cuts they cause, gangways, lifts and
//! their cabs, landings, the room-clearance and reachability acceptance (landingClear, unreachedKeys, connectivity),
//! and the walk links that join the surfaces. Each such point is a `SECOND PASS` marker below.

mod aisle;
mod board_grid;
mod scene;
mod columns;
mod geom;
mod mouth;
mod rooms;
mod surfaces;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::hardware::{eval_when, parse_when, Library, Model, Part};
use crate::grid::{Grid, NAV_CELL, NAV_STEP};
use crate::navgrid::{self, landing_clear_at, Slab};
use crate::walk::{SurfaceKind, WalkSet};
use std::cmp::Ordering;
use crate::Board;
use aisle::{exit_of, gangway_of, open_share, perimeter_open, Site};
use board_grid::board_grid;
pub use scene::{nav_scene, room_plan, NavScene, RoomPlan};
use columns::Lattice;
use geom::{belongs, segment_blocked, Frame, Rect};
use mouth::{mouth, Mouth};
use rooms::{Aisle, Cell, Config, MinTable, AISLE, RAMP_DEPTH, ROOM_SHARE};
use surfaces::{Assembly, PlateGeo, RoomGeo};

pub use aisle::{Exit, ExitKind, Gap, Gangway};
pub use mouth::MouthIds;
pub use rooms::Family;

/// A plate's thickness (ground.ts PLATE): a plate's top stands this far above the support it rests on, and its
/// underside is at most this far below its top. Unscaled: the web's groundSlots and restingSlab use it as written.
pub const PLATE: f64 = 0.2;
/// The PCB's height when the model does not set `pcb_top` (board-layout.ts layoutAtScale).
pub const PCB_TOP_DEFAULT: f64 = 0.4;
/// The world scale steps: the board grows by this factor until its plates seat every agent (BOARD_SCALE_STEP).
pub const BOARD_SCALE_STEP: f64 = 1.1;
/// A slot's declared top is raised to its drawn support only when the support is more than this above it.
const SLOT_RAISE: f64 = 0.005;
/// A part whose top is more than this above a slot's top blocks the slot's aisle strip (TALL).
const TALL: f64 = 0.25;
/// Samples per side of an occlusion grid (OCCLUSION_GRID) and their height above a slot's top (OCCLUSION_HEIGHT).
const OCCLUSION_GRID: usize = 3;
const OCCLUSION_HEIGHT: f64 = 1.0;
/// A slot is occluded when more than this share of its samples is blocked from the default camera (OCCLUSION_SHARE).
const OCCLUSION_SHARE: f64 = 1.0 / 3.0;
/// The reason a slot's aisle strip is blocked in every configuration (REASON_TALL).
const REASON_TALL: &str = "a part stands over the aisle strip of every configuration";
/// The reason a room cannot reach the main component through its plate (board-layout.ts REASON_ROOM).
const REASON_ROOM: &str = "a room cannot reach its door through this plate with its furniture in place";

/// An agent the layout seats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// The agent's id; homes are keyed by it, and agents of one room are ordered by it.
    pub id: String,
    /// The agent's role (roles.ts). Empty: the agent gets a room of its own in the `other` family. `"orchestrator"`
    /// is the orchestrator's seat (the centre room, nearest the atom).
    pub role: String,
    /// Reserved: the TS layout does not read a seat hint, so this is carried and ignored.
    pub seat_hint: Option<String>,
}

/// The inputs of a layout: the agents to seat, the GPU count (the board's `gpus` param, which its `when`s read), and
/// the largest world scale the board may grow to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutInput {
    pub agents: Vec<AgentSpec>,
    pub gpus: u32,
    pub scale_cap: f64,
}

/// A room's door side, in the room's own turn (turnAndDoor): `n` is the local -z side, `s` the +z side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Door {
    N,
    S,
    E,
    W,
}

/// The layout's footprint, centred on the origin, for camera framing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub cx: f64,
    pub cz: f64,
    pub w: f64,
    pub d: f64,
}

/// A usable slot at the world scale: its id, part, kind, rect [x, z, w, d], top, and what it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutSlot {
    pub id: String,
    /// The part the slot sits on.
    pub on: String,
    /// The slot's type (cpu, gpu, memory, ...).
    pub kind: String,
    pub rect: [f64; 4],
    /// The slot's top: the drawn support plus PLATE (or the declared top when that is higher).
    pub top: f64,
    /// Whether its aisle is double (down the middle, rooms on both sides) or single.
    pub double: bool,
    /// The rooms on this slot's plate (indices into `Layout::rooms`).
    pub rooms: Vec<usize>,
    /// The agents in those rooms (ids).
    pub agents: Vec<String>,
}

/// A slot the layout leaves out, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnusableSlot {
    pub slot: String,
    pub reason: String,
}

/// A plate (terrace): the usable slot it stands on, lifted to its drawn support. `id` is its index; plate `i` sits on
/// `Layout::slots[i]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plate {
    pub id: usize,
    pub slot: String,
    /// The plate's footprint [x, z, w, d] (the slot's rect at the world scale).
    pub rect: [f64; 4],
    /// The walking top of the plate.
    pub y: f64,
    /// The highest support sample under the footprint (columns::SUPPORT_CELL samples, at the world scale), 0 over the
    /// ground. The plate's underside is this sample.
    pub support: f64,
    /// The plate's underside: the highest support sample, so `y == bottom + PLATE` and nothing under the plate is cut.
    pub bottom: f64,
    /// The plate's exit to the main component (aisle.rs). Every plate in a layout has one.
    pub exit: Option<Exit>,
}

/// A column under a plate: a block of support samples, from its lowest sample up to the plate's underside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    /// The plate it holds up (index into `Layout::plates`).
    pub plate: usize,
    /// The leaf's footprint [x, z, w, d].
    pub rect: [f64; 4],
    /// The lowest support sample in its block (the column's foot).
    pub bottom: f64,
    /// The plate underside it holds.
    pub top: f64,
}

/// A room: a role's cell on a plate, with its footprint, floor and door.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Room {
    /// The room's index (its walk surface is `room:<id>`).
    pub id: usize,
    pub role: String,
    pub family: Family,
    /// The orchestrator's room is round (its footprint is a circle of radius `r`); every other room is square.
    pub round: bool,
    pub hue: f64,
    /// The room's radius: the half side of its cell's share (ROOM_SHARE), for the round room and the floor's label.
    pub r: f64,
    /// The room's centre in the plan.
    pub centre: [f64; 2],
    /// The cell's half sizes (w / 2, d / 2): the footprint is `centre` +- `half`.
    pub half: [f64; 2],
    /// The inner width and depth (innerOf of the cell's w and d): the minimum is checked against the first.
    pub inner: [f64; 2],
    /// The room's turn about y (radians).
    pub turn: f64,
    /// The height of the room's floor: the plate's top it stands on.
    pub floor_y: f64,
    /// The plate it stands on (index into `Layout::plates`).
    pub plate: usize,
    /// The door side (in the room's turn).
    pub door: Door,
    /// The world point where the door meets the room's edge, the midpoint of that side. (The second pass rasterises
    /// it into the nav grid's door cell.)
    pub door_at: [f64; 2],
    /// The world direction from the room to its aisle.
    pub aisle_dir: [f64; 2],
    /// The room's agents (ids), in home order.
    pub members: Vec<String>,
}

/// An agent's home: the point it sits at inside its room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Home {
    pub agent: String,
    /// The room it sits in (index into `Layout::rooms`).
    pub room: usize,
    pub at: [f64; 2],
}

/// A lift: its cab (a Cab surface on the board) and the bottom and top it joins (board-layout.ts lift; lift-ctrl.ts runs
/// it later). The cab's floor is flush with each stop: `stops[0]` is the bottom landing's height and `stops[1]` the top's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lift {
    /// The plate the lift serves (index into `Layout::plates`).
    pub plate: usize,
    /// The cab's walk surface.
    pub cab: u32,
    /// The cab's footprint [x, z, w, d].
    pub cab_rect: [f64; 4],
    /// The bottom landing (on the board) and the top (a landing on the plate, or a gangway's bridge), by surface id.
    pub bottom: u32,
    pub top: u32,
    /// The cab floor at the bottom stop and at the top stop.
    pub stops: [f64; 2],
    /// The cab's door side: the side facing the top.
    pub door: Door,
    /// Whether the lift is a gangway's (its top is a bridge at plate height).
    pub gangway: bool,
}

/// What a landing is: the board's bottom landing of a lift, a plate's top landing, or a gangway's bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LandingRole {
    Bottom,
    Top,
    Bridge,
}

/// A landing: a walk surface a lift stops at, with its height.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Landing {
    /// The surface id.
    pub surface: u32,
    /// The plate the landing serves.
    pub plate: usize,
    /// The height the cab is flush with at this stop.
    pub height: f64,
    pub role: LandingRole,
}

/// The layout of one board at one world scale. Serialises to the same JSON for the same input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// The board kind the layout is for.
    pub board_kind: String,
    /// The world scale the board was grown to (1 and up, at most the input's cap): board_scale.
    pub board_scale: f64,
    /// The PCB's top height at this scale.
    pub pcb_top: f64,
    /// The atom's point (the board's anchor, or the PCB's centre above the origin).
    pub atom: [f64; 3],
    /// The footprint for camera framing.
    pub bounds: Bounds,
    /// The usable slots, in candidate order (plate i sits on slot i).
    pub slots: Vec<LayoutSlot>,
    /// The slots the aisle rules leave out.
    pub unusable: Vec<UnusableSlot>,
    /// The plates (terraces), one per usable slot.
    pub plates: Vec<Plate>,
    /// The columns under the plates.
    pub columns: Vec<Column>,
    /// The rooms.
    pub rooms: Vec<Room>,
    /// One home per agent.
    pub homes: Vec<Home>,
    /// The gangways: the plates whose exit is a bridge over blocked cells (aisle.rs). The bridge's walk surface and links
    /// are the later pass's.
    pub gangways: Vec<Gangway>,
    /// SECOND PASS: lifts and their cabs (empty until the second pass).
    pub lifts: Vec<Lift>,
    /// SECOND PASS: landings (empty until the second pass).
    pub landings: Vec<Landing>,
    /// The walk surfaces the geometry produces: the PCB pieces, the plates' pieces, the room floors. Ids are the
    /// indices. The links are empty until the second pass.
    pub walk: WalkSet,
}

/// The most exclusion rounds a world scale tries (each excludes the candidate exits its pass rejected and plans again)
/// before it grows (board-layout.ts LANDING_ROUNDS).
const LANDING_ROUNDS: usize = 64;

/// Rounds in a row that may pass without another seat or a smaller bad-key count before a scale is given up. An
/// infeasible scale otherwise spends every LANDING_ROUNDS round, since its excluded set grows by about one key a round.
const STALE_ROUNDS: usize = 20;
/// Scales tried past the first one that seats everyone in parts (see `layout`'s `partial`) before that one is taken.
const PARTIAL_SCALES: usize = 3;
/// At most this many slots are dropped at one scale (their plates' exits unreachable) before the scale is given up.
const MAX_DROPPED_SLOTS: usize = 4;

/// The `slot|<id>` exclusion key of a rejected key: an exit's key starts with its slot's id (`slot_gpu2|D9.686|…`), a
/// room's is already one.
fn slot_key_of(key: &str) -> String {
    let id = key.strip_prefix("slot|").unwrap_or(key);
    format!("slot|{}", id.split('|').next().unwrap_or(id))
}

/// The layout of a board for the given agents, at the smallest world scale (1, then BOARD_SCALE_STEP steps up to the
/// input's `scale_cap`) that the acceptance test accepts. `board_kind` is the kind's name ("edge", "laptop", ...).
///
/// The acceptance (`at_scale`): every agent has a seat, every plate and every room reaches the main component of the walk,
/// and no exit's mouth overlaps a room (with its cliff band), another plate's mouth. A pass that rejects
/// candidate exits is planned again with them excluded, as board-layout.ts does, before the board grows.
///
/// Panics, naming the kind, when no scale up to the cap seats every agent (board-layout.ts throws there), or when the
/// kind is unknown. Deterministic: the same input gives the same `Layout`.
pub fn layout(board_kind: &str, input: &LayoutInput) -> Layout {
    let mut params = BTreeMap::new();
    params.insert("gpus".to_string(), f64::from(input.gpus));
    let board = Board::from_kind(board_kind, &params);
    let model = Library::get()
        .model(&format!("board/{board_kind}"))
        .unwrap_or_else(|| panic!("apz-board: no board kind \"{board_kind}\""));
    let mut merged = model.params.clone().unwrap_or_default();
    merged.extend(params);
    let ground = ground_targets(&board, model);
    let ctx = Ctx { board: &board, model, kind: board_kind, params: merged, table: rooms::min_table(), ground };
    let mut s = 1.0;
    let mut last = String::new();
    // The smallest scale's layout that seats everyone but leaves some plate's exit off the main walk, and the scales tried
    // since: a board whose cards wall each other in (a 2-GPU workstation) never passes, and is better walked in parts
    // (each robot keeps to its own component) than not walked at all.
    let mut partial: Option<(Layout, usize)> = None;
    // The scale grows by BOARD_SCALE_STEP (> 1), so this loop ends after a few steps at any finite cap.
    while s <= input.scale_cap + 1e-9 {
        let setup = scale_setup(&ctx, s);
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        let mut pass = at_scale(&ctx, input, &setup, &excluded);
        // The most seats and the fewest bad keys so far at this scale: STALE_ROUNDS rounds without either give up the scale.
        let mut best = (pass.layout.homes.len(), pass.rejected.len());
        let mut stale = 0;
        for _ in 0..LANDING_ROUNDS {
            if pass.rejected.is_empty() {
                break;
            }
            let before = excluded.len();
            excluded.extend(pass.rejected.iter().cloned());
            if excluded.len() == before {
                break;
            }
            pass = at_scale(&ctx, input, &setup, &excluded);
            let (seated, bad) = (pass.layout.homes.len(), pass.rejected.len());
            if seated > best.0 || bad < best.1 {
                best = (best.0.max(seated), best.1.min(bad));
                stale = 0;
            } else {
                stale += 1;
                if stale >= STALE_ROUNDS {
                    break;
                }
            }
        }
        if pass.layout.homes.len() == input.agents.len() && partial.is_none() {
            partial = Some((pass.layout.clone(), 0));
        }
        // A plate whose exits the walk still does not reach after the landing rounds (a GPU card hemmed in by the next
        // one): its slot is dropped, and the agents seat on the other plates and the board, rather than the board
        // having no layout (and so no walk) at all.
        for _ in 0..MAX_DROPPED_SLOTS {
            if pass.rejected.is_empty() {
                break;
            }
            let before = excluded.len();
            excluded.extend(pass.rejected.iter().map(|key| slot_key_of(key)));
            if excluded.len() == before {
                break;
            }
            pass = at_scale(&ctx, input, &setup, &excluded);
        }
        if pass.rejected.is_empty() && pass.layout.homes.len() == input.agents.len() {
            return pass.layout;
        }
        if let Some((layout, tried)) = partial.as_mut() {
            *tried += 1;
            if *tried > PARTIAL_SCALES {
                return layout.clone();
            }
        }
        last = format!(
            "last try at scale {s:.3}: {} plates, {} homes, rejected {:?}, unusable: {:?}",
            pass.layout.plates.len(),
            pass.layout.homes.len(),
            pass.rejected,
            pass.layout.unusable.iter().map(|u| u.reason.as_str()).collect::<Vec<_>>()
        );
        s *= BOARD_SCALE_STEP;
    }
    if let Some((layout, _)) = partial {
        return layout;
    }
    panic!(
        "apz-board: {} agents do not seat on {board_kind} within the world scale cap {} ({last})",
        input.agents.len(),
        input.scale_cap
    );
}

/// One layout pass at a scale: the layout, and the candidate keys its acceptance rejected (to exclude and plan again).
struct Pass {
    layout: Layout,
    rejected: Vec<String>,
}

/// Why a slot's aisle is unusable when no configuration exits (board-layout.ts REASON_ENCLOSED, REASON_PERIMETER).
const REASON_ENCLOSED: &str = "enclosed: no walk path leaves the plate on any side";

/// The reason for an aisle that finds no exit within LANDING_REACH cells of the main component.
fn reason_perimeter() -> String {
    format!("no perimeter point within {} cells of the main component", aisle::LANDING_REACH)
}

/// The per-layout inputs every scale shares: the board, its model, the merged params (the model's own, then the
/// input's), the minimums, and each model slot's ground top (unscaled).
struct Ctx<'a> {
    board: &'a Board,
    model: &'a Model,
    kind: &'a str,
    params: BTreeMap<String, f64>,
    table: MinTable,
    ground: Vec<f64>,
}

/// A slot candidate at the world scale.
#[derive(Debug, Clone)]
struct Cand {
    id: String,
    on: String,
    kind: String,
    rect: [f64; 4],
    top: f64,
    capacity: u32,
}

/// A resting part at the world scale: its name, its box and its highest upward face.
struct PartBox<'a> {
    name: &'a str,
    lo: [f64; 3],
    hi: [f64; 3],
}

/// A seat request: a role's agents, in one room (or the orchestrator's centre room).
struct Seat {
    role: String,
    family: Family,
    members: Vec<usize>,
    centre: bool,
    /// The aisle index the room is placed on, or None when no slot holds it at its minimum.
    slot: Option<usize>,
    min_side: f64,
}

/// A room built on an aisle's cell, before its homes and floor are set.
struct Built {
    role: String,
    family: Family,
    centre: bool,
    slot: usize,
    cell: Cell,
    members: Vec<usize>,
    min_side: f64,
}

/// The ground targets (groundSlots at scale 1): each model slot's top is its support's 5 x 5 sample max plus PLATE,
/// rounded to a millimetre, when that is more than the declared top; else the declared top.
fn ground_targets(board: &Board, model: &Model) -> Vec<f64> {
    let sup = |x: f64, z: f64| board.support_below(x, z, f64::INFINITY);
    model
        .slots
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|sl| {
            let top = sample_max(&sup, sl.rect);
            let target = ((top + PLATE) * 1000.0).round() / 1000.0;
            if target > sl.top + SLOT_RAISE {
                target
            } else {
                sl.top
            }
        })
        .collect()
}

/// The highest support over a 5 x 5 grid of sample points in a rect [x, z, w, d], 0 for the ground.
fn sample_max(sup: &dyn Fn(f64, f64) -> f64, rect: [f64; 4]) -> f64 {
    let [x0, z0, w, d] = rect;
    let mut top = 0.0_f64;
    for i in 0..5 {
        for j in 0..5 {
            top = top.max(sup(x0 + (i as f64 + 0.5) / 5.0 * w, z0 + (j as f64 + 0.5) / 5.0 * d));
        }
    }
    top
}

/// The parts hidden by their own or an ancestor's `when` (buildModel draws them): an unparseable one hides.
fn hidden_ids(parts: &[Part], params: &BTreeMap<String, f64>) -> BTreeSet<String> {
    fn walk(parts: &[Part], dark: bool, params: &BTreeMap<String, f64>, out: &mut BTreeSet<String>) {
        for p in parts {
            let mut off = dark;
            if let Some(expr) = &p.when {
                match parse_when(expr) {
                    Some(w) if eval_when(&w, params) => {}
                    _ => off = true,
                }
            }
            if let (Some(id), true) = (&p.id, off) {
                out.insert(id.clone());
            }
            if let Some(children) = &p.children {
                walk(children, off, params, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(parts, false, params, &mut out);
    out
}

/// The distance from a point to a rect [x, z, w, d] (0 inside it).
fn rect_distance(rect: [f64; 4], x: f64, z: f64) -> f64 {
    let [x0, z0, w, d] = rect;
    let dx = (x0 - x).max(0.0).max(x - (x0 + w));
    let dz = (z0 - z).max(0.0).max(z - (z0 + d));
    dx.hypot(dz)
}

/// The centre of a rect [x, z, w, d].
fn centre_of(rect: [f64; 4]) -> [f64; 2] {
    [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0]
}

/// Does a part stand over the strip higher than `top + TALL` (its box overlaps the strip's footprint)?
fn tall_on(b: &PartBox, strip: [f64; 4], top: f64) -> bool {
    let [x, z, w, d] = strip;
    b.hi[1] > top + TALL && b.lo[0] < x + w && b.hi[0] > x && b.lo[2] < z + d && b.hi[2] > z
}

/// The share of a slot's samples blocked from the camera by a part other than its own (slotBlockedShare).
fn blocked_share(cand: &Cand, cam: [f64; 3], boxes: &[PartBox]) -> f64 {
    let [x, z, w, d] = cand.rect;
    let n = OCCLUSION_GRID;
    let others: Vec<&PartBox> = boxes.iter().filter(|b| !belongs(b.name, &cand.on)).collect();
    let mut blocked = 0;
    for i in 0..n {
        for j in 0..n {
            let to = [x + w * (i as f64 + 0.5) / n as f64, cand.top + OCCLUSION_HEIGHT, z + d * (j as f64 + 0.5) / n as f64];
            if others.iter().any(|b| segment_blocked(cam, to, b.lo, b.hi)) {
                blocked += 1;
            }
        }
    }
    f64::from(blocked) / (n * n) as f64
}

/// The outside strip beyond a single aisle's plate side, where its open share is read (board-layout.ts planAisle).
fn outside_strip(f: &Frame, cfg: &Config) -> [f64; 4] {
    if cfg.aisle[0] > 1e-9 {
        f.rect(0.0, f.l, f.s, f.s + AISLE)
    } else {
        f.rect(0.0, f.l, -AISLE, 0.0)
    }
}

/// The exit site of a candidate's configuration (the plate, its frame, its rise and its configuration key).
fn site_of<'a>(c: &Cand, f: Frame, cfg: &Config, pcb_top: f64, tall: &'a dyn Fn([f64; 4]) -> bool) -> Site<'a> {
    let ck = format!("{}|{}{:.3}", c.id, if cfg.double { 'D' } else { 'S' }, cfg.aisle[0]);
    Site { rect: c.rect, f, rise: c.top - pcb_top, ck, tall }
}

/// The aisle a plate's candidate takes (board-layout.ts planAisle): the configurations are the double first (when it is
/// deep enough), then the singles with the most open ground first; a configuration whose aisle strip has a tall part is
/// out, and the first configuration whose exit reaches the main component is taken (the plain exits of every
/// configuration first, then the gangways). A candidate in `excluded` (its slot, or the exit keys the layout rejected) is
/// not used. An error names why no configuration works.
#[allow(clippy::too_many_arguments)]
fn plan_aisle(ci: usize, c: &Cand, boxes: &[PartBox], row_min: f64, grid: &Grid, main: i32, pcb_top: f64, excluded: &BTreeSet<String>) -> Result<(Aisle, Exit), String> {
    if excluded.contains(&format!("slot|{}", c.id)) {
        return Err(REASON_ROOM.to_string());
    }
    let f = Frame::of(c.rect);
    let (doubles, singles): (Vec<Config>, Vec<Config>) = rooms::configs(&f, row_min).into_iter().partition(|cfg| cfg.double);
    let mut keyed: Vec<(f64, Config)> = singles.into_iter().map(|cfg| (open_share(grid, outside_strip(&f, &cfg)), cfg)).collect();
    keyed.sort_by(|a, b| if (b.0 - a.0).abs() < 1e-9 { Ordering::Equal } else { b.0.total_cmp(&a.0) });
    let mut configs = doubles;
    configs.extend(keyed.into_iter().map(|(_, cfg)| cfg));
    let tall = |r: [f64; 4]| boxes.iter().any(|b| tall_on(b, r, c.top));
    let clear: Vec<Config> = configs.into_iter().filter(|cfg| !tall(f.rect(0.0, f.l, cfg.aisle[0], cfg.aisle[1]))).collect();
    if clear.is_empty() {
        return Err(REASON_TALL.to_string());
    }
    let found = clear
        .iter()
        .find_map(|cfg| exit_of(&site_of(c, f, cfg, pcb_top, &tall), cfg, grid, main, excluded).map(|e| (cfg, e)))
        .or_else(|| clear.iter().find_map(|cfg| gangway_of(&site_of(c, f, cfg, pcb_top, &tall), cfg, grid, main, excluded).map(|e| (cfg, e))));
    match found {
        Some((cfg, exit)) => {
            let mut rows = cfg.rows.clone();
            if let Some(g) = &exit.spur {
                rows[g.row].gap = Some(g.span);
            }
            let un = f.vec(1.0, 0.0);
            let end_side = if exit.spur.is_some() { 0.0 } else { exit.dir[0] * un[0] + exit.dir[1] * un[1] };
            let aisle = Aisle { cand: ci, f, rows, double: cfg.double, cap: c.capacity as usize, end_side };
            Ok((aisle, exit))
        }
        None if !perimeter_open(grid, &f) => Err(REASON_ENCLOSED.to_string()),
        None => Err(reason_perimeter()),
    }
}

/// The aisles with what each holds: the rooms placed on each (`load`) and the cell side its rooms need (`need`).
struct Fill<'a> {
    aisles: &'a [Aisle],
    cands: &'a [Cand],
    load: &'a [usize],
    need: &'a [f64],
}

impl Fill<'_> {
    /// The rooms of a minimum side `m` the aisle at index `i` still holds (`holds` at the larger of its need and m's cell).
    fn holds_at(&self, i: usize, m: f64) -> usize {
        rooms::holds(&self.aisles[i], self.need[i].max(rooms::cell_need_of(m)))
    }

    /// The aisle a room goes to (board-layout.ts place): the orchestrator takes the open aisle nearest the atom; any
    /// other room the open aisle with the lowest fill, the family's preferred slot type breaking a tie.
    fn place(&self, family: Family, centre: bool, m: f64, atom: [f64; 2]) -> Option<usize> {
        let open: Vec<usize> = (0..self.aisles.len()).filter(|&i| self.load[i] < self.holds_at(i, m)).collect();
        let rect = |i: usize| self.cands[self.aisles[i].cand].rect;
        if centre {
            return open.into_iter().reduce(|best, i| {
                if rect_distance(rect(i), atom[0], atom[1]) < rect_distance(rect(best), atom[0], atom[1]) {
                    i
                } else {
                    best
                }
            });
        }
        let fill = |i: usize| self.load[i] as f64 / self.holds_at(i, m).max(1) as f64;
        let preferred = |i: usize| {
            if rooms::family_types(family).contains(&self.cands[self.aisles[i].cand].kind.as_str()) {
                0
            } else {
                1
            }
        };
        open.into_iter().reduce(|best, i| {
            let (fi, fb) = (fill(i), fill(best));
            if (fi - fb).abs() > 1e-9 {
                if fi < fb {
                    i
                } else {
                    best
                }
            } else if preferred(i) < preferred(best) {
                i
            } else {
                best
            }
        })
    }
}

/// What every pass at one world scale shares: the setup that depends on the scale and not on the excluded keys (the
/// visible candidate slots, the parts' boxes, the planning grid and its main component, each candidate's lattice).
struct ScaleSetup<'a> {
    s: f64,
    pcb: Rect,
    pcb_top: f64,
    atom: [f64; 3],
    size: [f64; 2],
    boxes: Vec<PartBox<'a>>,
    /// Every part box (lo, hi), the GPU cards and attached parts included (board-layout.ts navBoardOf's partBoxes).
    parts: Vec<([f64; 3], [f64; 3])>,
    tall: Vec<Rect>,
    cands: Vec<Cand>,
    grid0: Grid,
    main: i32,
    lattices: Vec<Lattice>,
}

/// The setup at one world scale `s`: the slots the default camera sees, the parts' boxes, the planning grid over the PCB
/// and the candidate plates (aisle.rs), and each candidate's support lattice. Computed once per scale, then shared by
/// every pass at the scale.
fn scale_setup<'a>(ctx: &Ctx<'a>, s: f64) -> ScaleSetup<'a> {
    let (board, model, p) = (ctx.board, ctx.model, &ctx.params);
    let scale3 = |v: [f64; 3]| [v[0] * s, v[1] * s, v[2] * s];
    let pcb_top = p.get("pcb_top").copied().unwrap_or(PCB_TOP_DEFAULT) * s;
    let atom = model.anchors.as_ref().and_then(|a| a.atom).map(scale3).unwrap_or([0.0, pcb_top, 0.0]);
    let hidden = hidden_ids(&model.parts, p);
    // Every part's box (board-layout.ts scaleBoxes(partBoxes)): the camera's view, the aisles' tall test, the planning grid
    // and the nav grid all read them, the fitted GPU cards and the attached parts included.
    let boxes: Vec<PartBox<'a>> = board.part_boxes().iter().map(|it| PartBox { name: &it.name, lo: scale3(it.bounds.min), hi: scale3(it.bounds.max) }).collect();

    // The slots the default camera sees, on a part that shows; an explicit `occluded` overrides the camera's view.
    let camera = model.anchors.as_ref().and_then(|a| a.camera.as_ref()).map(|c| scale3(c.at));
    let mut visible = Vec::new();
    for (i, sl) in model.slots.as_deref().unwrap_or(&[]).iter().enumerate() {
        let cand = Cand {
            id: sl.id.clone(),
            on: sl.on.clone(),
            kind: sl.kind.clone(),
            rect: [sl.rect[0] * s, sl.rect[1] * s, sl.rect[2] * s, sl.rect[3] * s],
            top: ctx.ground[i] * s,
            capacity: sl.capacity,
        };
        // The camera test samples just above the slot's own top in the model (board-layout.ts slotBlockedShare uses
        // `slot.top`), not its ground: a slot under a GPU card grounds on the card, and rays from up there are never blocked.
        let seen_from = Cand { top: sl.top * s, ..cand.clone() };
        let occluded = match sl.occluded {
            Some(b) => b,
            None => camera.is_some_and(|cam| blocked_share(&seen_from, cam, &boxes) > OCCLUSION_SHARE),
        };
        if !hidden.contains(&sl.on) && !occluded {
            visible.push(cand);
        }
    }
    let size = [model.size[0] * s, model.size[2] * s];
    let cands: Vec<Cand> = if visible.is_empty() {
        vec![Cand {
            id: String::new(),
            on: String::new(),
            kind: "soc".to_string(),
            rect: [-size[0] / 2.0, -size[1] / 2.0, size[0], size[1]],
            top: pcb_top,
            capacity: 1,
        }]
    } else {
        visible
    };

    // The planning grid: the board-only nav grid over the PCB, the part boxes and the candidate plates (board_grid.rs).
    let pcb = Rect { x0: -size[0] / 2.0, z0: -size[1] / 2.0, x1: size[0] / 2.0, z1: size[1] / 2.0 };
    // The walk's holes are the parts the grid blocks (nav-grid.ts rule 4 on the PCB): every part box standing more than
    // NAV_STEP above it, as the planning grid takes them.
    let tall: Vec<Rect> = boxes
        .iter()
        .filter(|b| b.hi[1] - pcb_top > NAV_STEP)
        .map(|b| Rect { x0: b.lo[0], z0: b.lo[2], x1: b.hi[0], z1: b.hi[2] })
        .collect();
    let plate_list: Vec<([f64; 4], f64)> = cands.iter().map(|c| (c.rect, c.top)).collect();
    // Every part box counts here (board-layout.ts navBoardOf takes partBoxes), the GPU cards and attached parts too.
    let parts: Vec<([f64; 3], [f64; 3])> = boxes.iter().map(|b| (b.lo, b.hi)).collect();
    let grid0 = board_grid(size, pcb_top, &parts, &plate_list);
    let main = grid0.main_component();

    let sup_at = |x: f64, z: f64| s * board.support_below(x / s, z / s, f64::INFINITY);
    let lattices: Vec<Lattice> = cands.iter().map(|c| Lattice::of(c.rect, &sup_at)).collect();
    ScaleSetup { s, pcb, pcb_top, atom, size, boxes, parts, tall, cands, grid0, main, lattices }
}

/// One layout pass at the setup's scale with the `excluded` keys left out (board-layout.ts layoutAtScale, without the
/// exits and the acceptance's walk). Each pass plans the aisles again, since the excluded keys change them.
fn at_scale(ctx: &Ctx, input: &LayoutInput, setup: &ScaleSetup, excluded: &BTreeSet<String>) -> Pass {
    let table = &ctx.table;
    let (s, pcb, pcb_top, atom, size) = (setup.s, setup.pcb, setup.pcb_top, setup.atom, setup.size);
    let (boxes, tall, cands, grid0, main) = (&setup.boxes, &setup.tall, &setup.cands, &setup.grid0, setup.main);
    let roles: Vec<(&str, bool)> = input.agents.iter().map(|a| (a.role.as_str(), a.role == rooms::ORCHESTRATOR)).collect();
    let row_min = rooms::typical_row_depth(&roles, table);
    let mut aisles = Vec::new();
    let mut exits = Vec::new();
    let mut unusable = Vec::new();
    for (ci, c) in cands.iter().enumerate() {
        match plan_aisle(ci, c, boxes, row_min, grid0, main, pcb_top, excluded) {
            Ok((a, e)) => {
                aisles.push(a);
                exits.push(e);
            }
            Err(reason) => unusable.push(UnusableSlot { slot: c.id.clone(), reason }),
        }
    }
    let n = aisles.len();
    let aisle_cand = |i: usize| &cands[aisles[i].cand];

    // The plates' heights (the plate rule): each rests on the highest support sample under its footprint, plus PLATE, and
    // its columns run up from the lattice under it. The rooms stand on these tops, so they come first.
    let plate_geo: Vec<(f64, f64, Vec<columns::Leaf>)> = (0..n)
        .map(|i| {
            let lat = &setup.lattices[aisles[i].cand];
            let highest = lat.highest();
            (highest + PLATE, highest, lat.columns(highest))
        })
        .collect();
    let plate_top: Vec<f64> = plate_geo.iter().map(|g| g.0).collect();

    // Seats: the orchestrator first, then each role in name order (unnamed agents get a room of their own).
    let mut requests: Vec<(String, Family, Vec<usize>, bool)> = Vec::new();
    let orch: Vec<usize> = (0..input.agents.len()).filter(|&i| roles[i].1).collect();
    if !orch.is_empty() {
        requests.push((rooms::ORCHESTRATOR.to_string(), Family::Command, orch, true));
    }
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, a) in input.agents.iter().enumerate() {
        if roles[i].1 {
            continue;
        }
        let key = if a.role.is_empty() { format!("unnamed:{}", a.id) } else { a.role.clone() };
        groups.entry(key).or_default().push(i);
    }
    for (key, members) in groups {
        let role = if key.starts_with("unnamed:") { "other".to_string() } else { key };
        let family = rooms::family_of(&role);
        requests.push((role, family, members, false));
    }
    let mut load = vec![0usize; n];
    let mut need = vec![0.0_f64; n];
    let mut seats = Vec::with_capacity(requests.len());
    for (role, family, members, centre) in requests {
        let m = rooms::minimum_side(&role, members.len(), table).unwrap_or(0.0);
        let slot = Fill { aisles: &aisles, cands, load: &load, need: &need }.place(family, centre, m, [atom[0], atom[2]]);
        if let Some(i) = slot {
            load[i] += 1;
            need[i] = need[i].max(rooms::cell_need_of(m));
        }
        seats.push(Seat { role, family, members, centre, slot, min_side: m });
    }

    // Rooms: the orchestrator takes the cell nearest the atom; the others take the first free cell. A seat with no free
    // cell is joined below, as a seat with no slot is.
    let cells: Vec<Vec<Cell>> = (0..n).map(|i| rooms::room_cells(&aisles[i], need[i])).collect();
    let mut taken: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let mut built: Vec<Built> = Vec::new();
    let mut unseated: Vec<usize> = Vec::new();
    for (si, seat) in seats.iter().enumerate() {
        let Some(i) = seat.slot else {
            unseated.push(si);
            continue;
        };
        let free: Vec<usize> = (0..cells[i].len()).filter(|k| !taken[i].contains(k)).collect();
        let dist = |k: usize| (cells[i][k].cx - atom[0]).hypot(cells[i][k].cz - atom[2]);
        let pick = if seat.centre { free.into_iter().reduce(|best, k| if dist(k) < dist(best) { k } else { best }) } else { free.first().copied() };
        let Some(k) = pick else {
            unseated.push(si);
            continue;
        };
        taken[i].insert(k);
        built.push(Built {
            role: seat.role.clone(),
            family: seat.family,
            centre: seat.centre,
            slot: i,
            cell: cells[i][k],
            members: seat.members.clone(),
            min_side: seat.min_side,
        });
    }

    // Joins: an unseated role joins the nearest built room of its family that still seats the desk, else the largest
    // built room with a desk to spare. Joining adds desks, not cells.
    for &si in &unseated {
        let seat = &seats[si];
        if built.is_empty() {
            break;
        }
        let pref = (0..n).find(|&i| rooms::family_types(seat.family).contains(&aisle_cand(i).kind.as_str()));
        let (px, pz) = match pref {
            Some(i) => {
                let c = centre_of(aisle_cand(i).rect);
                (c[0], c[1])
            }
            None => (atom[0], atom[2]),
        };
        let dist = |slot: usize| rect_distance(aisle_cand(slot).rect, px, pz);
        let fits_seat = |b: &Built| b.members.len() + seat.members.len() <= rooms::desk_cap(&b.role, b.cell.w, b.cell.d, table);
        let mut order: Vec<usize> = (0..built.len()).filter(|&b| rooms::family_of(&built[b].role) == seat.family).collect();
        order.sort_by(|&x, &y| dist(built[x].slot).total_cmp(&dist(built[y].slot)).then_with(|| built[x].role.cmp(&built[y].role)));
        let host = match order.iter().copied().find(|&b| fits_seat(&built[b])) {
            Some(b) => b,
            None => {
                let spare = (0..built.len()).filter(|&b| fits_seat(&built[b])).reduce(|best, b| {
                    if built[b].cell.w.min(built[b].cell.d) > built[best].cell.w.min(built[best].cell.d) {
                        b
                    } else {
                        best
                    }
                });
                match spare {
                    Some(b) => b,
                    None => continue,
                }
            }
        };
        built[host].members.extend(seat.members.iter().copied());
        let (role, count, old) = (built[host].role.clone(), built[host].members.len(), built[host].min_side);
        built[host].min_side = rooms::minimum_side(&role, count, table).unwrap_or(old);
    }

    // Rooms, floors, doors and homes.
    let mut rooms_out = Vec::with_capacity(built.len());
    let mut homes = Vec::new();
    for (rid, b) in built.iter().enumerate() {
        let c = b.cell;
        let (turn, door) = rooms::turn_and_door(&c, b.centre);
        let [fx, fz] = c.face;
        let door_at = if fx.abs() > fz.abs() { [c.cx + fx.signum() * c.w / 2.0, c.cz] } else { [c.cx, c.cz + fz.signum() * c.d / 2.0] };
        let mut members: Vec<usize> = b.members.clone();
        members.sort_by(|&x, &y| input.agents[x].id.cmp(&input.agents[y].id));
        let side = ((members.len() as f64).sqrt().ceil() as usize).max(1);
        let step = if side > 1 { (0.8 * c.w.min(c.d) / (side - 1) as f64).min(3.0) } else { 0.0 };
        let span = step * (side - 1) as f64 / 2.0;
        for (k, &a) in members.iter().enumerate() {
            let at = [c.cx - span + (k % side) as f64 * step, c.cz - span + (k / side) as f64 * step];
            homes.push(Home { agent: input.agents[a].id.clone(), room: rid, at });
        }
        rooms_out.push(Room {
            id: rid,
            role: b.role.clone(),
            family: b.family,
            round: b.centre,
            hue: if b.centre { rooms::DAIS_HUE } else { rooms::role_hue(&b.role) },
            r: c.w.min(c.d) / 2.0 * ROOM_SHARE,
            centre: [c.cx, c.cz],
            half: [c.w / 2.0, c.d / 2.0],
            inner: [rooms::inner_of(c.w), rooms::inner_of(c.d)],
            turn,
            floor_y: plate_top[b.slot],
            plate: b.slot,
            door,
            door_at,
            aisle_dir: c.face,
            members: members.iter().map(|&a| input.agents[a].id.clone()).collect(),
        });
    }

    // Plates, their columns and the slots they hold.
    let mut plates = Vec::with_capacity(n);
    let mut columns_out = Vec::new();
    let mut slots_out = Vec::with_capacity(n);
    for (i, aisle) in aisles.iter().enumerate() {
        let c = aisle_cand(i);
        let (top, highest, leaves) = &plate_geo[i];
        for l in leaves {
            columns_out.push(Column { plate: i, rect: l.rect, bottom: l.bottom, top: *highest });
        }
        plates.push(Plate { id: i, slot: c.id.clone(), rect: c.rect, y: *top, support: *highest, bottom: *highest, exit: Some(exits[i].clone()) });
        let on_slot: Vec<usize> = rooms_out.iter().filter(|r| r.plate == i).map(|r| r.id).collect();
        let agents: Vec<String> = on_slot.iter().flat_map(|&r| rooms_out[r].members.clone()).collect();
        slots_out.push(LayoutSlot {
            id: c.id.clone(),
            on: c.on.clone(),
            kind: c.kind.clone(),
            rect: c.rect,
            top: *top,
            double: aisle.double,
            rooms: on_slot,
            agents,
        });
    }

    let gangways_out: Vec<Gangway> = exits
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == ExitKind::Gangway)
        .map(|(i, e)| Gangway { plate: i, dir: e.dir, bridge: e.bridge.clone(), landing: e.board_at })
        .collect();

    // The walk: each plate's mouth (mouth.rs) where it fits clear of the rooms (with their cliff band), the other plates and
    // the mouths already placed. The mouths give the walk surfaces and the lifts; the acceptance is the web's, below.
    let plate_geo: Vec<PlateGeo> = (0..n).map(|i| PlateGeo { id: i, rect: Rect::from_xywh(aisle_cand(i).rect), y: plate_top[i] }).collect();
    let room_geo: Vec<RoomGeo> = rooms_out
        .iter()
        .map(|r| RoomGeo {
            id: r.id,
            plate: r.plate,
            rect: Rect { x0: r.centre[0] - r.half[0], z0: r.centre[1] - r.half[1], x1: r.centre[0] + r.half[0], z1: r.centre[1] + r.half[1] },
            floor_y: r.floor_y,
            door_at: r.door_at,
            face: r.aisle_dir,
        })
        .collect();
    let mut mouths: Vec<(usize, Mouth)> = Vec::new();
    let mut accepted: Vec<(usize, Rect)> = Vec::new();
    for (i, e) in exits.iter().enumerate() {
        let Some(m) = mouth(e, aisle_cand(i).rect, plate_top[i], pcb_top) else { continue };
        let footprint = mouth::footprint(&m);
        let clear = footprint.iter().all(|r| {
            room_geo.iter().all(|rr| !overlaps(r, &grow(&rr.rect, RAMP_DEPTH)))
                && plate_geo.iter().all(|pg| pg.id == i || !overlaps(r, &pg.rect))
                && accepted.iter().all(|(j, ar)| *j == i || !overlaps(r, ar))
        });
        if clear {
            accepted.extend(footprint.iter().map(|r| (i, *r)));
            mouths.push((i, m));
        }
    }
    let asm = surfaces::assemble(&Assembly { pcb, pcb_top, tall, plates: &plate_geo, rooms: &room_geo, mouths: &mouths });
    #[cfg(test)]
    LAST_WALK.with(|w| *w.borrow_mut() = Some(asm.walk.clone()));

    // The acceptance (board-layout.ts layoutAtScale and unreachedKeys): a landing must be clear of the rooms (landingClear);
    // once every landing is, the nav grid with the rooms and their furniture in place must reach every plate (its own
    // landing search, rule 6), and every room's door must reach the main component, else the room's slot is taken out.
    let slabs: Vec<Slab> = rooms_out.iter().map(|r| Slab { cx: r.centre[0], cz: r.centre[1], half: r.half[0].min(r.half[1]), round: r.round }).collect();
    let slot_rects: Vec<[f64; 4]> = (0..n).map(|i| aisle_cand(i).rect).collect();
    let mut rejected: BTreeSet<String> = exits.iter().filter(|e| !landing_clear(e, &slabs, &slot_rects)).map(|e| e.key.clone()).collect();
    if rejected.is_empty() {
        let slots: Vec<([f64; 4], f64)> = (0..n).map(|i| (aisle_cand(i).rect, aisle_cand(i).top)).collect();
        let traces = scene::traces_of(ctx.model, s);
        let landings: Vec<([f64; 4], f64, &Exit)> = slots.iter().zip(&exits).map(|(&(rect, top), e)| (rect, top, e)).collect();
        let grid = scene::scene(setup.size, pcb_top, &setup.parts, traces, &landings, &rooms_out, None).grid;
        let first = rooms_out.len() as i32 + 1;
        for id in &grid.unreached {
            if let Some(e) = exits.get((id - first) as usize) {
                rejected.insert(e.key.clone());
            }
        }
        for tid in navgrid::connectivity(&grid).unconnected {
            if let Some(r) = rooms_out.get((tid - 1) as usize) {
                rejected.insert(format!("slot|{}", plates[r.plate].slot));
            }
        }
    }

    // Lifts and landings: each mouth that is a lift or a gangway, with its surfaces and stops.
    let mut lifts = Vec::new();
    let mut landings = Vec::new();
    for ((plate, m), ids) in mouths.iter().zip(&asm.mouths) {
        if let (Mouth::Lift { cab, bottom, top, top_kind, door, gangway, bottom_y, top_y, .. }, Some(cab_id), Some(bottom_id), Some(top_id)) = (m, ids.cab, ids.bottom, ids.top) {
            let _ = (cab, bottom, top);
            lifts.push(Lift {
                plate: *plate,
                cab: cab_id,
                cab_rect: cab.xywh(),
                bottom: bottom_id,
                top: top_id,
                stops: [*bottom_y, *top_y],
                door: *door,
                gangway: *gangway,
            });
            landings.push(Landing { surface: bottom_id, plate: *plate, height: *bottom_y, role: LandingRole::Bottom });
            let role = if *top_kind == SurfaceKind::Bridge { LandingRole::Bridge } else { LandingRole::Top };
            landings.push(Landing { surface: top_id, plate: *plate, height: *top_y, role });
        }
    }
    let rejected: Vec<String> = rejected.into_iter().collect();
    let layout = Layout {
        board_kind: ctx.kind.to_string(),
        board_scale: s,
        pcb_top,
        atom,
        bounds: Bounds { cx: 0.0, cz: 0.0, w: size[0], d: size[1] },
        slots: slots_out,
        unusable,
        plates,
        columns: columns_out,
        rooms: rooms_out,
        homes,
        gangways: gangways_out,
        lifts,
        landings,
        walk: asm.walk,
    };
    Pass { layout, rejected }
}

/// Is an exit's landing clear of the rooms (board-layout.ts landingClear, nav-grid.ts rule 6): the board cell it reaches
/// (and a lift's queue) is outside every slab and cliff band, its plate cell outside every slab, and a gangway's bridge
/// outside every slab (a bridge may cross a band, not a wall).
fn landing_clear(e: &Exit, slabs: &[Slab], plates: &[[f64; 4]]) -> bool {
    let on_plate = |x: f64, z: f64| plates.iter().any(|&[px, pz, w, d]| x >= px && x < px + w && z >= pz && z < pz + d);
    let [bx, bz] = e.board_at;
    if !landing_clear_at(slabs, bx, bz, on_plate(bx, bz)) {
        return false;
    }
    if e.kind == ExitKind::Gangway {
        return e.bridge.iter().all(|&[x, z]| landing_clear_at(slabs, x, z, true));
    }
    if !landing_clear_at(slabs, e.plate_at[0], e.plate_at[1], true) {
        return false;
    }
    if e.kind == ExitKind::Lift {
        let (qx, qz) = (bx + e.dir[0] * NAV_CELL, bz + e.dir[1] * NAV_CELL);
        if !landing_clear_at(slabs, qx, qz, on_plate(qx, qz)) {
            return false;
        }
    }
    true
}

/// Does a rect overlap another in area (touching edges do not count)?
fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0.max(b.x0) < a.x1.min(b.x1) - 1e-9 && a.z0.max(b.z0) < a.z1.min(b.z1) - 1e-9
}

/// A rect grown by `d` on every side.
fn grow(r: &Rect, d: f64) -> Rect {
    Rect { x0: r.x0 - d, z0: r.z0 - d, x1: r.x1 + d, z1: r.z1 + d }
}

// Test hook: the walk set of the latest grid pass of `layout` on this thread.
#[cfg(test)]
thread_local! {
    static LAST_WALK: std::cell::RefCell<Option<WalkSet>> = const { std::cell::RefCell::new(None) };
}

/// The walk set of the last grid pass of a one-agent layout (the orchestrator) on a board kind, for the grid tests.
#[cfg(test)]
pub(crate) fn real_walk(board_kind: &str) -> WalkSet {
    let agents = vec![AgentSpec { id: "agent-00".to_string(), role: "orchestrator".to_string(), seat_hint: None }];
    // Edge does not seat one agent under the cap, which panics; its last pass still holds a real board's walk.
    let input = LayoutInput { agents, gpus: 0, scale_cap: 3.0 };
    let _ = std::panic::catch_unwind(|| layout(board_kind, &input));
    LAST_WALK.with(|w| w.borrow_mut().take()).expect("layout runs a grid pass")
}
