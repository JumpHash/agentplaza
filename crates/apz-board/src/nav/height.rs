//! Gravity and heights (nav-height.ts): a robot's vertical body, its feet height, its ballistic anchor and its mode.
//!
//! Air modes (crouch is not one) use the anchor (y0, v0, t): y = y0 + v0 t - g t^2 / 2, v = v0 - g t. A jump lands at
//! t = NAV_JUMP_TIME when its surface is where it was launched; a fall lands on the first walk surface its anchor reaches.
//! A fall longer than NAV_DROP_DOWN (measured from its anchor), or a body with no floor under it, goes to out.

use crate::grid::{NAV_DROP_DOWN, NAV_JUMP_UP, NAV_STEP};
use crate::nav::{
    NAV_CROUCH_TIME, NAV_GLIDE_SPEED, NAV_GRAVITY, NAV_JUMP_TIME, NAV_LAND_TIME, NAV_RESPAWN_IN,
    NAV_RESPAWN_OUT,
};

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

/// Shares the epsilon of the landing test (a height within this of the surface has landed).
const EPS: f64 = 1e-4;
/// The smallest scale of the respawn fade (not 0: the group keeps a finite matrix).
const FADE_MIN: f64 = 0.02;
/// The launch speed of a planned jump: the arc rises by NAV_JUMP_TIME's ballistic shape to the new surface.
const JUMP_LIFT: f64 = 0.5 * NAV_GRAVITY * NAV_JUMP_TIME * NAV_JUMP_TIME;

/// A body standing at height y, on the ground.
pub fn new_body(y: f64) -> Body {
    Body {
        mode: BodyMode::Ground,
        y,
        y0: y,
        v0: 0.0,
        t: 0.0,
    }
}

/// The height of an air body at its anchor's time t.
pub fn air_y(b: &Body) -> f64 {
    b.y0 + b.v0 * b.t - 0.5 * NAV_GRAVITY * b.t * b.t
}

/// The vertical velocity of an air body now.
pub fn air_v(b: &Body) -> f64 {
    b.v0 - NAV_GRAVITY * b.t
}

/// Starts a new mode from the current height: its anchor is here, with vertical velocity v0.
fn enter(b: &mut Body, mode: BodyMode, v0: f64) {
    b.mode = mode;
    b.y0 = b.y;
    b.v0 = v0;
    b.t = 0.0;
}

/// Launches a planned rise from the current height to `target`: v0 such that the arc lands on target at NAV_JUMP_TIME.
fn launch(b: &mut Body, target: f64) {
    let v0 = (target - b.y + JUMP_LIFT) / NAV_JUMP_TIME;
    enter(b, BodyMode::Jump, v0);
}

/// Advances a body by dt seconds (`floats`: the species floats). Non-lift bodies only; NavWorld sets y while riding.
pub fn step_body(b: &mut Body, s: Surfaces, dt: f64, floats: bool) {
    match b.mode {
        BodyMode::Lift => {}
        BodyMode::Ground => {
            b.t += dt;
            let Some(walk) = s.walk_top else {
                let mode = if floats && s.floor_top.is_some() {
                    BodyMode::Glide
                } else {
                    BodyMode::Fall
                };
                return enter(b, mode, 0.0);
            };
            let d = walk - b.y;
            if d > NAV_STEP + EPS {
                // A rise over NAV_JUMP_UP is never planned: it is snapped up (the feet stay on the surface).
                if d > NAV_JUMP_UP + EPS {
                    b.y = walk;
                    return;
                }
                if floats {
                    return launch(b, walk);
                }
                return enter(b, BodyMode::Crouch, 0.0);
            }
            if d < -NAV_STEP - EPS {
                let mode = if floats {
                    BodyMode::Glide
                } else {
                    BodyMode::Fall
                };
                return enter(b, mode, 0.0);
            }
            // Exact contact: a grounded body's feet are on the surface every step (the audit counts any gap).
            b.y = walk;
        }
        BodyMode::Crouch => {
            b.t += dt;
            let Some(walk) = s.walk_top else {
                return enter(b, BodyMode::Fall, 0.0);
            };
            if b.t >= NAV_CROUCH_TIME {
                launch(b, walk);
            }
        }
        BodyMode::Jump | BodyMode::Fall | BodyMode::Out => {
            b.t += dt;
            let y = air_y(b);
            let v = air_v(b);
            b.y = y;
            if b.mode == BodyMode::Out {
                return;
            }
            if s.floor_top.is_none() || b.y0 - y > NAV_DROP_DOWN {
                return enter(b, BodyMode::Out, v);
            }
            if let Some(walk) = s.walk_top {
                if v <= 0.0 && y <= walk + EPS {
                    b.y = walk;
                    return enter(b, BodyMode::Land, 0.0);
                }
            }
        }
        BodyMode::Glide => {
            b.t += dt;
            if let Some(walk) = s.walk_top {
                if walk - b.y >= -NAV_STEP {
                    b.mode = BodyMode::Ground;
                    b.t = 0.0;
                    return;
                }
            }
            let Some(target) = s.walk_top.or(s.floor_top) else {
                return enter(b, BodyMode::Out, 0.0);
            };
            b.y = f64::max(target, b.y - NAV_GLIDE_SPEED * dt);
        }
        BodyMode::Land => {
            b.t += dt;
            let Some(walk) = s.walk_top else {
                return enter(b, BodyMode::Fall, 0.0);
            };
            b.y = walk;
            if b.t >= NAV_LAND_TIME {
                b.mode = BodyMode::Ground;
                b.t = 0.0;
            }
        }
        BodyMode::In => {
            b.t += dt;
            if b.t >= NAV_RESPAWN_IN {
                b.mode = BodyMode::Ground;
                b.t = 0.0;
            }
        }
    }
}

