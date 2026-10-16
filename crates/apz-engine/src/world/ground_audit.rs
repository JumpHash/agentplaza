//! The grounding audit (`ground.ts`, `groundAudit`): every resting object (a node with `rest` set)
//! must stand on what is under it, and must not sink into another solid. A solid is a drawn mesh
//! whose material is neither a decal nor additive. Until the gpu layer lands, anything named "gpu"
//! (or under such a node) is not an item, though it still counts as a solid.

use glam::{Mat4, Vec3};
use serde_json::{Value, json};

use apz_board::nav::drawn::GROUND_TOL;

use crate::scene::{Geo, Geometry, Kind, Mat, Obj};

/// How far in from an item's sides the extra ground samples sit (a fraction of its extent).
const INSET: f64 = 0.1;
/// How far above an item's bottom the ground ray starts.
const SEEK: f64 = 0.1;
/// Slack on the barycentric test, so a point on a shared edge still hits.
const BARY_EPS: f64 = 1e-9;

type V3 = [f64; 3];

/// One thing the audit found wrong with an item.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundFinding {
    pub item: String,
    /// "floating" (the item hangs above what is under it) or "sinking" (it is below the ground, or
    /// inside another solid).
    pub kind: &'static str,
    pub amount: f64,
    pub at: [f64; 3],
    /// What the item is measured against: a solid's name, or "ground".
    pub against: String,
}

/// The audit's result: how many items were checked and what was wrong with them.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundReport {
    pub items: usize,
    pub findings: Vec<GroundFinding>,
}

impl GroundReport {
    /// `{"items": n, "findings": [{"item", "kind", "amount", "at", "against"}]}`.
    pub fn to_json(&self) -> Value {
        let findings: Vec<Value> = self
            .findings
            .iter()
            .map(|f| {
                json!({
                    "item": f.item,
                    "kind": f.kind,
                    "amount": f.amount,
                    "at": f.at,
                    "against": f.against,
                })
            })
            .collect();
        json!({ "items": self.items, "findings": findings })
    }
}

#[derive(Debug, Clone, Copy)]
struct Aabb {
    min: V3,
    max: V3,
}

impl Aabb {
    fn empty() -> Aabb {
        Aabb {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
        }
    }

    fn is_empty(&self) -> bool {
        (0..3).any(|a| self.min[a] > self.max[a])
    }

    fn grow(&mut self, p: V3) {
        for (a, v) in p.iter().enumerate() {
            self.min[a] = self.min[a].min(*v);
            self.max[a] = self.max[a].max(*v);
        }
    }
}

/// One flattened node.
struct Record {
    parent: Option<usize>,
    name: String,
    /// three's `type`: "Group" or "Mesh" (an InstancedMesh's type is "Mesh" too), or the other kinds' own.
    ty: &'static str,
    world: Mat4,
    rest: bool,
    geo: Option<Geo>,
    instanced: bool,
    solid: bool,
    /// One matrix per drawn instance (the world matrix alone for a Mesh).
    draws: Vec<Mat4>,
}

/// A solid: one drawn mesh, or one drawn instance of an instanced mesh.
struct Solid {
    record: usize,
    instance: Option<usize>,
    geo: Geo,
    matrix: Mat4,
    bbox: Aabb,
}

/// A thing to audit: a resting node, or one drawn instance of a resting instanced mesh.
struct Item {
    name: String,
    rec: usize,
    inst: Option<usize>,
    bbox: Aabb,
}

#[derive(Default)]
struct Flat {
    records: Vec<Record>,
    solids: Vec<Solid>,
}

/// Audits the trees under `roots`.
pub fn ground_audit(roots: &[Obj]) -> GroundReport {
    let mut flat = Flat::default();
    for root in roots {
        visit(root, None, Mat4::IDENTITY, &mut flat);
    }

    for i in 0..flat.records.len() {
        let r = &flat.records[i];
        if !r.solid {
            continue;
        }
        let Some(geo) = r.geo.clone() else { continue };
        let mut made = Vec::new();
        for (k, m) in r.draws.iter().enumerate() {
            let bbox = aabb_of(&geo, m);
            if bbox.is_empty() {
                continue;
            }
            made.push(Solid {
                record: i,
                instance: r.instanced.then_some(k),
                geo: geo.clone(),
                matrix: *m,
                bbox,
            });
        }
        flat.solids.extend(made);
    }

    let gpu = gpu_flags(&flat.records);
    let items = items_of(&flat, &gpu);
    let mut findings = Vec::new();
    for item in &items {
        check_item(&flat, item, &mut findings);
    }
    GroundReport {
        items: items.len(),
        findings,
    }
}

