//! `floor3d/toon.ts`: toon materials, the ink outline, the painted sky, the drifting clouds and
//! the wind-swayed grass tufts.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

use super::geometry::{from_triangles, ring, sphere, sphere_part, transformed, weld_smooth};
use super::scene::{Color, Geo, Instance, Kind, Mat, Material, Obj, Shading, Side};
use super::textures::Rng;

/// `toonMat({ color, ... })`: a toon material with these values (roughness/metalness dropped).
pub fn toon_mat(material: Material) -> Mat {
    Mat::new(material)
}

/// `outline(root, minSize)`: gives every opaque, lit (toon) mesh under `root` whose bounding
/// radius is at least `min_size`, and that is not `no_outline`, its ink hull
/// (`geometry::weld_smooth(geo, 1e-3)`), set on the node's `hull`.
pub fn outline(root: &Obj, min_size: f32) {
    // Collect first, locking one node at a time; the hull of each geometry is welded once.
    let mut found: Vec<(Obj, Geo)> = Vec::new();
    root.traverse(&mut |o: &Obj| {
        let candidate = {
            let node = o.get();
            match &node.kind {
                Kind::Mesh { geo, mat } if !node.no_outline && node.hull.is_none() => Some((Arc::clone(geo), mat.clone())),
                _ => None,
            }
        };
        let Some((geo, mat)) = candidate else { return };
        let inked = {
            let m = mat.get();
            m.shading == Shading::Toon && !m.transparent
        };
        if inked && geo.radius >= min_size {
            found.push((o.clone(), geo));
        }
    });
    let mut hulls: HashMap<u64, Geo> = HashMap::new();
    for (obj, geo) in found {
        let hull = hulls.entry(geo.id).or_insert_with(|| weld_smooth(&geo, 1e-3)).clone();
        obj.get().hull = Some(hull);
    }
}

/// `sky(top, horizon, ground)`: the dome (sphere r 400, 32 × 16) with `Shading::Sky`, back side,
/// no depth write, render order -1.
pub fn sky(top: Color, horizon: Color, ground: Color) -> Obj {
    let geo = sphere(400.0, 32, 16);
    let mat = Mat::new(Material {
        shading: Shading::Sky { top, horizon, ground },
        side: Side::Back,
        depth_write: false,
        fog: false,
        ..Material::default()
    });
    let dome = Obj::mesh(&geo, &mat);
    dome.set_render_order(-1);
    dome
}

/// `clouds(n, radius, rand)`: the cloud ring; the caller turns it (`rotation.y = t * 0.006`).
pub fn clouds(n: u32, radius: f32, rand: &mut Rng) -> Obj {
    let group = Obj::group();
    let mat = toon_mat(Material {
        color: Color::WHITE,
        emissive: Color::hex(0xdde6f0),
        emissive_intensity: 0.35,
        ..Material::default()
    });
    let puff = sphere(1.0, 16, 12);
    for i in 0..n {
        let cloud = Obj::group();
        let lobes = 4 + (rand.next() * 3.0).floor() as u32;
        for k in 0..lobes {
            let s = 1.6 + rand.next() * 1.6;
            let m = Obj::mesh(&puff, &mat);
            m.set_scale(s * 1.3, s * 0.8, s);
            let x = (k as f32 - lobes as f32 / 2.0) * 1.9 + rand.next();
            let y = rand.next() * 0.8;
            let z = (rand.next() - 0.5) * 2.0;
            m.set_position(x, y, z);
            cloud.add(&m);
        }
        let a = (i as f32 / n as f32) * TAU + rand.next() * 0.4;
        let r = radius * (0.7 + rand.next() * 0.5);
        cloud.set_position(a.cos() * r, 48.0 + rand.next() * 14.0, a.sin() * r);
        cloud.set_rotation(0.0, rand.next() * PI, 0.0);
        group.add(&cloud);
    }
    outline(&group, 0.0);
    group.set_dynamic(true);
    group
}

/// `grassTufts(count, reach, keep, rand)`: the instanced tufts (`Shading::Grass`, double-sided,
/// receiving shadows, no outline). The wind runs in the shader from the frame's time.
pub fn grass_tufts(count: u32, reach: f32, keep: &dyn Fn(f32, f32) -> bool, rand: &mut Rng) -> Obj {
    // One tuft: three crossed blades, each a thin triangle.
    let mut pos: Vec<[f32; 3]> = Vec::new();
    for b in 0..3u32 {
        let bf = b as f32;
        let a = (bf / 3.0) * PI + 0.3;
        let dx = a.cos() * 0.07;
        let dz = a.sin() * 0.07;
        let lean = (bf - 1.0) * 0.06;
        pos.push([-dx, 0.0, -dz]);
        pos.push([dx, 0.0, dz]);
        pos.push([lean, 0.32 + bf * 0.05, 0.0]);
    }
    let geo = from_triangles(pos);
    let mat = toon_mat(Material {
        color: Color::WHITE,
        shading: Shading::Grass,
        side: Side::Double,
        ..Material::default()
    });

    let up = Vec3::Y;
    let mut instances: Vec<Instance> = Vec::new();
    let mut i = 0;
    while i < count * 4 && (instances.len() as u32) < count {
        i += 1;
        let x = (rand.next() * 2.0 - 1.0) * reach;
        let z = (rand.next() * 2.0 - 1.0) * reach;
        if x.hypot(z) > reach || !keep(x, z) {
            continue;
        }
        let s = 0.7 + rand.next() * 0.9;
        let q = Quat::from_axis_angle(up, rand.next() * TAU);
        let matrix = Mat4::from_scale_rotation_translation(Vec3::splat(s), q, Vec3::new(x, 0.0, z));
        let color = Color::hsl(0.24 + rand.next() * 0.07, 0.5 + rand.next() * 0.2, 0.42 + rand.next() * 0.14);
        instances.push(Instance { matrix, color });
    }

    let mesh = Obj::instanced(&geo, &mat, instances);
    mesh.set_shadows(false, true);
    mesh.no_outline();
    mesh
}

