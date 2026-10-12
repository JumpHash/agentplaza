//! `floor3d/robot.ts`: the plaza robot (boxy body, treads, binocular eyes on a neck, little
//! arms) and its animation. The scene places it; the robot animates itself.

use std::f32::consts::{FRAC_PI_2, TAU};

use glam::{Mat4, Vec3};

use super::geometry::{box_geo, circle, cylinder, rounded_box, sphere, transformed};
use super::scene::{Color, Geo, Mat, Material, Obj};
use super::toon::{outline, toon_mat};

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

// Layout in robot units: about one unit tall before the orchestrator's scale.
const BODY_Y: f32 = 0.46; // body centre
const BODY_H: f32 = 0.5;
const NECK_Y: f32 = 0.78;
const HEAD_Y: f32 = 0.82; // head pivot, at the top of the neck
const HEAD_H: f32 = 0.24;
const SCALE: f32 = 1.3; // a touch bigger than the furniture, so the robots read from afar
const ORCH_SCALE: f32 = 1.25 * SCALE;
const LABEL_LIFT: f32 = 0.35;

/// One robot. `group` goes into the scene (marked dynamic); position and turn it from outside.
pub struct Robot {
    pub group: Obj,
    palette: RobotPalette,
    head: Obj,
    body_group: Obj,
    tread_group: Obj,
    wheels: Vec<Obj>,
    arm_l: Obj,
    arm_r: Obj,
    pupils: Vec<Obj>,
    pupil_mat: Mat,
    /// The antenna light's material (orchestrator only).
    light_mat: Option<Mat>,
    /// The materials that take the highlight glow (the shell and running gear).
    shell: Vec<Mat>,
    state: RobotState,
    pupil_intensity: f32,
}

/// A toon material in `color` (roughness and metalness are not part of `Material`, as in toon.rs).
fn toon(color: Color) -> Mat {
    toon_mat(Material { color, ..Material::default() })
}

/// A toon material that takes the highlight glow: it is also recorded in `shell`.
fn shell_mat(shell: &mut Vec<Mat>, color: Color) -> Mat {
    let m = toon(color);
    shell.push(m.clone());
    m
}

/// `add(parent, geometry, material, x, y, z)`: a shadow-casting mesh at a position.
fn add(parent: &Obj, geo: &Geo, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
    let mesh = Obj::mesh(geo, mat);
    mesh.set_position(x, y, z).set_shadows(true, true);
    parent.add(&mesh);
    mesh
}

