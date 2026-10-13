//! Robot navigation on the nav grid (`crate::navgrid`): the planner (`plan`, nav-plan.ts), the vertical body
//! (`height`, nav-height.ts), the drawn tops the walk heights bake from (`drawn`, drawn-tops.ts), the lift cabs
//! (`lift`, lift-ctrl.ts) and the robot followers (`world`, nav-agents.ts NavWorld). The TS files' rules are the
//! cross-app contract; these ports follow them in order, with each float expression evaluated as written.

pub mod drawn;
pub mod height;
pub mod lift;
pub mod plan;
#[allow(dead_code)]
pub mod world;

/// The robots' footprint half-size (nav-types.ts NAV_RADIUS).
pub const NAV_RADIUS: f64 = 0.3;
/// Lift speed and dwell (nav-types.ts NAV_LIFT_SPEED, NAV_LIFT_DWELL).
pub const NAV_LIFT_SPEED: f64 = 1.2;
pub const NAV_LIFT_DWELL: f64 = 0.5;
/// Cached paths live this long, in simulation seconds (nav-types.ts NAV_PATH_TTL).
pub const NAV_PATH_TTL: f64 = 2.0;
/// The look-ahead along a path for the avoidance, and the separation distance (nav-types.ts).
pub const NAV_LOOKAHEAD: f64 = 1.2;
pub const NAV_SEPARATION: f64 = 0.7;
/// Gravity and heights (nav-types.ts): gravity, a planned jump's flight time, the crouch before it, the landing squash,
/// a floating species' glide speed, and the respawn fade (out, then in).
pub const NAV_GRAVITY: f64 = 20.0;
pub const NAV_JUMP_TIME: f64 = 0.45;
pub const NAV_CROUCH_TIME: f64 = 0.1;
pub const NAV_LAND_TIME: f64 = 0.3;
pub const NAV_GLIDE_SPEED: f64 = 1.5;
pub const NAV_RESPAWN_OUT: f64 = 0.25;
pub const NAV_RESPAWN_IN: f64 = 0.35;

/// The height move that ends at a path point: a jump (a rise above NAV_STEP) or a drop (a fall above NAV_STEP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Jump,
    Drop,
}

/// A point on a planned path (nav-types.ts NavPoint): `portal` is the portal index crossed to reach it, or -1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPoint {
    pub x: f64,
    pub z: f64,
    pub y: f64,
    pub portal: i32,
    pub edge: Option<Edge>,
}

/// A planned path (nav-types.ts NavPath): its points, the planner's cost (before smoothing) and the fixture hash.
#[derive(Debug, Clone, PartialEq)]
pub struct NavPath {
    pub points: Vec<NavPoint>,
    pub cost: f64,
    pub hash: String,
}

/// A goal: a world point, optionally with a facing (world yaw) for the arrival.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavGoal {
    pub x: f64,
    pub z: f64,
    pub face: Option<f64>,
}

/// A robot as the floor knows it (nav-types.ts NavRobotSpec): its id, its start, its gait and its priority (0 idle, 1
/// walking, 2 working with a deadline; higher yields less, ties keep the lower id's way).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavRobotSpec {
    pub id: u32,
    pub x: f64,
    pub z: f64,
    pub speed: f64,
    pub turn_rate: f64,
    pub priority: u32,
    pub floats: bool,
}

/// A robot's pose after a step (nav-types.ts NavPose): `y` is the body's feet height, `scale` 1 except in the respawn
/// fade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPose {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub heading: f64,
    pub moving: bool,
    pub waiting: bool,
    pub on_lift: bool,
    pub body: height::BodyMode,
    pub scale: f64,
    pub yielding: bool,
}
