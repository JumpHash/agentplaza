//! Tessellation of the hardware geometries, as three.js r186 builds them (BoxGeometry, RoundedBoxGeometry,
//! CylinderGeometry, SphereGeometry, TorusGeometry, PlaneGeometry, RingGeometry, ExtrudeGeometry) and the
//! transforms the renderer applies (rotateX, the object matrix).
//!
//! Vertex positions are stored as f32 in three.js, so every generated coordinate is rounded to f32 (`f32r`)
//! before any further arithmetic, and the instance matrices are rounded element by element (see `round_m4`).
//!
//! Deviation: the shape's side walls and bottom cap are not triangulated here. Their faces never face up, and
//! only the upward faces matter to the support field, so the triangle list keeps just the top cap. Their vertices
//! are still in `pos`, because the item boxes read every vertex.

use std::f64::consts::PI;

use crate::hardware::Geo;

/// A matrix in three.js's layout: 16 elements, column-major (element (row r, column c) is `m[c * 4 + r]`).
pub type M4 = [f64; 16];

pub const IDENTITY: M4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Rounds to the nearest f32 and back, as a value stored in a Float32Array.
pub fn f32r(x: f64) -> f64 {
    x as f32 as f64
}

/// Every element rounded to f32 (an InstancedMesh's instance matrix is a Float32Array).
pub fn round_m4(m: &M4) -> M4 {
    m.map(f32r)
}

/// a * b (three.js multiplyMatrices).
pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[c * 4 + k];
            }
            r[c * 4 + row] = s;
        }
    }
    r
}

