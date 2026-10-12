//! `floor3d/robot.ts`: the plaza robot (boxy body, treads, binocular eyes on a neck, little
//! arms) and its animation. The scene places it; the robot animates itself.

use glam::Vec3;

use super::scene::{Color, Obj};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotState {
    Idle,
    Working,
    Walking,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotPose {
    Wave,
    Proud,
    Think,
    Cheer,
}

/// The console palette the robot is painted in (`Palette` in robot.ts).
#[derive(Debug, Clone, Copy)]
pub struct RobotPalette {
    pub ink: Color,
    pub sheet: Color,
    pub paper: Color,
    pub rule: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
    pub ink_muted: Color,
}

/// One robot. `group` goes into the scene (marked dynamic); position and turn it from outside.
pub struct Robot {
    pub group: Obj,
    // contract: R — the parts it animates (head, neck, arms, eyes, treads, …) and its state.
}

impl Robot {
    /// `makeRobot(hue, orchestrator, palette, bodyHue)`; hues in degrees.
    pub fn new(hue: f32, orchestrator: bool, palette: &RobotPalette, body_hue: f32) -> Robot {
        let _ = (hue, orchestrator, palette, body_hue);
        let group = Obj::group();
        group.set_dynamic(true);
        Robot { group } // contract: R
    }
    /// The world point just above the head (the group is a direct child of the scene root).
    pub fn label_anchor(&self) -> Vec3 {
        self.group.position() + Vec3::Y * 1.6 // contract: R
    }
    pub fn set_state(&mut self, state: RobotState) {
        let _ = state; // contract: R
    }
    /// `dt` seconds since the last frame, `t` seconds since start.
    pub fn update(&mut self, dt: f32, t: f32) {
        let _ = (dt, t); // contract: R
    }
    pub fn set_highlighted(&mut self, on: bool) {
        let _ = on; // contract: R
    }
    pub fn pose(&mut self, kind: RobotPose) {
        let _ = kind; // contract: R
    }
}
