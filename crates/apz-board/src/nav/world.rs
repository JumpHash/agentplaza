//! The robot followers (nav-agents.ts NavWorld): goals, paths, lifts and avoidance on the nav grid. The TS header's
//! rules 1-14 and invariants 1-4 are the contract, and this port follows them in order. Within a step the followers
//! and the avoidance only propose a position for a robot; `settle` commits each proposal once, at the end of the step;
//! `watch` then runs the stuck timer. Robots are kept by id in ascending order (the TS's "ascending id").
//!
//! The methods are split by concern: `api` (the public surface, set_grid, add, set_goal, step, pose), `stuck` (goal
//! snapping, the path cache, planning, the stuck timer), `follow` (following a path, the commit and its sweep, heights
//! and the body) and `crowd` (avoidance, separation, lifts, respawn).

mod api;
mod crowd;
mod follow;
mod stuck;

use std::collections::{BTreeMap, HashMap};

use super::drawn::{DrawnTops, Support};
use super::height::Body;
use super::lift::LiftController;
use super::{NavGoal, NavPath, NavPoint};
use crate::navgrid::{NavGrid, NavLift};

pub use api::components_of;

const TAU: f64 = std::f64::consts::PI * 2.0;
/// The footprint's corner offsets (unit signs), scaled by NAV_RADIUS for the support.
const FOOTPRINT: [[f64; 2]; 4] = [[1.0, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]];
/// The audit's tolerance: a grounded body is floating above this, or sinking below it.
const AUDIT_EPS: f64 = 0.02;
/// The hover audit's tolerance: grounded feet this far above the drawn top under them hover.
const HOVER_EPS: f64 = 0.01;
/// A robot walks only within 30 degrees of its target heading.
const WALK_ANGLE: f64 = std::f64::consts::PI / 6.0;
/// Corner slow-down per radian of turn, the arrival easing radius and its minimum factor.
const CORNER_LOSS: f64 = 0.6;
const EASE_RADIUS: f64 = 1.0;
const ARRIVE_MIN: f64 = 0.25;
/// Speed change limits, in multiples of speed per second.
const ACCEL: f64 = 2.0;
const DECEL: f64 = 4.0;
/// Speeds below this are not walking; a finished path ends at this speed.
const WALK_SPEED: f64 = 0.02;
const MAX_DT: f64 = 0.1;
/// Invariant 1: a point within this of both cell centres of a portal edge makes the move a portal transition.
const PORTAL_EDGE: f64 = 0.05;
/// Invariant 2: progress step; replan, retry and give-up delays (s).
const PROGRESS_STEP: f64 = 0.05;
const REPLAN_AFTER: f64 = 1.0;
const RETRY_AFTER: f64 = 0.5;
const GIVE_UP_AFTER: f64 = 3.0;
/// Invariant 2: turning in place is legitimate for up to TURN_WAIT s.
const TURN_WAIT: f64 = 1.2;
/// Invariants 2 and 3: the crowd penalty's radius and weight, and the longest yield (s).
const CROWD_RADIUS: f64 = 1.5;
const CROWD_PENALTY: f64 = 1000.0;
const YIELD_MAX: f64 = 1.5;
/// Invariant 3: after a yield times out, no new yield to the same robot for this long (s).
const NO_YIELD_AFTER: f64 = 1.5;
/// A yield ends once the blocker has been clear this long (s).
const CLEAR_AFTER: f64 = 0.5;
/// Invariant 4: a goal snaps to a walk cell of its component within this many rings, then the whole grid.
const SNAP_COMPONENT_RINGS: i64 = 12;

/// What the world reports to its owner (the TS's onUnreachable and onRespawn handlers): drained by `take_events`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavEvent {
    Unreachable(u32),
    Respawn(u32),
}

/// A lift: its record on the grid and its cab (lift-ctrl.ts). Nothing but the controller moves a cab.
pub(crate) struct LiftSlot {
    pub(crate) def: NavLift,
    pub(crate) ctrl: LiftController,
}

/// The path progress of a followed proposal; applied only when the proposed point itself is taken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Walk {
    pub(crate) next: usize,
    pub(crate) from: NavPoint,
}

