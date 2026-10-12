//! `floor3d/outskirts.ts`: the land round the districts. A lamp along every path, then two neat
//! rings of toy sci-fi pieces (habitat domes, comm towers, glowing groves, landing pads with a
//! shuttle, cargo stacks, solar arrays), each turned to face the plaza and kept clear of the
//! plaza, the slabs, the paths and each other; and the backdrop on the horizon (mesas, lit city
//! towers, mounds in the middle distance). Toon-shaded like the rest of the Floor; every solid
//! piece reports its ground footprint.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

use glam::{Mat4, Vec3};

use super::geometry;
use super::props::{Box2, PropPalette, Site};
use super::scene::{Color, Geo, Kind, Mat, Material, Obj};
use super::textures::Rng;
use super::toon::{outline, toon_mat};

/// The lamp colour (`LAMP` in props.ts).
const LAMP: u32 = 0x7fe8ff;

/// The five accents of the toy kit.
const ACCENTS: [u32; 5] = [0xff7a6b, 0x2fd6c4, 0x5fb8ff, 0x9a7dff, 0xffc75f];

/// `part(parent, geo, mat, x, y, z)`: a mesh placed under `parent`.
fn part(parent: &Obj, geo: &Geo, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_position(x, y, z);
    parent.add(&m);
    m
}

/// `shadows(root)`: every mesh under `root` casts and receives shadows.
fn shadows(root: &Obj) {
    let children = {
        let mut n = root.get();
        if matches!(n.kind, Kind::Mesh { .. }) {
            n.cast_shadow = true;
            n.receive_shadow = true;
        }
        n.children.clone()
    };
    for c in &children {
        shadows(c);
    }
}

/// The footprint of `o`'s subtree, its parent's world matrix being `parent`.
fn footprint(o: &Obj, parent: Mat4) -> Box2 {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    expand_box(o, parent, &mut lo, &mut hi);
    Box2 { x0: lo.x, x1: hi.x, z0: lo.z, z1: hi.z }
}

fn expand_box(o: &Obj, parent: Mat4, lo: &mut Vec3, hi: &mut Vec3) {
    let world = parent * o.local_matrix();
    let (geo, children) = {
        let n = o.get();
        let geo = match &n.kind {
            Kind::Mesh { geo, .. } => Some(Arc::clone(geo)),
            _ => None,
        };
        (geo, n.children.clone())
    };
    if let Some(geo) = geo {
        let mut bmin = Vec3::splat(f32::INFINITY);
        let mut bmax = Vec3::splat(f32::NEG_INFINITY);
        for p in &geo.positions {
            let v = Vec3::from(*p);
            bmin = bmin.min(v);
            bmax = bmax.max(v);
        }
        if bmin.x <= bmax.x {
            for corner in 0..8 {
                let c = Vec3::new(
                    if corner & 1 == 0 { bmin.x } else { bmax.x },
                    if corner & 2 == 0 { bmin.y } else { bmax.y },
                    if corner & 4 == 0 { bmin.z } else { bmax.z },
                );
                let w = world.transform_point3(c);
                *lo = lo.min(w);
                *hi = hi.max(w);
            }
        }
    }
    for c in &children {
        expand_box(c, world, lo, hi);
    }
}

/// Distance from (px, pz) to the segment from the origin to (bx, bz).
fn seg_dist(px: f32, pz: f32, bx: f32, bz: f32) -> f32 {
    let len2 = (bx * bx + bz * bz).max(1e-6);
    let t = ((px * bx + pz * bz) / len2).clamp(0.0, 1.0);
    (px - bx * t).hypot(pz - bz * t)
}

/// `glowing(color, intensity)`: a toon material whose emissive is its colour.
fn glowing(color: Color, intensity: f32) -> Mat {
    toon_mat(Material { color, emissive: color, emissive_intensity: intensity, ..Material::default() })
}

