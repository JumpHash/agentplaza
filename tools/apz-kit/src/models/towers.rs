//! The kit's towers.

use glam::Vec3;

use super::common::*;
use crate::glb::{Node, Scene};

/// A model's scene from its full parts and its simple parts. Its block (lod2) is one box of its size, and the anchors
/// (the name tag above it, the ports behind it) go in every level.
fn model(kind: &str, size: [f32; 3], full: Vec<Node>, simple: Vec<Node>) -> Scene {
    let [w, h, d] = size;
    let with_anchors = |mut nodes: Vec<Node>| {
        nodes.push(anchor("label", 0.0, h + 0.3, 0.0));
        nodes.push(anchor("port", 0.0, h / 2.0, -d / 2.0));
        nodes
    };
    scene(
        kind,
        size,
        with_anchors(full),
        with_anchors(simple),
        with_anchors(block(w, h, d, M::Graphite)),
    )
}

/// The GPU node: a card in a graphite case with a front grille, and two shrouds on its right side, each with a fan
/// facing +x and a status bar. The full model spins the side fans' rotors. The simple one drops those rotors and the
/// side fans, and keeps the front fans as static rings.
fn gpu_parts(full: bool) -> Vec<Node> {
    let mut parts = vec![
        part("base", rbox(3.4, 0.08, 3.0, 0.06, 0.02, 0.0, 0.04, 0.0), M::Graphite),
        part("top", rbox(3.4, 0.1, 3.0, 0.06, 0.03, 0.0, 1.85, 0.0), M::Graphite),
        part("side", cube(0.08, 1.75, 3.0, -1.66, 0.95, 0.0), M::Graphite),
        part("back", cube(3.32, 1.75, 0.08, 0.0, 0.95, -1.46), M::GraphiteDark),
        part("interior", cube(3.2, 1.7, 2.8, -0.08, 0.95, -0.02), M::Interior),
        part("grille", cube(3.3, 1.72, 0.05, 0.0, 0.95, 1.48), M::Grille),
        part("glass", cube(0.03, 1.72, 2.92, 1.7, 0.95, 0.0), M::Glass),
        status_strip(3.0, 0.05, 0.0, 0.14, 1.52),
        plate(0.6, -1.25, 1.68, 1.515),
    ];
    for f in 0..3 {
        let mut mount = fan("fan_mount", 0.44, -1.05 + f as f32 * 1.05, 1.0, 1.53, false);
        if !full {
            mount.children.retain(|c| c.name != "fan");
        }
        parts.push(mount);
    }
    for k in 0..2 {
        let y = 0.5 + k as f32 * 0.72;
        parts.push(part("shroud", rbox(0.2, 0.56, 2.4, 0.04, 0.02, 1.52, y, 0.0), M::Shroud));
        if full {
            parts.push(fan("fan_mount", 0.21, 1.63, y, -0.55, true));
            parts.push(fan("fan_mount", 0.21, 1.63, y, 0.55, true));
        }
        parts.push(part("status", cube(0.02, 0.05, 0.6, 1.63, y + 0.24, 0.0), M::Status));
    }
    parts
}

pub fn gpu() -> Scene {
    model("gpu", [3.4, 1.9, 3.0], gpu_parts(true), gpu_parts(false))
}

/// The tower: a graphite case with a front grille, a glass side panel and a status bar down its front. The full model
/// adds the interior, the shroud and the knob on top; the simple one keeps the body, the grille, the glass, the status
/// bar and the feet.
fn tower_parts(full: bool) -> Vec<Node> {
    let mut parts = vec![
        part("body", rbox(1.5, 3.3, 3.0, 0.08, 0.04, 0.0, 1.7, 0.0), M::Graphite),
        part("grille", cube(1.24, 2.95, 0.03, 0.0, 1.72, 1.505), M::Grille),
        status_strip(0.04, 2.6, 0.66, 1.72, 1.52),
        part("glass", cube(0.03, 2.7, 2.5, 0.77, 1.75, 0.0), M::Glass),
    ];
    for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        parts.push(part("foot", cube(0.3, 0.06, 0.4, sx * 0.5, 0.03, sz * 1.15), M::Rubber));
    }
    if full {
        parts.push(part("interior", cube(0.02, 2.6, 2.4, 0.745, 1.75, 0.0), M::Interior));
        parts.push(part("shroud", cube(0.02, 0.5, 1.6, 0.73, 1.4, 0.1), M::Shroud));
        parts.push(part("knob", cyl(0.07, 0.07, 0.04, 24, -0.45, 3.37, 1.1), M::AluminiumDark));
    }
    parts
}

pub fn tower() -> Scene {
    model("tower", [1.5, 3.4, 3.0], tower_parts(true), tower_parts(false))
}

/// The edge box: a low graphite case with vent fins on top, a status bar on its front, and an antenna at the back right.
/// The full model adds the antenna's tip and three ports; the simple one drops them.
fn edge_parts(full: bool) -> Vec<Node> {
    let mut parts = vec![
        part("body", rbox(2.2, 0.7, 2.2, 0.12, 0.04, 0.0, 0.35, 0.0), M::Graphite),
        part("antenna", cyl(0.035, 0.045, 1.0, 14, 0.85, 1.2, -0.8), M::Polymer),
        status_strip(1.0, 0.05, -0.4, 0.2, 1.105),
    ];
    for i in 0..11 {
        parts.push(part("vent", cube(0.06, 0.22, 1.9, -0.95 + i as f32 * 0.19, 0.81, 0.0), M::GraphiteDark));
    }
    if full {
        let tip = crate::mesh::sphere(0.06, 16, 12).translated(Vec3::new(0.85, 1.72, -0.8));
        parts.push(part("antenna_tip", tip, M::Polymer));
        for p in 0..3 {
            let x = 0.35 + p as f32 * 0.25;
            parts.push(part("port", rbox(0.16, 0.08, 0.02, 0.02, 0.004, x, 0.4, 1.105), M::Bezel));
        }
    }
    parts
}

pub fn edge() -> Scene {
    model("edge", [2.2, 1.78, 2.2], edge_parts(true), edge_parts(false))
}
