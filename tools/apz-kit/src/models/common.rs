//! What every model is made of: the kit's materials (in a fixed order, so a model refers to one by its index), the
//! named parts the engine drives (the status strip, fans, VM slot lights, the screen, the name plate, the anchors)
//! and the scene that holds the three levels of detail.

use glam::{Mat4, Quat, Vec3};
use serde_json::json;

use crate::glb::{Material, Node, Scene};
use crate::mesh::{self, Mesh};

/// The kit's materials, in the order of [`materials`]: `M::Graphite as usize` is its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum M {
    Graphite,
    GraphiteDark,
    Aluminium,
    AluminiumDark,
    Polymer,
    Bezel,
    Bay,
    Grille,
    Interior,
    Shroud,
    Blade,
    Glass,
    Rubber,
    Status,
    Led,
    Signal,
    Screen,
    Plate,
}

/// The texture file the grille material names (written beside the models by `apz-kit build`).
pub const GRILLE_TEXTURE: &str = "grille.ktx2";

/// The kit's materials with neutral base colours (linear RGB). The engine recolours each by name per style.
pub fn materials() -> Vec<Material> {
    // name, sRGB hex, metallic, roughness, emissive, transparent
    let table: [(&str, u32, f32, f32, bool, bool); 18] = [
        ("graphite", 0x3c4762, 0.45, 0.55, false, false),
        ("graphite_dark", 0x2b3349, 0.4, 0.6, false, false),
        ("aluminium", 0xd7dbe2, 1.0, 0.34, false, false),
        ("aluminium_dark", 0x9ea6b3, 1.0, 0.4, false, false),
        ("polymer", 0x1c2029, 0.0, 0.78, false, false),
        ("bezel", 0x12151d, 0.15, 0.62, false, false),
        ("bay", 0x252b3a, 0.3, 0.5, false, false),
        ("grille", 0x1a1f2b, 0.4, 0.7, false, false),
        ("interior", 0x0a0d14, 0.0, 0.9, false, false),
        ("shroud", 0x4a5572, 0.7, 0.35, false, false),
        ("blade", 0x2a303d, 0.1, 0.5, false, false),
        ("glass", 0x9db6dc, 0.0, 0.05, false, true),
        ("rubber", 0x0d0f14, 0.0, 0.95, false, false),
        ("status", 0xff7a2e, 0.0, 0.5, true, false),
        ("led", 0x5fe0a0, 0.0, 0.5, true, false),
        ("signal", 0xff7a2e, 0.0, 0.5, true, false),
        ("screen", 0x0b1020, 0.0, 0.3, false, false),
        ("plate", 0x0c0f16, 0.0, 0.6, false, false),
    ];
    table
        .iter()
        .map(|&(name, hex, metallic, roughness, emissive, transparent)| {
            let c = linear(hex);
            Material {
                name: name.to_string(),
                base: if emissive {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    [c[0], c[1], c[2], if transparent { 0.22 } else { 1.0 }]
                },
                emissive: if emissive { c } else { [0.0; 3] },
                metallic,
                roughness,
                texture: (name == "grille").then(|| GRILLE_TEXTURE.to_string()),
                double_sided: matches!(name, "blade" | "glass" | "screen" | "plate"),
                transparent,
            }
        })
        .collect()
}

/// An sRGB hex colour in linear light.
pub fn linear(hex: u32) -> [f32; 3] {
    let channel = |v: u32| {
        let c = (v & 0xff) as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(hex >> 16), channel(hex >> 8), channel(hex)]
}

/// A static part: `mesh` (already in the model's frame) drawn with `m`.
pub fn part(name: &str, mesh: Mesh, m: M) -> Node {
    Node::mesh(name, mesh, m as usize)
}

/// A rounded box (see [`mesh::rounded_box`]) centred at (`x`, `y`, `z`).
pub fn rbox(w: f32, h: f32, d: f32, r: f32, bevel: f32, x: f32, y: f32, z: f32) -> Mesh {
    mesh::rounded_box(w, h, d, r, bevel).translated(Vec3::new(x, y, z))
}

/// A plain box centred at (`x`, `y`, `z`).
pub fn cube(w: f32, h: f32, d: f32, x: f32, y: f32, z: f32) -> Mesh {
    mesh::cuboid(w, h, d).translated(Vec3::new(x, y, z))
}

/// A cylinder along y centred at (`x`, `y`, `z`).
pub fn cyl(r_top: f32, r_bottom: f32, h: f32, segments: u32, x: f32, y: f32, z: f32) -> Mesh {
    mesh::cylinder(r_top, r_bottom, h, segments).translated(Vec3::new(x, y, z))
}

