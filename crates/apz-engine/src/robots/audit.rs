//! The robot design audit (port of apps/web/src/views/floor3d/robots/audit.ts and seat.ts): pure geometry over a
//! species, checked for eye fit and seating, contact, overlap, symmetry, ground, tool holds and proportions.
//! Every part is sampled from the geometry the rig bakes, placed by the bone chain (the rest pose, and the walk and
//! wave clips at mid-cycle), and checked. Tolerances and the style rules are in assets/robots/README.md ("Style").

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

use glam::{DMat4, DQuat, DVec3, EulerRot};

use super::clips::{eval_clip, resolve};
use super::files::registry;
use super::geometry::part_geometry;
use super::types::{Clip, Eye, Gait, MatRef, Part, Species, Tool};

// Tolerances. Robot units: a species is about 1 high.
const SAMPLES: usize = 48;
const CONTACT: f64 = 0.02;
const GROUND: f64 = 0.01;
const EYE_FIT: f64 = 0.5;
const BURIED: f64 = 0.3;
const LIMB: f64 = 0.03;
const SYM: f64 = 0.02;
const MEDIAN_TOL: f64 = 0.25;
const PIERCE: f64 = 0.01;
const MATTE: [&str; 9] = [
    "shell", "shell2", "panel", "paper", "trim", "ink", "signal", "glass", "accent",
];
const EPS: f64 = 1e-6;

/// The portrait cameras of the contact sheets (robot-lab.ts CAM_POS). Keep in step.
pub const PORTRAIT_CAMS: [(&str, [f64; 3]); 2] =
    [("front", [0.0, 1.1, 4.4]), ("q34", [-2.44, 1.84, 3.48])];
pub const LAB_TARGET: [f64; 3] = [0.0, 0.7, 0.0];
/// The pupil sits this far in front of its socket (geometry.ts eyeGeometries).
pub const PUPIL_LIFT: f64 = 0.004;
/// The probes of an eye: the pupil centre, then four rim points at 85% of the socket radius.
pub const EYE_PROBES: usize = 5;
/// Of the probes, at least this many must be clear from each portrait camera (the pupil always must be).
pub const EYE_CLEAR_MIN: usize = 4;
const RIM_INSET: f64 = 0.85;
/// A part hides a point only when it is at least this much nearer the camera than the point.
const HIDE_GAP: f64 = 0.002;
/// A rim point this far in front of the socket disc counts as clipped (the rounding of the seat).
const SEAT_SLACK: f64 = 0.001;
/// The seat tool lifts a socket this far clear of the highest rim point.
pub const SEAT_EPS: f64 = 0.005;

/// The style rules for the seat: eyes 0.35–0.45 of head width apart (half spacing 0.175–0.225, aimed at 0.205),
/// 45–60% up the head (aimed at 49%).
const HALF_MIN: f64 = 0.175;
const HALF_AIM: f64 = 0.205;
const HALF_MAX: f64 = 0.225;
const HEIGHT_MIN: f64 = 0.45;
const HEIGHT_AIM: f64 = 0.49;
const HEIGHT_MAX: f64 = 0.6;
/// A socket centre may float off the head surface by this share of its radius (the audit's eye-fit limit).
const FLOAT_SHARE: f64 = 0.5;
/// The lift search steps (robot units), and the spots tried after the current one.
const STEP: f64 = 0.0025;
const SPOTS: usize = 12;
/// A socket this close to the centre line is central (x = 0).
const CENTRE: f64 = 0.02;

const METRIC_KEYS: [&str; 8] = [
    "height",
    "width",
    "headToBody",
    "headToHeight",
    "eyeRelHead",
    "eyeSpacing",
    "eyeHeight",
    "inkCoverage",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    Rest,
    Walk,
    Wave,
}

impl Pose {
    pub fn name(self) -> &'static str {
        match self {
            Pose::Rest => "rest",
            Pose::Walk => "walk",
            Pose::Wave => "wave",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    EyeFit,
    EyePupil,
    EyeSilhouette,
    EyeOverlap,
    EyePair,
    EyeOccluded,
    FeatureFloat,
    FeatureBuried,
    Attach,
    LimbThroughBody,
    Symmetry,
    Ground,
    Tool,
    Proportion,
}

impl IssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueKind::EyeFit => "eye-fit",
            IssueKind::EyePupil => "eye-pupil",
            IssueKind::EyeSilhouette => "eye-silhouette",
            IssueKind::EyeOverlap => "eye-overlap",
            IssueKind::EyePair => "eye-pair",
            IssueKind::EyeOccluded => "eye-occluded",
            IssueKind::FeatureFloat => "feature-float",
            IssueKind::FeatureBuried => "feature-buried",
            IssueKind::Attach => "attach",
            IssueKind::LimbThroughBody => "limb-through-body",
            IssueKind::Symmetry => "symmetry",
            IssueKind::Ground => "ground",
            IssueKind::Tool => "tool",
            IssueKind::Proportion => "proportion",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub kind: IssueKind,
    pub severity: Severity,
    /// A JSON path into the species file (or the tool file), e.g. `parts[12]`, `eyes[1]`.
    pub path: String,
    /// The part's name, or `bone:geo:mat#index`.
    pub label: String,
    pub pose: Pose,
    pub message: String,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub id: String,
    pub locomotion: String,
    pub floats: bool,
    pub parts: usize,
    pub eyes: usize,
    /// Overall size (rest pose, robot units).
    pub height: f64,
    pub width: f64,
    pub depth: f64,
    pub min_y: f64,
    pub head_height: f64,
    pub head_width: f64,
    pub body_height: Option<f64>,
    /// head height / body height.
    pub head_to_body: Option<f64>,
    /// head height / overall height.
    pub head_to_height: f64,
    /// mean socket radius / head width.
    pub eye_rel_head: Option<f64>,
    /// mean nearest-neighbour distance between eye sockets / head width.
    pub eye_spacing: Option<f64>,
    /// mean socket height above the head's base, as a fraction of head height.
    pub eye_height: Option<f64>,
    /// Area share of the surface that carries the ink line.
    pub ink_coverage: f64,
}

/// The median of each proportion metric over a set of species, by metric name.
pub type Medians = BTreeMap<&'static str, f64>;

#[derive(Default)]
pub struct AuditOptions<'a> {
    /// Tools for the hold check (default: the registry's).
    pub tools: Option<&'a BTreeMap<String, Tool>>,
    /// Shared clips for the walk and wave poses (default: the registry's).
    pub clips: Option<&'a BTreeMap<String, Clip>>,
    /// Medians for the proportion check (default: the registry's species).
    pub medians: Option<&'a Medians>,
}

/// A part (or a tool part) placed in the world: its triangles, sample points and bounds.
pub struct Piece {
    pub path: String,
    pub label: String,
    pub bone: String,
    pub geo_kind: String,
    pub asym: bool,
    pub mat: String,
    /// A plane or ring: no volume at all.
    pub surface: bool,
    /// Every edge shared by two triangles: a solid, so a point can be inside it.
    pub closed: bool,
    pub inked: bool,
    /// World triangle soup: three vertices per triangle.
    pub verts: Vec<DVec3>,
    /// World sample points (a stride of the vertices).
    pub samples: Vec<DVec3>,
    pub min: DVec3,
    pub max: DVec3,
    pub centre: DVec3,
    /// The longest bounding edge.
    pub size: f64,
    /// Half the bounding diagonal.
    pub radius: f64,
    pub area: f64,
}

/// A bone's posed transform: position offset, XYZ Euler rotation (radians), uniform scale.
#[derive(Clone, Copy)]
struct BonePose {
    pos: DVec3,
    rot: DVec3,
    scale: f64,
}

/// A pose's placed parts and the bone matrices they hang from.
pub struct Scene {
    pub bones: HashMap<String, DMat4>,
    pub pieces: Vec<Piece>,
}

struct EyeView<'a> {
    cam: &'static str,
    /// Per probe: the piece that hides it from this camera, or None when it is clear.
    hiders: Vec<Option<&'a Piece>>,
}

