//! Builds a board's scene (as buildModelSteps does, without three.js): the tree of parts with their world matrices,
//! then the items (ground.ts itemsOf), the part keys (the size of the TS `parts` map) and the support field.
//!
//! Not built, because nothing here reads them: outline hulls (they repeat their mesh's triangles, so they add no new
//! height and no new box), the bake, the materials beyond their additive flag, and the detail and cutaway flags.

use std::collections::{BTreeMap, BTreeSet};

use crate::geom::{self, Geometry, IDENTITY, M4, mul, round_m4, trs};
use crate::hardware::{
    Count, DEFAULT_MATERIAL_COLOR, Geo, Library, Material, Model, Part, Repeat, Scale, eval_when,
    parse_when,
};
use crate::support::{Support, Tri, is_upward};
use crate::{Aabb, Item};

const ZERO: [f64; 3] = [0.0, 0.0, 0.0];
const ONE: [f64; 3] = [1.0, 1.0, 1.0];
/// Copies of a repeat are capped at this many (hardware.ts repeatOffsets).
const MAX_COPIES: f64 = 256.0;

/// The TEMPORARY GPU filter of ground.ts (TEMP_EXCLUDED_ITEM = /gpu/i): nothing under a part whose id matches is an
/// item. The parts still hold things up. REMOVE with the filter in ground.ts when the new GPU parts land.
fn is_gpu(id: &str) -> bool {
    id.to_ascii_lowercase().contains("gpu")
}

/// What a node is.
enum Kind {
    Group,
    /// One mesh; its world matrix is the node's.
    Mesh {
        geo: Geometry,
        solid: bool,
    },
    /// One InstancedMesh with its copies' local matrices (already rounded to f32); its own matrix is the node's.
    Instanced {
        geo: Geometry,
        locals: Vec<M4>,
        solid: bool,
    },
}

struct Node {
    /// The part's id ("" when it has none).
    name: String,
    /// A rest item: a solid that is not attached (not a decal), or a group that is not attached.
    rest: bool,
    /// Under a part whose id matches the TEMPORARY GPU filter.
    excluded: bool,
    /// A decal (plane, ring, label): drawn, but no part box (zfight.ts partBoxes gives decals none).
    decal: bool,
    world: M4,
    kind: Kind,
    children: Vec<usize>,
}

/// One drawn copy of a mesh: its world box, and its upward triangles when it is a solid.
struct Copy {
    bounds: Aabb,
    ups: Vec<Tri>,
}

/// A built board: its items in traversal order, the support field, and the number of part keys.
pub(crate) struct Scene {
    pub items: Vec<Item>,
    /// Every part's box, one per drawn copy, GPU parts and attached parts included, decals not (zfight.ts partBoxes):
    /// what the layout plans with (occlusion, tall parts), unlike `items`, the resting set the grounding audit reads.
    pub boxes: Vec<Item>,
    pub support: Support,
    pub part_count: usize,
}

/// Builds a model's scene with the given params (the model's own params are overridden by `params`).
pub(crate) fn build(lib: &Library, model: &Model, params: &BTreeMap<String, f64>) -> Scene {
    let mut merged = model.params.clone().unwrap_or_default();
    merged.extend(params.iter().map(|(k, v)| (k.clone(), *v)));
    let mut b = Builder {
        lib,
        params: merged,
        nodes: Vec::new(),
        keys: BTreeSet::new(),
    };
    let mut children = Vec::new();
    for p in &model.parts {
        if let Some(c) = b.build_part(p, "", &IDENTITY, false) {
            children.push(c);
        }
    }
    let root = b.push(Node {
        name: String::new(),
        rest: false,
        excluded: false,
        decal: false,
        world: IDENTITY,
        kind: Kind::Group,
        children,
    });
    b.finish(root)
}

struct Builder<'a> {
    lib: &'a Library,
    params: BTreeMap<String, f64>,
    nodes: Vec<Node>,
    /// The part keys (ctx.parts in buildModelSteps): "id", "id#n" for repeat copies, and "outer/" scopes.
    keys: BTreeSet<String>,
}

