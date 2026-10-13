//! The board layout (`docs/design/hardware-plazas.md`, "Rooms on components"): where each role's
//! room sits on a machine's board model, and the walkways between the rooms. Pure, no rendering.
//! The web mirror is `apps/web/src/views/floor3d/board-layout.ts`: the same algorithm, the same
//! numbers, the same order (the orchestrator first, then the roles by name).
//!
//! Nothing outside this module and its tests calls it yet: the Floor integration consumes it next.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::layout::role_hue;
use super::{AgentInfo, District};
use crate::console::floor3d::hardware::{self, Model, Params};
use crate::console::floor3d::rooms::room_style;
use crate::console::screens::society::role_family;

/// The surface the robots walk on between the rooms (`params.pcb_top`, the default).
const PCB_TOP: f32 = 0.4;
/// The sloped band around a room, in units: from the room's height down to the PCB top.
const RAMP: f32 = 1.0;
/// A room's footprint as a share of its cell.
const ROOM_SHARE: f32 = 0.9;
/// Homes sit this far apart at most, and never closer than 0.5.
const HOME_GAP: f32 = 3.0;
/// The margin kept between the outermost homes and the cell edge.
const HOME_MARGIN: f32 = 0.9;
/// The orchestrator's hue: the Floor's dais.
const ORCHESTRATOR_HUE: f32 = 25.0;

/// The layout's inputs besides the agents: the `when` params (as `hardware::build` takes them)
/// and the PCB top.
#[allow(dead_code)] // the Floor integration consumes it next
#[derive(Debug, Clone)]
pub struct BoardParams {
    pub params: Params,
    pub pcb_top: f32,
}

impl Default for BoardParams {
    fn default() -> Self {
        Self { params: Params::default(), pcb_top: PCB_TOP }
    }
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
}

/// One room: the orchestrator's agents, or a role's agents.
struct Group<'a> {
    role: String,
    orch: bool,
    members: Vec<&'a AgentInfo>,
}