impl Robot {
    /// `makeRobot(hue, orchestrator, palette, bodyHue)`; hues in degrees.
    pub fn new(hue: f32, orchestrator: bool, palette: &RobotPalette, body_hue: f32) -> Robot {
        let palette = *palette;
        let root = Obj::group();
        root.set_name(if orchestrator { "robot-orchestrator" } else { "robot" });
        root.set_scalar(if orchestrator { ORCH_SCALE } else { SCALE });
        root.set_dynamic(true);

        let mut shell = Vec::new();
        let sheet_mat = shell_mat(&mut shell, Color::hsl(body_hue / 360.0, 0.6, 0.64));
        let paper_mat = shell_mat(&mut shell, palette.paper);
        let panel_mat = shell_mat(&mut shell, Color::hsl(hue / 360.0, 0.5, 0.82));
        let rivet_mat = shell_mat(&mut shell, palette.rule);
        let ink_mat = shell_mat(&mut shell, palette.ink);
        let signal_mat = shell_mat(&mut shell, palette.signal);

        // Glass and lights are not part of the glow, so the highlight never washes out the eyes.
        let glass_mat = toon(palette.ink);
        let pupil_mat = toon_mat(Material {
            color: palette.ok,
            emissive: palette.ok,
            emissive_intensity: 1.2,
            ..Material::default()
        });

        // Treads and wheels: the tread group bobs when walking.
        let tread_group = Obj::group();
        root.add(&tread_group);
        let tread_geo = rounded_box(0.16, 0.22, 0.86, 3, 0.07);
        add(&tread_group, &tread_geo, &ink_mat, -0.36, 0.11, 0.0);
        add(&tread_group, &tread_geo, &ink_mat, 0.36, 0.11, 0.0);
        // Axle along x, so the wheel spins about rotation.x.
        let wheel_geo = transformed(&cylinder(0.08, 0.08, 0.03, 16), Mat4::from_rotation_z(FRAC_PI_2));
        let mut wheels = Vec::new();
        for side in [-1.0_f32, 1.0] {
            for z in [-0.28_f32, 0.0, 0.28] {
                wheels.push(add(&tread_group, &wheel_geo, &rivet_mat, side * 0.445, 0.11, z));
            }
        }

        // Body: rounded shell, a tinted front panel with a panel line and rivets.
        let body_group = Obj::group();
        root.add(&body_group);
        add(&body_group, &rounded_box(0.62, BODY_H, 0.6, 4, 0.12), &sheet_mat, 0.0, BODY_Y, 0.0);
        add(&body_group, &rounded_box(0.36, 0.26, 0.03, 2, 0.01), &panel_mat, 0.0, BODY_Y, 0.315);
        add(&body_group, &box_geo(0.28, 0.012, 0.012), &rivet_mat, 0.0, BODY_Y, 0.33);
        let rivet_geo = sphere(0.018, 8, 6);
        for x in [-0.15_f32, 0.15] {
            for y in [BODY_Y - 0.1, BODY_Y + 0.1] {
                add(&body_group, &rivet_geo, &rivet_mat, x, y, 0.33);
            }
        }
        add(&body_group, &cylinder(0.07, 0.08, 0.14, 12), &ink_mat, 0.0, NECK_Y, 0.0);

        // Little arms: a pivot at each shoulder, a simple two-finger gripper at the end.
        let arm_geo = transformed(
            &cylinder(0.045, 0.04, 0.3, 12),
            Mat4::from_translation(Vec3::new(0.0, -0.15, 0.0)),
        );
        let claw_base_geo = rounded_box(0.1, 0.05, 0.08, 2, 0.02);
        let finger_geo = box_geo(0.025, 0.08, 0.03);
        let make_arm = |side: f32| -> Obj {
            let pivot = Obj::group();
            pivot.set_position(side * 0.34, 0.6, 0.0);
            body_group.add(&pivot);
            add(&pivot, &arm_geo, &sheet_mat, 0.0, 0.0, 0.0);
            let grip = Obj::group();
            grip.set_position(0.0, -0.3, 0.0);
            pivot.add(&grip);
            add(&grip, &claw_base_geo, &ink_mat, 0.0, 0.0, 0.0);
            add(&grip, &finger_geo, &ink_mat, -0.035, -0.05, 0.0);
            add(&grip, &finger_geo, &ink_mat, 0.035, -0.05, 0.0);
            pivot
        };
        let arm_l = make_arm(-1.0);
        let arm_r = make_arm(1.0);

        // Head: a paper housing with two big eye-cylinders (dark glass, glowing pupils).
        let head = Obj::group();
        head.set_position(0.0, HEAD_Y, 0.0);
        body_group.add(&head);
        add(&head, &rounded_box(0.62, HEAD_H, 0.42, 3, 0.09), &paper_mat, 0.0, HEAD_H / 2.0, 0.0);
        // Axis along z, so the eyes face forward.
        let eye_geo = transformed(&cylinder(0.13, 0.13, 0.18, 24), Mat4::from_rotation_x(FRAC_PI_2));
        let glass_geo = circle(0.1, 24, 0.0, TAU);
        let pupil_geo = circle(0.07, 24, 0.0, TAU);
        let mut pupils = Vec::new();
        for x in [-0.16_f32, 0.16] {
            add(&head, &eye_geo, &paper_mat, x, 0.12, 0.2);
            add(&head, &glass_geo, &glass_mat, x, 0.12, 0.292);
            pupils.push(add(&head, &pupil_geo, &pupil_mat, x, 0.12, 0.296));
        }

        let mut light_mat = None;
        if orchestrator {
            add(&head, &cylinder(0.01, 0.01, 0.16, 6), &ink_mat, 0.22, HEAD_H + 0.08, -0.05);
            let antenna_mat = toon_mat(Material {
                color: palette.signal,
                emissive: palette.signal,
                emissive_intensity: 1.2,
                ..Material::default()
            });
            add(&head, &sphere(0.03, 10, 8), &antenna_mat, 0.22, HEAD_H + 0.17, -0.05);
            // Signal-orange trim across the front of the body, above the panel.
            add(&body_group, &rounded_box(0.36, 0.03, 0.02, 2, 0.008), &signal_mat, 0.0, 0.62, 0.31);
            light_mat = Some(antenna_mat);
        }

        outline(&root, 0.03);

        let mut robot = Robot {
            group: root,
            palette,
            head,
            body_group,
            tread_group,
            wheels,
            arm_l,
            arm_r,
            pupils,
            pupil_mat,
            light_mat,
            shell,
            state: RobotState::Idle,
            pupil_intensity: 1.2,
        };
        robot.apply_state(RobotState::Idle);
        robot
    }