impl Builder<'_> {
    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// buildPart: a part with a `when` that does not hold builds nothing; a repeat is one instanced mesh for a plain
    /// solid, else a holder group with a copy per offset.
    fn build_part(&mut self, p: &Part, scope: &str, parent: &M4, anc: bool) -> Option<usize> {
        if let Some(expr) = &p.when {
            match parse_when(expr) {
                Some(w) if eval_when(&w, &self.params) => {}
                _ => return None,
            }
        }
        let Some(r) = &p.repeat else {
            return Some(self.build_one(p, scope, 0, ZERO, parent, anc));
        };
        let plain =
            p.label.is_none() && p.children.is_none() && p.tag.is_none() && p.anim.is_none();
        if let (Some(geo), true) = (&p.geo, plain) {
            return self.build_instanced(p, geo, r, scope, parent, anc);
        }
        let offsets = repeat_offsets(r, &self.params);
        let mut children = Vec::with_capacity(offsets.len());
        for (i, off) in offsets.iter().enumerate() {
            children.push(self.build_one(p, scope, i, *off, parent, anc));
        }
        Some(self.push(Node {
            name: String::new(),
            rest: false,
            excluded: anc,
            decal: false,
            world: *parent,
            kind: Kind::Group,
            children,
        }))
    }

    /// buildOne: a group (a `use` or children only) or a mesh, with its children and its label.
    fn build_one(
        &mut self,
        p: &Part,
        scope: &str,
        inst: usize,
        offset: [f64; 3],
        parent: &M4,
        anc: bool,
    ) -> usize {
        let id = p.id.as_deref().filter(|s| !s.is_empty());
        let key = id.map(|id| {
            if inst > 0 {
                format!("{scope}{id}#{inst}")
            } else {
                format!("{scope}{id}")
            }
        });
        let child_scope = match &key {
            Some(k) => format!("{k}/"),
            None => scope.to_string(),
        };
        let excluded = anc || id.is_some_and(is_gpu);
        let at = p.at.unwrap_or(ZERO);
        let pos = [at[0] + offset[0], at[1] + offset[1], at[2] + offset[2]];
        let local = trs(pos, p.rot.unwrap_or(ZERO), scale_of(p.scale.as_ref()));
        let world = mul(parent, &local);
        let decal = is_decal(p);
        let mut top = 0.0;
        let mut children = Vec::new();
        let kind = if let Some(u) = &p.use_ {
            let lib = self.lib;
            if let Some(target) = lib.model(u) {
                top = target.size[1];
                for tp in &target.parts {
                    if let Some(c) = self.build_part(tp, &child_scope, &world, excluded) {
                        children.push(c);
                    }
                }
            }
            Kind::Group
        } else if let Some(g) = &p.geo {
            let geo = geom::make(g);
            top = geo.top();
            let solid = !decal && !self.additive(p);
            Kind::Mesh { geo, solid }
        } else {
            Kind::Group
        };
        if let Some(k) = &key {
            self.keys.insert(k.clone());
        }
        let rest = !decal && p.attached != Some(true);
        if let Some(cs) = &p.children {
            for c in cs {
                if let Some(ci) = self.build_part(c, &child_scope, &world, excluded) {
                    children.push(ci);
                }
            }
        }
        if let Some(l) = &p.label {
            let size = l.size.unwrap_or(0.6);
            let label_world = mul(&world, &trs([0.0, top + 0.02, 0.0], ZERO, ONE));
            let li = self.push(Node {
                name: String::new(),
                rest: false,
                excluded,
                decal: true,
                world: label_world,
                kind: Kind::Mesh {
                    geo: geom::label(size),
                    solid: true,
                },
                children: Vec::new(),
            });
            children.push(li);
        }
        self.push(Node {
            name: id.unwrap_or("").to_string(),
            rest,
            excluded,
            decal,
            world,
            kind,
            children,
        })
    }

    /// buildInstanced: one InstancedMesh whose copies are instance matrices (rounded to f32, as a Float32Array holds them).
    fn build_instanced(
        &mut self,
        p: &Part,
        geo: &Geo,
        r: &Repeat,
        scope: &str,
        parent: &M4,
        anc: bool,
    ) -> Option<usize> {
        let offsets = repeat_offsets(r, &self.params);
        if offsets.is_empty() {
            return None;
        }
        let id = p.id.as_deref().filter(|s| !s.is_empty());
        let excluded = anc || id.is_some_and(is_gpu);
        let at = p.at.unwrap_or(ZERO);
        let rot = p.rot.unwrap_or(ZERO);
        let scale = scale_of(p.scale.as_ref());
        let mut locals = Vec::with_capacity(offsets.len());
        for (i, off) in offsets.iter().enumerate() {
            let pos = [at[0] + off[0], at[1] + off[1], at[2] + off[2]];
            locals.push(round_m4(&trs(pos, rot, scale)));
            if let Some(id) = id {
                let key = if i > 0 {
                    format!("{scope}{id}#{i}")
                } else {
                    format!("{scope}{id}")
                };
                self.keys.insert(key);
            }
        }
        let decal = is_decal(p);
        let solid = !decal && !self.additive(p);
        let rest = !decal && p.attached != Some(true);
        let geo = geom::make(geo);
        Some(self.push(Node {
            name: id.unwrap_or("").to_string(),
            rest,
            excluded,
            decal,
            world: *parent,
            kind: Kind::Instanced { geo, locals, solid },
            children: Vec::new(),
        }))
    }

    /// The part's material is additive (an additive solid is not a support, and is not a solid at all).
    fn additive(&self, p: &Part) -> bool {
        material_of(self.lib, p).additive.unwrap_or(false)
    }

    /// Computes every mesh's copies, the support field and the items.
    fn finish(self, root: usize) -> Scene {
        let Builder { nodes, keys, .. } = self;
        let copies: Vec<Vec<Copy>> = nodes
            .iter()
            .map(|n| match &n.kind {
                Kind::Group => Vec::new(),
                Kind::Mesh { geo, solid } => vec![copy_of(geo, &n.world, *solid)],
                Kind::Instanced { geo, locals, solid } => locals
                    .iter()
                    .map(|l| copy_of(geo, &mul(&n.world, l), *solid))
                    .collect(),
            })
            .collect();
        let mut support = Support::default();
        for cs in &copies {
            for c in cs {
                for t in &c.ups {
                    support.add(*t);
                }
            }
        }
        let mut items = Vec::new();
        visit(&nodes, &copies, root, &mut items);
        let mut boxes = Vec::new();
        visit_boxes(&nodes, &copies, root, &mut boxes);
        Scene {
            items,
            boxes,
            support,
            part_count: keys.len(),
        }
    }
}