/// The kinds a family prefers, in order (the spec's table). Anything else takes `storage`.
fn prefers(family: &str) -> &'static [&'static str] {
    match family {
        "command" => &["cpu", "soc"],
        "build" => &["gpu", "cpu", "soc"],
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

/// The first slot with free capacity that `want` accepts. The orchestrator takes the one nearest
/// the atom; a role takes the first in the board's order.
fn first_free(live: &[Live], want: impl Fn(&Live) -> bool, orch: bool, atom: [f32; 2]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in live.iter().enumerate() {
        if s.rooms.len() >= s.capacity || !want(s) {
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

/// The slot for a family's room: the preferred kinds in order, then any slot with room, then (when
/// every slot is full) the slot with the most area per room, the new room counted.
fn choose(live: &[Live], family: &str, orch: bool, atom: [f32; 2]) -> usize {
    for kind in prefers(family) {
        if let Some(i) = first_free(live, |s| s.kind == *kind, orch, atom) {
            return i;
        }
    }
    if let Some(i) = first_free(live, |_| true, orch, atom) {
        return i;
    }
    let mut best = 0;
    let mut best_area = f32::NEG_INFINITY;
    for (i, s) in live.iter().enumerate() {
        let area = s.rect[2] * s.rect[3] / (s.rooms.len() + 1) as f32;
        if area > best_area {
            best = i;
            best_area = area;
        }
    }
    best
}

/// Cell `k` of `n` cells tiling `rect`: `(cx, cz, w, d)`. The columns run along the longer side.
fn cell(rect: [f32; 4], n: usize, k: usize) -> (f32, f32, f32, f32) {
    let [x, z, w, d] = rect;
    let along_x = w >= d;
    let (long, short) = if along_x { (w, d) } else { (d, w) };
    let n = n.max(1);
    let k_short = ((n as f32 * short / long).sqrt().floor() as usize).clamp(1, n);
    let k_long = (n + k_short - 1) / k_short;
    let (cell_long, cell_short) = (long / k_long as f32, short / k_short as f32);
    let (col, row) = (k % k_long, k / k_long);
    let along = (col as f32 + 0.5) * cell_long;
    let across = (row as f32 + 0.5) * cell_short;
    if along_x {
        (x + along, z + across, cell_long, cell_short)
    } else {
        (x + across, z + along, cell_short, cell_long)
    }
}

/// The homes of a room's agents: a square grid, 3 apart (less when the cell is small), centred on
/// the room. `members` are sorted by cid.
fn place_homes(homes: &mut HashMap<u64, (usize, f32, f32)>, index: usize, d: &District, members: &[&AgentInfo], cell_min: f32) {
    let side = ((members.len() as f32).sqrt().ceil().max(1.0)) as usize;
    let avail = cell_min / 2.0 - HOME_MARGIN;
    let gap = if side > 1 { (2.0 * avail / (side - 1) as f32).clamp(0.5, HOME_GAP) } else { HOME_GAP };
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

/// `layoutOnBoard(list, board, params)`: one room per role on the board's slots. The orchestrator
/// takes the cpu slot nearest the atom and sits on the atom; the roles follow by name. Slots whose
/// `on` part is not shown don't exist. No role is dropped: when every slot is full, the room
/// shrinks to fit the slot with the most area per room.
#[allow(dead_code)] // the Floor integration consumes it next
pub fn layout_on_board(list: &[AgentInfo], board: &Model, params: &BoardParams) -> BoardLayout {
    let pcb_top = params.pcb_top;
    let bounds = Bounds { cx: 0.0, cz: 0.0, w: board.size[0], d: board.size[2] };
    let atom = board.anchors.atom.unwrap_or([bounds.cx, pcb_top, bounds.cz]);
    let shown = shown_parts(board, &params.params);

    let mut live: Vec<Live> = board
        .slots
        .iter()
        .filter(|s| s.capacity > 0 && shown.contains(&s.on))
        .map(|s| Live {
            id: s.id.clone(),
            kind: s.kind.clone(),
            top: s.top,
            rect: s.rect,
            capacity: s.capacity as usize,
            rooms: Vec::new(),
        })
        .collect();
    if live.is_empty() {
        // A board with no usable slot still holds its roles: the whole footprint is one room's worth.
        live.push(Live {
            id: String::new(),
            kind: "any".into(),
            top: pcb_top,
            rect: [bounds.cx - bounds.w / 2.0, bounds.cz - bounds.d / 2.0, bounds.w, bounds.d],
            capacity: 1,
            rooms: Vec::new(),
        });
    }

    let mut by_role: BTreeMap<String, Vec<&AgentInfo>> = BTreeMap::new();
    let mut orch: Vec<&AgentInfo> = Vec::new();
    for a in list {
        if a.tier == "orchestrator" {
            orch.push(a);
        } else {
            by_role.entry(a.role.clone()).or_default().push(a);
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    if !orch.is_empty() {
        groups.push(Group { role: "orchestrator".into(), orch: true, members: orch });
    }
    for (role, members) in by_role {
        groups.push(Group { role, orch: false, members });
    }
    for g in &mut groups {
        g.members.sort_by_key(|a| a.cid);
    }

    let atom2 = [atom[0], atom[2]];
    for (gi, g) in groups.iter().enumerate() {
        let family = if g.orch { "command" } else { role_family(&g.role, "worker") };
        let si = choose(&live, family, g.orch, atom2);
        live[si].rooms.push(gi);
    }

    // Each room's cell in its slot, in order; the orchestrator takes the cell nearest the atom (the
    // first on ties) and the room that held that cell takes the orchestrator's, as the web does.
    let mut cell_of = vec![(0usize, 0usize); groups.len()];
    for (si, s) in live.iter().enumerate() {
        let mut ks: Vec<usize> = (0..s.rooms.len()).collect();
        if let Some(p) = s.rooms.iter().position(|&gi| groups[gi].orch) {
            let n = s.rooms.len().max(s.capacity);
            let dist = |k: usize| {
                let (cx, cz, _, _) = cell(s.rect, n, k);
                (cx - atom[0]).powi(2) + (cz - atom[2]).powi(2)
            };
            let best = (0..n).fold(0, |b, k| if dist(k) < dist(b) { k } else { b });
            match ks.iter().position(|&k| k == best) {
                Some(q) => ks.swap(p, q),
                None => ks[p] = best,
            }
        }
        for (i, &gi) in s.rooms.iter().enumerate() {
            cell_of[gi] = (si, ks[i]);
        }
    }

    let mut districts = Vec::with_capacity(groups.len());
    let mut homes = HashMap::new();
    for (gi, g) in groups.iter().enumerate() {
        let (si, k) = cell_of[gi];
        let s = &live[si];
        let (cx, cz, w, d) = cell(s.rect, s.rooms.len().max(s.capacity), k);
        let district = District {
            role: g.role.clone(),
            cx,
            cy: cz,
            r: w.min(d) / 2.0 * ROOM_SHARE,
            hue: if g.orch { ORCHESTRATOR_HUE } else { role_hue(&g.role) },
            round: g.orch || room_style(&g.role).round,
        };
        place_homes(&mut homes, districts.len(), &district, &g.members, w.min(d));
        districts.push(BoardDistrict { district, y: s.top, slot: s.id.clone(), w, d });
    }

    BoardLayout { districts, homes, atom, bounds, pcb_top }
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
    use crate::console::floor3d::hardware::MODELS;

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
        AgentInfo { cid, name: role.into(), role: role.into(), status: "idle".into(), tier: tier.into(), model: String::new() }
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

    #[test]
    fn fixture_families_take_their_preferred_slots() {
        let board = fixture();
        let list = fixture_roles();
        let layout = layout_on_board(&list, &board, &BoardParams::default());
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
        let layout = layout_on_board(&list, &board, &params);
        assert!(layout.districts.iter().all(|d| d.slot != "gpu"), "the hidden gpu slot does not exist");
        let slot = |role: &str| district(&layout, role).slot.clone();
        assert_eq!(slot("builder"), "cpu");
        assert_eq!(slot("orchestrator"), "cpu");
        assert_eq!(slot("finance"), "mem");
    }

    #[test]
    fn overflow_drops_no_role() {
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
        // Five rooms fit (cpu 2, gpu 2, memory 1); eight roles are placed anyway.
        let layout = layout_on_board(&list, &board, &BoardParams::default());
        let mut roles: Vec<String> = layout.districts.iter().map(|d| d.district.role.clone()).collect();
        roles.sort();
        assert_eq!(roles, vec!["builder", "finance", "librarian", "mayor", "orchestrator", "planner", "qa", "reviewer"]);
        assert_eq!(layout.homes.len(), 8);
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

    /// The 16 roles of docs/agents/initial-roles.json, as the fleet has them.
    fn initial_roles() -> Vec<AgentInfo> {
        let text = include_str!("../../../../../../docs/agents/initial-roles.json");
        let v: serde_json::Value = serde_json::from_str(text).expect("initial-roles.json parses");
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
        assert_eq!(list.len(), 16);
        for &(name, _) in MODELS.iter().filter(|(n, _)| n.starts_with("board/")) {
            let board = hardware::model(name).expect("the embedded board parses");
            let layout = layout_on_board(&list, board, &BoardParams::default());
            let mut got: Vec<&str> = layout.districts.iter().map(|d| d.district.role.as_str()).collect();
            got.sort();
            let mut want: Vec<&str> = list.iter().map(|a| a.role.as_str()).collect();
            want.sort();
            assert_eq!(got, want, "{name}: every role has a district");
            assert_eq!(layout.homes.len(), 16, "{name}: every agent has a home");
            println!("== {name} (pcb_top {PCB_TOP}, atom {:?})", layout.atom);
            for bd in &layout.districts {
                let family = if bd.district.role == "orchestrator" { "command" } else { role_family(&bd.district.role, "worker") };
                println!(
                    "  {:<18} {:<10} slot={:<10} at=({:7.2}, {:7.2}) cell={:5.2}x{:<5.2} y={:.2}",
                    bd.district.role, family, bd.slot, bd.district.cx, bd.district.cy, bd.w, bd.d, bd.y
                );
            }
        }
    }
}
