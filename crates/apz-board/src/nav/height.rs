//! Gravity and heights (nav-height.ts): a robot's vertical body, its feet height, its ballistic anchor and its mode.

/// The body's mode (nav-height.ts BodyMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyMode {
    Ground,
    Crouch,
    Jump,
    Fall,
    Glide,
    Land,
    Lift,
    Out,
    In,
}

/// A vertical body (nav-height.ts Body): the mode, the feet height, the air anchor (y0, v0) and the seconds in the mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub mode: BodyMode,
    pub y: f64,
    pub y0: f64,
    pub v0: f64,
    pub t: f64,
}

/// What is under a body's centre (nav-height.ts Surfaces): the top of a walk cell, and the top of any on-board cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surfaces {
    pub walk_top: Option<f64>,
    pub floor_top: Option<f64>,
}
