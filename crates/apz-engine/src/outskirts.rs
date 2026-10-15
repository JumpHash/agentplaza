//! `floor3d/outskirts.ts`: the land round the districts. A lamp along every path, then two neat
//! rings of toy sci-fi pieces (habitat domes, comm towers, glowing groves, landing pads with a
//! shuttle, cargo stacks, solar arrays), each turned to face the plaza and kept clear of the
//! plaza, the slabs, the paths and each other; and the backdrop on the horizon (mesas, lit city
//! towers, mounds in the middle distance). Toon-shaded like the rest of the Floor; every solid
//! piece reports its ground footprint.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{Mat4, Vec3};

use super::geometry;
use super::scene::{Color, Geo, Mat, Obj};
use super::textures::Rng;
use super::toon::outline;

/// `part(parent, geo, mat, x, y, z)`: a mesh placed under `parent`.
fn part(parent: &Obj, geo: &Geo, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_position(x, y, z);
    parent.add(&m);
    m
}

// The pieces, each built round its node's origin, facing +z; `k` picks the accent.

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
    let lit: Vec<Mat> = [0xffd27a, 0x7fe8ff, 0xff9ad8]
        .iter()
        .map(|&c| Mat::basic(Color::hex(c)))
        .collect();
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
            part(
                &node,
                &geometry::cylinder(w * 0.8, w, h, 28),
                &rock,
                0.0,
                h / 2.0,
                0.0,
            );
            let cap_geo = geometry::transformed(
                &geometry::sphere_part(w * 0.8, 28, 10, 0.0, TAU, 0.0, FRAC_PI_2),
                Mat4::from_scale(Vec3::new(1.0, 0.25, 1.0)),
            );
            part(&node, &cap_geo, &cap, 0.0, h, 0.0);
            if rand.next() < 0.6 {
                let w2 = w * 0.55;
                let h2 = h * (0.5 + rand.next() * 0.4);
                part(
                    &node,
                    &geometry::cylinder(w2 * 0.8, w2, h2, 24),
                    &rock,
                    w * 1.1,
                    h2 / 2.0,
                    2.0,
                );
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
                part(
                    &node,
                    &geometry::rounded_box(w, h, w, 3, 0.9),
                    &block,
                    x,
                    h / 2.0,
                    0.0,
                );
                part(
                    &node,
                    &geometry::rounded_box(w * 0.7, 1.4, w * 0.7, 3, 0.5),
                    &roof,
                    x,
                    h + 0.7,
                    0.0,
                );
                let light = &lit[(i + k) % lit.len()];
                let mut y = 3.0_f32;
                while y < h - 2.0 {
                    if rand.next() >= 0.25 {
                        part(
                            &node,
                            &geometry::box_geo(w * 0.62, 0.45, 0.1),
                            light,
                            x,
                            y,
                            w / 2.0 + 0.02,
                        );
                    }
                    y += 2.6;
                }
                part(
                    &node,
                    &geometry::cylinder(0.12, 0.12, 3.0, 6),
                    &roof,
                    x,
                    h + 2.9,
                    0.0,
                );
                part(
                    &node,
                    &geometry::sphere(0.45, 10, 8),
                    &lit[0],
                    x,
                    h + 4.5,
                    0.0,
                );
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