/// The night sky's furniture (`toon.ts` `starfield`): `n` stars scattered over the upper sky, a
/// bright blue layer (`n / 8`) and a bright violet one (`n / 16`), and a ringed planet low over the
/// horizon. Unfogged, so they read through the haze. The caller turns `group` slowly
/// (`rotation.y = t * 0.004`) and `planet` (`rotation.y = t * 0.05`).
pub struct Starfield {
    pub group: Obj,
    pub planet: Obj,
}

/// `starfield(n, radius, rand)`: see [`Starfield`].
pub fn starfield(n: u32, radius: f32, rand: &mut Rng) -> Starfield {
    let group = Obj::group();
    let mut layer = |count: u32, size: f32, color: u32, opacity: f32| {
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(count as usize);
        for _ in 0..count {
            // Uniform over the sky above 6 degrees of elevation.
            let a = rand.next() * TAU;
            let y = 0.1 + rand.next() * 0.9;
            let rim = (1.0 - y * y).sqrt();
            positions.push([a.cos() * rim * radius, y * radius, a.sin() * rim * radius]);
        }
        let mat = Mat::new(Material {
            shading: Shading::Points { size },
            color: Color::hex(color),
            transparent: true,
            opacity,
            depth_write: false,
            fog: false,
            ..Material::default()
        });
        group.add(&Obj::points(positions, &mat));
    };
    layer(n, 1.4, 0xdfe8ff, 0.85);
    layer(((n as f32) / 8.0).round() as u32, 2.6, 0x9fdcff, 0.95);
    layer(((n as f32) / 16.0).round() as u32, 2.4, 0xd9a8ff, 0.95);

    // A ringed planet low in the sky.
    let planet = Obj::group();
    let body = Obj::mesh(
        &sphere(18.0, 32, 20),
        &toon_mat(Material {
            color: Color::hex(0x6b4fd8),
            emissive: Color::hex(0x3a1f8f),
            emissive_intensity: 0.6,
            fog: false,
            ..Material::default()
        }),
    );
    let band = Obj::mesh(
        &sphere_part(18.15, 32, 6, 0.0, TAU, PI * 0.42, PI * 0.1),
        &toon_mat(Material {
            color: Color::hex(0x9d7dff),
            emissive: Color::hex(0x5a3bd0),
            emissive_intensity: 0.6,
            fog: false,
            ..Material::default()
        }),
    );
    let ring_geo = transformed(&ring(24.0, 33.0, 64, 1, 0.0, TAU), Mat4::from_rotation_x(-FRAC_PI_2));
    let ring_mat = Mat::new(Material {
        shading: Shading::Basic,
        color: Color::hex(0x8fd8ff),
        transparent: true,
        opacity: 0.45,
        side: Side::Double,
        depth_write: false,
        fog: false,
        ..Material::default()
    });
    let rim = Obj::mesh(&ring_geo, &ring_mat);
    rim.no_outline();
    planet.add(&body).add(&band).add(&rim);
    planet.set_rotation(0.35, 0.0, 0.3);
    planet.set_position(-150.0, 70.0, -210.0);
    group.add(&planet);
    outline(&planet, 1.0);
    Starfield { group, planet }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::floor3d::scene::Geometry;

    #[test]
    fn sky_is_a_back_side_dome_drawn_first() {
        let dome = sky(Color::hex(0x3a7bd5), Color::hex(0xf5d7a1), Color::hex(0x6b8f3a));
        assert_eq!(dome.get().render_order, -1);
        let mat = dome.material().expect("the dome is a mesh");
        let m = mat.get();
        assert!(matches!(m.shading, Shading::Sky { .. }));
        assert_eq!(m.side, Side::Back);
        assert!(!m.depth_write);
    }

    #[test]
    fn grass_tufts_make_count_instances_when_every_spot_is_kept() {
        let mut rng = Rng::new(7);
        let tufts = grass_tufts(25, 10.0, &|_, _| true, &mut rng);
        let node = tufts.get();
        match &node.kind {
            Kind::Instanced { count, .. } => assert_eq!(*count, 25),
            other => panic!("expected an instanced mesh, got {other:?}"),
        }
    }

    #[test]
    fn outline_inks_big_toon_meshes_and_skips_tiny_ones() {
        let triangle = |scale: f32| {
            Geometry::new(
                vec![[-scale, 0.0, 0.0], [scale, 0.0, 0.0], [0.0, scale, 0.0]],
                Vec::new(),
                Vec::new(),
                vec![0, 1, 2],
            )
        };
        let big = Obj::mesh(&triangle(1.0), &Mat::toon(Color::WHITE));
        let tiny = Obj::mesh(&triangle(0.01), &Mat::toon(Color::WHITE));
        let root = Obj::group();
        root.add(&big).add(&tiny);
        outline(&root, 0.12);
        assert!(big.get().hull.is_some());
        assert!(tiny.get().hull.is_none());
    }

    #[test]
    fn starfield_is_three_point_layers_and_a_planet() {
        let field = starfield(900, 330.0, &mut Rng::new(31));
        assert_eq!(field.group.get().children.len(), 4);
    }
}
