//! The board layout (`docs/design/hardware-plazas.md`, "Rooms on components"): where each role's
//! room sits on a machine's board model, and the walkways between the rooms. Pure, no rendering.
//! The web mirror is `apps/web/src/views/floor3d/board-layout.ts`: the same algorithm, the same
//! numbers, the same order (the orchestrator first, then the roles by name).
//!
//! Nothing outside this module and its tests calls it yet: the Floor integration consumes it next.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::AssertUnwindSafe;

use super::layout::role_hue;
use super::{AgentInfo, District};
use crate::hardware::{self, Model, Params};
use crate::rooms;
use crate::roles::role_family;

/// The surface the robots walk on between the rooms when the board has no `pcb_top` param (the web's `?? 0.4`).
const PCB_TOP: f32 = 0.4;
/// The sloped band around a room, in units: from the room's height down to the PCB top.
const RAMP: f32 = 1.0;
/// A room's footprint as a share of its cell.
const ROOM_SHARE: f32 = 0.9;
/// Homes sit this far apart at most.
const HOME_GAP: f32 = 3.0;
/// The share of a cell's short side that the homes of a row spread across (the web's `0.8`).
const HOME_SHARE: f32 = 0.8;
/// The orchestrator's hue: the Floor's dais.
const ORCHESTRATOR_HUE: f32 = 25.0;
/// The slack on size comparisons (the web's 1e-9, in f32).
const EPS: f32 = 1e-4;
/// The camera occlusion (the web's OCCLUSION_GRID, OCCLUSION_HEIGHT, OCCLUSION_SHARE): a slot's samples are a
/// 3 by 3 grid on its top, 1 unit above it; the slot is hidden when MORE than a third of them are blocked.
const OCCLUSION_GRID: usize = 3;
const OCCLUSION_HEIGHT: f64 = 1.0;
const OCCLUSION_SHARE: f64 = 1.0 / 3.0;

/// The layout's inputs besides the agents: the `when` params (as `hardware::build` takes them). The PCB top is the
/// board's `pcb_top` param, overridden by these (the web's `{...board.params, ...params}`).
#[allow(dead_code)] // the Floor integration consumes it next
#[derive(Debug, Clone, Default)]
pub struct BoardParams {
    pub params: Params,
}

/// The board's footprint, centred on the origin, as its `size` says.
#[allow(dead_code)] // the Floor integration consumes it next
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub cx: f32,
    pub cz: f32,
    pub w: f32,
    pub d: f32,
}

/// A role's room on a slot: the Floor's `District`, plus the slot's top, its id and the cell size.
#[allow(dead_code)] // the Floor integration consumes it next
#[derive(Debug, Clone)]
pub struct BoardDistrict {
    /// The room's turn and door from the shared layout (nav-scene.ts roomPlan reads the layout's): None for the engine's
    /// own layout, whose rooms face the centre and open to the nearest trace.
    pub plan: Option<(f32, crate::rooms::generate::Door)>,
    /// The room's key: its role, or `unnamed:<cid>` for an agent with no role (its `district.role` is then
    /// `other`, the family). The web keeps the key in its grouping only; this is the same key.
    pub key: String,
    pub district: District,
    /// The slot's top: the room's floor height.
    pub y: f32,
    /// The slot id.
    pub slot: String,
    /// The cell size (the room's footprint is `ROOM_SHARE` of it).
    pub w: f32,
    pub d: f32,
}

/// `layoutOnBoard`: the rooms, each agent's home (`cid` → district index, x, z), the atom and the
/// board's footprint.
#[allow(dead_code)] // the Floor integration consumes it next
#[derive(Debug, Clone)]
pub struct BoardLayout {
    pub districts: Vec<BoardDistrict>,
    pub homes: HashMap<u64, (usize, f32, f32)>,
    pub atom: [f32; 3],
    pub bounds: Bounds,
    pub pcb_top: f32,
    /// The shared layout this one came from and its GPU count (None for the engine's own layout): the board's
    /// navigation (nav.rs) is built from it.
    pub shared: Option<(std::sync::Arc<apz_board::layout::Layout>, u32)>,
}

/// The board's copper: nodes are the distinct trace points, edges join consecutive points.
#[allow(dead_code)] // the Floor integration consumes it next
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraceGraph {
    pub nodes: Vec<[f32; 2]>,
    pub edges: Vec<(usize, usize)>,
}

/// A slot that exists on this board (its `on` part is shown), with the rooms placed on it so far.
struct Live {
    id: String,
    kind: String,
    top: f32,
    rect: [f32; 4],
    capacity: usize,
    /// Indices into the groups, in placement order.
    rooms: Vec<usize>,
    /// The largest minimum side among its rooms, over `ROOM_SHARE` (the web's `need`): its cells must reach it.
    need: f32,
}

/// One room: the orchestrator's agents, or a role's agents.
struct Group<'a> {
    key: String,
    role: String,
    orch: bool,
    members: Vec<&'a AgentInfo>,
}

/// The kinds a family prefers, in order (the spec's table). Anything else takes `storage`.
fn prefers(family: &str) -> &'static [&'static str] {
    match family {
        "command" => &["cpu", "soc"],
        "build" => &["gpu", "npu", "cpu", "soc"],
        "knowledge" => &["memory", "storage"],
        "operations" => &["network", "io", "bmc", "modem"],
        "money" => &["power", "io"],
        _ => &["storage"],
    }
}

/// The distance from a point to a rect `[x, z, w, d]` (0 inside).
fn rect_distance(r: [f32; 4], p: [f32; 2]) -> f32 {
    let dx = (r[0] - p[0]).max(0.0).max(p[0] - (r[0] + r[2]));
    let dz = (r[1] - p[1]).max(0.0).max(p[1] - (r[1] + r[3]));
    (dx * dx + dz * dz).sqrt()
}

/// The short side of a cell when `rect` is split into `n` cells (the cells `cell` gives).
fn cell_side(rect: [f32; 4], n: usize) -> f32 {
    let (_, _, w, d) = cell(rect, n, 0);
    w.min(d)
}

/// The rooms a slot holds with cells at least `need` (already over `ROOM_SHARE`) on their short side: its capacity, lowered in count
/// as needed (fewer, larger cells). 0 when not even one cell reaches `need`.
fn holds_at_least(s: &Live, need: f32) -> usize {
    (1..=s.capacity).rev().find(|&n| cell_side(s.rect, n) >= need - EPS).unwrap_or(0)
}

/// Whether a room with minimum side `m` has a place in the slot: one more room, its cells at least `m / ROOM_SHARE`
/// (the room's footprint is `ROOM_SHARE` of its cell; the web's `hasRoom`), and at least the slot's largest so far.
fn fits(s: &Live, m: f32) -> bool {
    s.rooms.len() < holds_at_least(s, s.need.max(m / ROOM_SHARE))
}