/// The toy kit: white shells, a dark trim, the five accents and their glowing twins.
struct Kit {
    shell: Mat,
    trim: Mat,
    steel: Mat,
    soil: Mat,
    accent: Vec<Mat>,
    neon: Vec<Mat>,
    crowns: Vec<Mat>,
    warm: Mat,
    glass: Mat,
    panel: Mat,
    bulb: Mat,
}

fn kit() -> Kit {
    Kit {
        shell: Mat::toon(Color::hex(0xe4e8f6)),
        trim: Mat::toon(Color::hex(0x2a3050)),
        steel: Mat::toon(Color::hex(0x8792b8)),
        soil: Mat::toon(Color::hex(0x1d3a46)),
        accent: ACCENTS.iter().map(|&c| Mat::toon(Color::hex(c))).collect(),
        neon: ACCENTS.iter().map(|&c| Mat::basic(Color::hex(c))).collect(),
        crowns: [0x2fd6c4, 0x5fe0a0, 0xff8fd8, 0x7fc8ff]
            .iter()
            .map(|&c| glowing(Color::hex(c), 0.35))
            .collect(),
        warm: Mat::basic(Color::hex(0xffd27a)),
        glass: toon_mat(Material {
            color: Color::hex(0x7fe3ff),
            emissive: Color::hex(0x2ab8ff),
            emissive_intensity: 0.6,
            ..Material::default()
        }),
        panel: toon_mat(Material {
            color: Color::hex(0x2b3f9a),
            emissive: Color::hex(0x1a2a80),
            emissive_intensity: 0.4,
            ..Material::default()
        }),
        bulb: Mat::basic(Color::hex(LAMP)),
    }
}

/// `place(x, z)`: a piece's node at (x, z), facing the plaza (its front is +z).
fn place(g: &Obj, x: f32, z: f32) -> Obj {
    let node = Obj::group();
    node.set_position(x, 0.0, z);
    node.set_rotation(0.0, (-x).atan2(-z), 0.0);
    g.add(&node);
    node
}

/// A torus lying flat (`flat(r, tube, seg)` in outskirts.ts).
fn flat(r: f32, tube: f32, seg: u32) -> Geo {
    geometry::transformed(&geometry::torus(r, tube, 8, seg, TAU), Mat4::from_rotation_x(FRAC_PI_2))
}

/// Whether a piece of radius `rad` at (x, z) clears the plaza, the districts, the paths and the
/// boxes so far.
fn clear(sites: &[Site], boxes: &[Box2], x: f32, z: f32, rad: f32) -> bool {
    if x.hypot(z) < 6.4 + rad {
        return false;
    }
    for s in sites {
        if (x - s.cx).abs() < s.r + 3.0 + rad && (z - s.cy).abs() < s.r + 3.0 + rad {
            return false;
        }
        if seg_dist(x, z, s.cx, s.cy) < 1.4 + rad {
            return false;
        }
    }
    for b in boxes {
        if x + rad > b.x0 - 0.3 && x - rad < b.x1 + 0.3 && z + rad > b.z0 - 0.3 && z - rad < b.z1 + 0.3 {
            return false;
        }
    }
    true
}

/// A lamp: a slim pole with a round glowing head.
fn add_lamp(g: &Obj, kit: &Kit, lamps: &mut Vec<(f32, f32)>, boxes: &mut Vec<Box2>, x: f32, z: f32) {
    // Paths meet at the centre, so lamps of neighbouring paths can land on one spot: keep one.
    if lamps.iter().any(|&(lx, lz)| (lx - x).hypot(lz - z) < 3.0) {
        return;
    }
    lamps.push((x, z));
    let l = place(g, x, z);
    part(&l, &geometry::cylinder(0.16, 0.2, 0.14, 16), &kit.trim, 0.0, 0.07, 0.0);
    let pole = part(&l, &geometry::cylinder(0.04, 0.05, 2.1, 10), &kit.steel, 0.0, 1.1, 0.0);
    part(&l, &geometry::cylinder(0.17, 0.12, 0.08, 16), &kit.trim, 0.0, 2.18, 0.0);
    part(&l, &geometry::sphere(0.16, 16, 12), &kit.bulb, 0.0, 2.34, 0.0);
    boxes.push(footprint(&pole, l.local_matrix()));
}