/// An eye's geometry as the seat and the probes read it.
#[derive(Clone, Copy)]
struct EyeGeom {
    at: DVec3,
    r: f64,
    pupil: f64,
    glass_off: bool,
}

// ---- Geometry helpers ----

fn normal(v: DVec3) -> DVec3 {
    let l = v.length();
    v / if l == 0.0 { 1.0 } else { l }
}

fn ray_dir() -> DVec3 {
    normal(DVec3::new(0.3714, 0.5571, 0.7428))
}

fn at_of(e: &Eye) -> DVec3 {
    DVec3::new(f64::from(e.at[0]), f64::from(e.at[1]), f64::from(e.at[2]))
}

fn bone_of(m: &HashMap<String, DMat4>, name: &str) -> DMat4 {
    m.get(name).copied().unwrap_or(DMat4::IDENTITY)
}

/// A ring of nine points: the centre, then eight on the circle of radius r.
fn ring9(x: f64, y: f64, r: f64) -> Vec<(f64, f64)> {
    let mut out = vec![(x, y)];
    for i in 0..8 {
        let a = (i as f64 / 8.0) * TAU;
        out.push((x + r * a.cos(), y + r * a.sin()));
    }
    out
}

fn label_of(p: &Part, i: usize) -> String {
    match p.name.as_deref().filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => format!(
            "{}:{}:{}#{i}",
            p.bone.as_deref().unwrap_or("tool"),
            p.geo.kind,
            mat_name(p)
        ),
    }
}

fn mat_name(p: &Part) -> String {
    match &p.mat {
        MatRef::Name(n) => n.clone(),
        MatRef::Inline(_) => "inline".to_string(),
    }
}

/// Triangle soup in world space: the geometry's vertices (expanded from its indices), moved by `m`.
fn soup(positions: &[[f32; 3]], indices: &[u32], m: DMat4) -> Vec<DVec3> {
    let vert = |i: usize| {
        let p = positions[i];
        m.transform_point3(DVec3::new(
            f64::from(p[0]),
            f64::from(p[1]),
            f64::from(p[2]),
        ))
    };
    if indices.is_empty() {
        (0..positions.len()).map(vert).collect()
    } else {
        indices.iter().map(|&i| vert(i as usize)).collect()
    }
}

/// Whether the triangles form a closed surface (each edge used twice). Degenerate edges (a pole) are skipped.
fn is_closed(verts: &[DVec3]) -> bool {
    let key = |v: DVec3| {
        [
            (v.x * 1e4).round() as i64,
            (v.y * 1e4).round() as i64,
            (v.z * 1e4).round() as i64,
        ]
    };
    let mut edges: HashMap<([i64; 3], [i64; 3]), u32> = HashMap::new();
    for t in verts.as_chunks::<3>().0 {
        let k = [key(t[0]), key(t[1]), key(t[2])];
        for i in 0..3 {
            let (a, b) = (k[i], k[(i + 1) % 3]);
            if a == b {
                continue;
            }
            let e = if a < b { (a, b) } else { (b, a) };
            *edges.entry(e).or_insert(0) += 1;
        }
    }
    !edges.is_empty() && edges.values().all(|&n| n == 2)
}

fn make_piece(
    path: String,
    label: String,
    bone: String,
    p: &Part,
    verts: Vec<DVec3>,
    radius: f64,
) -> Piece {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &verts {
        min = min.min(*v);
        max = max.max(*v);
    }
    let stride = (verts.len() / SAMPLES).max(1);
    let samples: Vec<DVec3> = verts.iter().step_by(stride).copied().collect();
    let mut area = 0.0;
    for t in verts.as_chunks::<3>().0 {
        area += 0.5 * (t[1] - t[0]).cross(t[2] - t[0]).length();
    }
    let mat = mat_name(p);
    let extent = max - min;
    let diag = extent.length();
    let inked =
        MATTE.contains(&mat.as_str()) && p.outline.unwrap_or(mat != "ink") && radius >= 0.03;
    Piece {
        path,
        label,
        bone,
        geo_kind: p.geo.kind.clone(),
        asym: p.asym == Some(true),
        surface: p.geo.kind == "plane" || p.geo.kind == "ring",
        closed: is_closed(&verts),
        inked,
        mat,
        verts,
        samples,
        min,
        max,
        centre: (min + max) / 2.0,
        size: extent.max_element(),
        radius: diag / 2.0,
        area,
    }
}

fn pieces_of(parts: &[Part], prefix: &str, mat_of: impl Fn(&Part) -> DMat4) -> Vec<Piece> {
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (geo, radius) = part_geometry(p);
            let verts = soup(&geo.positions, &geo.indices, mat_of(p));
            let bone = p.bone.clone().unwrap_or_else(|| "root".to_string());
            make_piece(
                format!("{prefix}[{i}]"),
                label_of(p, i),
                bone,
                p,
                verts,
                f64::from(radius),
            )
        })
        .collect()
}

fn species_pieces(s: &Species, bones: &HashMap<String, DMat4>) -> Vec<Piece> {
    pieces_of(&s.parts, "parts", |p| {
        bone_of(bones, p.bone.as_deref().unwrap_or("root"))
    })
}

// ---- Bones: the rest pose and the posed bone matrices (rig: position = rest + pos, rotation XYZ, uniform scale) ----

fn bone_matrices(s: &Species, pose: Option<&HashMap<String, BonePose>>) -> HashMap<String, DMat4> {
    let mut out = HashMap::new();
    for name in s.bones.keys() {
        world(s, pose, name, 0, &mut out);
    }
    out
}

fn world(
    s: &Species,
    pose: Option<&HashMap<String, BonePose>>,
    name: &str,
    depth: u32,
    out: &mut HashMap<String, DMat4>,
) -> DMat4 {
    if let Some(m) = out.get(name) {
        return *m;
    }
    let Some(b) = s.bones.get(name) else {
        return DMat4::IDENTITY;
    };
    if name == "root" || depth > 32 {
        return DMat4::IDENTITY;
    }
    let p = pose.and_then(|m| m.get(name));
    let off = p.map_or(DVec3::ZERO, |p| p.pos);
    let rot = p.map_or(DVec3::ZERO, |p| p.rot);
    let sc = p.map_or(1.0, |p| p.scale);
    let at = DVec3::new(f64::from(b.at[0]), f64::from(b.at[1]), f64::from(b.at[2]));
    let q = DQuat::from_euler(EulerRot::XYZ, rot.x, rot.y, rot.z);
    let local = DMat4::from_scale_rotation_translation(DVec3::splat(sc), q, at + off);
    let m = world(s, pose, &b.parent, depth + 1, out) * local;
    out.insert(name.to_string(), m);
    m
}

fn clip_for<'a>(s: &'a Species, id: &str, shared: &'a BTreeMap<String, Clip>) -> Option<&'a Clip> {
    if let Some(own) = s.overrides.get(id)
        && let Some(c) = s.clips.get(own)
    {
        return Some(c);
    }
    s.clips.get(id).or_else(|| shared.get(id))
}

/// The pose of a clip at its mid-cycle (the first looping channel at half a period); one-shots at their peak.
fn pose_of(clip: &Clip, gait: &Gait) -> HashMap<String, BonePose> {
    let ch = clip
        .channels
        .iter()
        .find(|c| c.rate.is_none() && resolve(c.freq.as_ref(), gait, 0.0) > 0.0);
    let f = ch.map_or(0.0, |c| resolve(c.freq.as_ref(), gait, 0.0));
    let t = if f > 0.0 { 0.5 / f } else { 0.0 };
    let elapsed = match clip.dur {
        Some(d) if clip.looped == Some(false) && d != 0.0 => f64::from(d) / 2.0,
        _ => 0.0,
    };
    let mut out: HashMap<String, BonePose> = HashMap::new();
    eval_clip(clip, t, gait, elapsed, |ch, value| {
        let (bone, prop) = (ch.bone.as_str(), ch.prop.as_str());
        if bone.is_empty() || prop.is_empty() {
            return;
        }
        let p = out.entry(bone.to_string()).or_insert(BonePose {
            pos: DVec3::ZERO,
            rot: DVec3::ZERO,
            scale: 1.0,
        });
        let axis = match prop.chars().last() {
            Some('y') => 1,
            Some('z') => 2,
            _ => 0,
        };
        if prop.starts_with("rot.") {
            p.rot[axis] = value;
        } else if prop.starts_with("pos.") {
            p.pos[axis] = value;
        } else if prop == "scale" {
            p.scale = value;
        }
    });
    out
}

