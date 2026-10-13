//! The z-fight lint: two solid faces in one plane that overlap flicker on screen, and a plane or ring decal that sits
//! exactly on a surface with no lift. A port of apps/web/src/views/floor3d/zfight.ts with the same tolerances and the
//! same report format (see `describe`).
//!
//! The walk is the web's: each part's `when` (against the board's params), its `repeat` offsets, its transform (Euler
//! XYZ, then scale, then `at`), its geometry's planar faces in world space, its `use` (the target's parts under the
//! same transform, labelled `assets/hardware/<use>.json`) and its children. A copy whose rotation is not a multiple of
//! 90 degrees is skipped, as are curved faces (sphere, torus, and the sides of cylinders and cones).
//!
//! Differences from the web: the geometry is read as f64 from the JSON, not rounded to f32 (the web lint does not round
//! either). An inline material is named by its Debug form, not by JSON.stringify, so the name of an inline material in
//! a finding can differ from the web's; a palette name is the same in both.

use std::collections::{BTreeMap, HashMap};

use crate::geom::{apply, mul, trs, IDENTITY, M4};
use crate::hardware::{eval_when, parse_when, Geo, Library, MatRef, Model, Part, Scale};
use crate::scene::repeat_offsets;
use crate::Board;

/// Faces closer than this along their normal are coplanar.
pub const COPLANAR: f64 = 0.002;
/// Overlaps smaller than this (square units) do not count.
pub const MIN_OVERLAP: f64 = 1e-4;

type Vec3 = [f64; 3];
const ZERO: Vec3 = [0.0; 3];
const ONE: Vec3 = [1.0; 3];
const AXES: [char; 3] = ['x', 'y', 'z'];

/// A face of a solid in world space: an axis-aligned rectangle with its outward sign.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub model: String,
    /// The part's id, or its JSON path when it has none.
    pub id: String,
    /// The JSON file that holds the part.
    pub file: String,
    /// The part's JSON path inside that file, e.g. "parts[3].children[0]".
    pub path: String,
    /// The material's palette name, or a debug form of an inline material ("default" when there is none).
    pub mat: String,
    pub axis: usize,
    /// +1 when the face points along +axis, -1 when along -axis.
    pub sign: f64,
    pub plane: f64,
    /// The face's extent on the two other axes, in ascending axis order: (axis+1) % 3, then (axis+2) % 3.
    pub lo: [f64; 2],
    pub hi: [f64; 2],
    pub decal: bool,
    pub label: bool,
    /// The face is an approximation (a circle drawn as its square, a rounded corner drawn square).
    pub approx: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pair,
    Decal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub kind: Kind,
    pub why: &'static str,
    pub a: Face,
    pub b: Face,
    pub axis: usize,
    pub plane: f64,
    pub overlap: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Copies that were skipped: rotated off the 90-degree grid.
    pub skipped: Vec<String>,
    /// The number of planar faces that were checked.
    pub faces: usize,
}

/// The findings of a board's lint (see `zfight_report` for the skipped copies and the face count).
pub fn zfight(board: &Board) -> Vec<Finding> {
    zfight_report(board).findings
}

/// The lint of one board, built with the params it was built with.
pub fn zfight_report(board: &Board) -> Report {
    let lib = Library::get();
    let model = lib.model(&board.model).expect("apz-board: the board's model is in the library");
    lint_model(lib, &board.model, model, &board.params)
}

/// One line per finding, as the web's describe: the model, the reason, both parts with their files and JSON paths,
/// the axis, the plane and the overlap.
pub fn describe(f: &Finding) -> String {
    let ax = AXES[f.axis];
    format!("{} | {} | {} | {} | axis {ax} | plane {:.4} | overlap {:.3}", f.a.model, f.why, side(&f.a), side(&f.b), f.plane, f.overlap)
}

fn side(x: &Face) -> String {
    let ax = AXES[x.axis];
    let sign = if x.sign > 0.0 { '+' } else { '-' };
    let tilde = if x.approx { " ~" } else { "" };
    format!("{} [{}:{}] {}{sign}{ax}{tilde}", x.id, x.file, x.path, x.mat)
}

fn lint_model(lib: &Library, name: &str, model: &Model, params: &BTreeMap<String, f64>) -> Report {
    let mut ctx = Ctx { lib, model: name.to_string(), params: params.clone(), faces: Vec::new(), skipped: Vec::new() };
    ctx.walk(&model.parts, &IDENTITY, &format!("assets/hardware/{name}.json"), "parts");
    let Ctx { faces, skipped, .. } = ctx;
    Report { findings: lint_faces(&faces), skipped, faces: faces.len() }
}

