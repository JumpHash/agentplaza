//! `floor3d/robot.ts`: the plaza robot in one of five designs (a WALL·E-like rover, a hover bubble,
//! a TV head on a ball wheel, a can with a dome head, a kitty with ears) and its animation. Every
//! design has the same parts (a base, a body, a head with glowing eyes, two arms), so they share
//! one animation. The scene places it; the robot animates itself.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use glam::{Mat4, Vec3};

use super::geometry::{box_geo, circle, cone, cylinder, rounded_box, sphere, sphere_part, torus, transformed};
use super::scene::{Color, Geo, Mat, Material, Obj, Shading};
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

/// The robot designs (`RobotVariant` in robot.ts). An agent keeps its own; the orchestrator is
/// always the rover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotVariant {
    Rover,
    Hover,
    Tv,
    Can,
    Kitty,
}

pub const ROBOT_VARIANTS: [RobotVariant; 5] = [
    RobotVariant::Rover,
    RobotVariant::Hover,
    RobotVariant::Tv,
    RobotVariant::Can,
    RobotVariant::Kitty,
];

/// An agent's design: neighbouring cids (a district's members) get different ones.
pub fn robot_variant(cid: u64) -> RobotVariant {
    ROBOT_VARIANTS[(cid % ROBOT_VARIANTS.len() as u64) as usize]
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

/// The robots' own paint, the same in the light and the dark theme (`ROBOT_PALETTE` in robot.ts).
/// A function because `Color::hex` is not `const`.
pub fn robot_palette() -> RobotPalette {
    RobotPalette {
        ink: Color::hex(0x262c3a),
        sheet: Color::hex(0xffffff),
        paper: Color::hex(0xeef1f6),
        rule: Color::hex(0x9aa5b8),
        signal: Color::hex(0xff7a2e),
        ok: Color::hex(0x3ee08f),
        info: Color::hex(0x4cc9ff),
        ink_muted: Color::hex(0x5b6475),
    }
}

// Layout in robot units: about one unit tall before the orchestrator's scale.
const SCALE: f32 = 1.3; // a touch bigger than the furniture, so the robots read from afar
const ORCH_SCALE: f32 = 1.25 * SCALE;
const LABEL_LIFT: f32 = 0.35;

/// One robot. `group` goes into the scene (marked dynamic); position and turn it from outside.
pub struct Robot {
    pub group: Obj,
    palette: RobotPalette,
    head: Obj,
    head_top: f32,
    body_group: Obj,
    base_group: Obj,
    arm_l: Obj,
    arm_r: Obj,
    pupils: Vec<Obj>,
    /// Wheels and rotors: they turn about their own x (or y, when `yaw`) while walking.
    spin: Vec<(Obj, bool)>,
    /// Feet that step while walking.
    feet: Vec<Obj>,
    /// Floats on a thruster (bobs, lands when offline).
    hover: bool,
    pupil_mat: Mat,
    thruster_mat: Mat,
    /// The antenna light's material (orchestrator only).
    light_mat: Option<Mat>,
    /// The materials that take the highlight glow (the shell and running gear).
    shell: Vec<Mat>,
    state: RobotState,
    pupil_intensity: f32,
}

/// The materials a design is built from.
struct Mats {
    sheet: Mat,
    paper: Mat,
    panel: Mat,
    rivet: Mat,
    ink: Mat,
    signal: Mat,
    glass: Mat,
    pupil: Mat,
    thruster: Mat,
}

/// The parts a design builds; the shared animation drives them.
struct Parts {
    head: Obj,
    /// The top of the head above its pivot, for the label.
    head_top: f32,
    arm_l: Obj,
    arm_r: Obj,
    /// The glowing eyes: they blink (scale y).
    pupils: Vec<Obj>,
    spin: Vec<(Obj, bool)>,
    feet: Vec<Obj>,
    hover: bool,
}

/// A toon material in `color` (roughness and metalness are not part of `Material`, as in toon.rs).
fn toon(color: Color) -> Mat {
    toon_mat(Material { color, ..Material::default() })
}

/// A toon material that glows in `color` at `intensity`.
fn glow(color: Color, intensity: f32) -> Mat {
    toon_mat(Material {
        color,
        emissive: color,
        emissive_intensity: intensity,
        ..Material::default()
    })
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

fn rot_x(geo: &Geo, a: f32) -> Geo {
    transformed(geo, Mat4::from_rotation_x(a))
}

fn rot_y(geo: &Geo, a: f32) -> Geo {
    transformed(geo, Mat4::from_rotation_y(a))
}

fn rot_z(geo: &Geo, a: f32) -> Geo {
    transformed(geo, Mat4::from_rotation_z(a))
}

fn scaled(geo: &Geo, x: f32, y: f32, z: f32) -> Geo {
    transformed(geo, Mat4::from_scale(Vec3::new(x, y, z)))
}

/// A shoulder pivot with a straight arm hanging from it and a hand at its end.
fn arm(parent: &Obj, x: f32, y: f32, len: f32, arm_mat: &Mat, hand: &dyn Fn(&Obj)) -> Obj {
    let pivot = Obj::group();
    pivot.set_position(x, y, 0.0);
    parent.add(&pivot);
    let geo = transformed(&cylinder(0.045, 0.04, len, 12), Mat4::from_translation(Vec3::new(0.0, -len / 2.0, 0.0)));
    add(&pivot, &geo, arm_mat, 0.0, 0.0, 0.0);
    let grip = Obj::group();
    grip.set_position(0.0, -len, 0.0);
    pivot.add(&grip);
    hand(&grip);
    pivot
}

/// A round mitten hand.
fn mitten(grip: &Obj, mat: &Mat) {
    add(grip, &sphere(0.065, 14, 10), mat, 0.0, 0.0, 0.0);
}

/// Two eye sockets (dark glass, glowing pupils) on a face at depth `z`; returns the pupils.
fn eyes(head: &Obj, m: &Mats, xs: &[f32], y: f32, z: f32, glass_r: f32, pupil_r: f32) -> Vec<Obj> {
    let glass_geo = circle(glass_r, 24, 0.0, TAU);
    let pupil_geo = circle(pupil_r, 24, 0.0, TAU);
    xs.iter()
        .map(|&x| {
            add(head, &glass_geo, &m.glass, x, y, z);
            add(head, &pupil_geo, &m.pupil, x, y, z + 0.004)
        })
        .collect()
}

/// The original: boxy body on tank treads, binocular eyes on a neck, claw arms.
fn build_rover(body: &Obj, base: &Obj, m: &Mats) -> Parts {
    const BODY_Y: f32 = 0.46;
    const HEAD_H: f32 = 0.24;
    let tread = rounded_box(0.16, 0.22, 0.86, 3, 0.07);
    add(base, &tread, &m.ink, -0.36, 0.11, 0.0);
    add(base, &tread, &m.ink, 0.36, 0.11, 0.0);
    let wheel = rot_z(&cylinder(0.08, 0.08, 0.03, 16), FRAC_PI_2); // axle along x
    let mut spin = Vec::new();
    for side in [-1.0_f32, 1.0] {
        for z in [-0.28_f32, 0.0, 0.28] {
            spin.push((add(base, &wheel, &m.rivet, side * 0.445, 0.11, z), false));
        }
    }

    add(body, &rounded_box(0.62, 0.5, 0.6, 4, 0.12), &m.sheet, 0.0, BODY_Y, 0.0);
    add(body, &rounded_box(0.36, 0.26, 0.03, 2, 0.01), &m.panel, 0.0, BODY_Y, 0.315);
    add(body, &box_geo(0.28, 0.012, 0.012), &m.rivet, 0.0, BODY_Y, 0.33);
    let rivet = sphere(0.018, 8, 6);
    for x in [-0.15_f32, 0.15] {
        for y in [BODY_Y - 0.1, BODY_Y + 0.1] {
            add(body, &rivet, &m.rivet, x, y, 0.33);
        }
    }
    add(body, &cylinder(0.07, 0.08, 0.14, 12), &m.ink, 0.0, 0.78, 0.0);

    let claw_base = rounded_box(0.1, 0.05, 0.08, 2, 0.02);
    let finger = box_geo(0.025, 0.08, 0.03);
    let claw = |grip: &Obj| {
        add(grip, &claw_base, &m.ink, 0.0, 0.0, 0.0);
        add(grip, &finger, &m.ink, -0.035, -0.05, 0.0);
        add(grip, &finger, &m.ink, 0.035, -0.05, 0.0);
    };
    let arm_l = arm(body, -0.34, 0.6, 0.3, &m.sheet, &claw);
    let arm_r = arm(body, 0.34, 0.6, 0.3, &m.sheet, &claw);

    let head = Obj::group();
    head.set_position(0.0, 0.82, 0.0);
    body.add(&head);
    add(&head, &rounded_box(0.62, HEAD_H, 0.42, 3, 0.09), &m.paper, 0.0, HEAD_H / 2.0, 0.0);
    let eye = rot_x(&cylinder(0.13, 0.13, 0.18, 24), FRAC_PI_2); // axis along z, so the eyes face forward
    for x in [-0.16_f32, 0.16] {
        add(&head, &eye, &m.paper, x, 0.12, 0.2);
    }
    let pupils = eyes(&head, m, &[-0.16, 0.16], 0.12, 0.292, 0.1, 0.07);
    Parts { head, head_top: HEAD_H, arm_l, arm_r, pupils, spin, feet: Vec::new(), hover: false }
}

/// A round bubble floating on a glowing thruster ring, with a goggle visor and mitten hands.
fn build_hover(body: &Obj, base: &Obj, m: &Mats) -> Parts {
    // The thruster: a dark ring with a glowing disc under it; the ring turns.
    let rotor = Obj::group();
    rotor.set_position(0.0, 0.16, 0.0);
    base.add(&rotor);
    add(&rotor, &rot_x(&torus(0.22, 0.05, 10, 28, TAU), FRAC_PI_2), &m.ink, 0.0, 0.0, 0.0);
    let vane = box_geo(0.4, 0.02, 0.05);
    add(&rotor, &vane, &m.rivet, 0.0, 0.0, 0.0);
    add(&rotor, &rot_y(&vane, FRAC_PI_2), &m.rivet, 0.0, 0.0, 0.0);
    add(base, &rot_x(&circle(0.19, 24, 0.0, TAU), FRAC_PI_2), &m.thruster, 0.0, 0.13, 0.0); // faces down

    // The bubble body with a metal seam round its middle.
    add(body, &scaled(&sphere(0.31, 28, 20), 1.0, 0.92, 1.0), &m.sheet, 0.0, 0.46, 0.0);
    add(body, &rot_x(&torus(0.305, 0.022, 8, 36, TAU), FRAC_PI_2), &m.rivet, 0.0, 0.42, 0.0);
    add(body, &circle(0.08, 20, 0.0, TAU), &m.panel, 0.0, 0.36, 0.296);

    let hand = |grip: &Obj| mitten(grip, &m.paper);
    let arm_l = arm(body, -0.31, 0.5, 0.2, &m.sheet, &hand);
    let arm_r = arm(body, 0.31, 0.5, 0.2, &m.sheet, &hand);

    // The head: a white dome with a dark goggle visor wrapped round its front.
    let head = Obj::group();
    head.set_position(0.0, 0.7, 0.0);
    body.add(&head);
    add(&head, &sphere(0.25, 28, 20), &m.paper, 0.0, 0.14, 0.0);
    // The visor: a flattened dark lens bulging out of the dome's front.
    add(&head, &scaled(&sphere(0.21, 24, 14), 1.0, 0.55, 0.6), &m.glass, 0.0, 0.15, 0.15);
    let pupil_geo = scaled(&circle(0.045, 20, 0.0, TAU), 1.0, 1.35, 1.0);
    let pupils = [-0.085_f32, 0.085].iter().map(|&x| add(&head, &pupil_geo, &m.pupil, x, 0.15, 0.278)).collect();
    // Two little antenna nubs.
    let nub = cylinder(0.012, 0.012, 0.1, 6);
    let nub_ball = sphere(0.03, 10, 8);
    for x in [-0.12_f32, 0.12] {
        add(&head, &nub, &m.ink, x, 0.38, 0.0);
        add(&head, &nub_ball, &m.signal, x, 0.44, 0.0);
    }
    Parts { head, head_top: 0.44, arm_l, arm_r, pupils, spin: vec![(rotor, true)], feet: Vec::new(), hover: true }
}

/// A box head with a screen face (eyes and a smile) on a short neck, rolling on one ball wheel.
fn build_tv(body: &Obj, base: &Obj, m: &Mats) -> Parts {
    // The ball wheel in a fork; the stripe shows it rolling.
    let wheel = Obj::group();
    wheel.set_position(0.0, 0.15, 0.0);
    base.add(&wheel);
    add(&wheel, &sphere(0.15, 20, 14), &m.ink, 0.0, 0.0, 0.0);
    add(&wheel, &rot_y(&torus(0.15, 0.022, 8, 28, TAU), FRAC_PI_2), &m.rivet, 0.0, 0.0, 0.0); // round the axle (x)
    let fork = rounded_box(0.05, 0.2, 0.14, 2, 0.02);
    add(base, &fork, &m.sheet, -0.19, 0.2, 0.0);
    add(base, &fork, &m.sheet, 0.19, 0.2, 0.0);

    // A compact body with three buttons, and a neck.
    add(body, &rounded_box(0.48, 0.32, 0.4, 4, 0.1), &m.sheet, 0.0, 0.44, 0.0);
    let button = sphere(0.03, 10, 8);
    add(body, &button, &m.signal, -0.09, 0.44, 0.2);
    add(body, &button, &m.panel, 0.0, 0.44, 0.2);
    add(body, &button, &m.rivet, 0.09, 0.44, 0.2);
    add(body, &cylinder(0.06, 0.07, 0.1, 12), &m.ink, 0.0, 0.64, 0.0);

    let ball = |grip: &Obj| {
        add(grip, &sphere(0.055, 12, 10), &m.rivet, 0.0, 0.0, 0.0);
    };
    let arm_l = arm(body, -0.28, 0.54, 0.24, &m.ink, &ball);
    let arm_r = arm(body, 0.28, 0.54, 0.24, &m.ink, &ball);

    // The head: a white set with a dark screen; on it, two round eyes and a smile.
    let head = Obj::group();
    head.set_position(0.0, 0.68, 0.0);
    body.add(&head);
    add(&head, &rounded_box(0.62, 0.44, 0.44, 4, 0.12), &m.paper, 0.0, 0.22, 0.0);
    add(&head, &rounded_box(0.48, 0.32, 0.04, 3, 0.06), &m.glass, 0.0, 0.22, 0.212);
    let eye = circle(0.06, 22, 0.0, TAU);
    let pupils = [-0.11_f32, 0.11].iter().map(|&x| add(&head, &eye, &m.pupil, x, 0.25, 0.236)).collect();
    let smile = rot_z(&torus(0.06, 0.013, 6, 16, PI), PI); // the lower half: a smile
    add(&head, &smile, &m.pupil, 0.0, 0.17, 0.236);
    // An antenna with a ball on top.
    add(&head, &cylinder(0.012, 0.012, 0.14, 6), &m.ink, 0.16, 0.5, 0.0);
    add(&head, &sphere(0.04, 12, 8), &m.signal, 0.16, 0.58, 0.0);
    Parts { head, head_top: 0.6, arm_l, arm_r, pupils, spin: vec![(wheel, false)], feet: Vec::new(), hover: false }
}

/// A round can on two stubby feet, with a dome head and one big eye.
fn build_can(body: &Obj, base: &Obj, m: &Mats) -> Parts {
    let foot_geo = rounded_box(0.16, 0.08, 0.24, 2, 0.035);
    let leg_geo = cylinder(0.045, 0.05, 0.14, 10);
    let mut feet = Vec::new();
    for x in [-0.12_f32, 0.12] {
        let foot = Obj::group();
        foot.set_position(x, 0.0, 0.0);
        base.add(&foot);
        add(&foot, &foot_geo, &m.ink, 0.0, 0.04, 0.03);
        add(&foot, &leg_geo, &m.rivet, 0.0, 0.14, 0.0);
        feet.push(foot);
    }

    // The can, banded top and bottom, with a front panel.
    add(body, &cylinder(0.27, 0.27, 0.44, 28), &m.sheet, 0.0, 0.42, 0.0);
    let band = rot_x(&torus(0.27, 0.022, 8, 32, TAU), FRAC_PI_2);
    add(body, &band, &m.rivet, 0.0, 0.22, 0.0);
    add(body, &band, &m.rivet, 0.0, 0.62, 0.0);
    add(body, &rounded_box(0.2, 0.16, 0.04, 2, 0.015), &m.panel, 0.0, 0.42, 0.262);

    let hand = |grip: &Obj| mitten(grip, &m.ink);
    let arm_l = arm(body, -0.3, 0.56, 0.24, &m.sheet, &hand);
    let arm_r = arm(body, 0.3, 0.56, 0.24, &m.sheet, &hand);

    // The dome head with one big eye in a dark housing, and a light on top.
    let head = Obj::group();
    head.set_position(0.0, 0.64, 0.0);
    body.add(&head);
    add(&head, &sphere_part(0.27, 28, 14, 0.0, TAU, 0.0, FRAC_PI_2), &m.paper, 0.0, 0.0, 0.0);
    add(&head, &rot_x(&circle(0.27, 28, 0.0, TAU), FRAC_PI_2), &m.paper, 0.0, 0.0, 0.0); // the dome's floor
    add(&head, &rot_x(&cylinder(0.11, 0.11, 0.1, 24), FRAC_PI_2), &m.ink, 0.0, 0.12, 0.23);
    let pupils = eyes(&head, m, &[0.0], 0.12, 0.282, 0.09, 0.06);
    add(&head, &cylinder(0.035, 0.045, 0.05, 10), &m.ink, 0.0, 0.29, 0.0);
    add(&head, &sphere(0.04, 12, 8), &m.signal, 0.0, 0.33, 0.0);
    Parts { head, head_top: 0.36, arm_l, arm_r, pupils, spin: Vec::new(), feet, hover: false }
}

/// A big round head with ears, a small body on a four-wheeled chassis.
fn build_kitty(body: &Obj, base: &Obj, m: &Mats) -> Parts {
    add(base, &rounded_box(0.44, 0.1, 0.4, 2, 0.04), &m.ink, 0.0, 0.13, 0.0);
    let wheel = rot_z(&cylinder(0.085, 0.085, 0.06, 16), FRAC_PI_2);
    let mut spin = Vec::new();
    for x in [-0.23_f32, 0.23] {
        for z in [-0.12_f32, 0.12] {
            spin.push((add(base, &wheel, &m.rivet, x, 0.085, z), false));
        }
    }

    add(body, &rounded_box(0.42, 0.28, 0.36, 4, 0.1), &m.sheet, 0.0, 0.32, 0.0);
    add(body, &circle(0.09, 20, 0.0, TAU), &m.panel, 0.0, 0.31, 0.182); // the belly

    let hand = |grip: &Obj| mitten(grip, &m.paper);
    let arm_l = arm(body, -0.25, 0.4, 0.16, &m.sheet, &hand);
    let arm_r = arm(body, 0.25, 0.4, 0.16, &m.sheet, &hand);

    let head = Obj::group();
    head.set_position(0.0, 0.48, 0.0);
    body.add(&head);
    add(&head, &rounded_box(0.68, 0.46, 0.5, 5, 0.17), &m.paper, 0.0, 0.23, 0.0);
    // Ears: the body's colour outside, the district's inside.
    let ear_geo = cone(0.1, 0.17, 4);
    let inner_geo = cone(0.055, 0.1, 4);
    for side in [-1.0_f32, 1.0] {
        let ear = add(&head, &ear_geo, &m.sheet, side * 0.2, 0.5, 0.0);
        ear.set_rotation(0.0, FRAC_PI_4, -side * 0.3);
        let inner = add(&head, &inner_geo, &m.panel, side * 0.205, 0.49, 0.045);
        inner.set_rotation(0.0, FRAC_PI_4, -side * 0.3);
    }
    let pupils = eyes(&head, m, &[-0.14, 0.14], 0.25, 0.252, 0.1, 0.065);
    let cheek = circle(0.04, 16, 0.0, TAU);
    add(&head, &cheek, &m.panel, -0.25, 0.14, 0.252);
    add(&head, &cheek, &m.panel, 0.25, 0.14, 0.252);
    let mouth = rot_z(&torus(0.03, 0.01, 6, 12, PI), PI);
    add(&head, &mouth, &m.ink, -0.03, 0.15, 0.252);
    add(&head, &mouth, &m.ink, 0.03, 0.15, 0.252);
    Parts { head, head_top: 0.58, arm_l, arm_r, pupils, spin, feet: Vec::new(), hover: false }
}

impl Robot {
    /// `makeRobot(hue, orchestrator, palette, bodyHue)` with the rover design; hues in degrees.
    pub fn new(hue: f32, orchestrator: bool, palette: &RobotPalette, body_hue: f32) -> Robot {
        Robot::with_variant(hue, orchestrator, palette, body_hue, RobotVariant::Rover)
    }

    /// `makeRobot(hue, orchestrator, palette, bodyHue, variant)`: the orchestrator is always a
    /// rover, larger and with an antenna light; an agent takes its `variant`.
    pub fn with_variant(
        hue: f32,
        orchestrator: bool,
        palette: &RobotPalette,
        body_hue: f32,
        variant: RobotVariant,
    ) -> Robot {
        let palette = *palette;
        let kind = if orchestrator { RobotVariant::Rover } else { variant };
        let root = Obj::group();
        root.set_name(if orchestrator { "robot-orchestrator" } else { "robot" });
        root.set_scalar(if orchestrator { ORCH_SCALE } else { SCALE });
        root.set_dynamic(true);

        let mut shell = Vec::new();
        let m = Mats {
            sheet: shell_mat(&mut shell, Color::hsl(body_hue / 360.0, 0.62, 0.62)),
            paper: shell_mat(&mut shell, palette.paper),
            panel: shell_mat(&mut shell, Color::hsl(hue / 360.0, 0.55, 0.78)),
            rivet: shell_mat(&mut shell, palette.rule),
            ink: shell_mat(&mut shell, palette.ink),
            signal: shell_mat(&mut shell, palette.signal),
            // Glass and lights are not part of the glow, so the highlight never washes out the eyes.
            glass: toon(palette.ink),
            pupil: glow(palette.ok, 1.2),
            thruster: Mat::new(Material { shading: Shading::Basic, color: palette.info, ..Material::default() }),
        };
        let pupil_mat = m.pupil.clone();
        let thruster_mat = m.thruster.clone();

        // The base (treads, wheels, feet or thruster) bobs when walking; the body carries the rest.
        let base_group = Obj::group();
        root.add(&base_group);
        let body_group = Obj::group();
        root.add(&body_group);
        let parts = match kind {
            RobotVariant::Rover => build_rover(&body_group, &base_group, &m),
            RobotVariant::Hover => build_hover(&body_group, &base_group, &m),
            RobotVariant::Tv => build_tv(&body_group, &base_group, &m),
            RobotVariant::Can => build_can(&body_group, &base_group, &m),
            RobotVariant::Kitty => build_kitty(&body_group, &base_group, &m),
        };

        let mut light_mat = None;
        if orchestrator {
            add(&parts.head, &cylinder(0.01, 0.01, 0.16, 6), &m.ink, 0.22, parts.head_top + 0.08, -0.05);
            let light = glow(palette.signal, 1.2);
            add(&parts.head, &sphere(0.03, 10, 8), &light, 0.22, parts.head_top + 0.17, -0.05);
            // Signal-orange trim across the front of the body, above the panel.
            add(&body_group, &rounded_box(0.36, 0.03, 0.02, 2, 0.008), &m.signal, 0.0, 0.62, 0.31);
            light_mat = Some(light);
        }

        outline(&root, 0.03);

        let mut robot = Robot {
            group: root,
            palette,
            head: parts.head,
            head_top: parts.head_top,
            body_group,
            base_group,
            arm_l: parts.arm_l,
            arm_r: parts.arm_r,
            pupils: parts.pupils,
            spin: parts.spin,
            feet: parts.feet,
            hover: parts.hover,
            pupil_mat,
            thruster_mat,
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
        head_to_world.transform_point3(Vec3::new(0.0, self.head_top + LABEL_LIFT, 0.0))
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
        self.thruster_mat.get().color = if next == RobotState::Offline { self.palette.ink_muted } else { self.palette.info };
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

        // Body: slight bob when idle, a sway when walking, slumped when offline. A hover robot
        // floats on its thruster, and sets down when offline.
        let (body_y, body_sway) = match s {
            RobotState::Idle => (0.012 * (t * 1.6).sin(), 0.0),
            RobotState::Working => (0.004 * (t * 2.5).sin(), 0.0),
            RobotState::Walking => (0.01 * (t * 6.0).sin().abs(), 0.05 * (t * 6.0).sin()),
            RobotState::Offline => (-0.02, 0.0),
        };
        let lift = if self.hover {
            if s == RobotState::Offline { -0.07 } else { 0.035 * (t * 2.2).sin() }
        } else {
            0.0
        };
        {
            let mut body = self.body_group.get();
            body.position.y = body_y + lift;
            body.rotation.z = body_sway;
        }

        // The base bobs while walking (not on a thruster); wheels roll and feet step while walking;
        // a rotor always turns.
        let bob = if s == RobotState::Walking && !self.hover { 0.012 * (t * 10.0).sin().abs() } else { 0.0 };
        self.base_group.get().position.y = bob + lift;
        for (obj, yaw) in &self.spin {
            if *yaw {
                let speed = match s {
                    RobotState::Offline => 0.0,
                    RobotState::Walking => 9.0,
                    _ => 3.0,
                };
                obj.get().rotation.y += dt * speed;
            } else if s == RobotState::Walking {
                obj.get().rotation.x += dt * 8.0;
            }
        }
        for (i, foot) in self.feet.iter().enumerate() {
            let p = if s == RobotState::Walking { (t * 9.0 + i as f32 * PI).sin() } else { 0.0 };
            let mut node = foot.get();
            node.position.z = 0.06 * p;
            node.position.y = 0.035 * p.max(0.0);
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

    /// The local matrix of every node under `root`, depth first.
    fn local_matrices(root: &Obj) -> Vec<Mat4> {
        let mut out = Vec::new();
        root.traverse(&mut |o: &Obj| out.push(o.local_matrix()));
        out
    }

    fn mesh_count(root: &Obj) -> usize {
        let mut meshes = 0;
        root.traverse(&mut |o: &Obj| {
            if matches!(o.get().kind, Kind::Mesh { .. }) {
                meshes += 1;
            }
        });
        meshes
    }

    #[test]
    fn robot_builds_its_parts() {
        let robot = Robot::new(200.0, false, &robot_palette(), 120.0);
        assert!(mesh_count(&robot.group) > 0);
    }

    #[test]
    fn robot_walking_moves_its_parts() {
        let mut robot = Robot::new(200.0, false, &robot_palette(), 120.0);
        let before = local_matrices(&robot.group);
        robot.set_state(RobotState::Walking);
        robot.update(0.016, 1.0);
        assert_ne!(before, local_matrices(&robot.group));
    }

    #[test]
    fn every_variant_builds_and_walks() {
        for variant in ROBOT_VARIANTS {
            let mut robot = Robot::with_variant(200.0, false, &robot_palette(), 120.0, variant);
            assert!(mesh_count(&robot.group) > 0, "{variant:?} built no meshes");
            let before = local_matrices(&robot.group);
            robot.set_state(RobotState::Walking);
            robot.update(0.016, 1.0);
            assert_ne!(before, local_matrices(&robot.group), "{variant:?} did not move while walking");
        }
    }

    #[test]
    fn robot_variant_cycles_through_the_designs() {
        assert_eq!(robot_variant(0), RobotVariant::Rover);
        assert_eq!(robot_variant(4), RobotVariant::Kitty);
        assert_eq!(robot_variant(5), RobotVariant::Rover);
    }
}