/// The respawn fade's scale now: 1 except in out (it shrinks, ease in) and in (it grows back, ease out with a small overshoot).
pub fn body_scale(b: &Body) -> f64 {
    match b.mode {
        BodyMode::Out => {
            let u = (b.t / NAV_RESPAWN_OUT).min(1.0);
            1.0 - (1.0 - FADE_MIN) * u * u
        }
        BodyMode::In => {
            let u = (b.t / NAV_RESPAWN_IN).min(1.0);
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            let e = 1.0 + c3 * (u - 1.0).powi(3) + c1 * (u - 1.0).powi(2);
            FADE_MIN + (1.0 - FADE_MIN) * e
        }
        _ => 1.0,
    }
}

/// Whether a body is on the ground (its feet are on a walk surface and it is not in the air).
pub fn grounded(b: &Body) -> bool {
    matches!(b.mode, BodyMode::Ground | BodyMode::Land | BodyMode::In)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surf(walk: Option<f64>, floor: Option<f64>) -> Surfaces {
        Surfaces {
            walk_top: walk,
            floor_top: floor,
        }
    }

    #[test]
    fn grounded_body_keeps_its_height() {
        let mut b = new_body(0.0);
        for _ in 0..100 {
            step_body(&mut b, surf(Some(0.0), Some(0.0)), 0.01, false);
        }
        assert_eq!(b.mode, BodyMode::Ground);
        assert_eq!(b.y, 0.0);
    }

    #[test]
    fn fall_lands_within_a_second_then_grounds() {
        let s = surf(Some(0.0), Some(0.0));
        let mut b = new_body(1.5);
        let mut landed_at = None;
        let mut t = 0.0;
        let mut grounded_again = false;
        for _ in 0..300 {
            step_body(&mut b, s, 0.01, false);
            t += 0.01;
            if b.mode == BodyMode::Land && landed_at.is_none() {
                landed_at = Some(t);
            }
            if landed_at.is_some() && b.mode == BodyMode::Ground {
                grounded_again = true;
                break;
            }
        }
        let at = landed_at.expect("the fall lands");
        assert!(at <= 1.0, "landed at {at}");
        assert!(grounded_again, "the landing squash ends in ground");
        assert_eq!(b.y, 0.0);
    }

    #[test]
    fn fall_without_floor_goes_out() {
        let mut b = new_body(1.5);
        step_body(&mut b, surf(None, None), 0.01, false);
        assert_eq!(b.mode, BodyMode::Fall);
        step_body(&mut b, surf(None, None), 0.01, false);
        assert_eq!(b.mode, BodyMode::Out);
    }

    #[test]
    fn body_scale_is_one_on_ground_and_shrinks_out() {
        let b = new_body(0.0);
        assert_eq!(body_scale(&b), 1.0);
        let mut out = new_body(1.0);
        for _ in 0..2 {
            step_body(&mut out, surf(None, None), 0.01, false);
        }
        for _ in 0..10 {
            step_body(&mut out, surf(None, None), 0.01, false);
        }
        assert_eq!(out.mode, BodyMode::Out);
        assert!(body_scale(&out) < 1.0);
    }
}