// ---- Local faces -----------------------------------------------------------------------------------

/// A planar face in its own frame: the normal, the corners, and the flags.
struct LFace {
    n: Vec3,
    pts: Vec<Vec3>,
    decal: bool,
    approx: bool,
}

fn face(n: Vec3, pts: Vec<Vec3>, decal: bool, approx: bool) -> LFace {
    LFace { n, pts, decal, approx }
}

/// The four corners of a rectangle on the plane where coordinate `axis` equals `value`.
fn rect(axis: usize, value: f64, half: Vec3) -> Vec<Vec3> {
    let b = (axis + 1) % 3;
    let c = (axis + 2) % 3;
    let mut out = Vec::with_capacity(4);
    for sb in [-1.0, 1.0] {
        for sc in [-1.0, 1.0] {
            let mut p = ZERO;
            p[axis] = value;
            p[b] = sb * half[b];
            p[c] = sc * half[c];
            out.push(p);
        }
    }
    out
}

fn box_faces(half: Vec3, approx: bool) -> Vec<LFace> {
    let mut out = Vec::with_capacity(6);
    for (a, &h) in half.iter().enumerate() {
        for s in [-1.0, 1.0] {
            let mut n = ZERO;
            n[a] = s;
            out.push(face(n, rect(a, s * h, half), false, approx));
        }
    }
    out
}

/// The planar faces of a geometry, in its own frame. Curved faces are left out.
fn geo_faces(g: &Geo) -> Vec<LFace> {
    match g {
        Geo::Cube { w, h, d } | Geo::Rbox { w, h, d, .. } => box_faces([w / 2.0, h / 2.0, d / 2.0], false),
        Geo::Cyl { rt, rb, h, .. } => vec![
            face([0.0, 1.0, 0.0], rect(1, h / 2.0, [*rt, h / 2.0, *rt]), false, true),
            face([0.0, -1.0, 0.0], rect(1, -h / 2.0, [*rb, h / 2.0, *rb]), false, true),
        ],
        Geo::Cone { r, h, .. } => vec![face([0.0, -1.0, 0.0], rect(1, -h / 2.0, [*r, h / 2.0, *r]), false, true)],
        Geo::Sphere { .. } | Geo::Torus { .. } => Vec::new(),
        Geo::Plane { w, d } => vec![face([0.0, 1.0, 0.0], rect(1, 0.0, [w / 2.0, 0.0, d / 2.0]), true, false)],
        Geo::Ring { ro, .. } => vec![face([0.0, 1.0, 0.0], rect(1, 0.0, [*ro, 0.0, *ro]), true, true)],
        Geo::Shape { points, h } => shape_faces(points, *h),
    }
}

/// An outline in (x, z) extruded along +y by h: two caps and one side per axis-aligned edge (diagonal edges are skipped).
fn shape_faces(pts2: &[[f64; 2]], h: f64) -> Vec<LFace> {
    let n = pts2.len();
    if n == 0 {
        return Vec::new();
    }
    let at = |i: usize| pts2[i % n];
    let mut out = vec![
        face([0.0, -1.0, 0.0], pts2.iter().map(|&[x, z]| [x, 0.0, z]).collect(), false, true),
        face([0.0, 1.0, 0.0], pts2.iter().map(|&[x, z]| [x, h, z]).collect(), false, true),
    ];
    let mut area = 0.0;
    for i in 0..n {
        let [xi, zi] = at(i);
        let [xj, zj] = at(i + 1);
        area += xi * zj - xj * zi;
    }
    // CCW in (x, z): the outward normal of edge (ex, ez) is (ez, -ex).
    let flip = if area > 0.0 { 1.0 } else { -1.0 };
    for i in 0..n {
        let [x0, z0] = at(i);
        let [x1, z1] = at(i + 1);
        let (ex, ez) = (x1 - x0, z1 - z0);
        if (ex.abs() > 1e-9 && ez.abs() > 1e-9) || (ex.abs() < 1e-9 && ez.abs() < 1e-9) {
            continue;
        }
        if ex.abs() < 1e-9 {
            let s = sign_or_one(ez * flip);
            out.push(face([s, 0.0, 0.0], vec![[x0, 0.0, z0], [x0, 0.0, z1], [x0, h, z1], [x0, h, z0]], false, false));
        } else {
            let s = sign_or_one(-ex * flip);
            out.push(face([0.0, 0.0, s], vec![[x0, 0.0, z0], [x1, 0.0, z0], [x1, h, z0], [x0, h, z0]], false, false));
        }
    }
    out
}

fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// JavaScript's `Math.sign(x) || 1`.
fn sign_or_one(x: f64) -> f64 {
    let s = sign(x);
    if s == 0.0 { 1.0 } else { s }
}

// ---- Transforms ------------------------------------------------------------------------------------

/// The rotation-and-scale part of a matrix applied to a direction (the web's `turn`).
fn turn(m: &M4, v: Vec3) -> Vec3 {
    [0, 1, 2].map(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2])
}

/// True when every column of the rotation has one non-zero entry, so every axis maps onto an axis.
fn axis_aligned(m: &M4) -> bool {
    (0..3).all(|j| (0..3).filter(|&i| m[j * 4 + i].abs() > 1e-6).count() == 1)
}

fn scale_of(s: Option<&Scale>) -> Vec3 {
    match s {
        None => ONE,
        Some(Scale::Uniform(v)) => [*v; 3],
        Some(Scale::Xyz(v)) => *v,
    }
}

fn mat_key(m: Option<&MatRef>) -> String {
    match m {
        None => "default".to_string(),
        Some(MatRef::Name(n)) => n.clone(),
        Some(MatRef::Inline(x)) => format!("{x:?}"),
    }
}

/// The part a face came from: its id, its file and JSON path, and its material name.
#[derive(Clone)]
struct Meta {
    id: String,
    file: String,
    path: String,
    mat: String,
}

// ---- The walk --------------------------------------------------------------------------------------

struct Ctx<'a> {
    lib: &'a Library,
    model: String,
    params: BTreeMap<String, f64>,
    faces: Vec<Face>,
    skipped: Vec<String>,
}

impl Ctx<'_> {
    /// Walks a part list under the parent transform. `file` and `prefix` give each part its JSON path.
    fn walk(&mut self, parts: &[Part], parent: &M4, file: &str, prefix: &str) {
        for (i, p) in parts.iter().enumerate() {
            let path = format!("{prefix}[{i}]");
            if let Some(expr) = &p.when {
                match parse_when(expr) {
                    Some(w) if eval_when(&w, &self.params) => {}
                    _ => continue,
                }
            }
            let id = p.id.clone().unwrap_or_else(|| path.clone());
            let copies = match &p.repeat {
                Some(r) => repeat_offsets(r, &self.params),
                None => vec![ZERO],
            };
            for o in copies {
                let at = p.at.unwrap_or(ZERO);
                let pos = [at[0] + o[0], at[1] + o[1], at[2] + o[2]];
                let local = trs(pos, p.rot.unwrap_or(ZERO), scale_of(p.scale.as_ref()));
                let m = mul(parent, &local);
                if !axis_aligned(&m) {
                    self.skipped.push(format!("{} {file}:{path}", self.model));
                    continue;
                }
                let meta = Meta { id: id.clone(), file: file.to_string(), path: path.clone(), mat: mat_key(p.mat.as_ref()) };
                if let Some(g) = &p.geo {
                    let lf = geo_faces(g);
                    self.emit(&lf, &m, &meta, false);
                    // A label is a decal on the part's top face, drawn with no lift.
                    if let (Some(label), Some(top)) = (&p.label, lf.iter().find(|f| !f.decal && f.n[1] == 1.0)) {
                        let deco = [face(top.n, top.pts.clone(), true, true)];
                        let lmeta = Meta { mat: format!("label:{}", label.text), ..meta.clone() };
                        self.emit(&deco, &m, &lmeta, true);
                    }
                }
                if let Some(u) = &p.use_ {
                    let lib = self.lib;
                    if let Some(target) = lib.model(u) {
                        self.walk(&target.parts, &m, &format!("assets/hardware/{u}.json"), "parts");
                    }
                }
                if let Some(children) = &p.children {
                    self.walk(children, &m, file, &format!("{path}.children"));
                }
            }
        }
    }

    /// Adds the faces that survive the planarity test, in world space.
    fn emit(&mut self, lf: &[LFace], m: &M4, meta: &Meta, label: bool) {
        for f in lf {
            if f.pts.is_empty() {
                continue;
            }
            let pts: Vec<Vec3> = f.pts.iter().map(|&p| apply(m, p)).collect();
            let nw = turn(m, f.n);
            let axis = (1..3).fold(0, |best, k| if nw[k].abs() > nw[best].abs() { k } else { best });
            let count = pts.len() as f64;
            let plane = pts.iter().map(|p| p[axis]).sum::<f64>() / count;
            if pts.iter().any(|p| (p[axis] - plane).abs() > 1e-4) {
                continue;
            }
            let b = (axis + 1) % 3;
            let c = (axis + 2) % 3;
            let lo = [
                pts.iter().map(|p| p[b]).fold(f64::INFINITY, f64::min),
                pts.iter().map(|p| p[c]).fold(f64::INFINITY, f64::min),
            ];
            let hi = [
                pts.iter().map(|p| p[b]).fold(f64::NEG_INFINITY, f64::max),
                pts.iter().map(|p| p[c]).fold(f64::NEG_INFINITY, f64::max),
            ];
            self.faces.push(Face {
                model: self.model.clone(),
                id: meta.id.clone(),
                file: meta.file.clone(),
                path: meta.path.clone(),
                mat: meta.mat.clone(),
                axis,
                sign: sign(nw[axis]),
                plane,
                lo,
                hi,
                decal: f.decal,
                label,
                approx: f.approx,
            });
        }
    }
}