/// Applies an affine matrix to a point (w is 1 for every matrix built here).
pub fn apply(m: &M4, p: [f64; 3]) -> [f64; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Degrees to radians, as three.js MathUtils.degToRad.
pub fn deg_to_rad(d: f64) -> f64 {
    d * (PI / 180.0)
}

/// Translate(pos) * Rot(rot degrees, Euler XYZ) * Scale(scale), as an Object3D's matrix (three.js compose).
pub fn trs(pos: [f64; 3], rot_deg: [f64; 3], scale: [f64; 3]) -> M4 {
    let (a, b) = (deg_to_rad(rot_deg[0]).cos(), deg_to_rad(rot_deg[0]).sin());
    let (c, d) = (deg_to_rad(rot_deg[1]).cos(), deg_to_rad(rot_deg[1]).sin());
    let (e, f) = (deg_to_rad(rot_deg[2]).cos(), deg_to_rad(rot_deg[2]).sin());
    let (ae, af, be, bf) = (a * e, a * f, b * e, b * f);
    // three.js Matrix4.makeRotationFromEuler, order XYZ (column-major elements te[0..9]).
    let r = [
        c * e,
        af + be * d,
        bf - ae * d,
        0.0,
        -c * f,
        ae - bf * d,
        be + af * d,
        0.0,
        d,
        -b * c,
        a * c,
        0.0,
    ];
    let mut m = [0.0; 16];
    for col in 0..3 {
        for row in 0..3 {
            m[col * 4 + row] = r[col * 4 + row] * scale[col];
        }
    }
    m[12] = pos[0];
    m[13] = pos[1];
    m[14] = pos[2];
    m[15] = 1.0;
    m
}

/// Triangle-soup geometry: vertex positions (f32 values as f64) and index triangles into them.
#[derive(Debug, Clone, Default)]
pub struct Geometry {
    pub pos: Vec<[f64; 3]>,
    pub tris: Vec<[usize; 3]>,
}

impl Geometry {
    /// The largest y of any vertex (the value a label sits on), or 0 when there are none.
    pub fn top(&self) -> f64 {
        self.pos
            .iter()
            .map(|p| p[1])
            .fold(None, |m: Option<f64>, y| Some(m.map_or(y, |m| m.max(y))))
            .unwrap_or(0.0)
    }

    fn rotate_x(&mut self, angle: f64) {
        let (c, s) = (angle.cos(), angle.sin());
        for p in &mut self.pos {
            let [x, y, z] = *p;
            *p = [f32r(x), f32r(c * y - s * z), f32r(s * y + c * z)];
        }
    }
}

/// Builds a part's geometry as the renderer does (makeGeometry in hardware.ts).
pub fn make(geo: &Geo) -> Geometry {
    match geo {
        Geo::Cube { w, h, d } => box_geo(*w, *h, *d, 1, 1, 1),
        Geo::Rbox { w, h, d, r } => rounded_box(*w, *h, *d, *r),
        Geo::Cyl { rt, rb, h, seg } => cylinder(*rt, *rb, *h, seg.unwrap_or(24)),
        Geo::Cone { r, h, seg } => cylinder(0.0, *r, *h, seg.unwrap_or(24)),
        Geo::Sphere { r } => sphere(*r, 24, 16),
        Geo::Torus { r, tube, arc } => torus(*r, *tube, 10, 32, deg_to_rad(arc.unwrap_or(360.0))),
        Geo::Plane { w, d } => plane(*w, *d),
        Geo::Ring { ri, ro } => {
            let mut g = ring(*ri, *ro, 48);
            g.rotate_x(-PI / 2.0);
            g
        }
        Geo::Shape { points, h } => shape(points, *h),
    }
}

/// The geometry of a label: a plane 4 * size wide and size deep, laid flat (PlaneGeometry(size * 4, size)).
pub fn label(size: f64) -> Geometry {
    plane(size * 4.0, size)
}

/// BoxGeometry's buildPlane: one face of the box, a grid of gx by gy segments. The argument order and signs
/// follow three.js exactly (the -width and -height of the negative faces are passed as they are).
#[allow(clippy::too_many_arguments)]
fn build_plane(
    g: &mut Geometry,
    u: usize,
    v: usize,
    w: usize,
    udir: f64,
    vdir: f64,
    width: f64,
    height: f64,
    depth: f64,
    gx: usize,
    gy: usize,
) {
    let seg_w = width / gx as f64;
    let seg_h = height / gy as f64;
    let wh = width / 2.0;
    let hh = height / 2.0;
    let dh = depth / 2.0;
    let base = g.pos.len();
    for iy in 0..=gy {
        let y = iy as f64 * seg_h - hh;
        for ix in 0..=gx {
            let x = ix as f64 * seg_w - wh;
            let mut vec = [0.0; 3];
            vec[u] = x * udir;
            vec[v] = y * vdir;
            vec[w] = dh;
            g.pos.push(vec.map(f32r));
        }
    }
    let row = gx + 1;
    for iy in 0..gy {
        for ix in 0..gx {
            let a = base + ix + row * iy;
            let b = base + ix + row * (iy + 1);
            let c = base + (ix + 1) + row * (iy + 1);
            let d = base + (ix + 1) + row * iy;
            g.tris.push([a, b, d]);
            g.tris.push([b, c, d]);
        }
    }
}

/// BoxGeometry(w, h, d, sx, sy, sz).
fn box_geo(w: f64, h: f64, d: f64, sx: usize, sy: usize, sz: usize) -> Geometry {
    let mut g = Geometry::default();
    build_plane(&mut g, 2, 1, 0, -1.0, -1.0, d, h, w, sz, sy); // px
    build_plane(&mut g, 2, 1, 0, 1.0, -1.0, d, h, -w, sz, sy); // nx
    build_plane(&mut g, 0, 2, 1, 1.0, 1.0, w, d, h, sx, sz); // py
    build_plane(&mut g, 0, 2, 1, 1.0, -1.0, w, d, -h, sx, sz); // ny
    build_plane(&mut g, 0, 1, 2, 1.0, -1.0, w, h, d, sx, sy); // pz
    build_plane(&mut g, 0, 1, 2, -1.0, -1.0, w, h, -d, sx, sy); // nz
    g
}

fn js_sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// RoundedBoxGeometry(w, h, d, 3, r): BoxGeometry(1, 1, 1, 7, 7, 7), non-indexed, each grid point pushed onto the
/// rounded shell (RoundedBoxGeometry.js).
fn rounded_box(w: f64, h: f64, d: f64, r: f64) -> Geometry {
    let total = 3 * 2 + 1;
    let radius = (w / 2.0).min(h / 2.0).min(d / 2.0).min(r);
    let base = box_geo(1.0, 1.0, 1.0, total, total, total);
    let bx = [w / 2.0 - radius, h / 2.0 - radius, d / 2.0 - radius];
    let half_seg = 0.5 / total as f64;
    let mut out = Geometry::default();
    for tri in &base.tris {
        for &vi in tri {
            let p = base.pos[vi];
            let mut n = p.map(|c| c - js_sign(c) * half_seg);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let len = if len == 0.0 { 1.0 } else { len };
            n = n.map(|c| c / len);
            let q = [
                f32r(bx[0] * js_sign(p[0]) + n[0] * radius),
                f32r(bx[1] * js_sign(p[1]) + n[1] * radius),
                f32r(bx[2] * js_sign(p[2]) + n[2] * radius),
            ];
            out.pos.push(q);
        }
        let k = out.pos.len();
        out.tris.push([k - 3, k - 2, k - 1]);
    }
    out
}

/// CylinderGeometry(rt, rb, h, radialSegments, 1, false); a cone is the cylinder with rt = 0.
fn cylinder(rt: f64, rb: f64, h: f64, seg: u32) -> Geometry {
    let rs = seg as usize;
    let mut g = Geometry::default();
    let half = h / 2.0;
    // Torso: two rows (heightSegments = 1).
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for y in 0..=1usize {
        let mut row = Vec::new();
        let v = y as f64 / 1.0;
        let radius = v * (rb - rt) + rt;
        for x in 0..=rs {
            let u = x as f64 / rs as f64;
            let theta = u * (2.0 * PI);
            g.pos.push([
                f32r(radius * theta.sin()),
                f32r(-v * h + half),
                f32r(radius * theta.cos()),
            ]);
            row.push(g.pos.len() - 1);
        }
        rows.push(row);
    }
    for x in 0..rs {
        let (a, b, c, d) = (rows[0][x], rows[1][x], rows[1][x + 1], rows[0][x + 1]);
        if rt > 0.0 {
            g.tris.push([a, b, d]);
        }
        if rb > 0.0 {
            g.tris.push([b, c, d]);
        }
        // One height segment (y = 0): the first triangle needs rt > 0, the second needs rb > 0.
    }
    // Caps: a centre fan per cap, drawn only when the radius is above zero.
    for top in [true, false] {
        let radius = if top { rt } else { rb };
        if radius <= 0.0 {
            continue;
        }
        let sign = if top { 1.0 } else { -1.0 };
        let centre_start = g.pos.len();
        for _ in 0..rs {
            g.pos.push([0.0, f32r(half * sign), 0.0]);
        }
        let ring_start = g.pos.len();
        for x in 0..=rs {
            let u = x as f64 / rs as f64;
            let theta = u * (2.0 * PI);
            g.pos.push([
                f32r(radius * theta.sin()),
                f32r(half * sign),
                f32r(radius * theta.cos()),
            ]);
        }
        for x in 0..rs {
            let c = centre_start + x;
            let i = ring_start + x;
            if top {
                g.tris.push([i, i + 1, c]);
            } else {
                g.tris.push([i + 1, i, c]);
            }
        }
    }
    g
}

/// SphereGeometry(r, widthSegments, heightSegments) with thetaStart 0 and thetaLength pi.
fn sphere(r: f64, ws: usize, hs: usize) -> Geometry {
    let mut g = Geometry::default();
    let mut grid: Vec<Vec<usize>> = Vec::new();
    for iy in 0..=hs {
        let mut row = Vec::new();
        let v = iy as f64 / hs as f64;
        let theta = v * PI;
        let y = r * theta.cos();
        let ring_radius = (r * r - y * y).sqrt();
        for ix in 0..=ws {
            let u = ix as f64 / ws as f64;
            let phi = u * (2.0 * PI);
            g.pos.push([
                f32r(-ring_radius * phi.cos()),
                f32r(y),
                f32r(ring_radius * phi.sin()),
            ]);
            row.push(g.pos.len() - 1);
        }
        grid.push(row);
    }
    for iy in 0..hs {
        for ix in 0..ws {
            let a = grid[iy][ix + 1];
            let b = grid[iy][ix];
            let c = grid[iy + 1][ix];
            let d = grid[iy + 1][ix + 1];
            // thetaStart is 0, so the top row's first triangle is skipped; thetaEnd is pi, so the bottom row's second is.
            if iy != 0 {
                g.tris.push([a, b, d]);
            }
            if iy != hs - 1 {
                g.tris.push([b, c, d]);
            }
        }
    }
    g
}

/// TorusGeometry(r, tube, radialSegments, tubularSegments, arc).
fn torus(r: f64, tube: f64, radial: usize, tubular: usize, arc: f64) -> Geometry {
    let mut g = Geometry::default();
    for j in 0..=radial {
        let v = (j as f64 / radial as f64) * (2.0 * PI);
        for i in 0..=tubular {
            let u = i as f64 / tubular as f64 * arc;
            let rr = r + tube * v.cos();
            g.pos
                .push([f32r(rr * u.cos()), f32r(rr * u.sin()), f32r(tube * v.sin())]);
        }
    }
    let row = tubular + 1;
    for j in 1..=radial {
        for i in 1..=tubular {
            let a = row * j + i - 1;
            let b = row * (j - 1) + i - 1;
            let c = row * (j - 1) + i;
            let d = row * j + i;
            g.tris.push([a, b, d]);
            g.tris.push([b, c, d]);
        }
    }
    g
}

/// PlaneGeometry(w, d) with one segment, laid flat (rotateX(-pi / 2)): vertices are (x, -y, 0) as three.js stores them.
fn plane(w: f64, d: f64) -> Geometry {
    let mut g = Geometry::default();
    for iy in 0..=1usize {
        let y = iy as f64 * d - d / 2.0;
        for ix in 0..=1usize {
            let x = ix as f64 * w - w / 2.0;
            g.pos.push([f32r(x), f32r(-y), 0.0]);
        }
    }
    for iy in 0..1usize {
        for ix in 0..1usize {
            let a = ix + 2 * iy;
            let b = ix + 2 * (iy + 1);
            let c = (ix + 1) + 2 * (iy + 1);
            let dd = (ix + 1) + 2 * iy;
            g.tris.push([a, b, dd]);
            g.tris.push([b, c, dd]);
        }
    }
    g.rotate_x(-PI / 2.0);
    g
}

/// RingGeometry(ri, ro, thetaSegments, 1) (not yet laid flat).
fn ring(ri: f64, ro: f64, theta_segments: usize) -> Geometry {
    let mut g = Geometry::default();
    let step = ro - ri;
    let mut radius = ri;
    for _j in 0..=1usize {
        for i in 0..=theta_segments {
            let seg = i as f64 / theta_segments as f64 * (2.0 * PI);
            g.pos
                .push([f32r(radius * seg.cos()), f32r(radius * seg.sin()), 0.0]);
        }
        radius += step;
    }
    let level = theta_segments + 1;
    for i in 0..theta_segments {
        let a = i;
        let b = i + level;
        let c = i + level + 1;
        let d = i + 1;
        g.tris.push([a, b, d]);
        g.tris.push([b, c, d]);
    }
    g
}

/// ExtrudeGeometry of the outline (x, -z) with depth h and no bevel, laid flat (rotateX(-pi / 2)). Every face is
/// kept: the top and bottom caps (ear-clipped) and the side walls. Each face is wound so that its normal points out of
/// the solid in the geometry's own frame (three.js winds its faces outward), which is what decides whether a face is
/// upward once the part is rotated: a shape turned on its side has upward walls, not an upward top.
fn shape(points: &[[f64; 2]], h: f64) -> Geometry {
    let mut pts: Vec<[f64; 2]> = points.iter().map(|p| [p[0], -p[1]]).collect();
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    let mut g = Geometry::default();
    for p in &pts {
        g.pos.push([f32r(p[0]), f32r(p[1]), 0.0]);
    }
    for p in &pts {
        g.pos.push([f32r(p[0]), f32r(p[1]), f32r(h)]);
    }
    let ccw = signed_area(&pts, &(0..n).collect::<Vec<_>>()) > 0.0;
    let mut faces: Vec<([usize; 3], [f64; 3])> = Vec::new();
    for t in ear_clip(&pts) {
        // The top cap (z = h) faces +z; the bottom cap uses the same triangles reversed and faces -z.
        faces.push(([n + t[0], n + t[1], n + t[2]], [0.0, 0.0, 1.0]));
        faces.push(([t[0], t[2], t[1]], [0.0, 0.0, -1.0]));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (dx, dy) = (pts[j][0] - pts[i][0], pts[j][1] - pts[i][1]);
        // The outward normal of an edge of a counter-clockwise outline is (dy, -dx); the other way round, (-dy, dx).
        let out = if ccw { [dy, -dx, 0.0] } else { [-dy, dx, 0.0] };
        faces.push(([i, j, n + j], out));
        faces.push(([i, n + j, n + i], out));
    }
    for (tri, want) in faces {
        let (a, b, c) = (g.pos[tri[0]], g.pos[tri[1]], g.pos[tri[2]]);
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let nrm = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let facing = nrm[0] * want[0] + nrm[1] * want[1] + nrm[2] * want[2];
        if facing < 0.0 {
            g.tris.push([tri[0], tri[2], tri[1]]);
        } else {
            g.tris.push(tri);
        }
    }
    g.rotate_x(-PI / 2.0);
    g
}

/// Ear clipping of a simple polygon. Returns index triples into `pts`, each counter-clockwise in the plane.
fn ear_clip(pts: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    if signed_area(pts, &idx) < 0.0 {
        idx.reverse();
    }
    let mut out = Vec::new();
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (p, c, q) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            if cross(pts[p], pts[c], pts[q]) <= 1e-12 {
                continue;
            }
            let blocked = idx.iter().any(|&o| {
                o != p && o != c && o != q && inside_or_on(pts[o], pts[p], pts[c], pts[q])
            });
            if blocked {
                continue;
            }
            out.push([p, c, q]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // A degenerate polygon: fan what is left so that no area is lost silently.
            for k in 1..idx.len() - 1 {
                out.push([idx[0], idx[k], idx[k + 1]]);
            }
            return out;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

fn signed_area(pts: &[[f64; 2]], idx: &[usize]) -> f64 {
    let m = idx.len();
    (0..m)
        .map(|i| {
            let a = pts[idx[i]];
            let b = pts[idx[(i + 1) % m]];
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn inside_or_on(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let d1 = cross(a, b, p);
    let d2 = cross(b, c, p);
    let d3 = cross(c, a, p);
    d1 >= -1e-12 && d2 >= -1e-12 && d3 >= -1e-12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_has_six_faces_of_two_triangles_per_cell() {
        let g = make(&Geo::Cube {
            w: 2.0,
            h: 1.0,
            d: 3.0,
        });
        assert_eq!(g.pos.len(), 24);
        assert_eq!(g.tris.len(), 12);
        let top = g.pos.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        assert_eq!(top, 0.5);
    }

    #[test]
    fn rounded_box_stays_inside_its_size() {
        let g = make(&Geo::Rbox {
            w: 4.0,
            h: 2.0,
            d: 6.0,
            r: 0.5,
        });
        let max = |k: usize| g.pos.iter().map(|p| p[k]).fold(f64::MIN, f64::max);
        assert!((max(0) - 2.0).abs() < 1e-6);
        assert!((max(1) - 1.0).abs() < 1e-6);
        assert!((max(2) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn shape_top_cap_is_upward_at_its_height() {
        let g = make(&Geo::Shape {
            points: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]],
            h: 0.5,
        });
        // A square: two caps of two triangles, and four walls of two triangles.
        assert_eq!(g.tris.len(), 12);
        let upward: Vec<&[usize; 3]> = g
            .tris
            .iter()
            .filter(|t| t.iter().all(|&v| (g.pos[v][1] - 0.5).abs() < 1e-6))
            .collect();
        assert_eq!(
            upward.len(),
            2,
            "the top cap is the only flat face at the top"
        );
    }

    #[test]
    fn f32_rounding_is_applied_to_positions() {
        let g = make(&Geo::Sphere { r: 0.1 });
        for p in &g.pos {
            for c in p {
                assert_eq!(*c, *c as f32 as f64);
            }
        }
    }

    #[test]
    fn trs_matches_scale_then_translate() {
        let m = trs([1.0, 2.0, 3.0], [0.0, 0.0, 0.0], [2.0, 3.0, 4.0]);
        assert_eq!(apply(&m, [1.0, 1.0, 1.0]), [3.0, 5.0, 7.0]);
    }
}
