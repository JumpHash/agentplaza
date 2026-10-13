//! The mouths of the exits: what each plate's exit adds to the walk (board-layout.ts landingOf, gangwayOf and the lift). A
//! plate's exit leaves by one of three mouths, all axis-aligned, so the rasteriser sees rectangles:
//!
//! - a ramp: a sloped Ramp surface from the plate's edge down to the board, sloped so no cell steps above NAV_STEP;
//! - a lift: the cab on the board (its footprint is the board cell the exit reached), the bottom landing on the board
//!   beyond the cab (the queue, which the TS requires to be walk), and the top landing on the plate's edge cell. One Lift
//!   link joins the bottom and the top through the cab;
//! - a gangway: the same cab and queue, with the top a Bridge at plate height from the plate's edge to the cab.
//!
//! The cab's floor is the height of each landing it stops at (`Lift::bottom_y` and `top_y`), so it is flush with both.

use serde::{Deserialize, Serialize};

use super::aisle::{Exit, ExitKind};
use super::geom::Rect;
use super::Door;
use crate::grid::{NAV_CELL, NAV_STEP};
use crate::walk::{Plane, SurfaceKind};

/// The mouth's cell size: a landing, cab or ramp is whole grid cells.
pub const CELL: f64 = NAV_CELL;
/// Half a grid cell: the distance from a plate edge to its first outward sample (board-layout.ts EDGE).
pub const EDGE: f64 = NAV_CELL / 2.0;
/// Half the width of a mouth piece across its exit: 1.5 cells, so each piece holds whole cells whatever the grid's offset.
const HALF_WIDTH: f64 = 1.5 * NAV_CELL;

/// A plate's exit mouth. `edge` is the plate's edge point at the exit (the join with the board's side).
#[derive(Debug, Clone, PartialEq)]
pub enum Mouth {
    /// A ramp: its footprint, its plane (height over the footprint), the plate's edge point, the point at its far end, and
    /// the points the two links join at (a point inside the plate, and a point on the board beyond the far end).
    Ramp { rect: Rect, plane: Plane, edge: [f64; 2], far: [f64; 2], inside: [f64; 2], beyond: [f64; 2] },
    /// A lift (or a gangway): the cab's footprint, the bottom landing's, the top's (a landing on the plate, or a gangway's
    /// bridge), the top's kind, the cab's door side (the side facing the top), and the heights of the two stops.
    Lift { cab: Rect, bottom: Rect, top: Rect, top_kind: SurfaceKind, edge: [f64; 2], door: Door, gangway: bool, bottom_y: f64, top_y: f64 },
}

/// The mouth of an exit of a plate at height `plate_y` above a PCB at `pcb_top`. None when the exit cannot be laid out (a
/// gangway with no bridge length).
pub fn mouth(exit: &Exit, plate_y: f64, pcb_top: f64) -> Option<Mouth> {
    let dir = exit.dir;
    let edge = [exit.plate_at[0] + dir[0] * EDGE, exit.plate_at[1] + dir[1] * EDGE];
    let perp = [-dir[1], dir[0]];
    let centre = exit.board_at;
    match exit.kind {
        ExitKind::Ramp => {
            let rise = plate_y - pcb_top;
            if rise <= 0.0 {
                return None;
            }
            // Each cell steps at most NAV_STEP: the run is whole cells, ceil(rise / NAV_STEP) of them.
            let run = (rise / NAV_STEP).ceil().max(1.0) * CELL;
            let far = [edge[0] + dir[0] * run, edge[1] + dir[1] * run];
            let rect = piece(edge, dir, perp, 0.0, run);
            let k = rise / run;
            let plane = Plane { c: plate_y + k * dot(edge, dir), dx: -k * dir[0], dz: -k * dir[1] };
            let beyond = [far[0] + dir[0] * EDGE, far[1] + dir[1] * EDGE];
            Some(Mouth::Ramp { rect, plane, edge, far, inside: exit.plate_at, beyond })
        }
        ExitKind::Lift => {
            let top = piece(edge, dir, perp, -2.0 * CELL, 0.0);
            Some(lift(dir, piece(centre, dir, perp, -CELL, CELL), piece(centre, dir, perp, CELL, 3.0 * CELL), top, SurfaceKind::Landing, false, edge, pcb_top, plate_y))
        }
        ExitKind::Gangway => {
            // The bridge runs from the plate's edge to the cab's near side.
            let cab_near = offset(centre, dir, -CELL);
            if dot([cab_near[0] - edge[0], cab_near[1] - edge[1]], dir) <= 1e-6 {
                return None;
            }
            let bridge = bounds(&strip(edge, cab_near, perp, HALF_WIDTH));
            Some(lift(dir, piece(centre, dir, perp, -CELL, CELL), piece(centre, dir, perp, CELL, 3.0 * CELL), bridge, SurfaceKind::Bridge, true, edge, pcb_top, plate_y))
        }
    }
}