/// The first slot with a place for a room of minimum side `m` that `want` accepts. The orchestrator
/// takes the one nearest the atom; a role takes the first in the board's order.
fn first_free(live: &[Live], want: impl Fn(&Live) -> bool, orch: bool, atom: [f32; 2], m: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in live.iter().enumerate() {
        if !fits(s, m) || !want(s) {
            continue;
        }
        if !orch {
            return Some(i);
        }
        let d = rect_distance(s.rect, atom);
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// The slot for a family's room of minimum side `m`: the preferred kinds in order, then any slot with room.
/// None when no slot holds the room at `m` (the web's `place` returning -1): the room then joins a built room.
fn choose(live: &[Live], family: &str, orch: bool, atom: [f32; 2], m: f32) -> Option<usize> {
    for kind in prefers(family) {
        if let Some(i) = first_free(live, |s| s.kind == *kind, orch, atom, m) {
            return Some(i);
        }
    }
    first_free(live, |_| true, orch, atom, m)
}

/// A room's family: the orchestrator's is `command`, a role's is its name's (the web's `roleFamily`).
fn family_of(g: &Group<'_>) -> &'static str {
    if g.orch {
        "command"
    } else {
        role_family(&g.role, "worker")
    }
}

/// Cell `k` of `n` cells tiling `rect`: `(cx, cz, w, d)` (the web's `gridCells`). The factor pair of `n`
/// whose cells are nearest square splits the longer side; a tie goes to the later factor.
fn cell(rect: [f32; 4], n: usize, k: usize) -> (f32, f32, f32, f32) {
    let [x, z, w, d] = rect;
    let along_x = w >= d;
    let (long, short) = if along_x { (w, d) } else { (d, w) };
    let (long, short) = (long.max(1e-6), short.max(1e-6));
    let n = n.max(1);
    let mut along = 1;
    let mut best = f32::INFINITY;
    for f in (1..=n).filter(|f| n % f == 0) {
        let score = ((long / f as f32) / (short / (n / f) as f32)).ln().abs();
        if score <= best {
            best = score;
            along = f;
        }
    }
    let across = n / along;
    let (cell_long, cell_short) = (long / along as f32, short / across as f32);
    let along_at = ((k % along) as f32 + 0.5) * cell_long;
    let across_at = ((k / along) as f32 + 0.5) * cell_short;
    if along_x {
        (x + along_at, z + across_at, cell_long, cell_short)
    } else {
        (x + across_at, z + along_at, cell_short, cell_long)
    }
}

/// The gap between neighbouring homes (the web's `step`): 0.8 of the cell's short side shared by the
/// `side - 1` gaps of a row, at most `HOME_GAP`; 0 for a single column.
fn home_step(cell_min: f32, side: usize) -> f32 {
    if side > 1 {
        (HOME_SHARE * cell_min / (side - 1) as f32).min(HOME_GAP)
    } else {
        0.0
    }
}

/// The homes of a room's agents: a square grid of `side` = ceil(sqrt(agents)) per row, spaced by
/// `home_step`, centred on the room. `members` are sorted by cid.
fn place_homes(homes: &mut HashMap<u64, (usize, f32, f32)>, index: usize, d: &District, members: &[&AgentInfo], cell_min: f32) {
    let side = ((members.len() as f32).sqrt().ceil().max(1.0)) as usize;
    let gap = home_step(cell_min, side);
    let span = (side - 1) as f32 * gap / 2.0;
    for (i, a) in members.iter().enumerate() {
        let gx = d.cx - span + (i % side) as f32 * gap;
        let gy = d.cy - span + (i / side) as f32 * gap;
        homes.insert(a.cid, (index, gx, gy));
    }
}

/// The parts `hardware::build` shows for this board and params (a `when` that fails hides a part
/// and its children).
fn shown_parts(board: &Model, params: &Params) -> HashSet<String> {
    hardware::build(board, params).parts.into_keys().collect()
}

/// A room's minimum side for `agents` agents (`rooms::min_side`: the minsize table, else computed). A size
/// no side fits is 0, with a warning: the room is placed by the rules alone and its slot is not grown for it.
fn min_side_of(min_side: &impl Fn(&str, usize) -> f64, role: &str, agents: usize) -> f32 {
    let m = min_side(role, agents);
    if m.is_finite() {
        return m as f32;
    }
    eprintln!("agentplaza-desktop: board layout: no room size fits {role} with {agents} agent(s); its slot is left as it is");
    0.0
}

/// Is the segment from `from` to `to` blocked by the box (lo, hi) strictly between its ends? A slab test; a segment
/// that only touches a face does not count (the web's `segmentBlocked`).
fn segment_blocked(from: [f64; 3], to: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..3 {
        let o = from[k];
        let d = to[k] - o;
        if d.abs() < 1e-12 {
            if o < lo[k] || o > hi[k] {
                return false;
            }
            continue;
        }
        let (mut a, mut b) = ((lo[k] - o) / d, (hi[k] - o) / d);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return false;
        }
    }
    t1 - t0 > 1e-9 && t0 < 1.0 - 1e-9 && t1 > 1e-9
}

/// The part a box belongs to: its id, or its path under the id (`gpu_1/pcb` or `gpu_1.pcb`) (the web's `boxOfPart`).
fn box_of_part(b: &hardware::PartBox, part: &str) -> bool {
    b.id == part || b.path == part || b.path.strip_prefix(part).is_some_and(|rest| rest.starts_with('/') || rest.starts_with('.'))
}

/// The share of a slot's samples whose segment from the camera is blocked by a part other than the slot's own (the
/// web's `slotBlockedShare`). The samples are a grid over the slot's rect, OCCLUSION_HEIGHT above its top.
fn slot_blocked_share(slot: &hardware::Slot, camera: [f64; 3], boxes: &[hardware::PartBox]) -> f64 {
    let [x, z, w, d] = slot.rect.map(f64::from);
    let n = OCCLUSION_GRID;
    let others: Vec<([f64; 3], [f64; 3])> = boxes
        .iter()
        .filter(|b| !box_of_part(b, &slot.on))
        .map(|b| (b.lo.map(f64::from), b.hi.map(f64::from)))
        .collect();
    let mut blocked = 0usize;
    for i in 0..n {
        for j in 0..n {
            let to = [
                x + (w * (i as f64 + 0.5)) / n as f64,
                f64::from(slot.top) + OCCLUSION_HEIGHT,
                z + (d * (j as f64 + 0.5)) / n as f64,
            ];
            if others.iter().any(|&(lo, hi)| segment_blocked(camera, to, lo, hi)) {
                blocked += 1;
            }
        }
    }
    blocked as f64 / (n * n) as f64
}

/// The ids of the slots the board's default camera cannot see: more than OCCLUSION_SHARE of a slot's samples are blocked
/// (the web's `occludedSlotIds`). A board without a camera anchor occludes nothing.
fn occluded_slot_ids(board: &Model, boxes: &[hardware::PartBox]) -> HashSet<String> {
    let Some(camera) = board.anchors.camera.as_ref() else {
        return HashSet::new();
    };
    let at = camera.at.map(f64::from);
    board
        .slots
        .iter()
        .filter(|s| slot_blocked_share(s, at, boxes) > OCCLUSION_SHARE)
        .map(|s| s.id.clone())
        .collect()
}

/// `layoutOnBoard(list, board, params)`: one room per role on the board's slots. The orchestrator
/// takes the cpu slot nearest the atom and sits on the atom; the roles follow by name. Slots whose
/// `on` part is not shown don't exist. A room goes to the first slot (in its family's order) that
/// holds it at its minimum side; a room no slot holds that way moves on. The slot's cells grow to
/// its rooms' minimums. A role no slot holds at its minimum joins a built room (the nearest of its family that
/// still reaches its minimum, else the largest built room), so every agent keeps a desk.
#[allow(dead_code)] // the Floor integration consumes it next
pub fn layout_on_board(list: &[AgentInfo], board: &Model, params: &BoardParams) -> BoardLayout {
    layout_on_board_with(list, board, params, rooms::min_side)
}

/// `layout_on_board` with each room's minimum side from `min_side(role, agents)` (the web's `table`
/// argument): f64::INFINITY where no size fits. Tests that place by the rules alone pass `|_, _| 0.0`.
pub fn layout_on_board_with(
    list: &[AgentInfo],
    board: &Model,
    params: &BoardParams,
    min_side: impl Fn(&str, usize) -> f64,
) -> BoardLayout {
    // The web's `p.pcb_top ?? 0.4` over `{...board.params, ...params}`: the caller's params win.
    let pcb_top = params.params.values.get("pcb_top").or_else(|| board.params.get("pcb_top")).map_or(PCB_TOP, |v| *v as f32);
    let bounds = Bounds { cx: 0.0, cz: 0.0, w: board.size[0], d: board.size[2] };
    // The web's default for a board with no atom: the footprint's centre at the PCB top (`anchors?.atom ?? [0, pcbTop, 0]`).
    let atom = board.anchors.atom.unwrap_or([bounds.cx, pcb_top, bounds.cz]);
    let shown = shown_parts(board, &params.params);
    let occluded = occluded_slot_ids(board, &hardware::part_boxes(board, &params.params));

    let mut live: Vec<Live> = board
        .slots
        .iter()
        // A slot is used when its part shows and the camera sees it; an explicit `occluded` on the slot overrides the camera.
        .filter(|s| shown.contains(&s.on) && !s.occluded.unwrap_or_else(|| occluded.contains(&s.id)))
        .map(|s| Live {
            id: s.id.clone(),
            kind: s.kind.clone(),
            top: s.top,
            rect: s.rect,
            capacity: s.capacity as usize,
            rooms: Vec::new(),
            need: 0.0,
        })
        .collect();
    if live.is_empty() {
        // A board with no usable slot still holds its roles: the whole footprint is one room's worth.
        live.push(Live {
            id: String::new(),
            kind: "soc".into(),
            top: pcb_top,
            rect: [bounds.cx - bounds.w / 2.0, bounds.cz - bounds.d / 2.0, bounds.w, bounds.d],
            capacity: 1,
            rooms: Vec::new(),
            need: 0.0,
        });
    }

    // Rooms by key (sorted by key, as the web's `groups.keys().sort()`). An agent with no role gets a room of
    // its own, keyed `unnamed:<cid>` and built as the `other` family.
    let mut by_key: BTreeMap<String, (String, Vec<&AgentInfo>)> = BTreeMap::new();
    let mut orch: Vec<&AgentInfo> = Vec::new();
    for a in list {
        if a.tier == "orchestrator" {
            orch.push(a);
        } else if a.role.is_empty() {
            by_key.entry(format!("unnamed:{}", a.cid)).or_insert_with(|| ("other".into(), Vec::new())).1.push(a);
        } else {
            by_key.entry(a.role.clone()).or_insert_with(|| (a.role.clone(), Vec::new())).1.push(a);
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    if !orch.is_empty() {
        groups.push(Group { key: "orchestrator".into(), role: "orchestrator".into(), orch: true, members: orch });
    }
    for (key, (role, members)) in by_key {
        groups.push(Group { key, role, orch: false, members });
    }
    for g in &mut groups {
        g.members.sort_by_key(|a| a.cid);
    }

    let atom2 = [atom[0], atom[2]];
    // Each room in order (the orchestrator first, then the roles by name): its minimum side, and the slot it takes
    // (the web's seatRoom). A room no slot holds at that size has no slot: it joins a built room below.
    let mut min_sides = vec![0f32; groups.len()];
    let mut slot_of: Vec<Option<usize>> = vec![None; groups.len()];
    for (gi, g) in groups.iter().enumerate() {
        let m = min_side_of(&min_side, &g.role, g.members.len());
        min_sides[gi] = m;
        if let Some(si) = choose(&live, family_of(g), g.orch, atom2, m) {
            live[si].rooms.push(gi);
            live[si].need = live[si].need.max(m / ROOM_SHARE);
            slot_of[gi] = Some(si);
        }
    }

    // Each slot's cells: as many as it holds at its rooms' minimums (its capacity when none is known), or one per room
    // when it overflows. The orchestrator takes the free cell nearest the atom; the others take the first free cell.
    let cell_count: Vec<usize> = live.iter().map(|s| holds_at_least(s, s.need).max(s.rooms.len())).collect();
    let mut taken: Vec<Vec<bool>> = cell_count.iter().map(|&n| vec![false; n]).collect();
    let mut cells = vec![(0f32, 0f32, 0f32, 0f32); groups.len()];
    for (gi, g) in groups.iter().enumerate() {
        let Some(si) = slot_of[gi] else { continue };
        let (s, n) = (&live[si], cell_count[si]);
        let free = (0..n).filter(|&k| !taken[si][k]);
        let k = if g.orch {
            let dist = |k: usize| {
                let (cx, cz, _, _) = cell(s.rect, n, k);
                (cx - atom[0]).hypot(cz - atom[2])
            };
            free.min_by(|&a, &b| dist(a).total_cmp(&dist(b)))
        } else {
            free.into_iter().next()
        }
        .unwrap_or(0);
        taken[si][k] = true;
        let (cx, cz, w, d) = cell(s.rect, n, k);
        if w.min(d) < min_sides[gi] - EPS {
            eprintln!(
                "agentplaza-desktop: board layout: {}'s cell is {:.2} wide, under its minimum {}; slot {} is full",
                g.role,
                w.min(d),
                min_sides[gi],
                s.id
            );
        }
        cells[gi] = (cx, cz, w, d);
    }

    // Joins (the web's join step): a room no slot holds at its minimum takes no cell. Its agents join the nearest built
    // room of its family (nearest by the distance from that room's slot to the centre of the family's first slot kind,
    // else to the atom; ties by role) whose cell still reaches the minimum for its new desk count. If none does, the
    // largest built room on the board takes them, with a warning. Joining adds desks, not cells.
    let built: Vec<usize> = (0..groups.len()).filter(|&gi| slot_of[gi].is_some()).collect();
    let min_wide = |gi: usize| cells[gi].2.min(cells[gi].3);
    for gi in 0..groups.len() {
        if slot_of[gi].is_some() {
            continue;
        }
        if built.is_empty() {
            eprintln!("agentplaza-desktop: board layout: no room is built, so {}'s agents have no desk", groups[gi].role);
            break;
        }
        let family = family_of(&groups[gi]);
        let (px, pz) = match live.iter().find(|s| prefers(family).contains(&s.kind.as_str())) {
            Some(s) => (s.rect[0] + s.rect[2] / 2.0, s.rect[1] + s.rect[3] / 2.0),
            None => (atom2[0], atom2[1]),
        };
        let slot_rect = |h: usize| live[slot_of[h].expect("a built room has a slot")].rect;
        let mut hosts: Vec<usize> = built.iter().copied().filter(|&h| family_of(&groups[h]) == family).collect();
        hosts.sort_by(|&a, &b| {
            rect_distance(slot_rect(a), [px, pz])
                .total_cmp(&rect_distance(slot_rect(b), [px, pz]))
                .then_with(|| groups[a].role.cmp(&groups[b].role))
        });
        let joining = groups[gi].members.len();
        let fits = |h: usize| {
            let m = min_side(&groups[h].role, groups[h].members.len() + joining);
            m.is_finite() && min_wide(h) >= m as f32 - EPS
        };
        let host = match hosts.iter().copied().find(|&h| fits(h)) {
            Some(h) => h,
            None => {
                let best = built.iter().copied().reduce(|best, b| if min_wide(b) > min_wide(best) { b } else { best });
                let best = best.expect("a built room exists");
                eprintln!(
                    "agentplaza-desktop: board layout: no room reaches {}'s minimum with one more desk; {}'s room takes the desk",
                    groups[gi].role, groups[best].role
                );
                best
            }
        };
        let joiners = std::mem::take(&mut groups[gi].members);
        groups[host].members.extend(joiners);
    }

    // The rooms of the built groups, in group order; each room's agents (its own and any that joined) sit in a square
    // grid of homes, sorted by cid.
    let mut districts = Vec::with_capacity(built.len());
    let mut homes = HashMap::new();
    for &gi in &built {
        let g = &groups[gi];
        let s = &live[slot_of[gi].expect("a built room has a slot")];
        let (cx, cz, w, d) = cells[gi];
        let district = District {
            role: g.role.clone(),
            cx,
            cy: cz,
            r: w.min(d) / 2.0 * ROOM_SHARE,
            hue: if g.orch { ORCHESTRATOR_HUE } else { role_hue(&g.role) },
            // The web's `shape`: the centre seat is round, every role's room is square (`roomStyle`).
            round: g.orch,
        };
        let mut members = g.members.clone();
        members.sort_by_key(|a| a.cid);
        place_homes(&mut homes, districts.len(), &district, &members, w.min(d));
        districts.push(BoardDistrict { plan: None, key: g.key.clone(), district, y: s.top, slot: s.id.clone(), w, d });
    }

    BoardLayout { districts, homes, atom, bounds, pcb_top, shared: None }
}

/// The world scale the shared board core may grow a layout to (the apz-board tests' cap).
const SHARED_SCALE_CAP: f64 = 3.0;

/// The shared board core's layout (`apz_board::layout::layout`) of a board kind for a roster, with every agent seated.
/// Err with the reason when the kind is unknown or no scale up to the cap seats everyone (the shared layout panics).
/// What a shared layout is computed from: the board kind, its GPU count, and each agent's id and shared role, sorted
/// by id, so a roster asks for the same layout whatever order it came in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SharedKey {
    kind: String,
    gpus: u32,
    agents: Vec<(u64, String)>,
}

impl SharedKey {
    pub fn new(kind: &str, agents: &[AgentInfo], gpus: u32) -> Self {
        let mut seats: Vec<(u64, String)> = agents
            .iter()
            .map(|a| (a.cid, if a.tier == "orchestrator" { "orchestrator".to_string() } else { a.role.clone() }))
            .collect();
        seats.sort();
        Self { kind: kind.to_string(), gpus, agents: seats }
    }
}

/// The key of a board's roster: the model's kind without its "board/" prefix, and its `gpus` parameter.
pub fn shared_key_for(list: &[AgentInfo], board: &Model, params: &BoardParams) -> SharedKey {
    let kind = board.kind.strip_prefix("board/").unwrap_or(&board.kind);
    let gpus = params.params.values.get("gpus").map_or(0, |g| g.max(0.0) as u32);
    SharedKey::new(kind, list, gpus)
}

/// The shared board core's layout (`apz_board::layout::layout`) for a key, with every agent seated. Err with the reason
/// when the kind is unknown or no scale up to the cap seats everyone (the shared layout panics).
pub fn shared_layout_of(key: &SharedKey) -> Result<apz_board::layout::Layout, String> {
    let input = apz_board::layout::LayoutInput {
        agents: key
            .agents
            .iter()
            .map(|(cid, role)| apz_board::layout::AgentSpec { id: cid.to_string(), role: role.clone(), seat_hint: None })
            .collect(),
        gpus: key.gpus,
        scale_cap: SHARED_SCALE_CAP,
    };
    std::panic::catch_unwind(AssertUnwindSafe(|| apz_board::layout::layout(&key.kind, &input))).map_err(|e| {
        e.downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "the shared layout panicked".to_string())
    })
}

/// The shared layout for a key as the engine's `BoardLayout`; None (logged once) when the shared core cannot seat it.
fn compute(key: &SharedKey) -> Option<BoardLayout> {
    match shared_layout_of(key) {
        Ok(shared) => {
            let mut layout = board_layout_of(&shared);
            layout.shared = Some((std::sync::Arc::new(shared), key.gpus));
            Some(layout)
        }
        Err(reason) => {
            static LOGGED: std::sync::Once = std::sync::Once::new();
            LOGGED.call_once(|| eprintln!("apz-engine: the shared board layout failed ({reason}); the engine's own is used"));
            None
        }
    }
}

/// The shared layouts computed or computing, by key. The first request for a key starts its computation on a thread
/// (native); `poll` moves the finished ones into `done`, which `request` returns from. wasm32 has no threads, so
/// nothing starts there and `request` stays None: the caller keeps the engine's own layout. A web worker that
/// computes the layout and posts it back is the next step.
#[derive(Default)]
pub struct SharedLayouts {
    done: HashMap<SharedKey, Option<BoardLayout>>,
    running: HashMap<SharedKey, std::sync::mpsc::Receiver<Option<BoardLayout>>>,
    #[cfg(test)]
    spawned: usize,
}

impl SharedLayouts {
    /// The layout for `key` once it is ready: None while it computes, and None for a key the shared core cannot seat.
    pub fn request(&mut self, key: SharedKey) -> Option<BoardLayout> {
        if let Some(done) = self.done.get(&key) {
            return done.clone();
        }
        if !self.running.contains_key(&key) {
            self.start(key);
        }
        None
    }

    /// Whether `key` is computing.
    pub fn is_pending(&self, key: &SharedKey) -> bool {
        self.running.contains_key(key)
    }

    /// Moves the finished computations into the cache.
    pub fn poll(&mut self) {
        let mut finished = Vec::new();
        for (key, rx) in &self.running {
            match rx.try_recv() {
                Ok(layout) => finished.push((key.clone(), layout)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => finished.push((key.clone(), None)),
            }
        }
        for (key, layout) in finished {
            self.running.remove(&key);
            self.done.insert(key, layout);
        }
    }

    fn start(&mut self, key: SharedKey) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        #[cfg(test)]
        {
            self.spawned += 1;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let job = key.clone();
        std::thread::spawn(move || {
            let _ = tx.send(compute(&job));
        });
        self.running.insert(key, rx);
    }
}

/// The process's shared layouts: one cache for every Floor, since a key is the whole input of its layout.
fn shared_cache() -> &'static std::sync::Mutex<SharedLayouts> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<SharedLayouts>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The shared layout for `key` when it is ready (the cache polls first); else None, and the first request starts it.
pub fn request_shared(key: &SharedKey) -> Option<BoardLayout> {
    let mut cache = shared_cache().lock().ok()?;
    cache.poll();
    cache.request(key.clone())
}

/// Whether `key`'s shared layout is computing.
pub fn shared_pending(key: &SharedKey) -> bool {
    shared_cache().lock().is_ok_and(|cache| cache.is_pending(key))
}

/// The engine's `BoardLayout` from a shared layout: the districts are its rooms in its order (a home's room is the
/// district index), and each agent's home is keyed by its cid (the agent id the shared layout was given).
pub fn board_layout_of(shared: &apz_board::layout::Layout) -> BoardLayout {
    let districts = shared
        .rooms
        .iter()
        .map(|room| {
            let key = if room.role.is_empty() {
                format!("unnamed:{}", room.members.first().map_or("", String::as_str))
            } else {
                room.role.clone()
            };
            let door = match room.door {
                apz_board::layout::Door::N => crate::rooms::generate::Door::N,
                apz_board::layout::Door::S => crate::rooms::generate::Door::S,
                apz_board::layout::Door::E => crate::rooms::generate::Door::E,
                apz_board::layout::Door::W => crate::rooms::generate::Door::W,
            };
            BoardDistrict {
                plan: Some((room.turn as f32, door)),
                key,
                district: District {
                    role: if room.role.is_empty() { "other".to_string() } else { room.role.clone() },
                    cx: room.centre[0] as f32,
                    cy: room.centre[1] as f32,
                    r: room.r as f32,
                    hue: room.hue as f32,
                    round: room.round,
                },
                y: room.floor_y as f32,
                slot: shared.plates.get(room.plate).map_or_else(String::new, |p| p.slot.clone()),
                w: (room.half[0] * 2.0) as f32,
                d: (room.half[1] * 2.0) as f32,
            }
        })
        .collect();
    let homes = shared
        .homes
        .iter()
        .filter_map(|h| Some((h.agent.parse::<u64>().ok()?, (h.room, h.at[0] as f32, h.at[1] as f32))))
        .collect();
    let b = shared.bounds;
    let [ax, ay, az] = shared.atom;
    BoardLayout {
        districts,
        homes,
        atom: [ax as f32, ay as f32, az as f32],
        bounds: Bounds { cx: b.cx as f32, cz: b.cz as f32, w: b.w as f32, d: b.d as f32 },
        pcb_top: shared.pcb_top as f32,
        shared: None,
    }
}

/// `boardHeightAt(x, z, layout)`: the surface height for a robot at (x, z). Inside a room's cell it
/// is the room's height; within `RAMP` outside a cell it slopes down to the PCB top; elsewhere the
/// PCB top.
#[allow(dead_code)] // the Floor integration consumes it next
pub fn board_height_at(x: f32, z: f32, layout: &BoardLayout) -> f32 {
    let mut best = layout.pcb_top;
    for bd in &layout.districts {
        let dx = ((bd.district.cx - x).abs() - bd.w / 2.0).max(0.0);
        let dz = ((bd.district.cy - z).abs() - bd.d / 2.0).max(0.0);
        let dist = (dx * dx + dz * dz).sqrt();
        if dist == 0.0 {
            return bd.y;
        }
        if dist < RAMP {
            let h = bd.y + (layout.pcb_top - bd.y) * (dist / RAMP);
            if h > best {
                best = h;
            }
        }
    }
    best
}

/// `traceGraph(board)`: the copper as a graph. Equal points of different traces are one node.
#[allow(dead_code)] // the Floor integration consumes it next
pub fn trace_graph(board: &Model) -> TraceGraph {
    let mut g = TraceGraph::default();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for t in &board.traces {
        let mut prev: Option<usize> = None;
        for &p in &t.path {
            let i = match g.nodes.iter().position(|&n| n == p) {
                Some(i) => i,
                None => {
                    g.nodes.push(p);
                    g.nodes.len() - 1
                }
            };
            if let Some(j) = prev {
                if j != i && seen.insert((j.min(i), j.max(i))) {
                    g.edges.push((j, i));
                }
            }
            prev = Some(i);
        }
    }
    g
}

fn seg_len(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// The graph node nearest a point (the first on a tie).
fn nearest(g: &TraceGraph, p: [f32; 2]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &n) in g.nodes.iter().enumerate() {
        let d = (n[0] - p[0]).powi(2) + (n[1] - p[1]).powi(2);
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// The shortest node path from `s` to `t` (Dijkstra; the lowest index wins a tie), or None.
fn shortest(g: &TraceGraph, s: usize, t: usize) -> Option<Vec<usize>> {
    let n = g.nodes.len();
    let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    for &(a, b) in &g.edges {
        let w = seg_len(g.nodes[a], g.nodes[b]);
        adj[a].push((b, w));
        adj[b].push((a, w));
    }
    let mut dist = vec![f32::INFINITY; n];
    let mut prev: Vec<Option<usize>> = vec![None; n];
    let mut done = vec![false; n];
    dist[s] = 0.0;
    loop {
        let mut u: Option<usize> = None;
        for i in 0..n {
            if !done[i] && dist[i].is_finite() && u.is_none_or(|j| dist[i] < dist[j]) {
                u = Some(i);
            }
        }
        let Some(u) = u else { break };
        if u == t {
            break;
        }
        done[u] = true;
        for &(v, w) in &adj[u] {
            let nd = dist[u] + w;
            if nd < dist[v] {
                dist[v] = nd;
                prev[v] = Some(u);
            }
        }
    }
    if !dist[t].is_finite() {
        return None;
    }
    let mut path = vec![t];
    while let Some(p) = prev[*path.last().unwrap()] {
        path.push(p);
    }
    path.reverse();
    Some(path)
}

/// `pathOnTraces(graph, from, to)`: the walkway from `from` to `to`, joining each end to its
/// nearest trace point. Without a walkway between them, the straight line.
#[allow(dead_code)] // the Floor integration consumes it next
pub fn path_on_traces(graph: &TraceGraph, from: [f32; 2], to: [f32; 2]) -> Vec<[f32; 2]> {
    let (Some(s), Some(t)) = (nearest(graph, from), nearest(graph, to)) else {
        return vec![from, to];
    };
    let Some(route) = shortest(graph, s, t) else {
        return vec![from, to];
    };
    let mut out = vec![from];
    out.extend(route.iter().map(|&i| graph.nodes[i]));
    out.push(to);
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::MODELS;

    /// The fixture board, the same JSON in the web test: three slots (cpu cap 2 on cpu0 at top 1,
    /// gpu cap 2 on gpu0 at top 2, memory cap 1 on dimm at top 3), the gpu hidden by
    /// `when: "gpus >= 1"`, the atom at (-10, 1.2, 0), and one L-shaped trace.
    const FIXTURE: &str = r#"{
      "schema": 1, "kind": "fixture", "view": "board", "size": [40, 4, 30],
      "params": { "gpus": 1 },
      "parts": [
        { "id": "cpu0", "geo": { "type": "box", "w": 8, "h": 1, "d": 8 }, "at": [-10, 0.5, 0] },
        { "id": "gpu0", "geo": { "type": "box", "w": 8, "h": 1, "d": 8 }, "at": [10, 0.5, 0], "when": "gpus >= 1" },
        { "id": "dimm", "geo": { "type": "box", "w": 8, "h": 1, "d": 4 }, "at": [0, 0.5, -10] }
      ],
      "anchors": { "atom": [-10, 1.2, 0] },
      "slots": [
        { "id": "cpu", "on": "cpu0", "type": "cpu", "top": 1.0, "rect": [-14, -4, 8, 8], "capacity": 2 },
        { "id": "gpu", "on": "gpu0", "type": "gpu", "top": 2.0, "rect": [6, -4, 8, 8], "capacity": 2 },
        { "id": "mem", "on": "dimm", "type": "memory", "top": 3.0, "rect": [-4, -12, 8, 4], "capacity": 1 }
      ],
      "traces": [ { "path": [[0, 0], [10, 0], [10, 10]], "y": 0.5 } ]
    }"#;

    fn fixture() -> Model {
        serde_json::from_str(FIXTURE).expect("the fixture parses")
    }

    fn agent(cid: u64, role: &str, tier: &str) -> AgentInfo {
        AgentInfo { cid, name: role.into(), role: role.into(), status: "idle".into(), tier: tier.into(), model: String::new(), species: None }
    }

    fn fixture_roles() -> Vec<AgentInfo> {
        vec![
            agent(1, "orchestrator", "orchestrator"),
            agent(2, "builder", "worker"),
            agent(3, "qa", "worker"),
            agent(4, "librarian", "worker"),
            agent(5, "finance", "worker"),
        ]
    }

    /// Role → slot id, sorted by role.
    fn slots_of(layout: &BoardLayout) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> =
            layout.districts.iter().map(|d| (d.district.role.clone(), d.slot.clone())).collect();
        out.sort();
        out
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(r, s)| (r.to_string(), s.to_string())).collect()
    }

    fn district<'a>(layout: &'a BoardLayout, role: &str) -> &'a BoardDistrict {
        layout.districts.iter().find(|d| d.district.role == role).expect("the role has a district")
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// No minimum room sizes: the fixture's own rules (the web's `NO_MINIMUMS`).
    fn no_minimums(_: &str, _: usize) -> f64 {
        0.0
    }

    #[test]
    fn fixture_families_take_their_preferred_slots() {
        let board = fixture();
        let list = fixture_roles();
        let layout = layout_on_board_with(&list, &board, &BoardParams::default(), no_minimums);
        // orchestrator: cpu (nearest the atom); builder and qa: gpu; librarian: memory;
        // finance (money): power and io are absent, so the first slot with room, cpu.
        assert_eq!(
            slots_of(&layout),
            pairs(&[("builder", "gpu"), ("finance", "cpu"), ("librarian", "mem"), ("orchestrator", "cpu"), ("qa", "gpu")])
        );
        assert_eq!(layout.districts.len(), 5);
        assert_eq!(layout.homes.len(), 5);
        assert_eq!(layout.atom, [-10.0, 1.2, 0.0]);
        assert_eq!(layout.bounds, Bounds { cx: 0.0, cz: 0.0, w: 40.0, d: 30.0 });
        // The orchestrator is first and takes the cpu cell nearest the atom (the first of the two
        // equally near); cells of cpu (2 across) are 4 wide, 8 deep.
        assert_eq!(layout.districts[0].district.role, "orchestrator");
        let orch = district(&layout, "orchestrator");
        assert!(close(orch.district.cx, -12.0) && close(orch.district.cy, 0.0));
        assert!(close(orch.w, 4.0) && close(orch.d, 8.0) && close(orch.y, 1.0));
        assert!(close(orch.district.r, 1.8));
        // Cells: finance is the cpu's second cell; builder and qa split the gpu; librarian fills mem.
        let fin = district(&layout, "finance");
        assert!(close(fin.district.cx, -8.0) && close(fin.district.cy, 0.0));
        assert!(close(district(&layout, "builder").district.cx, 8.0));
        assert!(close(district(&layout, "qa").district.cx, 12.0));
        let lib = district(&layout, "librarian");
        assert!(close(lib.district.cx, 0.0) && close(lib.district.cy, -10.0));
        assert!(close(lib.w, 8.0) && close(lib.d, 4.0) && close(lib.y, 3.0));
        // Homes: the orchestrator's agent sits at the room's centre.
        let (idx, gx, gy) = layout.homes[&1];
        assert_eq!(idx, 0);
        assert!(close(gx, -12.0) && close(gy, 0.0));
    }

    #[test]
    fn hidden_gpu_sends_builders_to_cpu() {
        let board = fixture();
        let list = fixture_roles();
        let mut params = BoardParams::default();
        params.params = params.params.with("gpus", 0.0);
        let layout = layout_on_board_with(&list, &board, &params, no_minimums);
        assert!(layout.districts.iter().all(|d| d.slot != "gpu"), "the hidden gpu slot does not exist");
        let slot = |role: &str| district(&layout, role).slot.clone();
        assert_eq!(slot("builder"), "cpu");
        assert_eq!(slot("orchestrator"), "cpu");
        assert_eq!(slot("finance"), "mem");
    }

    /// More roles than rooms: the roles that find no free cell join a built room, so every agent keeps a desk.
    #[test]
    fn overflow_joins_drop_no_agent() {
        let board = fixture();
        let list = vec![
            agent(1, "orchestrator", "orchestrator"),
            agent(2, "builder", "worker"),
            agent(3, "qa", "worker"),
            agent(4, "reviewer", "worker"),
            agent(5, "librarian", "worker"),
            agent(6, "planner", "worker"),
            agent(7, "finance", "worker"),
            agent(8, "mayor", "worker"),
        ];
        // Five rooms fit (cpu 2, gpu 2, memory 1); the roles past them join a built room.
        let layout = layout_on_board(&list, &board, &BoardParams::default());
        assert!(layout.districts.len() <= 5, "at most five rooms: {}", layout.districts.len());
        assert_eq!(layout.homes.len(), 8, "every agent has a home");
        assert!(layout.homes.values().all(|&(room, _, _)| room < layout.districts.len()));
        assert!(layout.districts.iter().all(|d| d.w > 0.0 && d.d > 0.0));
    }

    #[test]
    fn board_height_inside_ramp_and_outside() {
        let board = fixture();
        let layout = layout_on_board(&fixture_roles(), &board, &BoardParams::default());
        // Inside a cell: the room's height (builder's cell is x 6..10 at top 2).
        assert!(close(board_height_at(9.0, 0.0, &layout), 2.0));
        assert!(close(board_height_at(0.0, -10.0, &layout), 3.0));
        // The ramp: half a unit outside the qa cell (x 10..14, top 2) the height is 2 - 1.6 * 0.5.
        assert!(close(board_height_at(14.5, 0.0, &layout), 1.2));
        // The orchestrator's cell spans x -14..-10 at top 1: half a unit out is 1 - 0.6 * 0.5.
        assert!(close(board_height_at(-14.5, 0.0, &layout), 0.7));
        // Beyond a unit from every room: the PCB top.
        assert!(close(board_height_at(-30.0, 0.0, &layout), 0.4));
    }

    #[test]
    fn trace_graph_and_walkway_on_an_l_shape() {
        let board = fixture();
        let g = trace_graph(&board);
        assert_eq!(g.nodes, vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        assert_eq!(g.edges, vec![(0, 1), (1, 2)]);
        // From (2, 1) joins the corner (0, 0); to (9, 10) joins the corner (10, 10); the walk turns once.
        let path = path_on_traces(&g, [2.0, 1.0], [9.0, 10.0]);
        assert_eq!(path, vec![[2.0, 1.0], [0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [9.0, 10.0]]);
        // Without a walkway between the ends, the straight line.
        let apart = TraceGraph { nodes: vec![[0.0, 0.0], [5.0, 5.0]], edges: vec![] };
        assert_eq!(path_on_traces(&apart, [0.0, 0.0], [5.0, 5.0]), vec![[0.0, 0.0], [5.0, 5.0]]);
    }

    /// docs/agents/initial-roles.json, parsed: `{ "source": ..., "roles": { role: ... } }`.
    fn initial_roles_json() -> serde_json::Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/agents/initial-roles.json");
        let text = std::fs::read_to_string(path).expect("docs/agents/initial-roles.json reads");
        serde_json::from_str(&text).expect("initial-roles.json parses")
    }

    /// The roles of docs/agents/initial-roles.json, one agent each, as the fleet has them.
    fn initial_roles() -> Vec<AgentInfo> {
        let v = initial_roles_json();
        let roles = v["roles"].as_object().expect("a roles object");
        roles
            .keys()
            .enumerate()
            .map(|(i, role)| agent(i as u64 + 1, role, if role == "orchestrator" { "orchestrator" } else { "worker" }))
            .collect()
    }

    #[test]
    fn every_board_model_places_all_initial_roles() {
        let list = initial_roles();
        let file_roles = initial_roles_json()["roles"].as_object().expect("a roles object").len();
        assert_eq!(list.len(), file_roles, "one agent per role in initial-roles.json");
        for &(name, _) in MODELS.iter().filter(|(n, _)| n.starts_with("board/")) {
            let board = hardware::model(name).expect("the embedded board parses");
            let layout = layout_on_board(&list, board, &BoardParams::default());
            // A role with no free cell joins a built room, so not every role has a room of its own; every room is a
            // role of the roster and every agent has a home in one of the rooms.
            let want: Vec<&str> = list.iter().map(|a| a.role.as_str()).collect();
            for bd in &layout.districts {
                assert!(want.contains(&bd.district.role.as_str()), "{name}: room {} is a role of the roster", bd.district.role);
            }
            assert_eq!(layout.homes.len(), file_roles, "{name}: every agent has a home");
            assert!(layout.homes.values().all(|&(room, _, _)| room < layout.districts.len()), "{name}: every home is in a room");
            println!("== {name} (pcb_top {}, atom {:?})", layout.pcb_top, layout.atom);
            for bd in &layout.districts {
                let family = if bd.district.role == "orchestrator" { "command" } else { role_family(&bd.district.role, "worker") };
                println!(
                    "  {:<18} {:<10} slot={:<10} at=({:7.2}, {:7.2}) cell={:5.2}x{:<5.2} y={:.2}",
                    bd.district.role, family, bd.slot, bd.district.cx, bd.district.cy, bd.w, bd.d, bd.y
                );
            }
        }
    }

    /// The web's home step: 0.8 of the cell's short side over the side - 1 gaps of a row, at most 3; 0 for one column.
    #[test]
    fn home_step_is_the_webs_formula() {
        for (cell, side, want) in [(6.0f32, 1usize, 0.0f32), (6.0, 2, 3.0), (3.0, 2, 2.4), (4.0, 3, 1.6), (5.0, 4, 4.0 / 3.0), (2.0, 3, 0.8)] {
            let got = home_step(cell, side);
            assert!((got - want).abs() < 1e-6, "cell {cell}, side {side}: got {got}, want {want}");
        }
    }

    /// Two agents with no role get two rooms of their own (keys unnamed:<cid>), both the `other` family.
    #[test]
    fn unnamed_agents_each_get_a_district_of_the_other_family() {
        let list = vec![agent(1, "orchestrator", "orchestrator"), agent(7, "", "worker"), agent(9, "", "worker")];
        let layout = layout_on_board_with(&list, &fixture(), &BoardParams::default(), no_minimums);
        assert_eq!(layout.districts.len(), 3, "the orchestrator and one room per unnamed agent");
        for key in ["unnamed:7", "unnamed:9"] {
            let d = layout.districts.iter().find(|d| d.key == key).unwrap_or_else(|| panic!("a district keyed {key}"));
            assert_eq!(d.district.role, "other");
            assert_eq!(role_family(&d.district.role, "worker"), "other");
        }
        assert_ne!(layout.homes[&7].0, layout.homes[&9].0, "each unnamed agent has its own room");
    }

    /// The web's measured case (floor3d/board-layout.ts OCCLUSION_SHARE note): on the workstation the m2_2 slot is 4 of 9
    /// samples blocked and hidden, the RAM slot 3 of 9 and usable.
    #[test]
    fn occlusion_hides_the_m2_slot_the_camera_misses() {
        let board = hardware::model("board/workstation").expect("the workstation parses");
        let boxes = hardware::part_boxes(board, &Params::default());
        let hidden = occluded_slot_ids(board, &boxes);
        assert!(hidden.contains("slot_m2_2"), "m2_2 is hidden: {hidden:?}");
        assert!(!hidden.contains("slot_mem"), "the RAM slot stays usable: {hidden:?}");
    }

    /// The PCB top is the board's `pcb_top` param (the workstation's 0.336), then the caller's override, then the default.
    #[test]
    fn pcb_top_comes_from_the_board_params() {
        let workstation = hardware::model("board/workstation").expect("the workstation parses");
        assert_eq!(layout_on_board(&[], workstation, &BoardParams::default()).pcb_top, 0.336);
        let laptop = hardware::model("board/laptop").expect("the laptop parses");
        assert_eq!(layout_on_board(&[], laptop, &BoardParams::default()).pcb_top, PCB_TOP);
        let over = BoardParams { params: Params::default().with("pcb_top", 0.5) };
        assert_eq!(layout_on_board(&[], workstation, &over).pcb_top, 0.5);
    }

    /// The shared board core seats every agent of every board kind and count the apz-board tests use, the same way
    /// twice (its layout is deterministic).
    #[test]
    #[cfg_attr(debug_assertions, ignore = "the shared layouts take seconds each in release, minutes in debug")]
    fn shared_layout_seats_every_agent_on_every_board_kind() {
        const KINDS: [&str; 6] = ["edge", "laptop", "phone", "apple-soc", "server", "workstation"];
        const COUNTS: [usize; 4] = [1, 4, 8, 20];
        let roles = ["coder", "writer", "analyst"];
        let agent = |cid: u64, role: &str| AgentInfo {
            cid,
            name: format!("agent {cid}"),
            role: role.into(),
            status: "idle".into(),
            tier: if role == "orchestrator" { "orchestrator" } else { "worker" }.into(),
            model: String::new(),
            species: None,
        };
        for kind in KINDS {
            for n in COUNTS {
                let list: Vec<AgentInfo> = (1..=n as u64)
                    .map(|cid| agent(cid, if cid == 1 { "orchestrator" } else { roles[cid as usize % roles.len()] }))
                    .collect();
                // gpus 0, as the apz-board suite uses: a workstation with a GPU fitted does not seat one agent
                // within the scale cap yet (see `shared_layout_seats_with_gpus_fitted`).
                let key = SharedKey::new(kind, &list, 0);
                let shared = shared_layout_of(&key).unwrap_or_else(|e| panic!("{kind} with {n} agents: {e}"));
                let mut seated: Vec<u64> = shared.homes.iter().map(|h| h.agent.parse().expect("a cid")).collect();
                seated.sort_unstable();
                assert_eq!(seated, (1..=n as u64).collect::<Vec<_>>(), "{kind} with {n} agents: each seated once");
                let again = shared_layout_of(&key).expect("the same roster lays out again");
                assert_eq!(shared, again, "{kind} with {n} agents: deterministic");
            }
        }
    }

    /// The first request for a key starts its layout and returns None; once it is done, polling moves it into the
    /// cache and requests return it, without computing it again.

    /// The same with GPUs fitted (real workstations and servers have them). apz-board cannot seat a workstation with
    /// one GPU and one agent within the scale cap 3 today; the engine falls back to its own layout meanwhile.
    #[test]
    #[ignore = "apz-board: workstation with gpus=1 and one agent does not seat within scale cap 3"]
    fn shared_layout_seats_with_gpus_fitted() {
        let list = vec![AgentInfo {
            cid: 1,
            name: "agent 1".into(),
            role: "orchestrator".into(),
            status: "idle".into(),
            tier: "orchestrator".into(),
            model: String::new(),
            species: None,
        }];
        for kind in ["workstation", "server"] {
            for gpus in [1, 2, 4] {
                let key = SharedKey::new(kind, &list, gpus);
                shared_layout_of(&key).unwrap_or_else(|e| panic!("{kind} with {gpus} GPUs: {e}"));
            }
        }
    }
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_shared_layout_is_computed_once_and_ready_after_polling() {
        let seat = |cid: u64, role: &str| AgentInfo {
            cid,
            name: format!("agent {cid}"),
            role: role.into(),
            status: "idle".into(),
            tier: if role == "orchestrator" { "orchestrator" } else { "worker" }.into(),
            model: String::new(),
            species: None,
        };
        let roster = [seat(1, "orchestrator"), seat(2, "coder")];
        let key = SharedKey::new("edge", &roster, 1);
        let mut cache = SharedLayouts::default();
        assert!(cache.request(key.clone()).is_none(), "the first request starts the layout");
        assert!(cache.request(key.clone()).is_none(), "a request while it computes does not start it again");
        assert_eq!(cache.spawned, 1, "one computation for the key");
        let started = std::time::Instant::now();
        let layout = loop {
            cache.poll();
            if let Some(layout) = cache.request(key.clone()) {
                break layout;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(120), "the edge layout is ready");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(layout.homes.len(), 2, "both agents are seated");
        assert!(cache.request(key).is_some(), "a finished key is returned at once");
        assert_eq!(cache.spawned, 1, "a finished key is not computed again");
    }
}