// The pieces, each built round its node's origin, facing +z; `k` picks the accent.

fn dome(n: &Obj, k: usize, kit: &Kit) {
    part(n, &geometry::cylinder(1.75, 1.85, 0.24, 40), &kit.trim, 0.0, 0.12, 0.0);
    part(n, &geometry::sphere_part(1.6, 36, 18, 0.0, TAU, 0.0, FRAC_PI_2), &kit.shell, 0.0, 0.24, 0.0);
    part(n, &flat(1.61, 0.07, 48), &kit.neon[k], 0.0, 0.42, 0.0);
    part(n, &geometry::rounded_box(0.72, 0.95, 0.6, 3, 0.16), &kit.accent[k], 0.0, 0.6, 1.38);
    part(n, &geometry::rounded_box(0.4, 0.5, 0.05, 2, 0.08), &kit.trim, 0.0, 0.62, 1.69);
    for side in [-1.0_f32, 1.0] {
        part(n, &geometry::sphere(0.17, 14, 10), &kit.warm, side * 0.8, 1.05, 1.08);
    }
    part(n, &geometry::cylinder(0.03, 0.03, 0.6, 6), &kit.steel, 0.55, 1.95, -0.3);
    part(n, &geometry::sphere(0.09, 10, 8), &kit.neon[k], 0.55, 2.28, -0.3);
}

fn tower(n: &Obj, k: usize, kit: &Kit) {
    part(n, &geometry::rounded_box(1.3, 0.4, 1.3, 3, 0.14), &kit.trim, 0.0, 0.2, 0.0);
    part(n, &geometry::cylinder(0.42, 0.56, 2.4, 24), &kit.shell, 0.0, 1.6, 0.0);
    part(n, &flat(0.5, 0.06, 32), &kit.neon[k], 0.0, 1.1, 0.0);
    part(n, &flat(0.46, 0.06, 32), &kit.neon[k], 0.0, 2.2, 0.0);
    part(n, &geometry::cylinder(0.3, 0.42, 1.2, 24), &kit.shell, 0.0, 3.4, 0.0);
    part(n, &geometry::sphere(0.42, 20, 14), &kit.accent[k], 0.0, 4.2, 0.0);
    let dish = part(n, &geometry::sphere_part(0.55, 20, 10, 0.0, TAU, 0.0, PI * 0.32), &kit.steel, 0.0, 4.25, 0.3);
    dish.get().rotation.x = 1.0;
    part(n, &geometry::cylinder(0.025, 0.025, 0.7, 6), &kit.steel, 0.0, 4.9, 0.0);
    part(n, &geometry::sphere(0.08, 10, 8), &kit.warm, 0.0, 5.28, 0.0);
}

fn grove(n: &Obj, k: usize, kit: &Kit) {
    part(n, &geometry::cylinder(1.25, 1.3, 0.36, 36), &kit.shell, 0.0, 0.18, 0.0);
    part(n, &flat(1.27, 0.05, 48), &kit.neon[k], 0.0, 0.3, 0.0);
    part(n, &geometry::cylinder(1.12, 1.12, 0.04, 36), &kit.soil, 0.0, 0.37, 0.0);
    for i in 0..3usize {
        let a = (i as f32 / 3.0) * TAU + k as f32;
        let h = 1.0 + ((i + k) % 3) as f32 * 0.3;
        let x = a.cos() * 0.55;
        let z = a.sin() * 0.55;
        part(n, &geometry::cylinder(0.06, 0.08, h, 8), &kit.trim, x, 0.37 + h / 2.0, z);
        let crown = &kit.crowns[(i + k) % kit.crowns.len()];
        part(n, &geometry::sphere(0.42 + (i % 2) as f32 * 0.12, 18, 14), crown, x, 0.37 + h + 0.3, z);
    }
}