fn visit(obj: &Obj, parent: Option<usize>, parent_world: Mat4, flat: &mut Flat) {
    let world = parent_world * obj.local_matrix();
    // Clone what is needed out of the guard; never recurse while holding it.
    let (name, kind, rest, children) = {
        let node = obj.get();
        (
            node.name.clone(),
            node.kind.clone(),
            node.rest,
            node.children.clone(),
        )
    };
    let index = flat.records.len();
    let mut rec = Record {
        parent,
        name,
        ty: "Group",
        world,
        rest,
        geo: None,
        instanced: false,
        solid: false,
        draws: Vec::new(),
    };
    match &kind {
        Kind::Group => {}
        Kind::Mesh { geo, mat } => {
            rec.ty = "Mesh";
            rec.geo = Some(geo.clone());
            rec.draws = vec![world];
            rec.solid = is_solid(mat);
        }
        Kind::Instanced {
            geo,
            mat,
            instances,
            count,
        } => {
            rec.ty = "Mesh";
            rec.instanced = true;
            rec.geo = Some(geo.clone());
            rec.draws = instances
                .iter()
                .take(*count)
                .map(|inst| world * inst.matrix)
                .collect();
            rec.solid = is_solid(mat);
        }
        Kind::Points { .. } => rec.ty = "Points",
        Kind::PointLight { .. } => rec.ty = "PointLight",
    }
    flat.records.push(rec);
    for child in &children {
        visit(child, Some(index), world, flat);
    }
}

fn is_solid(mat: &Mat) -> bool {
    let m = mat.get();
    !m.decal && !m.additive
}

/// The gpu filter: a record is filtered when its own name or any ancestor's contains "gpu".
fn gpu_flags(records: &[Record]) -> Vec<bool> {
    let mut out: Vec<bool> = Vec::with_capacity(records.len());
    for r in records {
        let own = r.name.to_lowercase().contains("gpu");
        let inherited = r.parent.is_some_and(|p| out[p]);
        out.push(own || inherited);
    }
    out
}

fn items_of(flat: &Flat, gpu: &[bool]) -> Vec<Item> {
    let mut out = Vec::new();
    for (i, r) in flat.records.iter().enumerate() {
        if !r.rest || gpu[i] {
            continue;
        }
        match (&r.geo, r.instanced) {
            (Some(geo), true) => {
                for (k, m) in r.draws.iter().enumerate() {
                    let name = format!("{}#{}", name_of(flat, i), k);
                    push_item(&mut out, name, i, Some(k), aabb_of(geo, m));
                }
            }
            (Some(geo), false) => {
                for m in &r.draws {
                    push_item(&mut out, name_of(flat, i), i, None, aabb_of(geo, m));
                }
            }
            (None, _) => {
                let mut b = Aabb::empty();
                for s in flat.solids.iter().filter(|s| within(flat, s.record, i)) {
                    b.grow(s.bbox.min);
                    b.grow(s.bbox.max);
                }
                push_item(&mut out, name_of(flat, i), i, None, b);
            }
        }
    }
    out
}

fn push_item(out: &mut Vec<Item>, name: String, rec: usize, inst: Option<usize>, bbox: Aabb) {
    if !bbox.is_empty() {
        out.push(Item {
            name,
            rec,
            inst,
            bbox,
        });
    }
}