    /// The world point just above the head (the group is a direct child of the scene root).
    /// Includes the body group the head hangs from, as three's `head.localToWorld` does.
    pub fn label_anchor(&self) -> Vec3 {
        let head_to_world = self.group.world_matrix_of(&self.head).unwrap_or(Mat4::IDENTITY);
        head_to_world.transform_point3(Vec3::new(0.0, HEAD_H + LABEL_LIFT, 0.0))
    }

    pub fn set_state(&mut self, state: RobotState) {
        if state != self.state {
            self.apply_state(state);
        }
    }

    fn apply_state(&mut self, next: RobotState) {
        self.state = next;
        let colour = match next {
            RobotState::Offline => self.palette.ink_muted,
            RobotState::Working => self.palette.info,
            _ => self.palette.ok,
        };
        {
            let mut pupil = self.pupil_mat.get();
            pupil.color = colour;
            pupil.emissive = colour;
        }
        self.pupil_intensity = match next {
            RobotState::Offline => 0.2,
            RobotState::Working => 2.0,
            _ => 1.3,
        };
    }

    /// `dt` seconds since the last frame, `t` seconds since start.
    pub fn update(&mut self, dt: f32, t: f32) {
        let k = (dt * 8.0).min(1.0);
        let s = self.state;

        // Head: idle tilts, working nods, walking glances, offline droops and stays still.
        let (head_x, head_z) = match s {
            RobotState::Idle => (0.03 * (t * 0.5).sin(), 0.12 * (t * 0.9).sin()),
            RobotState::Working => (0.14 + 0.08 * (t * 5.0).sin(), 0.04 * (t * 1.3).sin()),
            RobotState::Walking => (0.04, 0.06 * (t * 3.0).sin()),
            RobotState::Offline => (0.5, 0.12),
        };
        {
            let mut head = self.head.get();
            let rx = head.rotation.x;
            head.rotation.x = rx + (head_x - rx) * k;
            let rz = head.rotation.z;
            head.rotation.z = rz + (head_z - rz) * k;
        }

        // Body: slight bob when idle, a sway when walking, slumped when offline.
        let (body_y, body_sway) = match s {
            RobotState::Idle => (0.012 * (t * 1.6).sin(), 0.0),
            RobotState::Working => (0.004 * (t * 2.5).sin(), 0.0),
            RobotState::Walking => (0.01 * (t * 6.0).sin().abs(), 0.05 * (t * 6.0).sin()),
            RobotState::Offline => (-0.02, 0.0),
        };
        {
            let mut body = self.body_group.get();
            body.position.y = body_y;
            body.rotation.z = body_sway;
        }

        // Treads bob and wheels roll while walking.
        self.tread_group.get().position.y =
            if s == RobotState::Walking { 0.012 * (t * 10.0).sin().abs() } else { 0.0 };
        if s == RobotState::Walking {
            for w in &self.wheels {
                w.get().rotation.x += dt * 8.0;
            }
        }

        // Arms: quick alternating typing when working, a swing when walking, rest otherwise.
        let (arm_l_x, arm_r_x) = match s {
            RobotState::Working => {
                let p = (t * 11.0).sin();
                (-0.9 + 0.2 * p, -0.9 - 0.2 * p)
            }
            RobotState::Walking => {
                let w = 0.25 * (t * 6.0).sin();
                (w, -w)
            }
            RobotState::Idle => {
                let i = 0.05 * (t * 1.1).sin();
                (i, -i)
            }
            RobotState::Offline => (0.2, 0.2),
        };
        self.arm_l.get().rotation.x = arm_l_x;
        self.arm_r.get().rotation.x = arm_r_x;

        // Eyes: occasional blink when idle, half-lidded when offline, brighter and pulsing when working.
        let mut lid = 1.0_f32;
        if s == RobotState::Idle && t % 3.6 < 0.14 {
            lid = 0.1;
        } else if s == RobotState::Offline {
            lid = 0.45;
        }
        for p in &self.pupils {
            p.get().scale.y = lid;
        }
        self.pupil_mat.get().emissive_intensity = if s == RobotState::Working {
            self.pupil_intensity + 0.3 * (t * 5.0).sin()
        } else {
            self.pupil_intensity
        };

        if let Some(light) = &self.light_mat {
            light.get().emissive_intensity = 1.1 + 0.5 * (t * 3.0).sin();
        }
    }