fn pad(n: &Obj, k: usize, kit: &Kit) {
    part(n, &geometry::cylinder(2.0, 2.1, 0.16, 48), &kit.trim, 0.0, 0.08, 0.0);
    part(n, &flat(1.62, 0.05, 64), &kit.neon[k], 0.0, 0.17, 0.0);
    for rot in [0.6_f32, -0.6] {
        let stripe = part(n, &geometry::box_geo(2.2, 0.02, 0.12), &kit.neon[k], 0.0, 0.17, 0.0);
        stripe.set_rotation(0.0, rot, 0.0);
    }
    // A little shuttle: a round-ended body along z, wings, a fin, a glass canopy, three legs.
    let ship = Obj::group();
    ship.set_position(0.0, 0.72, 0.0);
    ship.set_rotation(0.0, 0.5, 0.0);
    n.add(&ship);
    let body = geometry::transformed(&geometry::cylinder(0.42, 0.42, 1.3, 24), Mat4::from_rotation_x(FRAC_PI_2));
    part(&ship, &body, &kit.shell, 0.0, 0.0, 0.0);
    part(&ship, &geometry::sphere(0.42, 24, 16), &kit.shell, 0.0, 0.0, 0.65);
    part(&ship, &geometry::sphere(0.4, 24, 16), &kit.accent[k], 0.0, 0.0, -0.65);
    part(&ship, &geometry::sphere(0.26, 18, 12), &kit.glass, 0.0, 0.22, 0.6);
    part(&ship, &geometry::rounded_box(1.9, 0.08, 0.55, 2, 0.04), &kit.accent[k], 0.0, -0.08, -0.1);
    part(&ship, &geometry::rounded_box(0.08, 0.5, 0.45, 2, 0.04), &kit.accent[k], 0.0, 0.42, -0.55);
    for (lx, lz) in [(-0.35_f32, -0.3_f32), (0.35, -0.3), (0.0, 0.45)] {
        part(&ship, &geometry::cylinder(0.04, 0.05, 0.5, 6), &kit.steel, lx, -0.42, lz);
    }
}

fn crates(n: &Obj, k: usize, kit: &Kit) {
    let box_geo = geometry::rounded_box(0.8, 0.8, 0.8, 3, 0.1);
    let band = geometry::box_geo(0.84, 0.12, 0.84);
    let stack: [(f32, f32, f32, usize); 4] = [(-0.45, 0.4, 0.0, 0), (0.45, 0.4, 0.05, 1), (0.0, 1.2, 0.0, 2), (0.2, 0.4, -0.9, 3)];
    for (x, y, z, i) in stack {
        let c = part(n, &box_geo, &kit.accent[(k + i) % kit.accent.len()], x, y, z);
        c.set_rotation(0.0, i as f32 * 0.25, 0.0);
        let b = part(n, &band, &kit.trim, x, y, z);
        b.set_rotation(0.0, i as f32 * 0.25, 0.0);
    }
}

fn solar(n: &Obj, k: usize, kit: &Kit) {
    part(n, &geometry::rounded_box(2.6, 0.2, 0.9, 2, 0.08), &kit.trim, 0.0, 0.1, 0.0);
    for x in [-0.9_f32, 0.0, 0.9] {
        part(n, &geometry::cylinder(0.04, 0.05, 0.7, 6), &kit.steel, x, 0.5, 0.0);
        let p = part(n, &geometry::rounded_box(0.8, 0.05, 1.0, 2, 0.03), &kit.panel, x, 0.9, 0.0);
        p.set_rotation(-0.55, 0.0, 0.0);
    }
    part(n, &geometry::sphere(0.08, 10, 8), &kit.neon[k], 1.2, 0.3, 0.38);
}

type Build = fn(&Obj, usize, &Kit);

/// A piece's footprint radius and its builder (`PIECES` in outskirts.ts).
fn piece(kind: &str) -> (f32, Build) {
    match kind {
        "dome" => (1.9, dome),
        "tower" => (0.9, tower),
        "grove" => (1.35, grove),
        "pad" => (2.1, pad),
        "crates" => (1.1, crates),
        "solar" => (1.4, solar),
        other => panic!("unknown outskirts piece: {other}"),
    }
}