fn check_item(flat: &Flat, item: &Item, findings: &mut Vec<GroundFinding>) {
    let b = item.bbox;
    let (ex, ez) = (b.max[0] - b.min[0], b.max[2] - b.min[2]);
    let samples = [
        ((b.min[0] + b.max[0]) / 2.0, (b.min[2] + b.max[2]) / 2.0),
        (b.min[0] + INSET * ex, b.min[2] + INSET * ez),
        (b.max[0] - INSET * ex, b.min[2] + INSET * ez),
        (b.min[0] + INSET * ex, b.max[2] - INSET * ez),
        (b.max[0] - INSET * ex, b.max[2] - INSET * ez),
    ];

    // Floating: the worst gap between the item's bottom and the top of what is under it.
    let mut worst: Option<(f64, V3, String)> = None;
    for (x, z) in samples {
        let (top_h, against) = match top_under(flat, item, x, z) {
            Some((h, si)) => (h, name_of(flat, flat.solids[si].record)),
            None => (0.0, "ground".to_string()),
        };
        let gap = b.min[1] - top_h;
        if worst.as_ref().is_none_or(|w| gap > w.0) {
            worst = Some((gap, [x, b.min[1], z], against));
        }
    }
    if let Some((gap, at, against)) = worst.filter(|w| w.0 > GROUND_TOL) {
        findings.push(GroundFinding {
            item: item.name.clone(),
            kind: "floating",
            amount: gap,
            at,
            against,
        });
    }

    if b.min[1] < -GROUND_TOL {
        findings.push(GroundFinding {
            item: item.name.clone(),
            kind: "sinking",
            amount: -b.min[1],
            at: b.min,
            against: "ground".to_string(),
        });
    }

    // Overlap: the deepest penetration into a solid that sits no higher than the item's bottom.
    let mut deepest: Option<(f64, V3, usize)> = None;
    for s in &flat.solids {
        if related(flat, s, item) || s.bbox.min[1] > b.min[1] + GROUND_TOL {
            continue;
        }
        let lo = |a: usize| b.min[a].max(s.bbox.min[a]);
        let hi = |a: usize| b.max[a].min(s.bbox.max[a]);
        let (dx, dy, dz) = (hi(0) - lo(0), hi(1) - lo(1), hi(2) - lo(2));
        if dx > GROUND_TOL && dy > GROUND_TOL && dz > GROUND_TOL {
            let depth = dx.min(dy).min(dz);
            if deepest.as_ref().is_none_or(|d| depth > d.0) {
                let at = [(lo(0) + hi(0)) / 2.0, lo(1), (lo(2) + hi(2)) / 2.0];
                deepest = Some((depth, at, s.record));
            }
        }
    }
    if let Some((depth, at, rec)) = deepest {
        findings.push(GroundFinding {
            item: item.name.clone(),
            kind: "sinking",
            amount: depth,
            at,
            against: name_of(flat, rec),
        });
    }
}

/// The highest up-facing triangle under (x, z) that the item can stand on, as (height, solid index).
fn top_under(flat: &Flat, item: &Item, x: f64, z: f64) -> Option<(f64, usize)> {
    let y0 = item.bbox.min[1] + SEEK;
    // Hits more than GROUND_TOL above the item's bottom are ignored (the item's own or intersecting
    // surfaces), so the effective ceiling is the lower of the two limits.
    let limit = y0.min(item.bbox.min[1] + GROUND_TOL);
    let mut best: Option<(f64, usize)> = None;
    for (si, s) in flat.solids.iter().enumerate() {
        if own(flat, s, item) || !contains_xz(&s.bbox, x, z) {
            continue;
        }
        let v: Vec<V3> = s
            .geo
            .positions
            .iter()
            .map(|p| xform(&s.matrix, p))
            .collect();
        for [i0, i1, i2] in tris(&s.geo) {
            let (a, b, c) = (v[i0], v[i1], v[i2]);
            // The y of (b - a) x (c - a): positive when the triangle faces up.
            let ny = (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]);
            if ny <= 0.0 {
                continue;
            }
            let det = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
            if det.abs() < 1e-12 {
                continue;
            }
            let l0 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / det;
            let l1 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / det;
            let l2 = 1.0 - l0 - l1;
            if l0 < -BARY_EPS || l1 < -BARY_EPS || l2 < -BARY_EPS {
                continue;
            }
            let h = l0 * a[1] + l1 * b[1] + l2 * c[1];
            if h > limit {
                continue;
            }
            if best.is_none_or(|(bh, _)| h > bh) {
                best = Some((h, si));
            }
        }
    }
    best
}