fn scene_of(s: &Species, bones: HashMap<String, DMat4>) -> Scene {
    let pieces = species_pieces(s, &bones);
    Scene { bones, pieces }
}

/// The rest scene, and the walk and wave scenes (the clips at mid-cycle, as the audit poses them).
pub fn scenes_of(s: &Species, clips: &BTreeMap<String, Clip>) -> Vec<(Pose, Scene)> {
    let mut out = vec![(Pose::Rest, scene_of(s, bone_matrices(s, None)))];
    let walk = clip_for(s, &format!("walk.{}", s.locomotion), clips)
        .or_else(|| clip_for(s, "walk.treads", clips));
    let wave = clip_for(s, "wave", clips);
    for (pose, clip) in [(Pose::Walk, walk), (Pose::Wave, wave)] {
        if let Some(c) = clip {
            out.push((
                pose,
                scene_of(s, bone_matrices(s, Some(&pose_of(c, &s.gait)))),
            ));
        }
    }
    out
}

// ---- Geometry queries: distance to a triangle, ray hits, containment (closed pieces only) ----

/// Squared distance from p to triangle t (Ericson, real-time collision detection).
fn tri_dist_sq(t: &[DVec3], p: DVec3) -> f64 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, ac) = (b - a, c - a);
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (p - a).length_squared();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (p - b).length_squared();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (p - (a + ab * v)).length_squared();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (p - c).length_squared();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (p - (a + ac * w)).length_squared();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length_squared();
    }
    let den = 1.0 / (va + vb + vc);
    let (v, w) = (vb * den, vc * den);
    (p - (a + ab * v + ac * w)).length_squared()
}

/// The distance from p to the piece's surface.
fn dist_to(pc: &Piece, p: DVec3) -> f64 {
    pc.verts
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| tri_dist_sq(t, p))
        .fold(f64::INFINITY, f64::min)
        .sqrt()
}

/// Möller–Trumbore, both faces: the ray parameter of a hit, or None.
fn ray_tri(t: &[DVec3], o: DVec3, d: DVec3) -> Option<f64> {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-12 {
        return None;
    }
    let f = 1.0 / det;
    let s = o - t[0];
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = f * d.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(f * e2.dot(q))
}

/// Every hit (t ≥ 0) of a ray on the pieces' surfaces, sorted.
pub fn ray_hits(list: &[&Piece], o: DVec3, d: DVec3) -> Vec<f64> {
    let mut out = Vec::new();
    for pc in list {
        for t in pc.verts.as_chunks::<3>().0 {
            if let Some(hit) = ray_tri(t, o, d)
                && hit >= -EPS
            {
                out.push(hit);
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    out
}

/// Inside a closed piece (parity of crossings along a fixed direction). Surfaces are never inside.
fn inside_of(pc: &Piece, p: DVec3) -> bool {
    if !pc.closed {
        return false;
    }
    let d = ray_dir();
    let n = pc
        .verts
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| ray_tri(t.as_slice(), p, d).is_some_and(|hit| hit > EPS))
        .count();
    n % 2 == 1
}

/// The first piece of the list that contains the point.
fn inside_any<'a>(list: &[&'a Piece], p: DVec3) -> Option<&'a Piece> {
    list.iter().copied().find(|pc| inside_of(pc, p))
}

/// The gap between two pieces: 0 when one samples inside the other, else the nearest sample-to-surface distance.
fn gap_of(a: &Piece, b: &Piece) -> f64 {
    let reach = a.radius + b.radius + 0.5;
    if (a.centre - b.centre).length() > reach {
        return f64::INFINITY;
    }
    let mut best = f64::INFINITY;
    for (x, y) in [(a, b), (b, a)] {
        for &p in &x.samples {
            if inside_of(y, p) {
                return 0.0;
            }
            best = best.min(dist_to(y, p));
        }
    }
    best
}

fn volume(pc: &Piece) -> f64 {
    let e = pc.max - pc.min;
    e.x * e.y * e.z
}

/// The biggest closed piece on a bone (the head shell or the body), or None.
fn largest_solid<'a>(list: &[&'a Piece]) -> Option<&'a Piece> {
    let mut best: Option<&'a Piece> = None;
    for &pc in list {
        if pc.surface || pc.mat == "clear" || pc.mat == "glass" {
            continue;
        }
        if best.is_none_or(|b| volume(pc) > volume(b)) {
            best = Some(pc);
        }
    }
    best
}

/// The nearest piece that hides `to` from `from` (its surface is nearer than `to` by HIDE_GAP or more), or None.
/// Clear parts do not hide.
fn hider_of<'a>(list: &[&'a Piece], from: DVec3, to: DVec3) -> Option<&'a Piece> {
    let d = to - from;
    let len = d.length();
    let limit = 1.0 - HIDE_GAP / if len == 0.0 { 1.0 } else { len };
    let mut best: Option<&'a Piece> = None;
    let mut best_t = limit;
    for &pc in list {
        if pc.mat == "clear" {
            continue;
        }
        for t in pc.verts.as_chunks::<3>().0 {
            if let Some(hit) = ray_tri(t, from, d)
                && hit > EPS
                && hit < best_t
            {
                best_t = hit;
                best = Some(pc);
            }
        }
    }
    best
}

// ---- Eyes: probes, views and visibility ----

fn eye_geom(e: &Eye) -> EyeGeom {
    EyeGeom {
        at: at_of(e),
        r: f64::from(e.r.unwrap_or(0.1)),
        pupil: f64::from(e.pupil),
        glass_off: e.glass == Some(false),
    }
}

/// An eye's probes in world space: the pupil centre, then the rim (the socket radius, or the pupil's without a socket).
fn eye_probes(g: EyeGeom, bone: DMat4) -> Vec<DVec3> {
    let base = if g.glass_off { g.pupil } else { g.r };
    let rr = base * RIM_INSET;
    let mut pts = vec![bone.transform_point3(g.at + DVec3::new(0.0, 0.0, PUPIL_LIFT))];
    for i in 0..EYE_PROBES - 1 {
        let a = i as f64 * PI / 2.0;
        pts.push(bone.transform_point3(g.at + DVec3::new(rr * a.cos(), rr * a.sin(), 0.0)));
    }
    pts
}

/// The eye's probes seen from each portrait camera, with what hides each one.
fn eye_views<'a>(g: EyeGeom, bone: DMat4, list: &[&'a Piece]) -> Vec<EyeView<'a>> {
    let probes = eye_probes(g, bone);
    PORTRAIT_CAMS
        .iter()
        .map(|&(cam, pos)| EyeView {
            cam,
            hiders: probes
                .iter()
                .map(|&p| hider_of(list, DVec3::from(pos), p))
                .collect(),
        })
        .collect()
}

fn clear_of(v: &EyeView) -> usize {
    v.hiders.iter().filter(|h| h.is_none()).count()
}

/// Whether an eye is seen: from each portrait camera its pupil is clear and at least EYE_CLEAR_MIN probes are.
fn eye_seen(views: &[EyeView]) -> bool {
    views
        .iter()
        .all(|v| v.hiders[0].is_none() && clear_of(v) >= EYE_CLEAR_MIN)
}

fn eye_label(e: &Eye, k: usize) -> String {
    format!("eye {} {} #{k}", e.bone, e.shape)
}

#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Issue>,
    kind: IssueKind,
    severity: Severity,
    path: &str,
    label: &str,
    pose: Pose,
    value: Option<f64>,
    message: String,
) {
    out.push(Issue {
        kind,
        severity,
        path: path.to_string(),
        label: label.to_string(),
        pose,
        message,
        value,
    });
}