/// The outskirts: a lamp along every path, then two neat rings of toy sci-fi pieces round the
/// districts, each turned to face the plaza and kept clear of the plaza, the slabs and their
/// ramps, the paths and each other. Returns the group and the footprints it reports.
pub fn build_scatter(sites: &[Site], _palette: &PropPalette) -> (Obj, Vec<Box2>) {
    let g = Obj::group();
    let kit = kit();
    let mut boxes: Vec<Box2> = Vec::new();
    let mut lamps: Vec<(f32, f32)> = Vec::new();

    let mut far = 10.0_f32;
    for s in sites {
        far = far.max(s.cx.hypot(s.cy) + s.r);
    }

    // One lamp per path, just off its edge where it reaches the district, sides alternating.
    for (i, s) in sites.iter().enumerate() {
        let dist = s.cx.hypot(s.cy);
        let d = dist - s.r - 2.6;
        if dist < 1.0 || d < 7.0 {
            continue;
        }
        let ux = s.cx / dist;
        let uz = s.cy / dist;
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        add_lamp(&g, &kit, &mut lamps, &mut boxes, ux * d - uz * side * 0.95, uz * d + ux * side * 0.95);
    }

    // Two rings of pieces round the districts, the outer one offset by half a step; a spot that
    // clashes with something is left empty.
    let rings: [(f32, f32, &[&str]); 2] = [
        (far + 5.5, 6.5, &["grove", "dome", "crates", "tower", "grove", "solar", "pad"]),
        (far + 14.0, 8.5, &["dome", "tower", "grove", "pad", "dome", "solar", "grove", "tower"]),
    ];
    for (ring, &(radius, step, kinds)) in rings.iter().enumerate() {
        let n = (TAU * radius / step).floor().max(8.0) as usize;
        for i in 0..n {
            let a = ((i as f32 + ring as f32 * 0.5) / n as f32) * TAU;
            let x = a.cos() * radius;
            let z = a.sin() * radius;
            let (rad, build) = piece(kinds[i % kinds.len()]);
            if !clear(sites, &boxes, x, z, rad) {
                continue;
            }
            let node = place(&g, x, z);
            build(&node, (i + ring * 2) % ACCENTS.len(), &kit);
            shadows(&node);
            boxes.push(Box2 { x0: x - rad, x1: x + rad, z0: z - rad, z1: z + rad });
        }
    }

    outline(&g, 0.1);
    (g, boxes)
}

