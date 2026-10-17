//! The kit's racks: the hub, the server and the VM rack. Each builder is a port of the reference's `build(kind)` branch
//! (base on y = 0, front faces +z). The `full` flag of each parts list drops the small details for level 1.

use std::f32::consts::FRAC_PI_2;

use glam::{Quat, Vec3};

use super::common::{anchor, block, cube, cyl, cyl_z, part, plate, rbox, scene, slot_light, status_strip, M};
use crate::glb::{Node, Scene};
use crate::mesh;

/// The hub's bounding box (w, h, d).
const HUB: [f32; 3] = [2.46, 4.65, 2.36];
/// The server's bounding box (w, h, d).
const SERVER: [f32; 3] = [3.5, 0.95, 2.6];
/// The VM rack's bounding box (w, h, d).
const RACK: [f32; 3] = [2.62, 3.92, 2.42];

/// Adds the label and port anchors to a level.
fn finish(mut nodes: Vec<Node>, size: [f32; 3]) -> Vec<Node> {
    let [_, h, d] = size;
    nodes.push(anchor("label", 0.0, h + 0.3, 0.0));
    nodes.push(anchor("port", 0.0, h / 2.0, -d / 2.0));
    nodes
}

pub fn hub() -> Scene {
    let [w, h, d] = HUB;
    scene(
        "hub",
        HUB,
        finish(hub_parts(true), HUB),
        finish(hub_parts(false), HUB),
        finish(block(w, h, d, M::Graphite), HUB),
    )
}

fn hub_parts(full: bool) -> Vec<Node> {
    let mut g = vec![
        part("body", rbox(2.4, 4.3, 2.3, 0.1, 0.04, 0.0, 2.2, 0.0), M::Graphite),
        part("base", rbox(2.44, 0.1, 2.34, 0.1, 0.02, 0.0, 0.08, 0.0), M::Polymer),
        part("top", rbox(2.46, 0.14, 2.36, 0.11, 0.02, 0.0, 4.42, 0.0), M::Signal),
        part("cap", rbox(2.4, 0.16, 2.3, 0.1, 0.03, 0.0, 4.57, 0.0), M::GraphiteDark),
        part("interior", cube(1.98, 3.5, 0.04, 0.0, 2.15, 1.16), M::Interior),
    ];
    for u in 0..8u32 {
        let y = 0.65 + u as f32 * 0.42;
        g.push(part("bay", cube(1.8, 0.34, 0.06, 0.0, y, 1.19), M::Bay));
        if full {
            g.push(part("handle", cube(1.2, 0.03, 0.02, 0.15, y, 1.225), M::Bezel));
            let material = if u % 3 == 1 { M::Polymer } else { M::Led };
            g.push(part("led", cube(0.06, 0.06, 0.02, -0.75, y, 1.225), material));
        }
    }
    g.push(part("door", cube(1.98, 3.5, 0.03, 0.0, 2.15, 1.24), M::Glass));
    for x in [-1.03, 1.03] {
        g.push(part("post", cube(0.08, 3.6, 0.06, x, 2.15, 1.18), M::GraphiteDark));
    }
    g.push(part("door handle", cube(0.05, 0.9, 0.06, 0.85, 2.15, 1.28), M::AluminiumDark));
    g.push(status_strip(1.6, 0.06, 0.0, 0.3, 1.18));
    g.push(plate(0.9, -0.45, 4.12, 1.155));
    for x in [-0.95, 0.95] {
        for z in [-0.9, 0.9] {
            g.push(part("foot", cyl(0.09, 0.11, 0.06, 24, x, 0.03, z), M::Rubber));
        }
    }
    g
}

pub fn server() -> Scene {
    let [w, h, d] = SERVER;
    scene(
        "server",
        SERVER,
        finish(server_parts(true), SERVER),
        finish(server_parts(false), SERVER),
        finish(block(w, h, d, M::Graphite), SERVER),
    )
}