struct Socket {
    k: usize,
    c: DVec3,
    r: f64,
}

/// The eyes: socket on the head surface, pupil inside the socket, socket inside the silhouette, eye vs eye.
fn check_eyes(
    s: &Species,
    pieces: &[&Piece],
    bones: &HashMap<String, DMat4>,
    issues: &mut Vec<Issue>,
) {
    let mut sockets: Vec<Socket> = Vec::new();
    for (k, e) in s.eyes.iter().enumerate() {
        let bone = bone_of(bones, &e.bone);
        let c = bone.transform_point3(at_of(e));
        // The socket faces +z in its bone's frame.
        let dir = normal(bone.transform_vector3(DVec3::Z));
        let r = f64::from(e.r.unwrap_or(0.1));
        let path = format!("eyes[{k}]");
        let label = eye_label(e, k);
        sockets.push(Socket { k, c, r });
        let on_bone: Vec<&Piece> = pieces
            .iter()
            .copied()
            .filter(|p| p.bone == e.bone)
            .collect();
        let Some(primary) = largest_solid(&on_bone) else {
            push(
                issues,
                IssueKind::EyeFit,
                Severity::Error,
                &path,
                &label,
                Pose::Rest,
                None,
                format!("no solid head on bone \"{}\" to seat the eye", e.bone),
            );
            continue;
        };
        // Fit: the head surface must lie behind the disc at its centre and at every rim point, and the centre may
        // float off the surface by at most 50% of r. The surface is the union of every solid on the eye's bone.
        let solids: Vec<&Piece> = on_bone
            .iter()
            .copied()
            .filter(|p| !p.surface && p.mat != "clear" && p.mat != "glass")
            .collect();
        let surface_z = |x: f64, y: f64| -> Option<f64> {
            ray_hits(
                &solids,
                DVec3::new(x, y, c.z + 10.0),
                DVec3::new(0.0, 0.0, -1.0),
            )
            .first()
            .map(|h| c.z + 10.0 - h)
        };
        let centre_z = surface_z(c.x, c.y);
        let samples = ring9(c.x, c.y, r);
        let clips: Vec<f64> = samples
            .iter()
            .filter_map(|&(x, y)| surface_z(x, y))
            .filter(|&z| z > c.z + SEAT_SLACK)
            .collect();
        match centre_z {
            None => push(
                issues,
                IssueKind::EyeFit,
                Severity::Error,
                &path,
                &label,
                Pose::Rest,
                None,
                "no head surface in front of the socket centre (the eye floats in the air)"
                    .to_string(),
            ),
            Some(_) if !clips.is_empty() => {
                let worst = clips.iter().copied().fold(f64::NEG_INFINITY, f64::max) - c.z;
                push(
                    issues,
                    IssueKind::EyeFit,
                    Severity::Error,
                    &path,
                    &label,
                    Pose::Rest,
                    Some(worst),
                    format!(
                        "the head surface clips the socket: {} of {} points lie up to {worst:.3} in front of the disc (seat the socket at the rim's highest surface plus {SEAT_EPS})",
                        clips.len(),
                        samples.len()
                    ),
                );
            }
            Some(cz) if c.z - cz > EYE_FIT * r => push(
                issues,
                IssueKind::EyeFit,
                Severity::Error,
                &path,
                &label,
                Pose::Rest,
                Some(c.z - cz),
                format!(
                    "socket floats {:.3} off the head (limit {:.3} = 50% of r {r})",
                    c.z - cz,
                    EYE_FIT * r
                ),
            ),
            Some(_) => {}
        }
        // Pupil: its footprint must sit inside the glass radius.
        let pupil = f64::from(e.pupil);
        let stretch = f64::from(e.stretch.unwrap_or(1.0));
        let hw = if e.shape == "visor" { 1.5 } else { 1.0 } * pupil;
        let hh = match e.shape.as_str() {
            "bar" => 0.25,
            "visor" => 0.45,
            _ => 1.0,
        } * pupil
            * stretch;
        if e.glass != Some(false) && (hw > r * 1.02 || hh > r * 1.02) {
            push(
                issues,
                IssueKind::EyePupil,
                Severity::Warn,
                &path,
                &label,
                Pose::Rest,
                Some(hw.max(hh) / r),
                format!(
                    "pupil {} ({}) spills out of the socket r {r}",
                    e.pupil, e.shape
                ),
            );
        }
        // Silhouette: the socket's rim must be over the head, seen from the front.
        let up = if dir.x.abs() < 0.9 {
            normal(dir.cross(DVec3::X))
        } else {
            normal(dir.cross(DVec3::Y))
        };
        let side = dir.cross(up);
        let mut missing = 0usize;
        for i in 0..8 {
            let a = (i as f64 / 8.0) * TAU;
            let p = c + r * (a.cos() * up + a.sin() * side);
            let from = p + dir * 10.0;
            if ray_hits(&[primary], from, -dir).is_empty() {
                missing += 1;
            }
        }
        if missing >= 2 {
            push(
                issues,
                IssueKind::EyeSilhouette,
                Severity::Error,
                &path,
                &label,
                Pose::Rest,
                Some(missing as f64),
                format!("{missing} of 8 rim points of the socket are off the head's silhouette"),
            );
        }
        // Overlap: a face feature (not a plate) lying under the socket's footprint.
        let features: Vec<&Piece> = on_bone
            .iter()
            .copied()
            .filter(|p| p.mat != "clear" && !std::ptr::eq(*p, primary) && p.mat != "paper")
            .collect();
        for f in features {
            let mut hits = 0usize;
            for &p in &f.samples {
                let l = p - c;
                let a = l.dot(dir);
                let lat = (l.length_squared() - a * a).max(0.0).sqrt();
                if lat <= 0.9 * r && a >= -0.3 * r && a <= 0.5 * r {
                    hits += 1;
                }
            }
            if hits > 0 {
                push(
                    issues,
                    IssueKind::EyeOverlap,
                    Severity::Error,
                    &path,
                    &label,
                    Pose::Rest,
                    Some(hits as f64),
                    format!(
                        "overlaps {} ({hits} sample points inside its footprint)",
                        f.label
                    ),
                );
            }
        }
    }
    // Eye vs eye: sockets on the same face must not overlap.
    for i in 0..sockets.len() {
        for j in i + 1..sockets.len() {
            let (a, b) = (&sockets[i], &sockets[j]);
            let d = (a.c - b.c).length();
            if d < 0.9 * (a.r + b.r) {
                push(
                    issues,
                    IssueKind::EyePair,
                    Severity::Error,
                    &format!("eyes[{}]", a.k),
                    &format!("eye #{} / eye #{}", a.k, b.k),
                    Pose::Rest,
                    Some(d),
                    format!(
                        "eye #{} and eye #{} overlap (centres {d:.3} apart, radii {} and {})",
                        a.k, b.k, a.r, b.r
                    ),
                );
            }
        }
    }
}

/// Eye mirror check: an eye off the centre line needs a twin at −x (same size, shape and pupil), unless `asym`.
fn check_eye_symmetry(s: &Species, bones: &HashMap<String, DMat4>, issues: &mut Vec<Issue>) {
    let socket = |e: &Eye| bone_of(bones, &e.bone).transform_point3(at_of(e));
    for (k, e) in s.eyes.iter().enumerate() {
        if e.asym == Some(true) {
            continue;
        }
        let c = socket(e);
        if c.x.abs() <= SYM {
            continue;
        }
        let r = e.r.unwrap_or(0.1);
        let twin = s.eyes.iter().enumerate().any(|(j, o)| {
            if j == k {
                return false;
            }
            let d = socket(o);
            (d.x + c.x).abs() <= SYM
                && (d.y - c.y).abs() <= SYM
                && (d.z - c.z).abs() <= SYM
                && o.r.unwrap_or(0.1) == r
                && o.shape == e.shape
                && o.pupil == e.pupil
        });
        if !twin {
            push(
                issues,
                IssueKind::Symmetry,
                Severity::Error,
                &format!("eyes[{k}]"),
                &eye_label(e, k),
                Pose::Rest,
                None,
                "no matching eye at the mirror position (or not the same size and shape); set \"asym\": true if intended".to_string(),
            );
        }
    }
}

