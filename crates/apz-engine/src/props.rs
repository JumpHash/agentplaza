//! `floor3d/props.ts`: the central plaza (paved ring, the xenon atom), its furniture (fountain,
//! market stalls), the paths with their street lamps, and the meadow's scatter (trees, flower
//! beds, benches, rocks). Every solid piece reports its ground footprint for collisions.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use glam::{EulerRot, Mat4, Vec3};

use super::geometry;
use super::scene::{Color, Geo, Instance, Kind, Mat, Material, Obj, Shading};
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

/// One electron shell of the xenon atom: its group (the track hangs in it), its Euler tilt, its
/// radius, its electron count and its angular speed (radians per second).
struct Shell {
    group: Obj,
    rot: Vec3,
    r: f32,
    n: usize,
    speed: f32,
}

/// The electrons of every shell at `t` seconds, scaled by the nucleus beat: one instance each, in
/// the globe's frame (a shell's tilt, then the electron on its circle).
fn electron_instances(shells: &[Shell], t: f32, beat: f32) -> Vec<Instance> {
    let mut out = Vec::with_capacity(shells.iter().map(|s| s.n).sum());
    for sh in shells {
        let tilt = Mat4::from_euler(EulerRot::XYZ, sh.rot.x, sh.rot.y, sh.rot.z);
        let scale = Mat4::from_scale(Vec3::splat(1.0 + 0.25 * beat));
        let n = sh.n as f32;
        for i in 0..sh.n {
            let a = t * sh.speed + (i as f32 / n) * TAU;
            let at = Mat4::from_translation(Vec3::new(a.cos() * sh.r, 0.0, a.sin() * sh.r));
            out.push(Instance { matrix: tilt * at * scale, color: Color::WHITE });
        }
    }
    out
}

/// `buildPlaza(palette)`: a round paved square with a darker ring, four benches and a slowly
/// turning hologram (the xenon atom, see [`atom`]) above the dais. The group is dynamic (the atom
/// turns).
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

    let (globe, anim) = atom(palette);
    g.add(&globe);
    shadows(&g);
    no_cast(&globe);
    outline(&g, 0.12);
    g.get().dynamic = true;
    (g, anim)
}