/// The backdrop on the horizon: a ring of toy mesas and city towers far out (the fog softens them
/// into the violet haze), the towers lit by rows of windows and a warm light on top; and low
/// mounds in the middle distance.
pub fn build_backdrop() -> Obj {
    let g = Obj::group();
    let mut rand = Rng::new(4242);
    let rock = Mat::toon(Color::hex(0x3b3572));
    let cap = Mat::toon(Color::hex(0x4d4794));
    let block = Mat::toon(Color::hex(0x323a72));
    let roof = Mat::toon(Color::hex(0x434c8e));
    let lit: Vec<Mat> = [0xffd27a, 0x7fe8ff, 0xff9ad8].iter().map(|&c| Mat::basic(Color::hex(c))).collect();
    const N: usize = 34;
    for i in 0..N {
        let a = (i as f32 / N as f32) * TAU + rand.next() * 0.08;
        let radius = 120.0 + rand.next() * 40.0;
        let (px, pz) = (a.cos() * radius, a.sin() * radius);
        let node = Obj::group();
        node.set_position(px, 0.0, pz);
        node.set_rotation(0.0, (-px).atan2(-pz), 0.0);
        if i % 3 != 1 {
            // A mesa: a wide rounded hill with a lighter cap, sometimes a second, smaller one beside it.
            let w = 8.0 + rand.next() * 9.0;
            let h = 4.0 + rand.next() * 7.0;
            part(&node, &geometry::cylinder(w * 0.8, w, h, 28), &rock, 0.0, h / 2.0, 0.0);
            let cap_geo = geometry::transformed(
                &geometry::sphere_part(w * 0.8, 28, 10, 0.0, TAU, 0.0, FRAC_PI_2),
                Mat4::from_scale(Vec3::new(1.0, 0.25, 1.0)),
            );
            part(&node, &cap_geo, &cap, 0.0, h, 0.0);
            if rand.next() < 0.6 {
                let w2 = w * 0.55;
                let h2 = h * (0.5 + rand.next() * 0.4);
                part(&node, &geometry::cylinder(w2 * 0.8, w2, h2, 24), &rock, w * 1.1, h2 / 2.0, 2.0);
                let cap2 = geometry::transformed(
                    &geometry::sphere_part(w2 * 0.8, 24, 8, 0.0, TAU, 0.0, FRAC_PI_2),
                    Mat4::from_scale(Vec3::new(1.0, 0.25, 1.0)),
                );
                part(&node, &cap2, &cap, w * 1.1, h2, 2.0);
            }
        } else {
            // A cluster of city towers, rounded, with lit window rows on their fronts.
            let count = 2 + (rand.next() * 3.0).floor() as usize;
            for k in 0..count {
                let w = 3.0 + rand.next() * 3.0;
                let h = 12.0 + rand.next() * 22.0;
                let x = (k as f32 - (count as f32 - 1.0) / 2.0) * (w + 1.5);
                part(&node, &geometry::rounded_box(w, h, w, 3, 0.9), &block, x, h / 2.0, 0.0);
                part(&node, &geometry::rounded_box(w * 0.7, 1.4, w * 0.7, 3, 0.5), &roof, x, h + 0.7, 0.0);
                let light = &lit[(i + k) % lit.len()];
                let mut y = 3.0_f32;
                while y < h - 2.0 {
                    if rand.next() >= 0.25 {
                        part(&node, &geometry::box_geo(w * 0.62, 0.45, 0.1), light, x, y, w / 2.0 + 0.02);
                    }
                    y += 2.6;
                }
                part(&node, &geometry::cylinder(0.12, 0.12, 3.0, 6), &roof, x, h + 2.9, 0.0);
                part(&node, &geometry::sphere(0.45, 10, 8), &lit[0], x, h + 4.5, 0.0);
            }
        }
        g.add(&node);
    }

    // The middle distance: low rolling mounds, so the land between the outskirts and the horizon
    // is not bare.
    let mound = Mat::toon(Color::hex(0x2c3366));
    let mound_top = Mat::toon(Color::hex(0x353d76));
    for i in 0..40usize {
        let a = rand.next() * TAU;
        let radius = 55.0 + rand.next() * 45.0;
        let w = 3.0 + rand.next() * 5.0;
        let mound_geo = geometry::transformed(
            &geometry::sphere_part(w, 24, 12, 0.0, TAU, 0.0, FRAC_PI_2),
            Mat4::from_scale(Vec3::new(1.4, 0.4, 1.0)),
        );
        let mat = if i % 2 == 1 { &mound } else { &mound_top };
        let m = part(&g, &mound_geo, mat, a.cos() * radius, 0.0, a.sin() * radius);
        m.get().rotation.y = rand.next() * PI;
    }
    outline(&g, 2.0);
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scatter_places_pieces_and_lamps() {
        let sites: Vec<Site> = (0..3)
            .map(|i| {
                let a = i as f32 / 3.0 * TAU;
                Site { cx: 12.0 * a.cos(), cy: 12.0 * a.sin(), r: 3.0 }
            })
            .collect();
        let palette = PropPalette {
            sheet: Color::hex(0xffffff),
            rule: Color::hex(0x888888),
            ink3: Color::hex(0x444444),
            signal: Color::hex(0xff0000),
            ok: Color::hex(0x00ff00),
            info: Color::hex(0x0000ff),
        };
        let (_group, boxes) = build_scatter(&sites, &palette);
        assert!(!boxes.is_empty());
    }

    #[test]
    fn backdrop_has_children() {
        let g = build_backdrop();
        assert!(!g.get().children.is_empty());
    }
}