/// Eye visibility from the portrait cameras: no part may hide the pupil, and EYE_CLEAR_MIN probes must be clear.
fn check_eye_visibility(s: &Species, scene: &Scene, pose: Pose, issues: &mut Vec<Issue>) {
    let list: Vec<&Piece> = scene.pieces.iter().collect();
    for (k, e) in s.eyes.iter().enumerate() {
        let bone = bone_of(&scene.bones, &e.bone);
        let mut faults: Vec<String> = Vec::new();
        let mut clear_min = EYE_PROBES;
        for v in eye_views(eye_geom(e), bone, &list) {
            let clear = clear_of(&v);
            if v.hiders[0].is_none() && clear >= EYE_CLEAR_MIN {
                continue;
            }
            clear_min = clear_min.min(clear);
            let mut who: Vec<String> = Vec::new();
            for h in v.hiders.iter().flatten() {
                let t = format!("{} '{}'", h.path, h.label);
                if !who.contains(&t) {
                    who.push(t);
                }
            }
            let who = who.join(", ");
            faults.push(if v.hiders[0].is_some() {
                format!("pupil hidden from the {} camera by {who}", v.cam)
            } else {
                format!("only {clear} of {EYE_PROBES} probes clear from the {} camera (hidden by {who})", v.cam)
            });
        }
        if !faults.is_empty() {
            push(
                issues,
                IssueKind::EyeOccluded,
                Severity::Error,
                &format!("eyes[{k}]"),
                &eye_label(e, k),
                pose,
                Some(clear_min as f64),
                faults.join("; "),
            );
        }
    }
}

/// Face features (anything on a face bone other than its shell): on the surface, not buried.
fn check_features(pieces: &[&Piece], face_bones: &[String], issues: &mut Vec<Issue>) {
    for bone in face_bones {
        let on: Vec<&Piece> = pieces.iter().copied().filter(|p| p.bone == *bone).collect();
        let Some(primary) = largest_solid(&on) else {
            continue;
        };
        for &f in &on {
            if std::ptr::eq(f, primary) || f.mat == "clear" {
                continue;
            }
            let mut gap = f64::INFINITY;
            for &o in &on {
                if !std::ptr::eq(o, f) {
                    gap = gap.min(gap_of(f, o));
                }
            }
            if gap > CONTACT {
                push(
                    issues,
                    IssueKind::FeatureFloat,
                    Severity::Error,
                    &f.path,
                    &f.label,
                    Pose::Rest,
                    Some(gap),
                    format!("floats {gap:.3} off the head (nothing on the bone touches it)"),
                );
            }
            if !f.surface {
                let mut depth = 0.0f64;
                for &p in &f.samples {
                    if inside_of(primary, p) {
                        depth = depth.max(dist_to(primary, p));
                    }
                }
                let ratio = depth / f.size.max(1e-6);
                if ratio > BURIED {
                    push(
                        issues,
                        IssueKind::FeatureBuried,
                        Severity::Warn,
                        &f.path,
                        &f.label,
                        Pose::Rest,
                        Some(ratio),
                        format!(
                            "buried {:.0}% of its size in the head (limit 30%)",
                            ratio * 100.0
                        ),
                    );
                }
            }
        }
    }
}

/// The bones related to a bone: itself, its parent, its children. Root-level bones (base, body) form the trunk.
fn related(s: &Species, bone: &str) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::from([bone.to_string()]);
    let parent = s.bones.get(bone).map(|b| b.parent.as_str());
    if let Some(p) = parent
        && p != "root"
    {
        out.insert(p.to_string());
    }
    let trunk = parent.is_none_or(|p| p == "root");
    for (name, b) in &s.bones {
        if b.parent == bone {
            out.insert(name.clone());
        }
        if trunk && b.parent == "root" {
            out.insert(name.clone());
        }
    }
    out
}

/// Every part touches or intersects a part on its bone, its parent bone or its child bones.
fn check_attach(s: &Species, pieces: &[&Piece], pose: Pose, issues: &mut Vec<Issue>) {
    for &pc in pieces {
        let near = related(s, &pc.bone);
        let mut gap = f64::INFINITY;
        for &o in pieces {
            if std::ptr::eq(o, pc) || !near.contains(&o.bone) {
                continue;
            }
            gap = gap.min(gap_of(pc, o));
            if gap <= CONTACT {
                break;
            }
        }
        if gap > CONTACT {
            let shown = if gap.is_finite() {
                format!("{gap:.3}")
            } else {
                "free".to_string()
            };
            push(
                issues,
                IssueKind::Attach,
                Severity::Error,
                &pc.path,
                &pc.label,
                pose,
                Some(gap),
                format!("floats {shown} from every part on its bone, its parent and its children"),
            );
        }
    }
}

/// Whether a bone hangs from the body (walks up the chain to `body`).
fn descends(s: &Species, bone: &str) -> bool {
    let mut b: Option<&str> = Some(bone);
    let mut d = 0;
    while let Some(name) = b {
        if name == "root" || d >= 32 {
            break;
        }
        if name == "body" {
            return true;
        }
        b = s.bones.get(name).map(|x| x.parent.as_str());
        d += 1;
    }
    false
}

/// Limbs (the body's descendants other than the body's head and carry bones) must not sink into the body.
fn check_limbs(s: &Species, pieces: &[&Piece], pose: Pose, issues: &mut Vec<Issue>) {
    let body: Vec<&Piece> = pieces
        .iter()
        .copied()
        .filter(|p| p.bone == "body")
        .collect();
    for &pc in pieces {
        if pc.bone == "body" || pc.bone == "carry" || !descends(s, &pc.bone) {
            continue;
        }
        let mut deepest = 0.0f64;
        for &p in &pc.samples {
            if let Some(inside) = inside_any(&body, p) {
                deepest = deepest.max(dist_to(inside, p));
            }
        }
        if deepest > LIMB {
            push(
                issues,
                IssueKind::LimbThroughBody,
                Severity::Warn,
                &pc.path,
                &pc.label,
                pose,
                Some(deepest),
                format!(
                    "sinks {deepest:.3} into the body in the {} pose",
                    pose.name()
                ),
            );
        }
    }
}

/// Mirror partners: a part off the centre line needs a twin at −x (same geometry, size and material), unless `asym`.
fn check_symmetry(pieces: &[&Piece], issues: &mut Vec<Issue>) {
    for &pc in pieces {
        if pc.centre.x.abs() <= SYM || pc.asym {
            continue;
        }
        let twin = pieces.iter().any(|&o| {
            !std::ptr::eq(o, pc)
                && o.geo_kind == pc.geo_kind
                && o.mat == pc.mat
                && (o.centre.x + pc.centre.x).abs() <= SYM
                && (o.centre.y - pc.centre.y).abs() <= SYM
                && (o.centre.z - pc.centre.z).abs() <= SYM
                && (o.size - pc.size).abs() <= SYM + 0.01
        });
        if !twin {
            push(
                issues,
                IssueKind::Symmetry,
                Severity::Error,
                &pc.path,
                &pc.label,
                Pose::Rest,
                None,
                format!(
                    "no mirror partner at x {:.3} (set \"asym\": true if intended)",
                    -pc.centre.x
                ),
            );
        }
    }
}

/// The lowest point of a grounded species sits on the floor; a floating one must not sink into it.
fn check_ground(s: &Species, pieces: &[&Piece], issues: &mut Vec<Issue>) {
    let low = pieces.iter().map(|p| p.min.y).fold(f64::INFINITY, f64::min);
    if !s.floats && low.abs() > GROUND {
        let feet = if low > 0.0 { "float" } else { "sink" };
        push(
            issues,
            IssueKind::Ground,
            Severity::Error,
            "parts",
            "lowest point",
            Pose::Rest,
            Some(low),
            format!("lowest point at y {low:.3}, not 0 (feet {feet})"),
        );
    } else if s.floats && low < -GROUND {
        push(
            issues,
            IssueKind::Ground,
            Severity::Error,
            "parts",
            "lowest point",
            Pose::Rest,
            Some(low),
            format!("floats but sinks to y {low:.3}"),
        );
    }
}

