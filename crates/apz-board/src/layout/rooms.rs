//! Room sizing and the aisle's room rows (board-layout.ts: FAMILY_TYPES, roleHue, minimumSide, deskCapOf, innerOf,
//! cellNeedOf, typicalRowDepth, configs of planAisle, spansAt, holds, roomCells, turnAndDoor). The rows carry the
//! spur gap and the end cut of the exit (the second pass).

use std::collections::BTreeMap;

use super::Door;
use super::geom::Frame;

/// The room minimums (assets/rooms/minsize.json): role -> the smallest square side for 1..6 agents; null = none fits.
pub type MinTable = BTreeMap<String, Vec<Option<f64>>>;

/// The minimums the layout reads (the same file the web reads).
pub fn min_table() -> MinTable {
    serde_json::from_str(include_str!("../../../../assets/rooms/minsize.json"))
        .expect("apz-board: minsize.json is valid")
}

/// The orchestrator tier's role name; the input has no tier, so this role is the orchestrator's seat.
pub const ORCHESTRATOR: &str = "orchestrator";
/// The room's share of its cell's width (the room's radius and inner width).
pub const ROOM_SHARE: f64 = 0.9;
/// The wall margin: a room's inner width is its cell's share less this.
pub const ROOM_MARGIN: f64 = 0.15;
/// The most desks a room seats: the table's entries run for 1 to 6 agents.
pub const DESKS_MAX: usize = 6;
/// The orchestrator's room hue (the Floor's dais).
pub const DAIS_HUE: f64 = 25.0;
/// The aisle's width: the lane the rooms open onto.
pub const AISLE: f64 = 1.0;
/// The Floor's curated district hues (floor.ts roleHue).
const HUES: [f64; 10] = [
    8.0, 28.0, 46.0, 88.0, 140.0, 172.0, 198.0, 222.0, 262.0, 340.0,
];

/// A role's family (roles.ts). Orchestrator is `Command`; a role the map does not list is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Command,
    Build,
    Operations,
    Knowledge,
    Money,
    Other,
}

/// The family of a role (roles.ts roleFamily without a tier: the orchestrator role is `Command`).
pub fn family_of(role: &str) -> Family {
    match role {
        "orchestrator" | "mayor" | "mayor-office" => Family::Command,
        "architect" | "builder" | "qa" | "reviewer" | "release" => Family::Build,
        "edge-operator" | "incident" | "security-reviewer" => Family::Operations,
        "librarian" | "researcher" | "planner" => Family::Knowledge,
        "finance" | "auditor" => Family::Money,
        _ => Family::Other,
    }
}

/// The slot types each family prefers, in order (FAMILY_TYPES).
pub fn family_types(f: Family) -> &'static [&'static str] {
    match f {
        Family::Command => &["cpu", "soc"],
        Family::Build => &["gpu", "npu", "cpu", "soc"],
        Family::Knowledge => &["memory", "storage"],
        Family::Operations => &["network", "io", "bmc", "modem"],
        Family::Money => &["power", "io"],
        Family::Other => &["storage"],
    }
}

/// The hue of a role's room: a hash of the role's UTF-16 units over the curated hues (floor.ts roleHue).
pub fn role_hue(role: &str) -> f64 {
    let mut hash: u32 = 0;
    for u in role.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(u as u32);
    }
    HUES[hash as usize % HUES.len()]
}

/// The smallest square side a room of `role` needs with `agents` agents (the table's entry at min(agents, len) - 1).
/// `Some(0.0)` for a role the table does not list; `None` when no size fits.
pub fn minimum_side(role: &str, agents: usize, table: &MinTable) -> Option<f64> {
    match table.get(role) {
        None => Some(0.0),
        Some(row) if row.is_empty() => Some(0.0),
        Some(row) => row[agents.max(1).min(row.len()) - 1],
    }
}

/// The inner width of a room from its cell's width (innerOf).
pub fn inner_of(cell_w: f64) -> f64 {
    ROOM_SHARE * cell_w - ROOM_MARGIN
}