/// The xenon atom, its globe at rest (y 4.2, scale 1.5): a pulsing nucleus of 24 nucleons (12
/// protons, 12 neutrons), a halo and an aura (additive spheres), a point light, and 54 electrons on
/// five tilted shells. Each nucleon kind and the electrons are one instanced draw, and the animation
/// rewrites their instance matrices each frame; the halo, aura and light change in place, so nothing
/// is rebuilt. The globe bobs as a whole, so every part of the atom moves. Returns the globe and its
/// animation (`t` seconds since start).
pub fn atom(palette: &PropPalette) -> (Obj, Anim) {
    let xenon = Color::hex(0x9fa8ff);
    let globe = Obj::group();
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
    // The rest positions: index 0 the neutrons (even i), index 1 the protons (odd i).
    let mut nucleons: [Vec<Vec3>; 2] = [Vec::new(), Vec::new()];
    for i in 0..24usize {
        let v = Vec3::new(nr.next() - 0.5, nr.next() - 0.5, nr.next() - 0.5).normalize() * (nr.next().cbrt() * 0.24);
        nucleons[usize::from(i % 2 == 1)].push(v);
    }
    let nucleus: Vec<Obj> = [&neutron, &proton]
        .into_iter()
        .zip(&nucleons)
        .map(|(mat, rest)| {
            let instances = rest.iter().map(|v| Instance { matrix: Mat4::from_translation(*v), color: Color::WHITE }).collect();
            let obj = Obj::instanced(&nucleon_geo, mat, instances);
            obj.set_shadows(false, true);
            globe.add(&obj);
            obj
        })
        .collect();
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
        let rot = Vec3::new(nr.next() * PI, nr.next() * PI, nr.next() * PI);
        shell.set_rotation(rot.x, rot.y, rot.z);
        let track = Obj::mesh(&geometry::torus(r, 0.012, 6, 96, TAU), &ring_mat);
        track.set_rotation(PI / 2.0, 0.0, 0.0);
        track.no_outline(); // a fine glowing track, not an inked hoop
        shell.add(&track);
        globe.add(&shell);
        let speed = 1.9 / (1.0 + k as f32 * 0.45) * if k % 2 == 1 { -1.0 } else { 1.0 };
        shells.push(Shell { group: shell, rot, r, n, speed });
    }
    let electrons = Obj::instanced(&electron_geo, &electron_mat, electron_instances(&shells, 0.0, 0.0));
    electrons.set_shadows(false, true);
    globe.add(&electrons);
    shadows(&globe);
    no_cast(&globe);
    outline(&globe, 0.05);
    globe.set_scalar(1.5);
    globe.set_position(0.0, 4.2, 0.0);

    let globe_anim = globe.clone();
    let animate: Anim = Box::new(move |t: f32| {
        globe_anim.set_position(0.0, 4.2 + (t * 0.9).sin() * 0.1, 0.0);
        // The nucleus beats, its halo and light breathing with it.
        let beat = (t * 2.4).sin().max(0.0).powf(6.0);
        let spin = Mat4::from_rotation_y(t * 0.6) * Mat4::from_scale(Vec3::splat(1.0 + 0.12 * beat));
        for (obj, rest) in nucleus.iter().zip(&nucleons) {
            obj.set_instances(rest.iter().map(|v| Instance { matrix: spin * Mat4::from_translation(*v), color: Color::WHITE }).collect());
        }
        halo.set_scalar(1.0 + 0.35 * beat + 0.05 * (t * 7.0).sin());
        halo_mat.get().opacity = 0.3 + 0.35 * beat;
        aura_mat.get().opacity = 0.07 + 0.08 * beat;
        aura.set_scalar(1.0 + 0.1 * beat);
        if let Kind::PointLight { intensity, .. } = &mut glow.get().kind {
            *intensity = 1.6 + 2.4 * beat;
        }
        for sh in shells.iter_mut() {
            sh.rot.y += 0.002;
            sh.group.get().rotation = sh.rot;
        }
        electrons.set_instances(electron_instances(&shells, t, beat));
    });
    (globe, animate)
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

    /// The instance matrices must equal the scene-graph transforms they replace: an electron under its
    /// tilted shell (rotation, then position and scale), and a nucleon under the beating nucleus.
    #[test]
    fn atom_instances_match_the_scene_graph_they_replace() {
        let (t, beat) = (0.7f32, 0.5f32);
        let (rot, r, n) = (Vec3::new(0.3, 1.1, -0.4), 0.9f32, 3usize);
        let shells = vec![Shell { group: Obj::group(), rot, r, n, speed: 1.3 }];
        let instances = electron_instances(&shells, t, beat);
        let shell = Obj::group();
        shell.set_rotation(rot.x, rot.y, rot.z);
        for (i, inst) in instances.iter().enumerate() {
            let a = t * 1.3 + (i as f32 / n as f32) * TAU;
            let e = Obj::group();
            e.set_position(a.cos() * r, 0.0, a.sin() * r).set_scalar(1.0 + 0.25 * beat);
            shell.add(&e);
            let reference = shell.local_matrix() * e.local_matrix();
            shell.remove(&e);
            assert!(inst.matrix.abs_diff_eq(reference, 1e-5), "electron {i}");
        }
        let v = Vec3::new(0.1, -0.2, 0.05);
        let spin = Mat4::from_rotation_y(t * 0.6) * Mat4::from_scale(Vec3::splat(1.0 + 0.12 * beat));
        let nucleus = Obj::group();
        nucleus.set_rotation(0.0, t * 0.6, 0.0).set_scalar(1.0 + 0.12 * beat);
        let nucleon = Obj::group();
        nucleon.set_position(v.x, v.y, v.z);
        nucleus.add(&nucleon);
        assert!((spin * Mat4::from_translation(v)).abs_diff_eq(nucleus.local_matrix() * nucleon.local_matrix(), 1e-5));
    }
}