fn is_decal(p: &Part) -> bool {
    matches!(p.geo, Some(Geo::Plane { .. }) | Some(Geo::Ring { .. }))
}

/// The part's material: a palette name (the default when it is missing) or an inline material.
fn material_of(lib: &Library, p: &Part) -> Material {
    use crate::hardware::MatRef;
    let default = || Material {
        color: DEFAULT_MATERIAL_COLOR.to_string(),
        emissive: None,
        glow: None,
        opacity: None,
        additive: None,
        flat: None,
    };
    match &p.mat {
        None => default(),
        Some(MatRef::Name(n)) => lib.palette.get(n).cloned().unwrap_or_else(default),
        Some(MatRef::Inline(m)) => m.clone(),
    }
}

fn scale_of(s: Option<&Scale>) -> [f64; 3] {
    match s {
        None => ONE,
        Some(Scale::Uniform(v)) => [*v, *v, *v],
        Some(Scale::Xyz(v)) => *v,
    }
}

/// A count read from the params: a number, a param's value, or a param's value over n rounded up (hardware.ts repeatCount).
fn repeat_count(count: &Count, params: &BTreeMap<String, f64>) -> f64 {
    match count {
        Count::N(n) => f64::from(*n),
        Count::Param(s) => match split_count_ref(s) {
            Some((name, over)) => {
                (js_round(params.get(name).copied().unwrap_or(0.0)) / over).ceil()
            }
            None => js_round(params.get(s.as_str()).copied().unwrap_or(0.0)),
        },
    }
}

/// "name/n" with name a param name and n a whole number 1 or more.
fn split_count_ref(s: &str) -> Option<(&str, f64)> {
    let (name, over) = s.split_once('/')?;
    let mut nc = name.chars();
    let first = nc.next()?;
    if !(first.is_ascii_lowercase() || first == '_')
        || !nc.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let mut oc = over.chars();
    let o1 = oc.next()?;
    if !('1'..='9').contains(&o1) || !oc.all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((name, over.parse().ok()?))
}

/// JavaScript's Math.round: halves go up.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Where each copy of a repeat sits: copy i at step * col + row * line (col and line from wrap), as repeatOffsets.
pub(crate) fn repeat_offsets(r: &Repeat, params: &BTreeMap<String, f64>) -> Vec<[f64; 3]> {
    let [sx, sy, sz] = r.step;
    let [rx, ry, rz] = r.row.unwrap_or(ZERO);
    let wrap = r.wrap.filter(|w| *w > 0).map(|w| w as usize);
    let limit = repeat_count(&r.count, params).min(MAX_COPIES);
    let mut out = Vec::new();
    let mut i = 0usize;
    while (i as f64) < limit {
        let (col, line) = match wrap {
            Some(w) => ((i % w) as f64, (i / w) as f64),
            None => (i as f64, 0.0),
        };
        out.push([
            sx * col + rx * line,
            sy * col + ry * line,
            sz * col + rz * line,
        ]);
        i += 1;
    }
    out
}

