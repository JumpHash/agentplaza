//! `floor3d/props.ts`: the central plaza (paved ring, the xenon atom), its furniture (fountain,
//! market stalls), the paths with their street lamps, and the meadow's scatter (trees, flower
//! beds, benches, rocks). Every solid piece reports its ground footprint for collisions.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use glam::{Mat4, Vec3};

use super::geometry;
use super::scene::{Color, Geo, Geometry, Kind, Mat, Material, Obj, Shading, Side, Tex, Texture};
use super::textures::{self, Rng};
use super::toon::{outline, toon_mat};

/// The palette colours the props need.
#[derive(Debug, Clone, Copy)]
pub struct PropPalette {
    pub sheet: Color,
    pub rule: Color,
    pub ink3: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
}

/// A ground footprint, grid units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box2 {
    pub x0: f32,
    pub x1: f32,
    pub z0: f32,
    pub z1: f32,
}

/// A district as laid out: centre and slab half-width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Site {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

/// A per-frame animation: `t` seconds since start.
pub type Anim = Box<dyn FnMut(f32) + Send>;

/// Plaza furniture: its group, footprints and animation.
pub struct Furniture {
    pub group: Obj,
    pub boxes: Vec<Box2>,
    pub animate: Anim,
}

/// The street lamps' bulb colour.
pub const LAMP: u32 = 0x7fe8ff;
/// Dark hull metal, for benches, stalls and planters.
const HULL: u32 = 0x56607e;

/// A toon colour that also glows a little: the plaza's bioluminescence and crystals.
pub fn glowing(color: u32, intensity: f32) -> Mat {
    let c = Color::hex(color);
    toon_mat(Material { color: c, emissive: c, emissive_intensity: intensity, ..Material::default() })
}

/// `toonMat({ color, roughness, metalness })`: toon.ts drops roughness and metalness.
fn std_mat(color: Color) -> Mat {
    toon_mat(Material { color, ..Material::default() })
}

/// `toonMat({ map })`.
fn textured(map: Tex) -> Mat {
    toon_mat(Material { map: Some(map), ..Material::default() })
}

/// A CSS `rgb()` string for a colour (`getStyle()`).
fn css(c: Color) -> String {
    let [r, g, b] = c.to_srgb_u8();
    format!("rgb({r},{g},{b})")
}

/// `part(parent, geo, mat, x, y, z)`: a mesh placed under `parent`.
pub fn part(parent: &Obj, geo: &Geo, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_position(x, y, z);
    parent.add(&m);
    m
}

