//! three.js's geometry generators, vertex for vertex (same order, normals and uvs), so textures
//! and the toon ramp land exactly where they do on the web. Flat-shaped ones here; the round
//! ones (sphere, cylinder, cone, torus, dodecahedron) in `round`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use glam::Mat4;

use super::scene::{Geo, Geometry};

pub mod edges;
mod round;
pub use round::{cone, cylinder, cylinder_full, dodecahedron, sphere, sphere_part, torus};

type V3 = [f64; 3];

/// A BufferGeometry's arrays before `Geometry::new` takes them. Empty `indices` = non-indexed.
#[derive(Default)]
struct Raw {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Raw {
    fn finish(self) -> Geo {
        Geometry::new(self.positions, self.normals, self.uvs, self.indices)
    }
}

fn to_f32(v: V3) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

fn to_f64(v: [f32; 3]) -> V3 {
    [v[0] as f64, v[1] as f64, v[2] as f64]
}

/// `Math.sign`, which is 0 at 0 (unlike `f64::signum`).
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `Vector3.cross`.
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `Vector3.normalize`: divides by the length, or by 1 for a zero vector.
fn normalized(a: V3) -> V3 {
    let len = dot(a, a).sqrt();
    let len = if len == 0.0 { 1.0 } else { len };
    [a[0] / len, a[1] / len, a[2] / len]
}

/// One `buildPlane` call of three's BoxGeometry / PlaneGeometry: axes are 0=x, 1=y, 2=z.
struct Plane {
    u: usize,
    v: usize,
    w: usize,
    udir: f64,
    vdir: f64,
    width: f64,
    height: f64,
    depth: f64,
    gx: u32,
    gy: u32,
}

fn build_plane(raw: &mut Raw, p: &Plane) {
    let seg_w = p.width / p.gx as f64;
    let seg_h = p.height / p.gy as f64;
    let (half_w, half_h, half_d) = (p.width / 2.0, p.height / 2.0, p.depth / 2.0);
    let (gx1, gy1) = (p.gx + 1, p.gy + 1);
    let offset = raw.positions.len() as u32;
    let normal_w = if p.depth > 0.0 { 1.0 } else { -1.0 };
    for iy in 0..gy1 {
        let y = iy as f64 * seg_h - half_h;
        for ix in 0..gx1 {
            let x = ix as f64 * seg_w - half_w;
            let mut pos = [0.0; 3];
            pos[p.u] = x * p.udir;
            pos[p.v] = y * p.vdir;
            pos[p.w] = half_d;
            let mut nrm = [0.0; 3];
            nrm[p.w] = normal_w;
            raw.positions.push(to_f32(pos));
            raw.normals.push(to_f32(nrm));
            raw.uvs.push([
                (ix as f64 / p.gx as f64) as f32,
                (1.0 - iy as f64 / p.gy as f64) as f32,
            ]);
        }
    }
    for iy in 0..p.gy {
        for ix in 0..p.gx {
            let a = offset + ix + gx1 * iy;
            let b = offset + ix + gx1 * (iy + 1);
            let c = offset + (ix + 1) + gx1 * (iy + 1);
            let d = offset + (ix + 1) + gx1 * iy;
            raw.indices.extend_from_slice(&[a, b, d, b, c, d]);
        }
    }
}

/// three's `BoxGeometry(w, h, d, ws, hs, ds)`: six faces in the order px, nx, py, ny, pz, nz.
fn box_raw(w: f64, h: f64, d: f64, ws: u32, hs: u32, ds: u32) -> Raw {
    let faces = [
        Plane {
            u: 2,
            v: 1,
            w: 0,
            udir: -1.0,
            vdir: -1.0,
            width: d,
            height: h,
            depth: w,
            gx: ds,
            gy: hs,
        },
        Plane {
            u: 2,
            v: 1,
            w: 0,
            udir: 1.0,
            vdir: -1.0,
            width: d,
            height: h,
            depth: -w,
            gx: ds,
            gy: hs,
        },
        Plane {
            u: 0,
            v: 2,
            w: 1,
            udir: 1.0,
            vdir: 1.0,
            width: w,
            height: d,
            depth: h,
            gx: ws,
            gy: ds,
        },
        Plane {
            u: 0,
            v: 2,
            w: 1,
            udir: 1.0,
            vdir: -1.0,
            width: w,
            height: d,
            depth: -h,
            gx: ws,
            gy: ds,
        },
        Plane {
            u: 0,
            v: 1,
            w: 2,
            udir: 1.0,
            vdir: -1.0,
            width: w,
            height: h,
            depth: d,
            gx: ws,
            gy: hs,
        },
        Plane {
            u: 0,
            v: 1,
            w: 2,
            udir: -1.0,
            vdir: -1.0,
            width: w,
            height: h,
            depth: -d,
            gx: ws,
            gy: hs,
        },
    ];
    let mut raw = Raw::default();
    for face in &faces {
        build_plane(&mut raw, face);
    }
    raw
}

/// `BoxGeometry(w, h, d, 1, 1, 1)`.
pub fn box_geo(w: f32, h: f32, d: f32) -> Geo {
    box_raw(w as f64, h as f64, d as f64, 1, 1, 1).finish()
}

/// `getUv` of three's RoundedBoxGeometry: the position along one rounded edge, in [0, 1].
fn round_uv(dir: V3, normal: V3, uv_axis: usize, proj_axis: usize, radius: f64, side: f64) -> f64 {
    use std::f64::consts::PI;
    let tot_arc = 2.0 * PI * radius / 4.0;
    let center = (side - 2.0 * radius).max(0.0);
    let half_arc = PI / 4.0;

    let mut flat = normal;
    flat[proj_axis] = 0.0;
    let flat = normalized(flat);

    let arc_uv_ratio = 0.5 * tot_arc / (tot_arc + center);
    let theta = dot(flat, dir) / (dot(flat, flat) * dot(dir, dir)).sqrt();
    let angle = theta.clamp(-1.0, 1.0).acos();
    let arc_angle_ratio = 1.0 - angle / half_arc;

    if sign(flat[uv_axis]) == 1.0 {
        arc_angle_ratio * arc_uv_ratio
    } else {
        let len_uv = center / (tot_arc + center);
        len_uv + arc_uv_ratio + arc_uv_ratio * (1.0 - arc_angle_ratio)
    }
}

/// three's `RoundedBoxGeometry(w, h, d, segments, radius)` (examples/jsm/geometries).
pub fn rounded_box(w: f32, h: f32, d: f32, segments: u32, radius: f32) -> Geo {
    let (w, h, d) = (w as f64, h as f64, d as f64);
    let total = segments * 2 + 1;
    let radius = (w / 2.0).min(h / 2.0).min(d / 2.0).min(radius as f64);
    // three returns the plain unit box here, unscaled
    if total == 1 {
        return box_geo(1.0, 1.0, 1.0);
    }

    // toNonIndexed(): the box's triangles in index order, so six faces of 6 * total^2 vertices
    let boxed = box_raw(1.0, 1.0, 1.0, total, total, total);
    let per_side = 6 * (total as usize) * (total as usize);
    let half_seg = 0.5 / total as f64;
    let (bx, by, bz) = (w / 2.0 - radius, h / 2.0 - radius, d / 2.0 - radius);

    let mut raw = Raw::default();
    for (v, &k) in boxed.indices.iter().enumerate() {
        let position = to_f64(boxed.positions[k as usize]);
        let mut nrm = position;
        for c in nrm.iter_mut() {
            *c -= sign(*c) * half_seg;
        }
        let nrm = normalized(nrm);
        let pos = [
            bx * sign(position[0]) + nrm[0] * radius,
            by * sign(position[1]) + nrm[1] * radius,
            bz * sign(position[2]) + nrm[2] * radius,
        ];
        raw.positions.push(to_f32(pos));
        raw.normals.push(to_f32(nrm));

        let uv = match v / per_side {
            0 => {
                let dir = [1.0, 0.0, 0.0];
                [
                    round_uv(dir, nrm, 2, 1, radius, d),
                    1.0 - round_uv(dir, nrm, 1, 2, radius, h),
                ]
            }
            1 => {
                let dir = [-1.0, 0.0, 0.0];
                [
                    1.0 - round_uv(dir, nrm, 2, 1, radius, d),
                    1.0 - round_uv(dir, nrm, 1, 2, radius, h),
                ]
            }
            2 => {
                let dir = [0.0, 1.0, 0.0];
                [
                    1.0 - round_uv(dir, nrm, 0, 2, radius, w),
                    round_uv(dir, nrm, 2, 0, radius, d),
                ]
            }
            3 => {
                let dir = [0.0, -1.0, 0.0];
                [
                    1.0 - round_uv(dir, nrm, 0, 2, radius, w),
                    1.0 - round_uv(dir, nrm, 2, 0, radius, d),
                ]
            }
            4 => {
                let dir = [0.0, 0.0, 1.0];
                [
                    1.0 - round_uv(dir, nrm, 0, 1, radius, w),
                    1.0 - round_uv(dir, nrm, 1, 0, radius, h),
                ]
            }
            _ => {
                let dir = [0.0, 0.0, -1.0];
                [
                    round_uv(dir, nrm, 0, 1, radius, w),
                    1.0 - round_uv(dir, nrm, 1, 0, radius, h),
                ]
            }
        };
        raw.uvs.push([uv[0] as f32, uv[1] as f32]);
    }
    raw.finish()
}

/// `PlaneGeometry(w, h, ws, hs)` in the XY plane, facing +Z.
pub fn plane(w: f32, h: f32, ws: u32, hs: u32) -> Geo {
    let (w, h) = (w as f64, h as f64);
    let (half_w, half_h) = (w / 2.0, h / 2.0);
    let (seg_w, seg_h) = (w / ws as f64, h / hs as f64);
    let (gx1, gy1) = (ws + 1, hs + 1);
    let mut raw = Raw::default();
    for iy in 0..gy1 {
        let y = iy as f64 * seg_h - half_h;
        for ix in 0..gx1 {
            let x = ix as f64 * seg_w - half_w;
            raw.positions.push(to_f32([x, -y, 0.0]));
            raw.normals.push([0.0, 0.0, 1.0]);
            raw.uvs.push([
                (ix as f64 / ws as f64) as f32,
                (1.0 - iy as f64 / hs as f64) as f32,
            ]);
        }
    }
    for iy in 0..hs {
        for ix in 0..ws {
            let a = ix + gx1 * iy;
            let b = ix + gx1 * (iy + 1);
            let c = (ix + 1) + gx1 * (iy + 1);
            let d = (ix + 1) + gx1 * iy;
            raw.indices.extend_from_slice(&[a, b, d, b, c, d]);
        }
    }
    raw.finish()
}

/// `CircleGeometry(r, segments, thetaStart, thetaLength)` in the XY plane, facing +Z.
pub fn circle(r: f32, segments: u32, theta_start: f32, theta_length: f32) -> Geo {
    let r = r as f64;
    let segments = segments.max(3);
    let (theta_start, theta_length) = (theta_start as f64, theta_length as f64);
    let mut raw = Raw::default();
    raw.positions.push([0.0; 3]);
    raw.normals.push([0.0, 0.0, 1.0]);
    raw.uvs.push([0.5, 0.5]);
    for s in 0..=segments {
        let theta = theta_start + s as f64 / segments as f64 * theta_length;
        let (x, y) = (r * theta.cos(), r * theta.sin());
        raw.positions.push(to_f32([x, y, 0.0]));
        raw.normals.push([0.0, 0.0, 1.0]);
        raw.uvs
            .push([((x / r + 1.0) / 2.0) as f32, ((y / r + 1.0) / 2.0) as f32]);
    }
    for i in 1..=segments {
        raw.indices.extend_from_slice(&[i, i + 1, 0]);
    }
    raw.finish()
}

/// `RingGeometry(inner, outer, thetaSegments, phiSegments, thetaStart, thetaLength)`.
pub fn ring(
    inner: f32,
    outer: f32,
    theta_segments: u32,
    phi_segments: u32,
    theta_start: f32,
    theta_length: f32,
) -> Geo {
    let (inner, outer) = (inner as f64, outer as f64);
    let theta_segments = theta_segments.max(3);
    let phi_segments = phi_segments.max(1);
    let (theta_start, theta_length) = (theta_start as f64, theta_length as f64);
    let step = (outer - inner) / phi_segments as f64;
    let mut radius = inner;
    let mut raw = Raw::default();
    for _ in 0..=phi_segments {
        for i in 0..=theta_segments {
            let theta = theta_start + i as f64 / theta_segments as f64 * theta_length;
            let (x, y) = (radius * theta.cos(), radius * theta.sin());
            raw.positions.push(to_f32([x, y, 0.0]));
            raw.normals.push([0.0, 0.0, 1.0]);
            raw.uvs.push([
                ((x / outer + 1.0) / 2.0) as f32,
                ((y / outer + 1.0) / 2.0) as f32,
            ]);
        }
        radius += step;
    }
    for j in 0..phi_segments {
        let level = j * (theta_segments + 1);
        for i in 0..theta_segments {
            let seg = i + level;
            let (a, b, c, d) = (
                seg,
                seg + theta_segments + 1,
                seg + theta_segments + 2,
                seg + 1,
            );
            raw.indices.extend_from_slice(&[a, b, d, b, c, d]);
        }
    }
    raw.finish()
}

/// A non-indexed triangle soup with three's `computeVertexNormals` (flat per triangle), uvs 0.
pub fn from_triangles(positions: Vec<[f32; 3]>) -> Geo {
    let n = positions.len() - positions.len() % 3;
    let mut normals = Vec::with_capacity(n);
    for tri in positions[..n].as_chunks::<3>().0 {
        let (a, b, c) = (to_f64(tri[0]), to_f64(tri[1]), to_f64(tri[2]));
        let nrm = to_f32(normalized(cross(sub(c, b), sub(a, b))));
        normals.extend_from_slice(&[nrm; 3]);
    }
    let mut positions = positions;
    positions.truncate(n);
    Geometry::new(positions, normals, Vec::new(), Vec::new())
}

/// The geometry moved by `m` (three's `geometry.applyMatrix4` / `translate` / `rotateX` …).
pub fn transformed(geo: &Geo, m: Mat4) -> Geo {
    super::scene::merge(&[(std::sync::Arc::clone(geo), m)])
}

/// The outline hull of `toon.ts`: vertices welded within `tolerance` (three's `mergeVertices`
/// on positions only), then smooth normals (`computeVertexNormals`). Cached by geometry id.
pub fn weld_smooth(geo: &Geo, tolerance: f32) -> Geo {
    static CACHE: OnceLock<Mutex<HashMap<u64, Geo>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = cache.lock().expect("weld cache").get(&geo.id).cloned();
    if let Some(hit) = cached {
        return hit;
    }
    let welded = weld(geo, tolerance);
    // The first hull stored for the geometry wins, so concurrent callers share one (and one geometry id).
    Arc::clone(
        cache
            .lock()
            .expect("weld cache")
            .entry(geo.id)
            .or_insert(welded),
    )
}

/// `mergeVertices` on positions (quantised to round(v / tolerance)), then `computeVertexNormals`.
fn weld(geo: &Geometry, tolerance: f32) -> Geo {
    // three clamps the tolerance to Number.EPSILON
    let tol = (tolerance as f64).max(f64::EPSILON);
    let mut lookup: HashMap<[i64; 3], u32> = HashMap::new();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut remap = Vec::with_capacity(geo.positions.len());
    for &p in &geo.positions {
        let key = [0, 1, 2].map(|k| (p[k] as f64 / tol).round() as i64);
        let next = positions.len() as u32;
        let idx = *lookup.entry(key).or_insert(next);
        if idx == next {
            positions.push(p);
        }
        remap.push(idx);
    }

    let source: Vec<u32> = if geo.indices.is_empty() {
        (0..geo.positions.len() as u32).collect()
    } else {
        geo.indices.clone()
    };
    let indices: Vec<u32> = source.iter().map(|&i| remap[i as usize]).collect();

    let mut sums: Vec<V3> = vec![[0.0; 3]; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let (pa, pb, pc) = (
            to_f64(positions[a]),
            to_f64(positions[b]),
            to_f64(positions[c]),
        );
        let area = cross(sub(pc, pb), sub(pa, pb));
        for v in [a, b, c] {
            for k in 0..3 {
                sums[v][k] += area[k];
            }
        }
    }
    let normals: Vec<[f32; 3]> = sums.into_iter().map(|s| to_f32(normalized(s))).collect();
    Geometry::new(positions, normals, Vec::new(), indices)
}
