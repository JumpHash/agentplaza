//! three.js's round geometries, vertex for vertex (same order, normals and uvs).

use super::super::scene::{Geo, Geometry};
use std::f32::consts::{PI, TAU};

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `Vector3.lerp`: `a + (b - a) * t`.
fn lerp(a: V3, b: V3, t: f32) -> V3 {
    add(a, scale(sub(b, a), t))
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `Vector3.normalize()`: a zero vector stays zero.
fn normalize(a: V3) -> V3 {
    let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if len > 0.0 { scale(a, 1.0 / len) } else { a }
}

/// Position, normal and uv buffers under construction (one entry per vertex).
#[derive(Default)]
struct Mesh {
    p: Vec<V3>,
    n: Vec<V3>,
    uv: Vec<[f32; 2]>,
    i: Vec<u32>,
}

impl Mesh {
    fn push(&mut self, p: V3, n: V3, uv: [f32; 2]) -> u32 {
        self.p.push(p);
        self.n.push(n);
        self.uv.push(uv);
        self.p.len() as u32 - 1
    }

    fn finish(self) -> Geo {
        Geometry::new(self.p, self.n, self.uv, self.i)
    }
}

/// `SphereGeometry(r, widthSegments, heightSegments)`.
pub fn sphere(r: f32, width_segments: u32, height_segments: u32) -> Geo {
    sphere_part(r, width_segments, height_segments, 0.0, TAU, 0.0, PI)
}

/// `SphereGeometry(r, ws, hs, phiStart, phiLength, thetaStart, thetaLength)`.
pub fn sphere_part(
    r: f32,
    ws: u32,
    hs: u32,
    phi_start: f32,
    phi_length: f32,
    theta_start: f32,
    theta_length: f32,
) -> Geo {
    let (ws, hs) = (ws.max(3) as usize, hs.max(2) as usize);
    let theta_end = (theta_start + theta_length).min(PI);
    let mut m = Mesh::default();
    let mut grid: Vec<Vec<u32>> = Vec::with_capacity(hs + 1);
    for iy in 0..=hs {
        let v = iy as f32 / hs as f32;
        let u_offset = if iy == 0 && theta_start == 0.0 {
            0.5 / ws as f32
        } else if iy == hs && theta_end == PI {
            -0.5 / ws as f32
        } else {
            0.0
        };
        let theta = theta_start + v * theta_length;
        let row = (0..=ws)
            .map(|ix| {
                let u = ix as f32 / ws as f32;
                let phi = phi_start + u * phi_length;
                let p = [
                    -r * phi.cos() * theta.sin(),
                    r * theta.cos(),
                    r * phi.sin() * theta.sin(),
                ];
                m.push(p, normalize(p), [u + u_offset, 1.0 - v])
            })
            .collect();
        grid.push(row);
    }
    for iy in 0..hs {
        for ix in 0..ws {
            let a = grid[iy][ix + 1];
            let b = grid[iy][ix];
            let c = grid[iy + 1][ix];
            let d = grid[iy + 1][ix + 1];
            // The pole rows collapse to a point: their degenerate triangle is skipped, as three does.
            if iy != 0 || theta_start > 0.0 {
                m.i.extend_from_slice(&[a, b, d]);
            }
            if iy != hs - 1 || theta_end < PI {
                m.i.extend_from_slice(&[b, c, d]);
            }
        }
    }
    m.finish()
}

/// `CylinderGeometry(rTop, rBottom, h, radialSegments)` (one height segment, capped).
pub fn cylinder(r_top: f32, r_bottom: f32, h: f32, radial: u32) -> Geo {
    cylinder_full(r_top, r_bottom, h, radial, 1, false, 0.0, TAU)
}

/// `CylinderGeometry` with every parameter.
#[allow(clippy::too_many_arguments)]
pub fn cylinder_full(
    r_top: f32,
    r_bottom: f32,
    h: f32,
    radial: u32,
    height_segments: u32,
    open_ended: bool,
    theta_start: f32,
    theta_length: f32,
) -> Geo {
    let radial = radial.max(1) as usize;
    let hs = height_segments.max(1) as usize;
    let half = h / 2.0;
    let slope = (r_bottom - r_top) / h;
    let mut m = Mesh::default();

    // Torso: slope-corrected normals, uv (u, 1 - v).
    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(hs + 1);
    for y in 0..=hs {
        let v = y as f32 / hs as f32;
        let radius = v * (r_bottom - r_top) + r_top;
        let row = (0..=radial)
            .map(|x| {
                let u = x as f32 / radial as f32;
                let theta = u * theta_length + theta_start;
                let (s, c) = (theta.sin(), theta.cos());
                m.push(
                    [radius * s, -v * h + half, radius * c],
                    normalize([s, slope, c]),
                    [u, 1.0 - v],
                )
            })
            .collect();
        rows.push(row);
    }
    for x in 0..radial {
        for y in 0..hs {
            let a = rows[y][x];
            let b = rows[y + 1][x];
            let c = rows[y + 1][x + 1];
            let d = rows[y][x + 1];
            if r_top > 0.0 || y != 0 {
                m.i.extend_from_slice(&[a, b, d]);
            }
            if r_bottom > 0.0 || y != hs - 1 {
                m.i.extend_from_slice(&[b, c, d]);
            }
        }
    }

    if !open_ended {
        if r_top > 0.0 {
            cap(&mut m, r_top, half, radial, theta_start, theta_length, true);
        }
        if r_bottom > 0.0 {
            cap(
                &mut m,
                r_bottom,
                half,
                radial,
                theta_start,
                theta_length,
                false,
            );
        }
    }
    m.finish()
}

/// three's `generateCap`: one centre vertex per segment (uv 0.5, 0.5), then the rim ring.
fn cap(
    m: &mut Mesh,
    radius: f32,
    half: f32,
    radial: usize,
    theta_start: f32,
    theta_length: f32,
    top: bool,
) {
    let sign = if top { 1.0 } else { -1.0 };
    let centre = m.p.len() as u32;
    for _ in 0..radial {
        m.push([0.0, half * sign, 0.0], [0.0, sign, 0.0], [0.5, 0.5]);
    }
    let ring = m.p.len() as u32;
    for x in 0..=radial {
        let u = x as f32 / radial as f32;
        let theta = u * theta_length + theta_start;
        let (s, c) = (theta.sin(), theta.cos());
        m.push(
            [radius * s, half * sign, radius * c],
            [0.0, sign, 0.0],
            [c * 0.5 + 0.5, s * 0.5 * sign + 0.5],
        );
    }
    for x in 0..radial as u32 {
        let (c, i) = (centre + x, ring + x);
        if top {
            m.i.extend_from_slice(&[i, i + 1, c]);
        } else {
            m.i.extend_from_slice(&[i + 1, i, c]);
        }
    }
}

/// `ConeGeometry(r, h, radialSegments)`: a cylinder with a zero top radius.
pub fn cone(r: f32, h: f32, radial: u32) -> Geo {
    cylinder_full(0.0, r, h, radial, 1, false, 0.0, TAU)
}

/// `TorusGeometry(r, tube, radialSegments, tubularSegments, arc)`.
pub fn torus(r: f32, tube: f32, radial: u32, tubular: u32, arc: f32) -> Geo {
    let radial = radial.max(1) as usize;
    let tubular = tubular.max(1) as usize;
    let mut m = Mesh::default();
    for j in 0..=radial {
        for i in 0..=tubular {
            let u = i as f32 / tubular as f32 * arc;
            let v = j as f32 / radial as f32 * TAU;
            let (cu, su) = (u.cos(), u.sin());
            let (cv, sv) = (v.cos(), v.sin());
            let p = [(r + tube * cv) * cu, (r + tube * cv) * su, tube * sv];
            let centre = [r * cu, r * su, 0.0];
            m.push(
                p,
                normalize(sub(p, centre)),
                [i as f32 / tubular as f32, j as f32 / radial as f32],
            );
        }
    }
    let row = tubular + 1;
    for j in 1..=radial {
        for i in 1..=tubular {
            let a = (row * j + i - 1) as u32;
            let b = (row * (j - 1) + i - 1) as u32;
            let c = (row * (j - 1) + i) as u32;
            let d = (row * j + i) as u32;
            m.i.extend_from_slice(&[a, b, d, b, c, d]);
        }
    }
    m.finish()
}

/// The 36 faces (108 indices) of three's `DodecahedronGeometry`.
const DODECA_FACES: [usize; 108] = [
    3, 11, 7, 3, 7, 15, 3, 15, 13, 7, 19, 17, 7, 17, 6, 7, 6, 15, 17, 4, 8, 17, 8, 10, 17, 10, 6,
    8, 0, 16, 8, 16, 2, 8, 2, 10, 0, 12, 1, 0, 1, 18, 0, 18, 16, 6, 10, 2, 6, 2, 13, 6, 13, 15, 2,
    16, 18, 2, 18, 3, 2, 3, 13, 18, 1, 9, 18, 9, 11, 18, 11, 3, 4, 14, 12, 4, 12, 0, 4, 0, 8, 11,
    9, 5, 11, 5, 19, 11, 19, 7, 19, 5, 14, 19, 14, 4, 19, 4, 17, 1, 12, 14, 1, 14, 5, 1, 5, 9,
];

/// `DodecahedronGeometry(r, detail)`: a `PolyhedronGeometry` (non-indexed).
///
/// Detail 0 gets flat face normals (three's `computeVertexNormals`); detail > 0 gets
/// the normalized positions (three's `normalizeNormals`). Uvs follow three's spherical mapping.
pub fn dodecahedron(r: f32, detail: u32) -> Geo {
    // The 20 vertices of three's DodecahedronGeometry (t = golden ratio, q = 1 / t).
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let q = 1.0 / t;
    let verts: [V3; 20] = [
        [-1.0, -1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, 1.0, 1.0],
        [1.0, -1.0, -1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, -1.0],
        [1.0, 1.0, 1.0],
        [0.0, -q, -t],
        [0.0, -q, t],
        [0.0, q, -t],
        [0.0, q, t],
        [-q, -t, 0.0],
        [-q, t, 0.0],
        [q, -t, 0.0],
        [q, t, 0.0],
        [-t, 0.0, -q],
        [t, 0.0, -q],
        [-t, 0.0, q],
        [t, 0.0, q],
    ];

    // Subdivide each face, then project onto the sphere of radius r.
    let mut tris: Vec<V3> = Vec::new();
    for f in DODECA_FACES.chunks(3) {
        subdivide_face(
            verts[f[0]],
            verts[f[1]],
            verts[f[2]],
            detail as usize,
            &mut tris,
        );
    }
    for p in tris.iter_mut() {
        *p = scale(normalize(*p), r);
    }

    let normals: Vec<V3> = if detail == 0 {
        let mut n = Vec::with_capacity(tris.len());
        for t in tris.chunks(3) {
            let face = normalize(cross(sub(t[2], t[1]), sub(t[0], t[1])));
            n.extend_from_slice(&[face, face, face]);
        }
        n
    } else {
        tris.iter().map(|p| normalize(*p)).collect()
    };

    // three's spherical uv mapping, then correctUVs and correctSeam.
    let mut uvs: Vec<[f32; 2]> = tris
        .iter()
        .map(|p| {
            let azimuth = p[2].atan2(-p[0]);
            let inclination = (-p[1]).atan2((p[0] * p[0] + p[2] * p[2]).sqrt());
            [azimuth / 2.0 / PI + 0.5, 1.0 - (inclination / PI + 0.5)]
        })
        .collect();
    for (t, tri) in tris.chunks(3).enumerate() {
        let centroid = scale(add(add(tri[0], tri[1]), tri[2]), 1.0 / 3.0);
        let azi = centroid[2].atan2(-centroid[0]);
        for (k, vert) in tri.iter().enumerate() {
            let vi = t * 3 + k;
            if azi < 0.0 && uvs[vi][0] == 1.0 {
                uvs[vi][0] -= 1.0;
            }
            if vert[0] == 0.0 && vert[2] == 0.0 {
                uvs[vi][0] = azi / 2.0 / PI + 0.5;
            }
        }
    }
    for tri in uvs.chunks_mut(3) {
        let (x0, x1, x2) = (tri[0][0], tri[1][0], tri[2][0]);
        let max = x0.max(x1).max(x2);
        let min = x0.min(x1).min(x2);
        if max > 0.9 && min < 0.1 {
            for uv in tri.iter_mut() {
                if uv[0] < 0.2 {
                    uv[0] += 1.0;
                }
            }
        }
    }

    let indices: Vec<u32> = (0..tris.len() as u32).collect();
    Geometry::new(tris, normals, uvs, indices)
}

/// three's `subdivideFace`: appends the triangles of an (detail + 1)-way subdivided face.
fn subdivide_face(a: V3, b: V3, c: V3, detail: usize, out: &mut Vec<V3>) {
    let cols = detail + 1;
    let mut v: Vec<Vec<V3>> = Vec::with_capacity(cols + 1);
    for i in 0..=cols {
        let t = i as f32 / cols as f32;
        let aj = lerp(a, c, t);
        let bj = lerp(b, c, t);
        let rows = cols - i;
        let row = (0..=rows)
            .map(|j| {
                if j == 0 && i == cols {
                    aj
                } else {
                    lerp(aj, bj, j as f32 / rows as f32)
                }
            })
            .collect();
        v.push(row);
    }
    for i in 0..cols {
        for j in 0..(2 * (cols - i) - 1) {
            let k = j / 2;
            if j % 2 == 0 {
                out.extend_from_slice(&[v[i][k + 1], v[i + 1][k], v[i][k]]);
            } else {
                out.extend_from_slice(&[v[i][k + 1], v[i + 1][k + 1], v[i + 1][k]]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_sphere_has_63_vertices_on_the_unit_sphere() {
        let g = sphere(1.0, 8, 6);
        assert_eq!(g.positions.len(), 63);
        for p in &g.positions {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "length {len}");
        }
    }

    #[test]
    fn round_cylinder_torso_normals_are_horizontal() {
        let g = cylinder(1.0, 1.0, 2.0, 8);
        assert_eq!(g.normals.len(), g.positions.len());
        // The torso is the first (1 + 1) rows of (8 + 1) vertices.
        for n in &g.normals[..18] {
            assert!(n[1].abs() < 1e-6, "normal y {}", n[1]);
        }
    }

    #[test]
    fn round_cone_has_zero_radius_top() {
        let g = cone(1.0, 2.0, 8);
        for p in &g.positions[..9] {
            assert!(p[0].abs() < 1e-6 && p[2].abs() < 1e-6, "top vertex {p:?}");
            assert!((p[1] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn round_dodecahedron_has_108_vertices() {
        let g = dodecahedron(1.0, 0);
        assert_eq!(g.positions.len(), 108);
        assert_eq!(g.indices.len() / 3, 36);
    }
}
