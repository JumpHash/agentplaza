//! The kit's devices.

use std::f32::consts::{FRAC_PI_2, TAU};

use glam::{Mat4, Vec3};

use super::common::*;
use crate::glb::{Node, Scene};
use crate::math;
use crate::mesh::Mesh;

/// A round status light on a front face: an emissive dot the engine lights by the machine's status.
fn status_dot(x: f32, y: f32, z: f32) -> Node {
    part("status", cyl_z(0.03, 0.01, 14, x, y, z), M::Status)
}

/// The anchors every level carries: the name tag above the model and the port anchor at its back.
fn anchored(mut nodes: Vec<Node>, size: [f32; 3]) -> Vec<Node> {
    let [_, h, d] = size;
    nodes.push(anchor("label", 0.0, h + 0.3, 0.0));
    nodes.push(anchor("port", 0.0, h / 2.0, -d / 2.0));
    nodes
}

pub fn mini() -> Scene {
    let size = [2.4, 0.62, 2.4];
    let model = |full: bool| {
        let mut v = vec![
            part(
                "foot",
                cyl(0.95, 0.95, 0.06, 48, 0.0, 0.03, 0.0),
                M::Polymer,
            ),
            part(
                "body",
                rbox(2.4, 0.56, 2.4, 0.42, 0.07, 0.0, 0.34, 0.0),
                M::Aluminium,
            ),
        ];
        if full {
            v.push(part(
                "port",
                rbox(0.12, 0.06, 0.02, 0.025, 0.005, -0.75, 0.33, 1.205),
                M::Bezel,
            ));
            v.push(part(
                "port",
                rbox(0.12, 0.06, 0.02, 0.025, 0.005, -0.55, 0.33, 1.205),
                M::Bezel,
            ));
        }
        v.push(status_dot(0.95, 0.33, 1.2));
        v
    };
    scene(
        "mini",
        size,
        anchored(model(true), size),
        anchored(model(false), size),
        anchored(block(size[0], size[1], size[2], M::Aluminium), size),
    )
}

pub fn studio() -> Scene {
    let size = [2.4, 1.19, 2.4];
    let model = |full: bool| {
        let mut v = vec![part(
            "foot",
            cyl(1.0, 1.0, 0.08, 48, 0.0, 0.04, 0.0),
            M::Polymer,
        )];
        if full {
            let mut vents = Mesh::default();
            for i in 0..36 {
                let a = i as f32 / 36.0 * TAU;
                vents.append(&cyl(
                    0.035,
                    0.035,
                    0.01,
                    10,
                    math::cos(a) * 0.85,
                    0.085,
                    math::sin(a) * 0.85,
                ));
            }
            v.push(part("foot_vent", vents, M::Interior));
        }
        v.push(part(
            "body",
            rbox(2.4, 1.1, 2.4, 0.4, 0.08, 0.0, 0.64, 0.0),
            M::Aluminium,
        ));
        if full {
            v.push(part(
                "port",
                rbox(0.1, 0.16, 0.02, 0.04, 0.005, -0.8, 0.62, 1.205),
                M::Bezel,
            ));
            v.push(part(
                "port",
                rbox(0.1, 0.16, 0.02, 0.04, 0.005, -0.6, 0.62, 1.205),
                M::Bezel,
            ));
            v.push(part(
                "card_slot",
                cube(0.4, 0.035, 0.02, -0.15, 0.62, 1.205),
                M::Bezel,
            ));
        }
        v.push(status_dot(0.95, 0.62, 1.2));
        v
    };
    scene(
        "studio",
        size,
        anchored(model(true), size),
        anchored(model(false), size),
        anchored(block(size[0], size[1], size[2], M::Aluminium), size),
    )
}

pub fn laptop() -> Scene {
    let size = [3.2, 2.26, 2.87];
    // The lid is a group at (0, 0.14, -0.8) turned about x by -1.83; its parts are baked into that frame.
    let lid = Mat4::from_translation(Vec3::new(0.0, 0.14, -0.8)) * math::mat_rot_x(-1.83);
    let model = |full: bool| {
        let mut v = vec![
            part(
                "base",
                rbox(3.2, 0.14, 2.2, 0.14, 0.03, 0.0, 0.07, 0.3),
                M::Aluminium,
            ),
            part(
                "keyboard_well",
                cube(2.8, 0.01, 1.0, 0.0, 0.142, 0.0),
                M::Polymer,
            ),
            part(
                "trackpad",
                rbox(1.1, 0.01, 0.62, 0.05, 0.003, 0.0, 0.142, 0.9),
                M::AluminiumDark,
            ),
            status_strip(0.5, 0.03, 0.0, 0.07, 1.41),
        ];
        if full {
            let mut keys = Mesh::default();
            for r in 0..5 {
                for c in 0..14 {
                    keys.append(&cube(
                        0.16,
                        0.025,
                        0.15,
                        -1.3 + c as f32 * 0.2,
                        0.155,
                        -0.4 + r as f32 * 0.19,
                    ));
                }
            }
            v.push(part("key", keys, M::Polymer));
            let hinge = cyl(0.07, 0.07, 2.6, 20, 0.0, 0.0, 0.0).transformed(
                Mat4::from_translation(Vec3::new(0.0, 0.14, -0.8)) * math::mat_rot_z(FRAC_PI_2),
            );
            v.push(part("hinge", hinge, M::AluminiumDark));
        }
        v.push(part(
            "lid",
            rbox(3.2, 0.07, 2.2, 0.14, 0.02, 0.0, 0.035, 1.1).transformed(lid),
            M::Aluminium,
        ));
        v.push(part(
            "lid",
            cube(3.06, 0.005, 2.02, 0.0, -0.001, 1.1).transformed(lid),
            M::Bezel,
        ));
        v.push(screen(
            2.86,
            1.8,
            lid * Mat4::from_translation(Vec3::new(0.0, -0.006, 1.12)) * math::mat_rot_x(FRAC_PI_2),
        ));
        v
    };
    scene(
        "laptop",
        size,
        anchored(model(true), size),
        anchored(model(false), size),
        anchored(block(size[0], size[1], size[2], M::Aluminium), size),
    )
}

pub fn phone() -> Scene {
    let size = [1.5, 2.2, 1.2];
    // The upright slab and its screen sit in a group at (0, 0.25, 0.05) turned about x by -0.22; they are baked into it.
    let frame = Mat4::from_translation(Vec3::new(0.0, 0.25, 0.05)) * math::mat_rot_x(-0.22);
    let model = |full: bool| {
        let mut v = vec![part(
            "base",
            rbox(1.5, 0.24, 1.2, 0.18, 0.05, 0.0, 0.12, 0.0),
            M::Polymer,
        )];
        if full {
            v.push(part(
                "bezel",
                cube(1.0, 0.04, 0.3, 0.0, 0.25, 0.05),
                M::Bezel,
            ));
        }
        let slab = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)) * math::mat_rot_x(FRAC_PI_2);
        v.push(part(
            "body",
            rbox(0.95, 0.09, 2.0, 0.16, 0.03, 0.0, 0.0, 0.0).transformed(frame * slab),
            M::GraphiteDark,
        ));
        v.push(screen(
            0.85,
            1.86,
            frame * Mat4::from_translation(Vec3::new(0.0, 1.0, 0.047)),
        ));
        v.push(status_strip(0.6, 0.03, 0.0, 0.1, 0.61));
        v
    };
    scene(
        "phone",
        size,
        anchored(model(true), size),
        anchored(model(false), size),
        anchored(block(size[0], size[1], size[2], M::Polymer), size),
    )
}