/// The lift's pieces (shared by a lift and a gangway): the cab, its queue (the bottom landing), the top, and the door.
#[allow(clippy::too_many_arguments)]
fn lift(dir: [f64; 2], cab: Rect, bottom: Rect, top: Rect, top_kind: SurfaceKind, gangway: bool, edge: [f64; 2], pcb_top: f64, plate_y: f64) -> Mouth {
    Mouth::Lift { cab, bottom, top, top_kind, edge, door: door_of([-dir[0], -dir[1]]), gangway, bottom_y: pcb_top, top_y: plate_y }
}

/// The axis-aligned box of the strip from `a` to `b` along a direction, `half` wide to either side of the line.
fn strip(a: [f64; 2], b: [f64; 2], perp: [f64; 2], half: f64) -> [[f64; 2]; 4] {
    [offset(a, perp, half), offset(a, perp, -half), offset(b, perp, half), offset(b, perp, -half)]
}

/// The mouth piece from `c` out along `dir` from `t0` to `t1` (distances), HALF_WIDTH wide across.
fn piece(c: [f64; 2], dir: [f64; 2], perp: [f64; 2], t0: f64, t1: f64) -> Rect {
    bounds(&strip(offset(c, dir, t0), offset(c, dir, t1), perp, HALF_WIDTH))
}

/// The bounding rect of a set of points.
fn bounds(pts: &[[f64; 2]]) -> Rect {
    let mut r = Rect { x0: f64::INFINITY, z0: f64::INFINITY, x1: f64::NEG_INFINITY, z1: f64::NEG_INFINITY };
    for p in pts {
        r.x0 = r.x0.min(p[0]);
        r.z0 = r.z0.min(p[1]);
        r.x1 = r.x1.max(p[0]);
        r.z1 = r.z1.max(p[1]);
    }
    r
}

/// A square of side `w` centred on `c`.
fn square(c: [f64; 2], w: f64) -> Rect {
    Rect { x0: c[0] - w / 2.0, z0: c[1] - w / 2.0, x1: c[0] + w / 2.0, z1: c[1] + w / 2.0 }
}

/// The point `p` moved `t` along `dir`.
fn offset(p: [f64; 2], dir: [f64; 2], t: f64) -> [f64; 2] {
    [p[0] + dir[0] * t, p[1] + dir[1] * t]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// The room door side a world direction faces (the room's N is -z, S is +z, E is +x, W is -x).
fn door_of(v: [f64; 2]) -> Door {
    if v[0].abs() > v[1].abs() {
        if v[0] > 0.0 {
            Door::E
        } else {
            Door::W
        }
    } else if v[1] > 0.0 {
        Door::S
    } else {
        Door::N
    }
}

/// The footprint of a mouth's pieces that the acceptance checks (the cab and queue, and the top or the ramp).
pub fn footprint(m: &Mouth) -> Vec<Rect> {
    match m {
        Mouth::Ramp { rect, .. } => vec![*rect],
        Mouth::Lift { cab, bottom, top, .. } => vec![*cab, *bottom, *top],
    }
}

/// The serialisable record of a lift: the surface ids of its pieces and its stops, emitted as `Layout::lifts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MouthIds {
    /// The plate the mouth serves.
    pub plate: usize,
    /// The ramp's surface, for a ramp.
    pub ramp: Option<u32>,
    /// The cab, the bottom landing and the top (landing or bridge), for a lift or a gangway.
    pub cab: Option<u32>,
    pub bottom: Option<u32>,
    pub top: Option<u32>,
}