/// Whether solid `s` is the item itself (for an instance: only that same instance) or inside it.
fn own(flat: &Flat, s: &Solid, item: &Item) -> bool {
    match item.inst {
        Some(k) if s.record == item.rec => s.instance == Some(k),
        _ => within(flat, s.record, item.rec),
    }
}

/// Like `own`, but an ancestor of the item counts too.
fn related(flat: &Flat, s: &Solid, item: &Item) -> bool {
    match item.inst {
        Some(k) if s.record == item.rec => s.instance == Some(k),
        _ => within(flat, s.record, item.rec) || within(flat, item.rec, s.record),
    }
}

/// Whether record `a` is record `b` or one of its descendants.
fn within(flat: &Flat, mut a: usize, b: usize) -> bool {
    loop {
        if a == b {
            return true;
        }
        match flat.records[a].parent {
            Some(p) => a = p,
            None => return false,
        }
    }
}

fn name_of(flat: &Flat, rec: usize) -> String {
    let r = &flat.records[rec];
    if !r.name.is_empty() {
        return r.name.clone();
    }
    let t = r.world.w_axis;
    format!("{}@{:.2},{:.2},{:.2}", r.ty, t.x, t.y, t.z)
}

fn contains_xz(b: &Aabb, x: f64, z: f64) -> bool {
    x >= b.min[0] && x <= b.max[0] && z >= b.min[2] && z <= b.max[2]
}

fn xform(m: &Mat4, p: &[f32; 3]) -> V3 {
    let v = m.transform_point3(Vec3::from_array(*p));
    [v.x as f64, v.y as f64, v.z as f64]
}

fn aabb_of(geo: &Geometry, m: &Mat4) -> Aabb {
    let mut b = Aabb::empty();
    for p in &geo.positions {
        b.grow(xform(m, p));
    }
    b
}

/// The triangles of a geometry as vertex indices (sequential when it has no index buffer).
fn tris(geo: &Geometry) -> Vec<[usize; 3]> {
    if geo.indices.is_empty() {
        (0..geo.positions.len() / 3)
            .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
            .collect()
    } else {
        geo.indices
            .chunks_exact(3)
            .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three.js build's grounding audit (ground.ts on the bare boards, `testdata/ground-audit-ts.json`, written by the
    /// retired build at 338f89c) and this one agree item for item and finding for finding.
    #[test]
    fn the_bare_boards_audit_as_the_three_js_build_did() {
        let golden: serde_json::Value =
            serde_json::from_str(include_str!("../../testdata/ground-audit-ts.json")).unwrap();
        for (key, want) in golden.as_object().unwrap() {
            let (name, params) = key.split_once('|').unwrap();
            let params: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(params).unwrap();
            let model = crate::hardware::model(name).unwrap();
            let mut p = crate::hardware::Params::default();
            for (k, v) in &params {
                p = p.with(k, *v);
            }
            let built = crate::hardware::build(model, &p);
            let (root, board) = (Obj::group(), Obj::group());
            board.add(&built.root);
            board.add(&crate::hardware::traces(model));
            root.add(&board);
            let got = ground_audit(&[root]);
            assert_eq!(
                got.items as u64,
                want["items"].as_u64().unwrap(),
                "{key}: items"
            );
            let mut want: Vec<(String, String, f64, String)> = want["findings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    (
                        f[0].as_str().unwrap().into(),
                        f[1].as_str().unwrap().into(),
                        f[2].as_f64().unwrap(),
                        f[3].as_str().unwrap().into(),
                    )
                })
                .collect();
            let mut got: Vec<(String, String, f64, String)> = got
                .findings
                .iter()
                .map(|f| {
                    (
                        f.item.clone(),
                        f.kind.to_string(),
                        (f.amount * 1000.0).round() / 1000.0,
                        f.against.clone(),
                    )
                })
                .collect();
            want.sort_by(|a, b| a.partial_cmp(b).unwrap());
            got.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(got.len(), want.len(), "{key}: findings");
            for (g, w) in got.iter().zip(&want) {
                assert!(
                    g.0 == w.0 && g.1 == w.1 && g.3 == w.3 && (g.2 - w.2).abs() <= 0.002,
                    "{key}: {g:?} != {w:?}"
                );
            }
        }
    }

    use crate::scene::Obj;
}
