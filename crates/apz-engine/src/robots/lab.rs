//! The robot lab (`APZ_ROBOT_LAB`), as the web's `#/robot-lab`: a line-up of species (or one) under the
//! floor's lighting, each turned to the camera. Env, all optional except the first:
//!   APZ_ROBOT_LAB      a species id, or `all` (the whole line-up from index.json)
//!   APZ_ROBOT_STATE    idle | working | walking | offline (default idle)
//!   APZ_ROBOT_CLIP     a shared or own clip id (empty: the state's clip)
//!   APZ_ROBOT_POSE     wave | proud | think | cheer (a still pose, after the clip)
//!   APZ_ROBOT_TOOL     a tool id, held in the grip
//!   APZ_ROBOT_CARRY    an item id, carried
//!   APZ_ROBOT_FX       sparks | zzz | dots | hearts | keys, spawned every 0.7 s


use super::super::robot::{robot_palette, Robot, RobotPose, RobotState};
use super::super::world::layout::AZIMUTH;
use super::super::world::Floor;
use super::files::registry;

/// The distance between two robots of the line-up, along x.
const SPACING: f32 = 1.6;
const FX_EVERY: f32 = 0.7;

/// The robots of a line-up, and their clock.
pub struct Lab {
    robots: Vec<Robot>,
    t: f32,
    fx: Option<String>,
    fx_timer: f32,
}

fn var(name: &str) -> Option<String> {
    crate::env_var(name).filter(|v| !v.is_empty())
}

/// The line-up, when `APZ_ROBOT_LAB` asks for one: its robots join the floor's root, and the camera
/// frames them. None otherwise (the floor is then the plaza).
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let want = var("APZ_ROBOT_LAB")?;
    let ids: Vec<String> = if want == "all" { registry().index.species.clone() } else { vec![want] };
    let palette = robot_palette();
    let state = match var("APZ_ROBOT_STATE").as_deref() {
        Some("working") => RobotState::Working,
        Some("walking") => RobotState::Walking,
        Some("offline") => RobotState::Offline,
        _ => RobotState::Idle,
    };
    let clip = var("APZ_ROBOT_CLIP");
    let pose = var("APZ_ROBOT_POSE").and_then(|p| match p.as_str() {
        "wave" => Some(RobotPose::Wave),
        "proud" => Some(RobotPose::Proud),
        "think" => Some(RobotPose::Think),
        "cheer" => Some(RobotPose::Cheer),
        _ => None,
    });
    let tool = var("APZ_ROBOT_TOOL");
    let carry = var("APZ_ROBOT_CARRY");

    let n = ids.len();
    let mut robots = Vec::with_capacity(n);
    for (i, id) in ids.iter().enumerate() {
        let mut robot = Robot::with_variant(200.0, false, &palette, 200.0, id);
        let x = (i as f32 - (n as f32 - 1.0) / 2.0) * SPACING;
        robot.group.set_position(x, 0.0, 0.0).set_rotation(0.0, AZIMUTH, 0.0);
        robot.set_state(state);
        robot.hold(tool.as_deref());
        robot.carry(carry.as_deref());
        robot.play(clip.as_deref(), None);
        if let Some(p) = pose {
            robot.pose(p);
        }
        floor.root.add(&robot.group);
        robots.push(robot);
    }
    // The whole row in view, from the floor's own camera angle.
    let half = n as f32 * SPACING * 0.5 + 0.6;
    floor.fit_sphere([0.0, 0.7, 0.0], half, 1.5);
    Some(Lab { robots, t: 0.0, fx: var("APZ_ROBOT_FX"), fx_timer: 0.0 })
}

impl Lab {
    /// Advances the line-up by `dt` seconds (each frame), and spawns the chosen effect on its interval.
    pub fn step(&mut self, dt: f32) {
        self.t += dt;
        let t = self.t;
        for r in &mut self.robots {
            r.update(dt, t);
        }
        if let Some(kind) = &self.fx {
            self.fx_timer -= dt;
            if self.fx_timer <= 0.0 {
                self.fx_timer = FX_EVERY;
                for r in &mut self.robots {
                    r.fx(kind);
                }
            }
        }
    }
}