/// A cylinder along z (a knob, a screw head or a port seen from the front) centred at (`x`, `y`, `z`).
pub fn cyl_z(r: f32, h: f32, segments: u32, x: f32, y: f32, z: f32) -> Mesh {
    mesh::cylinder(r, r, h, segments)
        .rotated(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
        .translated(Vec3::new(x, y, z))
}

/// The status strip: an emissive bar `w` wide and `h` tall on a front face, centred at (`x`, `y`, `z`). The engine colours
/// it by the machine's status. A model may have several (`status` is matched by name prefix).
pub fn status_strip(w: f32, h: f32, x: f32, y: f32, z: f32) -> Node {
    part("status", cube(w, h, 0.02, x, y, z), M::Status)
}

/// A fan facing +z (or +x with `face_x`) at (`x`, `y`, `z`): its frame ring and backing are static; its rotor (seven
/// pitched blades and a hub) is the child node `fan`, which the engine spins about its local z axis. `name` names the
/// mount.
pub fn fan(name: &str, r: f32, x: f32, y: f32, z: f32, face_x: bool) -> Node {
    let mut frame = mesh::torus(r * 0.94, r * 0.07, 10, 40);
    frame.append(&mesh::ring(r * 0.2, r * 0.9, 40).translated(Vec3::new(0.0, 0.0, -0.03)));
    let mut rotor = Mesh::default();
    for i in 0..7 {
        let blade = mesh::cuboid(r * 0.62, r * 0.26, 0.012)
            .rotated(Quat::from_rotation_x(0.55))
            .translated(Vec3::new(r * 0.5, 0.0, 0.0))
            .rotated(Quat::from_rotation_z(i as f32 / 7.0 * std::f32::consts::TAU));
        rotor.append(&blade);
    }
    let hub = cyl_z(r * 0.2, 0.06, 24, 0.0, 0.0, 0.0);
    let rotation = if face_x {
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
    } else {
        Quat::IDENTITY
    };
    let mut mount = Node::group(name).at(x, y, z);
    mount.rotation = rotation.to_array();
    mount
        .with(part("fan_frame", frame, M::Polymer))
        .with(Node::group("fan").with(part("fan_blades", rotor, M::Blade)).with(part("fan_hub", hub, M::Polymer)))
}

/// A VM slot's light `n` (1-based): an emissive bar the engine lights by the slot's state.
pub fn slot_light(n: u32, w: f32, x: f32, y: f32, z: f32) -> Node {
    part(&format!("slot:{n}"), cube(w, 0.05, 0.02, x, y, z), M::Led)
}

/// The name plate: a `w` by `w / 4` plane facing +z at (`x`, `y`, `z`); the engine paints the machine's name on it.
pub fn plate(w: f32, x: f32, y: f32, z: f32) -> Node {
    part("plate", mesh::plane(w, w / 4.0).translated(Vec3::new(x, y, z)), M::Plate)
}

/// The screen: a `w` by `h` plane carried by `m` (from a plane facing +z centred on the origin); the engine paints the
/// agent console on it, or leaves it dark.
pub fn screen(w: f32, h: f32, m: Mat4) -> Node {
    part("screen", mesh::plane(w, h).transformed(m), M::Screen)
}

/// An empty anchor node: `anchor:label` (where the name tag sits) or `anchor:port`.
pub fn anchor(name: &str, x: f32, y: f32, z: f32) -> Node {
    Node::group(&format!("anchor:{name}")).at(x, y, z)
}

/// The block level of detail: one box `w` × `h` × `d` standing on y = 0 in `m`, and the status strip across its front.
pub fn block(w: f32, h: f32, d: f32, m: M) -> Vec<Node> {
    vec![
        part("body", cube(w, h, d, 0.0, h / 2.0, 0.0), m),
        status_strip(w * 0.86, 0.06, 0.0, h.min(0.4) / 2.0, d / 2.0 + 0.011),
    ]
}

/// The scene of one model: `kind`, its size (w, h, d in kit units), and its three levels of detail as the roots `lod0`,
/// `lod1` and `lod2`. Anchors go in every level (pass them in each list).
pub fn scene(kind: &str, size: [f32; 3], lod0: Vec<Node>, lod1: Vec<Node>, lod2: Vec<Node>) -> Scene {
    let level = |name: &str, nodes: Vec<Node>| {
        let mut root = Node::group(name);
        root.children = nodes;
        root
    };
    Scene {
        name: kind.to_string(),
        materials: materials(),
        roots: vec![level("lod0", lod0), level("lod1", lod1), level("lod2", lod2)],
        extras: json!({ "kind": kind, "size": size }),
    }
}