/// The cell width a room needs for an inner width of at least `m` (cellNeedOf); 0 when there is no minimum.
pub fn cell_need_of(m: f64) -> f64 {
    if m > 0.0 {
        (m + ROOM_MARGIN) / ROOM_SHARE
    } else {
        0.0
    }
}

/// The most desks a room of `role` seats on a cell of `w` x `d` (deskCapOf).
pub fn desk_cap(role: &str, w: f64, d: f64, table: &MinTable) -> usize {
    let inner = inner_of(w.min(d));
    let cap = table
        .get(role)
        .map_or(DESKS_MAX, |row| DESKS_MAX.min(row.len()));
    let mut n = 0;
    for k in 1..=cap {
        match minimum_side(role, k, table) {
            Some(m) if m <= inner + 1e-9 => n = k,
            _ => break,
        }
    }
    n
}

/// The room depth a double aisle's rows must reach: the median of the present roles' minimums at one agent, and the
/// orchestrator's when it is bigger (typicalRowDepth). `agents` is (role, is_orchestrator) per agent.
pub fn typical_row_depth(agents: &[(&str, bool)], table: &MinTable) -> f64 {
    let mut sides: Vec<f64> = agents
        .iter()
        .filter_map(|(role, _)| {
            minimum_side(if role.is_empty() { "other" } else { role }, 1, table)
        })
        .filter(|v| *v > 0.0)
        .collect();
    if sides.is_empty() {
        return 0.0;
    }
    sides.sort_by(|a, b| a.total_cmp(b));
    let mut m = sides[(sides.len() - 1) / 2];
    let orchestrator = agents.iter().any(|(_, orch)| *orch);
    if let Some(o) = minimum_side(ORCHESTRATOR, 1, table).filter(|o| orchestrator && *o > 0.0) {
        m = m.max(o);
    }
    cell_need_of(m)
}

/// A room cell on an aisle: its centre, its size and the world direction from the room to its aisle (the door faces it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub cx: f64,
    pub cz: f64,
    pub w: f64,
    pub d: f64,
    pub face: [f64; 2],
}

/// A room row: its span across the aisle [v0, v1] in frame coordinates, the frame direction to its aisle, and the spur gap
/// cut through it (`[u0, u1]` along the plate, when the aisle leaves by a spur through this row).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub v0: f64,
    pub v1: f64,
    pub face: [f64; 2],
    pub gap: Option<[f64; 2]>,
}

/// An aisle configuration: double (aisle down the middle, rows both sides) or single; its aisle span across the plate.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub double: bool,
    pub aisle: [f64; 2],
    pub rows: Vec<Row>,
}

/// The configurations of a plate, in planAisle's order: the double first (when both rows reach `row_min`), then the
/// single along the +v side, then the single along the -v side. The single order by openShare needs the board-only grid
/// (SECOND PASS); with no grid the two keep this order, which is the TS order on a tie.
pub fn configs(f: &Frame, row_min: f64) -> Vec<Config> {
    let s = f.s;
    let half = (s - AISLE) / 2.0;
    let mut out = Vec::new();
    if half > 0.0 && half >= row_min - 1e-9 {
        out.push(Config {
            double: true,
            aisle: [half, half + AISLE],
            rows: vec![
                Row {
                    v0: 0.0,
                    v1: half,
                    face: [0.0, 1.0],
                    gap: None,
                },
                Row {
                    v0: half + AISLE,
                    v1: s,
                    face: [0.0, -1.0],
                    gap: None,
                },
            ],
        });
    }
    out.push(Config {
        double: false,
        aisle: [s - AISLE, s],
        rows: vec![Row {
            v0: 0.0,
            v1: s - AISLE,
            face: [0.0, 1.0],
            gap: None,
        }],
    });
    out.push(Config {
        double: false,
        aisle: [0.0, AISLE],
        rows: vec![Row {
            v0: AISLE,
            v1: s,
            face: [0.0, -1.0],
            gap: None,
        }],
    });
    out
}

/// An aisle on a plate: its frame, its rows, whether it is double, the slot's capacity (its room count bound), and the
/// side its exit leaves by along u: -1 the -u end, +1 the +u end, 0 neither (a spur, or a gangway along the long side).
#[derive(Debug, Clone)]
pub struct Aisle {
    /// The index of the plate's candidate slot.
    pub cand: usize,
    pub f: Frame,
    pub rows: Vec<Row>,
    pub double: bool,
    pub cap: usize,
    pub end_side: f64,
}

