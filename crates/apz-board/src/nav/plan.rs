//! Path planning on the nav grid (nav-plan.ts): A* over the plannable cells, string pulling, one corner-cutting pass,
//! and the fixture hash. The TS file's RULES 1-11 are the contract.

use super::NavPath;
use crate::navgrid::NavGrid;

/// Rule 2: the plannable cell nearest to (x, z), or None (nav-plan.ts snapWalk).
pub fn snap_walk(grid: &NavGrid, x: f64, z: f64) -> Option<usize> {
    let _ = (grid, x, z);
    todo!("nav-plan.ts snapWalk")
}

/// Rules 3 to 9: the path from `from` to `to` (nav-plan.ts findPath), or None when either end cannot be snapped or no
/// path exists. `penalty` adds per cell to the move cost; `no_lifts` leaves the lift portals out.
pub fn find_path(grid: &NavGrid, from: [f64; 2], to: [f64; 2], penalty: Option<&[f64]>, no_lifts: bool) -> Option<NavPath> {
    let _ = (grid, from, to, penalty, no_lifts);
    todo!("nav-plan.ts findPath")
}

/// Rule 10: the fixture hash of quantised points (FNV-1a, 32-bit, 8 hex digits).
pub fn path_hash(points: &[super::NavPoint]) -> String {
    let _ = points;
    todo!("nav-plan.ts pathHash")
}