/// A proposed position (rule A).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) x: f64,
    pub(crate) z: f64,
    pub(crate) lift: bool,
    pub(crate) walk: Option<Walk>,
}

/// A touched cell entered by a jump or drop the path emits (rule 5 of the sweep): its cell and its move kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EdgeCell {
    pub(crate) cell: usize,
    pub(crate) kind: i32,
}

/// One robot (nav-agents.ts RobotState).
#[derive(Debug, Clone)]
pub(crate) struct RobotState {
    pub(crate) id: u32,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) z: f64,
    pub(crate) heading: f64,
    pub(crate) speed: f64,
    pub(crate) turn_rate: f64,
    pub(crate) priority: u32,
    /// Current walking speed.
    pub(crate) v: f64,
    pub(crate) goal: Option<NavGoal>,
    pub(crate) path: Option<Vec<NavPoint>>,
    /// Index of the next point to reach.
    pub(crate) next: usize,
    /// The segment origin: the start, or the last point reached.
    pub(crate) from: NavPoint,
    pub(crate) final_face: Option<f64>,
    pub(crate) riding: bool,
    /// The lift the robot rides or waits for (-1: none).
    pub(crate) lift_id: i64,
    pub(crate) waiting: bool,
    pub(crate) moving: bool,
    pub(crate) proposal: Option<Proposal>,
    /// Invariant 1: the time of the first failed commit (-1 when the last commit moved the robot).
    pub(crate) blocked_since: f64,
    pub(crate) turning: bool,
    pub(crate) turn_t: f64,
    pub(crate) lift_wait: bool,
    pub(crate) stuck_t: f64,
    pub(crate) replan_at: f64,
    pub(crate) ref_path: Option<Vec<NavPoint>>,
    pub(crate) ref_next: usize,
    pub(crate) ref_dist: f64,
    pub(crate) body: Body,
    pub(crate) home_x: f64,
    pub(crate) home_z: f64,
    pub(crate) floats: bool,
    /// A forced move (shove) this step: the commit does not project it back onto the walk.
    pub(crate) forced: bool,
}

/// Per robot avoidance state (rule 10, invariant 3).
#[derive(Debug, Clone, Default)]
pub(crate) struct AvoidState {
    pub(crate) yielding: bool,
    /// The spot a held robot stands on, or None.
    pub(crate) hold: Option<[f64; 2]>,
    /// The blocker's id (-1: none).
    pub(crate) blocker: i64,
    pub(crate) clear: f64,
    pub(crate) yield_t: f64,
    pub(crate) saved: Option<NavGoal>,
    /// Blocker id -> the simulation time until which this robot does not yield to it.
    pub(crate) no_yield: BTreeMap<u32, f64>,
}

/// The ground audit (Gravity and heights, invariant G5).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NavAudit {
    pub frames: u64,
    pub floating: u64,
    pub sinking: u64,
    pub on_wall: u64,
    pub hover: u64,
    pub hover_undrawn: u64,
    pub fallback: u64,
    pub projections: u64,
}

/// A read-only view of one cell (nav-agents.ts debugCell).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugCell {
    pub cell: usize,
    pub walk: bool,
    pub blocked: bool,
    pub inflated: bool,
    pub region: i32,
    pub component: i32,
    pub y: f64,
}

/// The robots on one nav grid (nav-agents.ts NavWorld).
pub struct NavWorld {
    pub(crate) grid: NavGrid,
    /// Invariant 4: the connected component of each walk cell.
    pub(crate) comp: Vec<i32>,
    pub(crate) avoidance: BTreeMap<u32, AvoidState>,
    pub(crate) robots: BTreeMap<u32, RobotState>,
    pub(crate) lifts: BTreeMap<usize, LiftSlot>,
    /// The path cache, keyed by (start cell, goal cell): the path and the simulation time it was made.
    pub(crate) cache: HashMap<(usize, usize), (NavPath, f64)>,
    pub(crate) sim_time: f64,
    pub(crate) events: Vec<NavEvent>,
    pub(crate) audit: NavAudit,
    /// The drawn supports the walk heights bake from (set_drawn), and the bake of the current grid.
    pub(crate) sups: Option<Vec<Support>>,
    pub(crate) drawn: Option<DrawnTops>,
}