#[derive(Default)]
struct GripHit {
    n: usize,
    names: Vec<String>,
    /// (tool id, depth, the body part it pierces).
    worst: Option<(String, f64, String)>,
}

/// A held tool (one hand, or two) must not pierce the head or body. The grip's frame is the rig's (holdTool).
fn check_tools(
    bones: &HashMap<String, DMat4>,
    tools: &BTreeMap<String, Tool>,
    pieces: &[&Piece],
    issues: &mut Vec<Issue>,
) {
    let body: Vec<&Piece> = pieces
        .iter()
        .copied()
        .filter(|p| (p.bone == "head" || p.bone == "body") && !p.surface)
        .collect();
    let origin_r = bone_of(bones, "gripR").transform_point3(DVec3::ZERO);
    let origin_l = bone_of(bones, "gripL").transform_point3(DVec3::ZERO);
    let mut by_grip: Vec<(String, GripHit)> = Vec::new();
    for (id, tool) in tools {
        if tool.grip != "one" && tool.grip != "two" {
            continue;
        }
        let m = if tool.grip == "two" {
            let dir = normal(origin_l - origin_r);
            DMat4::from_scale_rotation_translation(
                DVec3::ONE,
                DQuat::from_rotation_arc(DVec3::X, dir),
                origin_r,
            )
        } else {
            let rx = match tool.aim.as_deref().unwrap_or("forward") {
                "forward" => -PI / 2.0,
                "up" => PI,
                _ => 0.0,
            };
            DMat4::from_scale_rotation_translation(
                DVec3::ONE,
                DQuat::from_euler(EulerRot::XYZ, rx, 0.0, 0.0),
                origin_r,
            )
        };
        let parts = pieces_of(&tool.parts, &format!("tools/{id}.json parts"), |_| m);
        let mut worst: Option<(f64, String)> = None;
        for tp in &parts {
            for bp in &body {
                if tp.max.x < bp.min.x - 0.02
                    || tp.min.x > bp.max.x + 0.02
                    || tp.max.y < bp.min.y - 0.02
                    || tp.min.y > bp.max.y + 0.02
                    || tp.max.z < bp.min.z - 0.02
                    || tp.min.z > bp.max.z + 0.02
                {
                    continue;
                }
                for &p in &tp.samples {
                    if inside_of(bp, p) {
                        let depth = dist_to(bp, p);
                        if depth > worst.as_ref().map_or(0.0, |w| w.0) {
                            worst = Some((depth, bp.label.clone()));
                        }
                    }
                }
            }
        }
        if let Some((depth, into)) = worst
            && depth > PIERCE
        {
            let idx = match by_grip.iter().position(|(g, _)| *g == tool.grip) {
                Some(i) => i,
                None => {
                    by_grip.push((tool.grip.clone(), GripHit::default()));
                    by_grip.len() - 1
                }
            };
            let hit = &mut by_grip[idx].1;
            hit.n += 1;
            hit.names.push(id.clone());
            if hit.worst.as_ref().is_none_or(|w| depth > w.1) {
                hit.worst = Some((id.clone(), depth, into));
            }
        }
    }
    // One issue per grip: the count of tools that pierce, and the worst of them.
    for (grip, hit) in by_grip {
        if let Some((wid, wdepth, winto)) = hit.worst {
            push(
                issues,
                IssueKind::Tool,
                Severity::Error,
                "parts",
                &format!("{grip}-hand grip"),
                Pose::Rest,
                Some(wdepth),
                format!(
                    "{} {grip}-handed tool(s) pierce the head or body ({}); worst {wid} into {winto} by {wdepth:.3}",
                    hit.n,
                    hit.names.join(", ")
                ),
            );
        }
    }
}

fn metric(m: &Metrics, k: &str) -> Option<f64> {
    match k {
        "height" => Some(m.height),
        "width" => Some(m.width),
        "headToBody" => m.head_to_body,
        "headToHeight" => Some(m.head_to_height),
        "eyeRelHead" => m.eye_rel_head,
        "eyeSpacing" => m.eye_spacing,
        "eyeHeight" => m.eye_height,
        "inkCoverage" => Some(m.ink_coverage),
        _ => None,
    }
}

fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        None
    } else {
        Some(xs.iter().sum::<f64>() / xs.len() as f64)
    }
}