fn server_parts(full: bool) -> Vec<Node> {
    let mut g = vec![
        part("body", rbox(3.5, 0.95, 2.6, 0.05, 0.03, 0.0, 0.475, 0.0), M::Graphite),
        part("bezel", cube(3.24, 0.74, 0.04, 0.0, 0.475, 1.31), M::Bezel),
    ];
    for r in 0..2u32 {
        for c in 0..6u32 {
            let x = -1.5 + 0.2 + c as f32 * 0.37;
            let y = 0.3 + r as f32 * 0.29;
            g.push(part("bay", cube(0.32, 0.23, 0.05, x, y, 1.335), M::Bay));
            if full {
                g.push(part("handle", cube(0.22, 0.03, 0.03, x - 0.02, y - 0.06, 1.37), M::AluminiumDark));
                let material = if (r + c) % 4 == 3 { M::Polymer } else { M::Led };
                g.push(part("led", cube(0.03, 0.03, 0.01, x + 0.12, y + 0.07, 1.365), material));
            }
        }
    }
    g.push(part("grille", cube(0.78, 0.56, 0.02, 1.12, 0.5, 1.34), M::Grille));
    for s in [-1.0, 1.0] {
        g.push(part("ear", cube(0.12, 0.95, 0.36, s * 1.81, 0.475, 1.12), M::Polymer));
        if full {
            g.push(part("screw", cyl_z(0.035, 0.02, 12, s * 1.81, 0.25, 1.31), M::AluminiumDark));
            g.push(part("screw", cyl_z(0.035, 0.02, 12, s * 1.81, 0.7, 1.31), M::AluminiumDark));
        }
    }
    g.push(status_strip(3.0, 0.05, 0.0, 0.14, 1.345));
    g.push(part("seam", cube(0.02, 0.012, 2.4, 0.4, 0.955, 0.0), M::Bezel));
    g.push(plate(0.7, -1.2, 0.8, 1.335));
    g
}

pub fn rack() -> Scene {
    let [w, h, d] = RACK;
    scene(
        "rack",
        RACK,
        finish(rack_parts(true), RACK),
        finish(rack_parts(false), RACK),
        finish(block(w, h, d, M::Polymer), RACK),
    )
}

fn rack_parts(full: bool) -> Vec<Node> {
    let mut g = Vec::new();
    for (px, pz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        g.push(part("post", cube(0.12, 3.7, 0.12, px * 1.24, 2.0, pz * 1.14), M::Polymer));
        // A caster: a cylinder turned onto its side, so the wheel runs along x.
        let wheel = mesh::cylinder(0.1, 0.1, 0.08, 18)
            .rotated(Quat::from_rotation_z(FRAC_PI_2))
            .translated(Vec3::new(px * 1.1, 0.08, pz * 1.0));
        g.push(part("wheel", wheel, M::Rubber));
    }
    g.push(part("base", rbox(2.62, 0.1, 2.42, 0.04, 0.02, 0.0, 0.2, 0.0), M::Polymer));
    g.push(part("top", rbox(2.62, 0.12, 2.42, 0.04, 0.02, 0.0, 3.86, 0.0), M::Polymer));
    // Not in the reference: the engine's tests need a status strip on every level, so the rack's top front carries one.
    g.push(status_strip(2.2, 0.06, 0.0, 3.86, 1.22));
    for k in 0..6u32 {
        let y = 0.52 + k as f32 * 0.54;
        g.push(part("unit", rbox(2.24, 0.44, 2.1, 0.03, 0.02, 0.0, y, 0.0), M::Graphite));
        g.push(part("bezel", cube(2.1, 0.32, 0.03, 0.0, y, 1.06), M::Bezel));
        g.push(slot_light(k + 1, 0.8, -0.55, y, 1.085));
        if full {
            g.push(part("grille", cube(0.62, 0.22, 0.02, 0.62, y, 1.08), M::Grille));
        }
        g.push(plate(0.36, -0.05, y, 1.085));
    }
    g
}