    pub fn set_highlighted(&mut self, on: bool) {
        for m in &self.shell {
            let mut mat = m.get();
            mat.emissive = if on { self.palette.signal } else { Color::BLACK };
            mat.emissive_intensity = if on { 0.35 } else { 0.0 };
        }
    }

    /// Strikes a still pose (for a portrait); the next `update` moves it again.
    pub fn pose(&mut self, kind: RobotPose) {
        self.arm_l.get().rotation = Vec3::ZERO;
        self.arm_r.get().rotation = Vec3::ZERO;
        self.head.get().rotation = Vec3::ZERO;
        match kind {
            RobotPose::Wave => {
                self.arm_r.get().rotation.z = 2.5;
                self.arm_l.get().rotation.z = -0.25;
                self.head.get().rotation.z = -0.15;
            }
            RobotPose::Proud => {
                self.arm_l.get().rotation.z = -0.55;
                self.arm_r.get().rotation.z = 0.55;
                self.head.get().rotation.x = -0.12;
            }
            RobotPose::Think => {
                self.arm_r.get().rotation.x = -1.9;
                self.arm_r.get().rotation.z = -0.35;
                self.head.get().rotation.z = 0.2;
                self.head.get().rotation.x = 0.08;
            }
            RobotPose::Cheer => {
                self.arm_l.get().rotation.z = -2.7;
                self.arm_r.get().rotation.z = 2.7;
                self.head.get().rotation.x = -0.15;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::scene::Kind;
    use super::*;

    fn palette() -> RobotPalette {
        RobotPalette {
            ink: Color::hex(0x1b1d22),
            sheet: Color::hex(0xe9e4d8),
            paper: Color::hex(0xf4f1ea),
            rule: Color::hex(0xc9c3b4),
            signal: Color::hex(0xff6a1a),
            ok: Color::hex(0x3ec28a),
            info: Color::hex(0x4a90e2),
            ink_muted: Color::hex(0x7a7f88),
        }
    }

    /// The local matrix of every node under `root`, depth first.
    fn local_matrices(root: &Obj) -> Vec<Mat4> {
        let mut out = Vec::new();
        root.traverse(&mut |o: &Obj| out.push(o.local_matrix()));
        out
    }

    #[test]
    fn robot_builds_its_parts() {
        let robot = Robot::new(200.0, false, &palette(), 120.0);
        let mut meshes = 0;
        robot.group.traverse(&mut |o: &Obj| {
            if matches!(o.get().kind, Kind::Mesh { .. }) {
                meshes += 1;
            }
        });
        assert!(meshes > 0);
    }

    #[test]
    fn robot_walking_moves_its_parts() {
        let mut robot = Robot::new(200.0, false, &palette(), 120.0);
        let before = local_matrices(&robot.group);
        robot.set_state(RobotState::Walking);
        robot.update(0.016, 1.0);
        assert_ne!(before, local_matrices(&robot.group));
    }
}