/// The metrics: the numbers the style guide is written from.
pub fn metrics(s: &Species) -> Metrics {
    let bones = bone_matrices(s, None);
    let pieces_owned = species_pieces(s, &bones);
    let pieces: Vec<&Piece> = pieces_owned.iter().collect();
    let span = |list: &[&Piece], k: usize| -> f64 {
        if list.is_empty() {
            0.0
        } else {
            list.iter()
                .map(|p| p.max[k])
                .fold(f64::NEG_INFINITY, f64::max)
                - list.iter().map(|p| p.min[k]).fold(f64::INFINITY, f64::min)
        }
    };
    let head: Vec<&Piece> = pieces
        .iter()
        .copied()
        .filter(|p| p.bone == "head")
        .collect();
    let body: Vec<&Piece> = pieces
        .iter()
        .copied()
        .filter(|p| p.bone == "body")
        .collect();
    let height = span(&pieces, 1);
    let head_height = span(&head, 1);
    let head_width = span(&head, 0);
    let head_min_y = if head.is_empty() {
        0.0
    } else {
        head.iter().map(|p| p.min.y).fold(f64::INFINITY, f64::min)
    };
    let sockets: Vec<(f64, DVec3)> = s
        .eyes
        .iter()
        .map(|e| {
            (
                f64::from(e.r.unwrap_or(0.1)),
                bone_of(&bones, &e.bone).transform_point3(at_of(e)),
            )
        })
        .collect();
    let nearest: Vec<f64> = (0..sockets.len())
        .map(|i| {
            (0..sockets.len())
                .filter(|&j| j != i)
                .map(|j| (sockets[i].1 - sockets[j].1).length())
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    let mut total = 0.0;
    let mut inked = 0.0;
    for p in &pieces {
        total += p.area;
        if p.inked {
            inked += p.area;
        }
    }
    let body_height = if body.is_empty() {
        None
    } else {
        Some(span(&body, 1))
    };
    Metrics {
        id: s.id.clone(),
        locomotion: s.locomotion.clone(),
        floats: s.floats,
        parts: s.parts.len(),
        eyes: s.eyes.len(),
        height,
        width: span(&pieces, 0),
        depth: span(&pieces, 2),
        min_y: if pieces.is_empty() {
            0.0
        } else {
            pieces.iter().map(|p| p.min.y).fold(f64::INFINITY, f64::min)
        },
        head_height,
        head_width,
        body_height,
        head_to_body: body_height.filter(|b| *b > 0.0).map(|b| head_height / b),
        head_to_height: if height > 0.0 {
            head_height / height
        } else {
            0.0
        },
        eye_rel_head: if head_width > 0.0 {
            mean(
                &sockets
                    .iter()
                    .map(|(r, _)| r / head_width)
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        },
        eye_spacing: if head_width > 0.0 {
            mean(
                &nearest
                    .iter()
                    .filter(|d| d.is_finite())
                    .map(|d| d / head_width)
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        },
        eye_height: if head_height > 0.0 {
            mean(
                &sockets
                    .iter()
                    .map(|(_, c)| (c.y - head_min_y) / head_height)
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        },
        ink_coverage: if total > 0.0 { inked / total } else { 0.0 },
    }
}

/// The median of each proportion metric over a set of species (absent values skipped).
pub fn medians(list: &[Metrics]) -> Medians {
    METRIC_KEYS
        .iter()
        .map(|&k| {
            let mut xs: Vec<f64> = list.iter().filter_map(|m| metric(m, k)).collect();
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let n = xs.len();
            let v = if n == 0 {
                0.0
            } else if n % 2 == 1 {
                xs[(n - 1) / 2]
            } else {
                (xs[n / 2 - 1] + xs[n / 2]) / 2.0
            };
            (k, v)
        })
        .collect()
}

/// The medians over the registry's species, computed once.
fn default_medians() -> &'static Medians {
    static MEDIANS: OnceLock<Medians> = OnceLock::new();
    MEDIANS.get_or_init(|| {
        let list: Vec<Metrics> = registry().species.values().map(metrics).collect();
        medians(&list)
    })
}

fn check_proportions(m: &Metrics, med: &Medians, issues: &mut Vec<Issue>) {
    for k in METRIC_KEYS {
        let Some(v) = metric(m, k) else { continue };
        let x = med.get(k).copied().unwrap_or(0.0);
        if x == 0.0 || x.is_nan() {
            continue;
        }
        let rel = (v - x) / x.abs();
        if rel.abs() > MEDIAN_TOL {
            push(
                issues,
                IssueKind::Proportion,
                Severity::Warn,
                "metrics",
                k,
                Pose::Rest,
                Some(v),
                format!(
                    "{k} {v:.3} is {:.0}% from the median {x:.3} of the species",
                    rel * 100.0
                ),
            );
        }
    }
}

/// The audit of one species. Tools and clips default to the registry's; medians to the registry's species.
pub fn audit_species(s: &Species, opts: &AuditOptions) -> Vec<Issue> {
    let reg = registry();
    let tools = opts.tools.unwrap_or(&reg.tools);
    let shared = opts.clips.unwrap_or(&reg.clips);
    let med = opts.medians.unwrap_or(default_medians());
    let mut issues = Vec::new();
    let bones = bone_matrices(s, None);
    let pieces_owned = species_pieces(s, &bones);
    let pieces: Vec<&Piece> = pieces_owned.iter().collect();
    let mut face_bones: Vec<String> = vec!["head".to_string()];
    for e in &s.eyes {
        if !face_bones.contains(&e.bone) {
            face_bones.push(e.bone.clone());
        }
    }
    let scenes = scenes_of(s, shared);

    check_eyes(s, &pieces, &bones, &mut issues);
    check_eye_symmetry(s, &bones, &mut issues);
    for (pose, scene) in &scenes {
        check_eye_visibility(s, scene, *pose, &mut issues);
    }
    check_features(&pieces, &face_bones, &mut issues);
    check_attach(s, &pieces, Pose::Rest, &mut issues);
    check_limbs(s, &pieces, Pose::Rest, &mut issues);
    check_symmetry(&pieces, &mut issues);
    check_ground(s, &pieces, &mut issues);
    check_tools(&bones, tools, &pieces, &mut issues);
    check_proportions(&metrics(s), med, &mut issues);
    for (pose, scene) in &scenes {
        if *pose == Pose::Rest {
            continue;
        }
        let posed: Vec<&Piece> = scene.pieces.iter().collect();
        check_attach(s, &posed, *pose, &mut issues);
        check_limbs(s, &posed, *pose, &mut issues);
    }
    issues
}

/// The audit of every species, with the proportion medians taken over the whole set (unless given).
pub fn audit_all(list: &[Species], opts: &AuditOptions) -> BTreeMap<String, Vec<Issue>> {
    let owned: Medians;
    let med: &Medians = match opts.medians {
        Some(m) => m,
        None => {
            let metrics_list: Vec<Metrics> = list.iter().map(metrics).collect();
            owned = medians(&metrics_list);
            &owned
        }
    };
    let o = AuditOptions {
        tools: opts.tools,
        clips: opts.clips,
        medians: Some(med),
    };
    list.iter()
        .map(|s| (s.id.clone(), audit_species(s, &o)))
        .collect()
}

// ---- The eye seat (seat.ts): where each socket goes, under the style rules. A socket is a flat disc facing +z.
// The seat puts the disc on the head's front: the surface behind the disc at every rim point (plus SEAT_EPS), the
// centre floating at most 50% of r, and the least lift that leaves the pupil and rim visible from the portrait
// cameras in the rest, walk and wave poses. The lateral spot is the current one when it works; otherwise the flattest
// spot in the style's band. A mirror partner copies its twin's seat.

#[derive(Debug, Clone, PartialEq)]
pub struct SeatNote {
    /// The eye's index, or None for a species without a rest scene.
    pub k: Option<usize>,
    pub text: String,
    /// The eye stays hidden in some pose: the named parts must shrink or move (a hand fix).
    pub blocked: bool,
}

pub struct Seat {
    /// Every eye: the seated ones at their new `at`, the rest copies.
    pub eyes: Vec<Eye>,
    pub notes: Vec<SeatNote>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Seen {
    All,
    Rest,
    Hidden,
}

#[derive(Clone)]
struct Spot {
    x: f64,
    y: f64,
    z: f64,
    seen: Seen,
    blockers: Vec<String>,
    /// The rim's highest surface minus the centre's: how far the disc sags on the head.
    drop: f64,
}

struct SeenRes {
    all: bool,
    rest: bool,
    blockers: Vec<String>,
}

/// Whether the eye is seen in every pose, and in the rest pose; and the parts that hide it in a pose that fails.
fn seen_in(poses: &[(Pose, Scene)], g: EyeGeom, bone: &str) -> SeenRes {
    let mut blockers: Vec<String> = Vec::new();
    let mut all = true;
    let mut rest = false;
    for name in [Pose::Rest, Pose::Walk, Pose::Wave] {
        let Some((_, sc)) = poses.iter().find(|(p, _)| *p == name) else {
            continue;
        };
        let list: Vec<&Piece> = sc.pieces.iter().collect();
        let views = eye_views(g, bone_of(&sc.bones, bone), &list);
        let ok = eye_seen(&views);
        if name == Pose::Rest {
            rest = ok;
        }
        if ok {
            continue;
        }
        all = false;
        for v in &views {
            for h in v.hiders.iter().flatten() {
                let text = format!("{} {} \"{}\"", name.name(), h.path, h.label);
                if !blockers.contains(&text) {
                    blockers.push(text);
                }
            }
        }
    }
    SeenRes {
        all,
        rest,
        blockers,
    }
}

/// Seats every eye of a species. Pure: the species is not changed.
pub fn seat_eyes(s: &Species, clips: &BTreeMap<String, Clip>) -> Seat {
    let mut eyes = s.eyes.clone();
    let poses = scenes_of(s, clips);
    let Some(rest) = poses
        .iter()
        .find(|(p, _)| *p == Pose::Rest)
        .map(|(_, sc)| sc)
    else {
        return Seat {
            eyes,
            notes: vec![SeatNote {
                k: None,
                text: "no rest scene".to_string(),
                blocked: true,
            }],
        };
    };
    let head: Vec<&Piece> = rest.pieces.iter().filter(|p| p.bone == "head").collect();
    let width = if head.is_empty() {
        0.0
    } else {
        head.iter()
            .map(|p| p.max.x)
            .fold(f64::NEG_INFINITY, f64::max)
            - head.iter().map(|p| p.min.x).fold(f64::INFINITY, f64::min)
    };
    let min_y = if head.is_empty() {
        0.0
    } else {
        head.iter().map(|p| p.min.y).fold(f64::INFINITY, f64::min)
    };
    let height = if head.is_empty() {
        0.0
    } else {
        head.iter()
            .map(|p| p.max.y)
            .fold(f64::NEG_INFINITY, f64::max)
            - min_y
    };

    let orig: Vec<DVec3> = s
        .eyes
        .iter()
        .map(|e| bone_of(&rest.bones, &e.bone).transform_point3(at_of(e)))
        .collect();
    let mut placed: Vec<DVec3> = Vec::new();
    let mut notes: Vec<SeatNote> = Vec::new();

    for (k, e) in s.eyes.iter().enumerate() {
        let bone = bone_of(&rest.bones, &e.bone);
        let inv = bone.inverse();
        let geom = eye_geom(e);
        let r = geom.r;
        let on_head = e.bone == "head" && e.asym != Some(true);
        let c = orig[k];

        // A mirror partner (an earlier eye at the mirror position, the same shape and size) copies its seat.
        let twin = (0..k).find(|&j| {
            let o = &s.eyes[j];
            if o.asym == Some(true) || e.asym == Some(true) || c.x.abs() <= CENTRE {
                return false;
            }
            let d = orig[j];
            (d.x + c.x).abs() <= 0.02
                && (d.y - c.y).abs() <= 0.02
                && (d.z - c.z).abs() <= 0.02
                && o.shape == e.shape
                && o.r.unwrap_or(0.1) == e.r.unwrap_or(0.1)
                && o.pupil == e.pupil
        });

        // The surface the socket sits on: the eye's own bone, and the head when the eye has its own bone.
        let surface: Vec<&Piece> = rest
            .pieces
            .iter()
            .filter(|p| {
                p.mat != "clear" && (p.bone == e.bone || (e.bone != "head" && p.bone == "head"))
            })
            .collect();
        let front_z = |px: f64, py: f64| -> Option<f64> {
            ray_hits(
                &surface,
                DVec3::new(px, py, 50.0),
                DVec3::new(0.0, 0.0, -1.0),
            )
            .first()
            .map(|h| 50.0 - h)
        };

        // One spot: the lift over the rim's highest surface (or the float limit); the first lift seen in every pose wins.
        let spot = |x: f64, y: f64| -> Option<Spot> {
            let zs: Vec<f64> = ring9(x, y, r)
                .iter()
                .filter_map(|&(px, py)| front_z(px, py))
                .collect();
            let centre_z = front_z(x, y)?;
            if zs.is_empty() {
                return None;
            }
            let hi = zs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let base = hi + SEAT_EPS;
            let drop = hi - centre_z;
            if base - centre_z > FLOAT_SHARE * r {
                return Some(Spot {
                    x,
                    y,
                    z: base,
                    seen: Seen::Hidden,
                    blockers: vec![],
                    drop,
                });
            }
            let top = base.max(centre_z + FLOAT_SHARE * r);
            let mut fallback: Option<Spot> = None;
            let mut last = Spot {
                x,
                y,
                z: top,
                seen: Seen::Hidden,
                blockers: vec![],
                drop,
            };
            let mut z = base;
            while z <= top + 1e-9 {
                let res = seen_in(
                    &poses,
                    EyeGeom {
                        at: inv.transform_point3(DVec3::new(x, y, z)),
                        ..geom
                    },
                    &e.bone,
                );
                if res.all {
                    return Some(Spot {
                        x,
                        y,
                        z,
                        seen: Seen::All,
                        blockers: vec![],
                        drop,
                    });
                }
                if res.rest && fallback.is_none() {
                    fallback = Some(Spot {
                        x,
                        y,
                        z,
                        seen: Seen::Rest,
                        blockers: res.blockers.clone(),
                        drop,
                    });
                }
                last = Spot {
                    x,
                    y,
                    z,
                    seen: Seen::Hidden,
                    blockers: res.blockers,
                    drop,
                };
                z += STEP;
            }
            Some(fallback.unwrap_or(last))
        };

        // Lateral and height: the current spot when it works, else the flattest spots of the style's band.
        let (mut x, mut y) = (c.x, c.y);
        if let Some(t) = twin {
            x = -placed[t].x;
            y = placed[t].y;
        }
        let mut order: Vec<(f64, f64)> = vec![(x, y)];
        if twin.is_none() && on_head && width > 0.0 && height > 0.0 && x.abs() > CENTRE {
            let sign = x.signum();
            let mut grid: Vec<(f64, f64, f64)> = Vec::new();
            let mut sh = HALF_MIN;
            while sh <= HALF_MAX + 1e-9 {
                let mut up = HEIGHT_MIN;
                while up <= HEIGHT_MAX + 1e-9 {
                    let gx = sign * sh * width;
                    let gy = min_y + up * height;
                    let ring = ring9(gx, gy, r);
                    let zs: Vec<f64> = ring
                        .iter()
                        .filter_map(|&(px, py)| front_z(px, py))
                        .collect();
                    if let Some(cz) = front_z(gx, gy)
                        && zs.len() == ring.len()
                    {
                        grid.push((
                            gx,
                            gy,
                            zs.iter().copied().fold(f64::NEG_INFINITY, f64::max) - cz,
                        ));
                    }
                    up += 0.01;
                }
                sh += 0.005;
            }
            grid.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(Ordering::Equal));
            for g in grid.iter().take(SPOTS) {
                order.push((g.0, g.1));
            }
        }
        if twin.is_none() && on_head && width > 0.0 && height > 0.0 && c.x.abs() > CENTRE {
            let share = c.x.abs() / width;
            let up = (c.y - min_y) / height;
            if !(HALF_MIN..=HALF_MAX).contains(&share) || !(HEIGHT_MIN..=HEIGHT_MAX).contains(&up) {
                order[0] = (c.x.signum() * HALF_AIM * width, min_y + HEIGHT_AIM * height);
            }
        }

        let mut pick: Option<Spot> = None;
        let mut tried: Option<Spot> = None;
        for &(px, py) in &order {
            let Some(res) = spot(px, py) else { continue };
            if tried.is_none() {
                tried = Some(res.clone());
            }
            if res.seen == Seen::All {
                pick = Some(res);
                break;
            }
            if res.seen == Seen::Rest && pick.as_ref().is_none_or(|p| p.seen == Seen::Hidden) {
                pick = Some(res);
            }
        }
        let Some(pick) = pick.or(tried) else {
            placed.push(c);
            notes.push(SeatNote {
                k: Some(k),
                text: format!(
                    "{} {}: no head surface at ({x:.3}, {y:.3}); left as it is",
                    e.bone, e.shape
                ),
                blocked: true,
            });
            continue;
        };
        if pick.seen == Seen::Hidden {
            // No spot is seen (or the rim sags past the float limit): the eye stays where it is, for a hand fix.
            placed.push(c);
            let why = if pick.blockers.is_empty() {
                "the rim sags past the float limit".to_string()
            } else {
                pick.blockers.join(", ")
            };
            notes.push(SeatNote {
                k: Some(k),
                text: format!(
                    "{} {}: no seat is clear ({why}); left as it is",
                    e.bone, e.shape
                ),
                blocked: true,
            });
            continue;
        }
        let p = DVec3::new(pick.x, pick.y, pick.z);
        eyes[k].at = inv.transform_point3(p).to_array().map(|v| v as f32);
        placed.push(p);
        let moved = (pick.x - c.x).abs() > 1e-4 || (pick.y - c.y).abs() > 1e-4;
        let mut text = format!(
            "{} {}: socket z {:.4} -> {:.4} (rim drop {:.4}), at {:.4}, {:.4}{}",
            e.bone,
            e.shape,
            c.z,
            pick.z,
            pick.drop,
            pick.x,
            pick.y,
            if moved { " (moved)" } else { "" }
        );
        if let Some(t) = twin {
            text.push_str(&format!(", mirror of eye #{t}"));
        }
        let blocked = pick.seen != Seen::All;
        if blocked {
            let why = if pick.blockers.is_empty() {
                "float limit".to_string()
            } else {
                pick.blockers.join(", ")
            };
            text.push_str(&format!("; still hidden: {why}"));
        }
        notes.push(SeatNote {
            k: Some(k),
            text,
            blocked,
        });
    }
    Seat { eyes, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_species() -> Vec<Species> {
        registry().species.values().cloned().collect()
    }

    #[test]
    fn eyes_are_strict_in_every_pose() {
        let list = all_species();
        let results = audit_all(&list, &AuditOptions::default());
        let eye_errors: Vec<String> = list
            .iter()
            .flat_map(|s| {
                results[&s.id]
                    .iter()
                    .filter(|i| {
                        i.severity == Severity::Error && i.kind.as_str().starts_with("eye-")
                    })
                    .map(move |i| {
                        format!("{} {} {} {}", s.id, i.pose.name(), i.path, i.kind.as_str())
                    })
            })
            .collect();
        assert!(eye_errors.is_empty(), "eye errors: {eye_errors:#?}");
    }
}