// ---- The pairs and the decals ----------------------------------------------------------------------

/// The area two faces of one axis share in their extents (0 when they do not overlap).
fn overlap(a: &Face, b: &Face) -> f64 {
    let w = a.hi[0].min(b.hi[0]) - a.lo[0].max(b.lo[0]);
    let h = a.hi[1].min(b.hi[1]) - a.lo[1].max(b.lo[1]);
    if w > 0.0 && h > 0.0 {
        w * h
    } else {
        0.0
    }
}

/// The plane bucket of a value, of width COPLANAR: two planes within COPLANAR are in the same or adjacent buckets.
fn cell(plane: f64) -> i64 {
    (plane / COPLANAR).floor() as i64
}

/// Pairs of solid faces that are coplanar, overlap, and either differ in material or face the same way; then the
/// decals that sit exactly on a solid face. Solids are bucketed by axis and plane, so only faces in the same or an
/// adjacent plane bucket are compared.
fn lint_faces(faces: &[Face]) -> Vec<Finding> {
    let mut buckets: HashMap<(usize, i64), Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate().filter(|(_, f)| !f.decal) {
        buckets.entry((f.axis, cell(f.plane))).or_default().push(i);
    }
    // The solid faces of f's axis whose plane is within COPLANAR of f's, in face order.
    let near = |f: &Face| -> Vec<usize> {
        let c = cell(f.plane);
        let mut out: Vec<usize> = (c - 1..=c + 1)
            .filter_map(|k| buckets.get(&(f.axis, k)))
            .flatten()
            .copied()
            .filter(|&j| (faces[j].plane - f.plane).abs() < COPLANAR)
            .collect();
        out.sort_unstable();
        out
    };

    let mut out = Vec::new();
    for axis in 0..3 {
        for (i, a) in faces.iter().enumerate() {
            if a.decal || a.axis != axis {
                continue;
            }
            for j in near(a) {
                if j <= i {
                    continue;
                }
                // The lower plane is the first face (ties keep face order), as the web's sorted sweep has it.
                let (ia, ib) = if faces[j].plane < faces[i].plane { (j, i) } else { (i, j) };
                let (pa, pb) = (&faces[ia], &faces[ib]);
                if pa.sign != pb.sign && pa.mat == pb.mat {
                    continue; // face to face, one material: a contact, never seen
                }
                let area = overlap(pa, pb);
                if area <= MIN_OVERLAP {
                    continue;
                }
                let why = if pa.mat != pb.mat {
                    "coplanar, different materials"
                } else if pa.sign == pb.sign {
                    "coplanar, same material, same way"
                } else {
                    "contact"
                };
                out.push(Finding { kind: Kind::Pair, why, a: pa.clone(), b: pb.clone(), axis, plane: pa.plane, overlap: area });
            }
        }
    }
    for d in faces.iter().filter(|f| f.decal) {
        for j in near(d) {
            let s = &faces[j];
            let area = overlap(d, s);
            if area <= MIN_OVERLAP {
                continue;
            }
            let why = if d.label { "label on a surface, no lift" } else { "decal on a surface, no lift" };
            out.push(Finding { kind: Kind::Decal, why, a: d.clone(), b: s.clone(), axis: d.axis, plane: d.plane, overlap: area });
        }
    }
    out
}