/// The world copies of a mesh: its box over every vertex, and its upward triangles when it is a solid.
fn copy_of(geo: &Geometry, world: &M4, solid: bool) -> Copy {
    let mut bounds = Aabb::empty();
    for p in &geo.pos {
        bounds.include(geom::apply(world, *p));
    }
    let mut ups = Vec::new();
    if solid {
        for t in &geo.tris {
            let tri = [
                geom::apply(world, geo.pos[t[0]]),
                geom::apply(world, geo.pos[t[1]]),
                geom::apply(world, geo.pos[t[2]]),
            ];
            if is_upward(&tri) {
                ups.push(tri);
            }
        }
    }
    Copy { bounds, ups }
}

fn is_solid(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Mesh { solid: true, .. } | Kind::Instanced { solid: true, .. }
    )
}

/// The union of the boxes of the solids under a group (itself included), and the highest of their upward faces.
fn group_of(
    nodes: &[Node],
    copies: &[Vec<Copy>],
    idx: usize,
    bounds: &mut Option<Aabb>,
    top: &mut Option<f64>,
) {
    if is_solid(&nodes[idx].kind) {
        for c in &copies[idx] {
            *bounds = Some(match *bounds {
                Some(b) => b.union(&c.bounds),
                None => c.bounds,
            });
            for t in &c.ups {
                let y = t.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
                *top = Some(top.map_or(y, |m| m.max(y)));
            }
        }
    }
    for &ch in &nodes[idx].children {
        group_of(nodes, copies, ch, bounds, top);
    }
}

/// itemsOf: pre-order over the tree; a rest node that is not under the GPU filter is an item (a group's box is its solids').
fn visit(nodes: &[Node], copies: &[Vec<Copy>], idx: usize, out: &mut Vec<Item>) {
    let n = &nodes[idx];
    if n.rest && !n.excluded {
        match &n.kind {
            Kind::Group => {
                let (mut bounds, mut top) = (None, None);
                group_of(nodes, copies, idx, &mut bounds, &mut top);
                if let Some(b) = bounds {
                    out.push(Item {
                        name: n.name.clone(),
                        bounds: b,
                        top,
                    });
                }
            }
            Kind::Mesh { .. } => {
                let c = &copies[idx][0];
                if !c.bounds.is_empty() {
                    out.push(Item {
                        name: n.name.clone(),
                        bounds: c.bounds,
                        top: top_of(&c.ups),
                    });
                }
            }
            Kind::Instanced { .. } => {
                for (i, c) in copies[idx].iter().enumerate() {
                    out.push(Item {
                        name: format!("{}#{i}", n.name),
                        bounds: c.bounds,
                        top: top_of(&c.ups),
                    });
                }
            }
        }
    }
    for &ch in &n.children {
        visit(nodes, copies, ch, out);
    }
}

/// partBoxes: pre-order over the tree; every mesh copy that is not a decal gives its box, under the GPU filter or not.
fn visit_boxes(nodes: &[Node], copies: &[Vec<Copy>], idx: usize, out: &mut Vec<Item>) {
    let n = &nodes[idx];
    if !n.decal {
        match &n.kind {
            Kind::Group => {}
            Kind::Mesh { .. } => {
                let c = &copies[idx][0];
                if !c.bounds.is_empty() {
                    out.push(Item {
                        name: n.name.clone(),
                        bounds: c.bounds,
                        top: top_of(&c.ups),
                    });
                }
            }
            Kind::Instanced { .. } => {
                for c in &copies[idx] {
                    if !c.bounds.is_empty() {
                        out.push(Item {
                            name: n.name.clone(),
                            bounds: c.bounds,
                            top: top_of(&c.ups),
                        });
                    }
                }
            }
        }
    }
    for &ch in &n.children {
        visit_boxes(nodes, copies, ch, out);
    }
}

fn top_of(ups: &[Tri]) -> Option<f64> {
    ups.iter()
        .flat_map(|t| t.iter().map(|p| p[1]))
        .fold(None, |m: Option<f64>, y| Some(m.map_or(y, |m| m.max(y))))
}