/// The room depth kept off a spur gap and off the aisle end the exit leaves by (board-layout.ts NAV_RAMP_DEPTH).
pub const RAMP_DEPTH: f64 = 1.0;

/// The room spans of one row along u: the row's length less its spur gap, and less RAMP_DEPTH at a leaving end
/// (board-layout.ts rowSpans).
fn row_spans(row: &Row, a: &Aisle) -> Vec<(f64, f64)> {
    let l = a.f.l;
    let lo = if a.end_side < -0.5 { RAMP_DEPTH } else { 0.0 };
    let hi = if a.end_side > 0.5 { l - RAMP_DEPTH } else { l };
    let cut = match row.gap {
        Some(g) => vec![(0.0, g[0] - RAMP_DEPTH), (g[1] + RAMP_DEPTH, l)],
        None => vec![(0.0, l)],
    };
    cut.into_iter()
        .map(|(u0, u1)| (u0.max(lo), u1.min(hi)))
        .filter(|(u0, u1)| u1 - u0 > 1e-9)
        .collect()
}

/// A run of room cells in one row: its frame span [u0, u1] and how many cells of the needed side fit in it.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub row: usize,
    pub u0: f64,
    pub u1: f64,
    pub k: usize,
}

/// The spans an aisle offers at a minimum side `need` (spansAt): per row and per span of it, how many cells of `need` fit.
pub fn spans_at(a: &Aisle, need: f64) -> Vec<Span> {
    let mut out = Vec::new();
    for (row, r) in a.rows.iter().enumerate() {
        if r.v1 - r.v0 < need - 1e-9 {
            continue;
        }
        for (u0, u1) in row_spans(r, a) {
            let k = if need <= 0.0 {
                a.cap
            } else {
                ((u1 - u0) / need + 1e-9).floor() as usize
            };
            if k > 0 {
                out.push(Span { row, u0, u1, k });
            }
        }
    }
    out
}

/// The rooms an aisle holds at `need`: its capacity, lowered as its spans allow (holds).
pub fn holds(a: &Aisle, need: f64) -> usize {
    if a.cap < 1 {
        return 0;
    }
    a.cap.min(spans_at(a, need).iter().map(|g| g.k).sum())
}

/// The cells of an aisle at `need`: `holds` of them, each span split into equal cells across its row's depth (roomCells).
pub fn room_cells(a: &Aisle, need: f64) -> Vec<Cell> {
    let mut left = holds(a, need);
    let mut cells = Vec::new();
    for g in spans_at(a, need) {
        let n = g.k.min(left);
        if n == 0 {
            continue;
        }
        left -= n;
        let len = (g.u1 - g.u0) / n as f64;
        let row = &a.rows[g.row];
        for j in 0..n {
            let r = a.f.rect(
                g.u0 + j as f64 * len,
                g.u0 + (j + 1) as f64 * len,
                row.v0,
                row.v1,
            );
            cells.push(Cell {
                cx: r[0] + r[2] / 2.0,
                cz: r[1] + r[3] / 2.0,
                w: r[2],
                d: r[3],
                face: a.f.vec(row.face[0], row.face[1]),
            });
        }
    }
    cells
}

/// The turn about y and the door side of a room on its cell (turnAndDoor). A square room turns so its local +z side
/// faces its aisle; a round room turns to the board's centre and its door is the side nearest its aisle.
pub fn turn_and_door(cell: &Cell, round: bool) -> (f64, Door) {
    let [ux, uz] = cell.face;
    let turn = if round {
        (-cell.cx).atan2(-cell.cz)
    } else {
        ux.atan2(uz)
    };
    let (c, s) = (turn.cos(), turn.sin());
    let lx = ux * c - uz * s;
    let lz = ux * s + uz * c;
    let door = if lx.abs() > lz.abs() {
        if lx > 0.0 { Door::E } else { Door::W }
    } else if lz > 0.0 {
        Door::S
    } else {
        Door::N
    };
    (turn, door)
}