/// `shadows(root)`: every mesh under `root` casts and receives shadows.
pub fn shadows(root: &Obj) {
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

/// `traverse(o => o.castShadow = false)`.
fn no_cast(root: &Obj) {
    let children = {
        let mut n = root.get();
        n.cast_shadow = false;
        n.children.clone()
    };
    for c in &children {
        no_cast(c);
    }
}

/// The footprint of `o`'s subtree, its world matrix being `parent` times its own local matrix.
/// Like three's `Box3.setFromObject`, each mesh's local bounds are transformed corner by corner.
pub fn footprint(o: &Obj, parent: Mat4) -> Box2 {
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

/// The smallest angle between two bearings, in radians.
fn ang_dist(a: f32, b: f32) -> f32 {
    let d = a - b;
    d.sin().atan2(d.cos()).abs()
}

/// Distance from (px, pz) to the segment from the origin to (bx, bz).
pub fn seg_dist(px: f32, pz: f32, bx: f32, bz: f32) -> f32 {
    let len2 = (bx * bx + bz * bz).max(1e-6);
    let t = ((px * bx + pz * bz) / len2).clamp(0.0, 1.0);
    (px - bx * t).hypot(pz - bz * t)
}

/// A bench at the origin, its seat facing +z and its back at -z.
fn bench(mat: &Mat) -> Obj {
    let b = Obj::group();
    part(&b, &geometry::box_geo(1.3, 0.08, 0.42), mat, 0.0, 0.42, 0.0);
    part(&b, &geometry::box_geo(0.08, 0.4, 0.36), mat, -0.55, 0.2, 0.0);
    part(&b, &geometry::box_geo(0.08, 0.4, 0.36), mat, 0.55, 0.2, 0.0);
    part(&b, &geometry::box_geo(1.3, 0.4, 0.07), mat, 0.0, 0.66, -0.2);
    b
}

/// One electron shell of the xenon atom.
struct Shell {
    group: Obj,
    electrons: Vec<Obj>,
    r: f32,
    speed: f32,
}

/// `buildPlaza(palette)`: a round paved square with a darker ring, four benches and a slowly
/// turning hologram (the xenon atom) above the dais. The group is dynamic (the atom turns).
pub fn build_plaza(palette: &PropPalette) -> (Obj, Anim) {
    let g = Obj::group();
    let stone = palette.rule.lerp(palette.sheet, 0.4);
    let pave = part(
        &g,
        &geometry::cylinder(4.3, 4.45, 0.06, 64),
        &std_mat(stone),
        0.0,
        0.03,
        0.0,
    );
    pave.get().receive_shadow = true;
    let ring = part(&g, &geometry::ring(3.7, 3.95, 72, 1, 0.0, TAU), &std_mat(stone.scale(0.72)), 0.0, 0.066, 0.0);
    ring.set_rotation(-PI / 2.0, 0.0, 0.0);
    let inlay = part(&g, &geometry::torus(3.3, 0.025, 6, 72, TAU), &std_mat(palette.rule), 0.0, 0.065, 0.0);
    inlay.set_rotation(PI / 2.0, 0.0, 0.0);

    let bench_metal = toon_mat(Material {
        map: Some(textures::metal()),
        color: Color::hex(HULL),
        ..Material::default()
    });
    for i in 0..4 {
        let a = (i as f32 / 4.0) * PI * 2.0;
        let x = a.cos() * 3.6;
        let z = a.sin() * 3.6;
        let b = bench(&bench_metal);
        b.set_position(x, 0.0, z);
        b.set_rotation(0.0, (-x).atan2(-z), 0.0); // the seat faces the centre, the back faces out
        g.add(&b);
    }

    // A xenon atom above the orchestrator: a pulsing, glowing nucleus and its 54 electrons on five
    // shells (2, 8, 18, 18, 8), each shell tilted its own way, the inner ones spinning faster.
    let xenon = Color::hex(0x9fa8ff);
    let globe = Obj::group();
    let nucleus = Obj::group();
    let proton = toon_mat(Material {
        color: palette.signal,
        emissive: palette.signal,
        emissive_intensity: 0.45,
        ..Material::default()
    });
    let neutron = toon_mat(Material {
        color: palette.sheet,
        emissive: Color::hex(0xffe2b0),
        emissive_intensity: 0.25,
        ..Material::default()
    });
    let nucleon_geo = geometry::sphere(0.13, 14, 10);
    let mut nr = Rng::new(54);
    for i in 0..24 {
        let v = Vec3::new(nr.next() - 0.5, nr.next() - 0.5, nr.next() - 0.5).normalize() * (nr.next().cbrt() * 0.24);
        let m = if i % 2 == 1 { &proton } else { &neutron };
        part(&nucleus, &nucleon_geo, m, v.x, v.y, v.z);
    }
    globe.add(&nucleus);
    let halo_mat = Mat::new(Material {
        shading: Shading::Basic,
        color: xenon,
        transparent: true,
        opacity: 0.35,
        depth_write: false,
        additive: true,
        ..Material::default()
    });
    let halo = Obj::mesh(&geometry::sphere(0.62, 24, 16), &halo_mat);
    globe.add(&halo);
    let aura_mat = Mat::new(Material {
        shading: Shading::Basic,
        color: xenon,
        transparent: true,
        opacity: 0.1,
        depth_write: false,
        additive: true,
        ..Material::default()
    });
    let aura = Obj::mesh(&geometry::sphere(1.9, 24, 16), &aura_mat);
    globe.add(&aura);
    let glow = Obj::point_light(xenon, 2.2, 9.0, 1.6);
    globe.add(&glow);
    let ring_mat = toon_mat(Material {
        color: Color::hex(0xc9ccff),
        emissive: xenon,
        emissive_intensity: 0.6,
        ..Material::default()
    });
    let electron_mat = toon_mat(Material {
        color: Color::hex(0x8ff0ff),
        emissive: Color::hex(0x5fd8ff),
        emissive_intensity: 1.1,
        ..Material::default()
    });
    let electron_geo = geometry::sphere(0.065, 12, 8);
    let mut shells: Vec<Shell> = Vec::new();
    for (k, &n) in [2usize, 8, 18, 18, 8].iter().enumerate() {
        let r = 0.62 + k as f32 * 0.27;
        let shell = Obj::group();
        let (rx, ry, rz) = (nr.next() * PI, nr.next() * PI, nr.next() * PI);
        shell.set_rotation(rx, ry, rz);
        let track = Obj::mesh(&geometry::torus(r, 0.012, 6, 96, TAU), &ring_mat);
        track.set_rotation(PI / 2.0, 0.0, 0.0);
        track.no_outline(); // a fine glowing track, not an inked hoop
        shell.add(&track);
        let mut electrons = Vec::with_capacity(n);
        for _ in 0..n {
            electrons.push(part(&shell, &electron_geo, &electron_mat, 0.0, 0.0, 0.0));
        }
        globe.add(&shell);
        let speed = 1.9 / (1.0 + k as f32 * 0.45) * if k % 2 == 1 { -1.0 } else { 1.0 };
        shells.push(Shell { group: shell, electrons, r, speed });
    }
    outline(&globe, 0.05);
    globe.set_scalar(1.5);
    globe.set_position(0.0, 4.2, 0.0);
    g.add(&globe);

    let globe_anim = globe.clone();
    let animate: Anim = Box::new(move |t: f32| {
        globe_anim.set_position(0.0, 4.2 + (t * 0.9).sin() * 0.1, 0.0);
        // The nucleus beats, its halo and light breathing with it.
        let beat = (t * 2.4).sin().max(0.0).powf(6.0);
        nucleus.set_scalar(1.0 + 0.12 * beat);
        nucleus.set_rotation(0.0, t * 0.6, 0.0);
        halo.set_scalar(1.0 + 0.35 * beat + 0.05 * (t * 7.0).sin());
        halo_mat.get().opacity = 0.3 + 0.35 * beat;
        aura_mat.get().opacity = 0.07 + 0.08 * beat;
        aura.set_scalar(1.0 + 0.1 * beat);
        if let Kind::PointLight { intensity, .. } = &mut glow.get().kind {
            *intensity = 1.6 + 2.4 * beat;
        }
        for sh in shells.iter() {
            sh.group.get().rotation.y += 0.002;
            let n = sh.electrons.len() as f32;
            for (i, e) in sh.electrons.iter().enumerate() {
                let a = t * sh.speed + (i as f32 / n) * TAU;
                e.set_position(a.cos() * sh.r, 0.0, a.sin() * sh.r);
                e.set_scalar(1.0 + 0.25 * beat);
            }
        }
    });
    shadows(&g);
    no_cast(&globe);
    outline(&g, 0.12);
    g.get().dynamic = true;
    (g, animate)
}

/// `buildPlazaFurniture(palette, bearings)`: the fountain and market stalls on the plaza ring,
/// placed in the gaps between the paths.
pub fn build_plaza_furniture(palette: &PropPalette, bearings: &[f32]) -> Furniture {
    let g = Obj::group();
    let mut boxes: Vec<Box2> = Vec::new();
    const RING: f32 = 4.0;

    // Eight candidate spots around the ring, kept off the paths and spread at least two apart.
    let mut chosen: Vec<f32> = Vec::new();
    for k in 0..8 {
        let a = PI / 8.0 + (k as f32 * PI) / 4.0;
        if bearings.iter().any(|&b| ang_dist(a, b) < 0.3) {
            continue;
        }
        if chosen.iter().all(|&c| ang_dist(a, c) > 1.0) {
            chosen.push(a);
        }
    }

    let water_base: Tex = textures::water();
    let stone_mat = textured(textures::polish(palette.sheet));
    let water_mat = toon_mat(Material {
        map: Some(Arc::clone(&water_base)),
        emissive: Color::hex(0x2ad0ff),
        emissive_intensity: 0.55,
        transparent: true,
        opacity: 0.9,
        ..Material::default()
    });
    let counter_mat = toon_mat(Material {
        map: Some(textures::metal()),
        color: Color::hex(HULL),
        ..Material::default()
    });
    let pole_mat = textured(textures::metal());
    let awning_key = format!("awning:{}:{}", css(palette.signal), css(palette.sheet));
    let signal = palette.signal;
    let awning_tex = textures::canvas_texture(&awning_key, 256, 64, 1.0, |cg, w, h, _| {
        for i in 0..8 {
            let c = if i % 2 == 1 { signal } else { Color::hex(0xd8deef) };
            cg.set_fill(&css(c));
            cg.fill_rect(i as f32 * w / 8.0, 0.0, w / 8.0, h);
        }
    });
    let awning_mat = toon_mat(Material {
        map: Some(awning_tex),
        side: Side::Double,
        ..Material::default()
    });

    for (i, &a) in chosen.iter().enumerate() {
        let x = a.cos() * RING;
        let z = a.sin() * RING;
        let node = Obj::group();
        node.set_position(x, 0.0, z);
        if i == 0 {
            // The fountain: a basin with a glowing pool and a spout of light.
            let basin = part(&node, &geometry::cylinder(0.62, 0.7, 0.26, 32), &stone_mat, 0.0, 0.13, 0.0);
            let pool = part(&node, &geometry::circle(0.55, 32, 0.0, TAU), &water_mat, 0.0, 0.27, 0.0);
            pool.set_rotation(-PI / 2.0, 0.0, 0.0);
            part(&node, &geometry::cylinder(0.05, 0.07, 0.6, 8), &stone_mat, 0.0, 0.56, 0.0);
            part(&node, &geometry::sphere(0.11, 12, 8), &water_mat, 0.0, 0.92, 0.0);
            boxes.push(footprint(&basin, node.local_matrix()));
        } else {
            // A market stall: a metal counter under a striped awning, turned to face the plaza's centre.
            node.set_rotation(0.0, (-x).atan2(-z), 0.0);
            let counter = part(&node, &geometry::box_geo(1.1, 0.72, 0.5), &counter_mat, 0.0, 0.36, 0.0);
            part(&node, &geometry::box_geo(0.26, 0.18, 0.26), &std_mat(palette.ok), 0.25, 0.81, 0.0);
            part(&node, &geometry::box_geo(0.24, 0.16, 0.24), &std_mat(palette.signal), -0.3, 0.8, 0.05);
            for px in [-0.5f32, 0.5] {
                part(&node, &geometry::cylinder(0.03, 0.03, 2.0, 6), &pole_mat, px, 1.0, 0.42);
            }
            let awning = part(&node, &geometry::box_geo(1.4, 0.04, 0.95), &awning_mat, 0.0, 2.0, 0.1);
            awning.set_rotation(0.2, 0.0, 0.0); // the front edge drops toward the customers
            boxes.push(footprint(&counter, node.local_matrix()));
        }
        shadows(&node);
        g.add(&node);
    }

    let animate: Anim = Box::new(move |t: f32| {
        // The water scrolls: the same texture, offset along v.
        water_mat.get().map = Some(Arc::new(Texture {
            offset: [0.0, (t * 0.04) % 1.0],
            ..(*water_base).clone()
        }));
    });
    outline(&g, 0.12);
    Furniture { group: g, boxes, animate }
}

/// `buildPaths(from, palette)`: plated strips from the origin out to each point (y = 0), for the
/// paths between the centre and the districts, each with a light line down either edge.
pub fn build_paths(from: &[Vec3], palette: &PropPalette) -> Obj {
    let g = Obj::group();
    let mat = std_mat(palette.rule.lerp(palette.sheet, 0.35));
    let light = Mat::basic(palette.info);
    for to in from {
        let len = to.x.hypot(to.z);
        if len < 0.5 {
            continue;
        }
        // Repeat the stones along the strip instead of stretching one tile over it.
        let base = geometry::box_geo(len, 0.03, 0.9);
        let k = len / 0.9;
        let uvs: Vec<[f32; 2]> = base.uvs.iter().map(|uv| [uv[0] * k, uv[1]]).collect();
        let geo = Geometry::new(base.positions.clone(), base.normals.clone(), uvs, base.indices.clone());
        let strip = Obj::mesh(&geo, &mat);
        strip.set_position(to.x / 2.0, 0.015, to.z / 2.0);
        strip.set_rotation(0.0, -to.z.atan2(to.x), 0.0);
        strip.get().receive_shadow = true;
        for side in [-0.41f32, 0.41] {
            let line = Obj::mesh(&geometry::box_geo(len, 0.02, 0.05), &light);
            line.set_position(0.0, 0.02, side);
            strip.add(&line);
        }
        g.add(&strip);
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> PropPalette {
        PropPalette {
            sheet: Color::hex(0xf5efe6),
            rule: Color::hex(0xc9bfae),
            ink3: Color::hex(0x6b6357),
            signal: Color::hex(0xe8603c),
            ok: Color::hex(0x4fa36a),
            info: Color::hex(0x3c7fd6),
        }
    }

    fn count_meshes(o: &Obj) -> usize {
        let (is_mesh, children) = {
            let n = o.get();
            (matches!(n.kind, Kind::Mesh { .. }), n.children.clone())
        };
        usize::from(is_mesh) + children.iter().map(count_meshes).sum::<usize>()
    }

    #[test]
    fn build_plaza_returns_a_non_empty_group() {
        let (g, _anim) = build_plaza(&palette());
        assert!(count_meshes(&g) > 0);
    }
}
