//! Mesh primitives for the kit: indexed triangles with normals and uvs, y up, right-handed (as glTF).

use glam::{Mat4, Quat, Vec3};
use std::f32::consts::{PI, TAU};

/// Segments per rounded corner of a `rounded_box` footprint.
const CORNER_SEGMENTS: u32 = 8;

/// Indexed triangles: one normal and one uv per position, counter-clockwise front faces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Adds `other`'s triangles to this mesh.
    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        self.indices.extend(other.indices.iter().map(|&i| i + base));
    }

    /// This mesh with every position carried by `m` and every normal by its inverse transpose (renormalised).
    pub fn transformed(&self, m: Mat4) -> Mesh {
        let normal_matrix = m.inverse().transpose();
        let flip = m.determinant() < 0.0;
        let positions = self
            .positions
            .iter()
            .map(|p| m.transform_point3(Vec3::from_array(*p)).to_array())
            .collect();
        let normals = self
            .normals
            .iter()
            .map(|n| {
                normal_matrix
                    .transform_vector3(Vec3::from_array(*n))
                    .normalize_or_zero()
                    .to_array()
            })
            .collect();
        let mut indices = self.indices.clone();
        if flip {
            for tri in indices.chunks_exact_mut(3) {
                tri.swap(1, 2);
            }
        }
        Mesh {
            positions,
            normals,
            uvs: self.uvs.clone(),
            indices,
        }
    }

    pub fn translated(&self, t: Vec3) -> Mesh {
        self.transformed(Mat4::from_translation(t))
    }

    pub fn rotated(&self, q: Quat) -> Mesh {
        self.transformed(Mat4::from_quat(q))
    }

    /// The axis-aligned bounds (min, max); (0, 0) for an empty mesh.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut points = self.positions.iter().map(|p| Vec3::from_array(*p));
        let Some(first) = points.next() else {
            return (Vec3::ZERO, Vec3::ZERO);
        };
        points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)))
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// A quad `centre` + `u` * (±`hu`) + `v` * (±`hv`) with normal `n`, where `u` x `v` = `n` so the front face is
/// counter-clockwise seen from outside. uv (0, 0) sits at (-u, -v) and (1, 1) at (+u, +v).
fn push_quad(m: &mut Mesh, centre: Vec3, n: Vec3, u: Vec3, v: Vec3, hu: f32, hv: f32) {
    let base = m.positions.len() as u32;
    for (su, sv, tu, tv) in [
        (-1.0, -1.0, 0.0, 0.0),
        (1.0, -1.0, 1.0, 0.0),
        (1.0, 1.0, 1.0, 1.0),
        (-1.0, 1.0, 0.0, 1.0),
    ] {
        m.positions
            .push((centre + u * (su * hu) + v * (sv * hv)).to_array());
        m.normals.push(n.to_array());
        m.uvs.push([tu, tv]);
    }
    m.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A box `w` (x) by `h` (y) by `d` (z), centred on the origin, flat-shaded (24 vertices, 12 triangles).
pub fn cuboid(w: f32, h: f32, d: f32) -> Mesh {
    let (hw, hh, hd) = (w / 2.0, h / 2.0, d / 2.0);
    let (x, y, z) = (Vec3::X, Vec3::Y, Vec3::Z);
    let mut m = Mesh::default();
    // Each face: normal, then the in-plane axes u, v with u x v = normal, then their half extents.
    push_quad(&mut m, x * hw, x, y, z, hh, hd);
    push_quad(&mut m, -x * hw, -x, z, y, hd, hh);
    push_quad(&mut m, y * hh, y, z, x, hd, hw);
    push_quad(&mut m, -y * hh, -y, x, z, hw, hd);
    push_quad(&mut m, z * hd, z, x, y, hw, hh);
    push_quad(&mut m, -z * hd, -z, y, x, hh, hw);
    m
}

/// The footprint (x, z) of a rounded box with half extents `hw`, `hd` and corner radius `r`, offset outwards by `o`
/// (negative insets it). Returns `(position, outward direction)` for each perimeter point: `CORNER_SEGMENTS` steps per
/// corner, counter-clockwise seen from above, the straight edges running between the corner arcs.
fn footprint_ring(hw: f32, hd: f32, r: f32, o: f32) -> Vec<([f32; 2], [f32; 2])> {
    let (ew, ed) = ((hw + o).max(0.0), (hd + o).max(0.0));
    let rr = (r + o).max(0.0);
    let mut out = Vec::with_capacity(4 * (CORNER_SEGMENTS as usize + 1));
    for corner in 0..4u32 {
        let (sx, sz) = match corner {
            0 => (1.0, 1.0),
            1 => (-1.0, 1.0),
            2 => (-1.0, -1.0),
            _ => (1.0, -1.0),
        };
        let (cx, cz) = (sx * (ew - rr), sz * (ed - rr));
        for j in 0..=CORNER_SEGMENTS {
            let theta = (90.0 * (corner as f32 + j as f32 / CORNER_SEGMENTS as f32)).to_radians();
            let (s, c) = theta.sin_cos();
            out.push(([cx + rr * c, cz + rr * s], [c, s]));
        }
    }
    out
}

/// A box `w` by `h` by `d` centred on the origin whose footprint (x, z) has corners rounded with radius `r`, and whose
/// top and bottom edges are bevelled by `bevel` (a quarter round in 3 segments). `r` is clamped to just under half the
/// smaller of `w` and `d`, `bevel` to under a quarter of `h`; `bevel` 0 gives sharp top and bottom edges. Each rounded
/// corner has 8 segments. Normals are smooth around the corners and the bevels and flat on the caps.
pub fn rounded_box(w: f32, h: f32, d: f32, r: f32, bevel: f32) -> Mesh {
    let r = r.min(0.499 * w.min(d)).max(0.0);
    let b = bevel.min(0.249 * h).max(0.0);
    let (hw, hh, hd) = (w / 2.0, h / 2.0, d / 2.0);

    // Profile rings from bottom to top: (outward offset, y, outward part of the normal, y part of the normal).
    let mut profile: Vec<(f32, f32, f32, f32)> = Vec::new();
    if b > 0.0 {
        // Bottom quarter round: centre (-b, -hh + b), from the bottom cap edge up to the wall.
        for j in 0..=3 {
            let a = (-90.0 + 30.0 * j as f32).to_radians();
            profile.push((-b + b * a.cos(), -hh + b + b * a.sin(), a.cos(), a.sin()));
        }
        // Top quarter round: centre (-b, hh - b), from the wall up to the top cap edge.
        for j in 0..=3 {
            let a = (30.0 * j as f32).to_radians();
            profile.push((-b + b * a.cos(), hh - b + b * a.sin(), a.cos(), a.sin()));
        }
    } else {
        profile.push((0.0, -hh, 1.0, 0.0));
        profile.push((0.0, hh, 1.0, 0.0));
    }

    let per = 4 * (CORNER_SEGMENTS + 1);
    let mut m = Mesh::default();

    // Side rings.
    let mut ring_bases = Vec::with_capacity(profile.len());
    for (row, &(o, y, no, ny)) in profile.iter().enumerate() {
        ring_bases.push(m.positions.len() as u32);
        let v = row as f32 / (profile.len() - 1) as f32;
        for (i, (p, dir)) in footprint_ring(hw, hd, r, o).into_iter().enumerate() {
            let n = Vec3::new(no * dir[0], ny, no * dir[1]).normalize_or_zero();
            m.positions.push([p[0], y, p[1]]);
            m.normals.push(n.to_array());
            m.uvs.push([i as f32 / per as f32, v]);
        }
    }
    for pair in ring_bases.windows(2) {
        let (lower, upper) = (pair[0], pair[1]);
        for p in 0..per {
            let q = (p + 1) % per;
            let (a, b, c, d) = (lower + p, lower + q, upper + p, upper + q);
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    // Caps: a fan from each face's centre, with their own vertices so the normals are flat.
    let cap_offset = profile[0].0;
    let ring = footprint_ring(hw, hd, r, cap_offset);
    let bottom_y = profile[0].1;
    let top_y = profile[profile.len() - 1].1;
    for (y, ny, flip) in [(bottom_y, -1.0, false), (top_y, 1.0, true)] {
        let centre = m.positions.len() as u32;
        m.positions.push([0.0, y, 0.0]);
        m.normals.push([0.0, ny, 0.0]);
        m.uvs.push([0.5, 0.5]);
        let first = m.positions.len() as u32;
        for (p, _) in &ring {
            m.positions.push([p[0], y, p[1]]);
            m.normals.push([0.0, ny, 0.0]);
            m.uvs.push([p[0] / w + 0.5, p[1] / d + 0.5]);
        }
        for i in 0..per {
            let (a, b) = (first + i, first + (i + 1) % per);
            if flip {
                m.indices.extend_from_slice(&[centre, b, a]);
            } else {
                m.indices.extend_from_slice(&[centre, a, b]);
            }
        }
    }
    m
}

/// A cylinder along y, centred on the origin, radius `r_top` at +h/2 and `r_bottom` at -h/2, with caps.
pub fn cylinder(r_top: f32, r_bottom: f32, h: f32, segments: u32) -> Mesh {
    let segments = segments.max(3);
    let hh = h / 2.0;
    let slope = (r_bottom - r_top) / h;
    let mut m = Mesh::default();

    // Side: a top and a bottom vertex per segment (the last column repeats the first for the uv seam).
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        let (s, c) = (t * TAU).sin_cos();
        let n = Vec3::new(s, slope, c).normalize_or_zero().to_array();
        m.positions.push([r_top * s, hh, r_top * c]);
        m.normals.push(n);
        m.uvs.push([t, 0.0]);
        m.positions.push([r_bottom * s, -hh, r_bottom * c]);
        m.normals.push(n);
        m.uvs.push([t, 1.0]);
    }
    for i in 0..segments {
        let top = 2 * i;
        let bottom = top + 1;
        let (next_top, next_bottom) = (top + 2, top + 3);
        m.indices
            .extend_from_slice(&[top, bottom, next_top, next_top, bottom, next_bottom]);
    }

    // Caps.
    for (y, ny, r, flip) in [(hh, 1.0, r_top, false), (-hh, -1.0, r_bottom, true)] {
        let centre = m.positions.len() as u32;
        m.positions.push([0.0, y, 0.0]);
        m.normals.push([0.0, ny, 0.0]);
        m.uvs.push([0.5, 0.5]);
        let first = m.positions.len() as u32;
        for i in 0..=segments {
            let (s, c) = (i as f32 / segments as f32 * TAU).sin_cos();
            m.positions.push([r * s, y, r * c]);
            m.normals.push([0.0, ny, 0.0]);
            m.uvs.push([0.5 + 0.5 * s, 0.5 + 0.5 * c]);
        }
        for i in 0..segments {
            let (a, b) = (first + i, first + i + 1);
            if flip {
                m.indices.extend_from_slice(&[centre, b, a]);
            } else {
                m.indices.extend_from_slice(&[centre, a, b]);
            }
        }
    }
    m
}

/// A torus in the xy plane (its axis along z), ring radius `r`, tube radius `tube`.
pub fn torus(r: f32, tube: f32, radial: u32, tubular: u32) -> Mesh {
    let (radial, tubular) = (radial.max(3), tubular.max(3));
    let mut m = Mesh::default();
    for j in 0..=radial {
        let v = j as f32 / radial as f32 * TAU;
        let (sv, cv) = v.sin_cos();
        for i in 0..=tubular {
            let u = i as f32 / tubular as f32 * TAU;
            let (su, cu) = u.sin_cos();
            let rho = r + tube * cv;
            m.positions.push([rho * cu, rho * su, tube * sv]);
            m.normals.push([cu * cv, su * cv, sv]);
            m.uvs
                .push([i as f32 / tubular as f32, j as f32 / radial as f32]);
        }
    }
    let row = tubular + 1;
    for j in 0..radial {
        for i in 0..tubular {
            let a = j * row + i;
            let b = (j + 1) * row + i;
            let c = (j + 1) * row + i + 1;
            let d = j * row + i + 1;
            m.indices.extend_from_slice(&[a, d, b, b, d, c]);
        }
    }
    m
}

/// A flat ring (annulus) in the xy plane facing +z.
pub fn ring(r_in: f32, r_out: f32, segments: u32) -> Mesh {
    let segments = segments.max(3);
    let mut m = Mesh::default();
    let uv = |rad: f32, c: f32, s: f32| {
        let k = 0.5 * rad / r_out;
        [0.5 + k * c, 0.5 + k * s]
    };
    for i in 0..=segments {
        let (s, c) = (i as f32 / segments as f32 * TAU).sin_cos();
        m.positions.push([r_in * c, r_in * s, 0.0]);
        m.normals.push([0.0, 0.0, 1.0]);
        m.uvs.push(uv(r_in, c, s));
        m.positions.push([r_out * c, r_out * s, 0.0]);
        m.normals.push([0.0, 0.0, 1.0]);
        m.uvs.push(uv(r_out, c, s));
    }
    for i in 0..segments {
        let inner = 2 * i;
        let outer = inner + 1;
        let (next_inner, next_outer) = (inner + 2, inner + 3);
        m.indices.extend_from_slice(&[
            inner,
            outer,
            next_outer,
            inner,
            next_outer,
            next_inner,
        ]);
    }
    m
}

/// A `w` by `h` rectangle in the xy plane facing +z, uv (0, 0) at the bottom left and (1, 1) at the top right.
pub fn plane(w: f32, h: f32) -> Mesh {
    let mut m = Mesh::default();
    push_quad(&mut m, Vec3::ZERO, Vec3::Z, Vec3::X, Vec3::Y, w / 2.0, h / 2.0);
    m
}

/// A uv sphere of radius `r`.
pub fn sphere(r: f32, width_segments: u32, height_segments: u32) -> Mesh {
    let (ws, hs) = (width_segments.max(3), height_segments.max(2));
    let mut m = Mesh::default();
    for iy in 0..=hs {
        let theta = iy as f32 / hs as f32 * PI;
        let (st, ct) = theta.sin_cos();
        for ix in 0..=ws {
            let phi = ix as f32 / ws as f32 * TAU;
            let (sp, cp) = phi.sin_cos();
            let n = Vec3::new(st * cp, ct, st * sp);
            m.positions.push((n * r).to_array());
            m.normals.push(n.to_array());
            m.uvs
                .push([ix as f32 / ws as f32, 1.0 - iy as f32 / hs as f32]);
        }
    }
    let row = ws + 1;
    for iy in 0..hs {
        for ix in 0..ws {
            let a = iy * row + ix;
            let b = (iy + 1) * row + ix;
            let c = (iy + 1) * row + ix + 1;
            let d = iy * row + ix + 1;
            // The poles have no area: skip the triangles that would collapse onto them.
            if iy != 0 {
                m.indices.extend_from_slice(&[a, d, b]);
            }
            if iy != hs - 1 {
                m.indices.extend_from_slice(&[b, d, c]);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_well_formed(name: &str, m: &Mesh) {
        assert_eq!(m.indices.len() % 3, 0, "{name}: index count");
        assert!(
            m.indices.iter().all(|&i| (i as usize) < m.positions.len()),
            "{name}: index out of range"
        );
        assert_eq!(m.normals.len(), m.positions.len(), "{name}: normals");
        assert_eq!(m.uvs.len(), m.positions.len(), "{name}: uvs");
        for n in &m.normals {
            let len = Vec3::from_array(*n).length();
            assert!((len - 1.0).abs() < 1e-3, "{name}: normal length {len}");
        }
    }

    /// Every triangle faces away from the origin, which lies inside these convex solids.
    fn assert_outward(name: &str, m: &Mesh) {
        for t in m.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from_array(m.positions[i as usize]));
            let n = (b - a).cross(c - a);
            let centroid = (a + b + c) / 3.0;
            assert!(n.dot(centroid) > 0.0, "{name}: inward triangle {t:?}");
        }
    }

    #[test]
    fn primitives_are_well_formed() {
        let meshes = [
            ("cuboid", cuboid(1.0, 2.0, 3.0)),
            ("rounded_box", rounded_box(2.0, 1.0, 2.0, 0.3, 0.1)),
            ("rounded_box sharp", rounded_box(2.0, 1.0, 2.0, 0.0, 0.0)),
            ("cylinder", cylinder(1.0, 0.5, 2.0, 16)),
            ("cone", cylinder(0.0, 1.0, 1.0, 12)),
            ("torus", torus(1.0, 0.25, 8, 16)),
            ("ring", ring(0.5, 1.0, 16)),
            ("plane", plane(2.0, 1.0)),
            ("sphere", sphere(1.0, 16, 8)),
        ];
        for (name, m) in &meshes {
            assert_well_formed(name, m);
        }
    }

    #[test]
    fn cuboid_bounds_are_exact() {
        let (lo, hi) = cuboid(2.0, 4.0, 6.0).bounds();
        assert_eq!(lo, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(hi, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn rounded_box_bounds() {
        let (lo, hi) = rounded_box(2.0, 1.0, 2.0, 0.3, 0.1).bounds();
        assert!((lo - Vec3::new(-1.0, -0.5, -1.0)).length() < 1e-5, "{lo:?}");
        assert!((hi - Vec3::new(1.0, 0.5, 1.0)).length() < 1e-5, "{hi:?}");
    }

    #[test]
    fn cuboid_winding_faces_outward() {
        assert_outward("cuboid", &cuboid(2.0, 4.0, 6.0));
    }

    #[test]
    fn rounded_box_winding_faces_outward() {
        assert_outward("rounded_box", &rounded_box(2.0, 1.0, 2.0, 0.3, 0.1));
    }

    #[test]
    fn cylinder_and_sphere_winding_faces_outward() {
        assert_outward("cylinder", &cylinder(1.0, 0.5, 2.0, 16));
        assert_outward("sphere", &sphere(1.0, 16, 8));
    }

    #[test]
    fn append_offsets_indices() {
        let mut a = cuboid(1.0, 1.0, 1.0);
        let n = a.positions.len() as u32;
        a.append(&plane(1.0, 1.0));
        assert_eq!(a.triangles(), 12 + 2);
        assert_eq!(a.indices[36..39], [n, n + 1, n + 2]);
        assert_well_formed("appended", &a);
    }
}
