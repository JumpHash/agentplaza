//! The layout verifier, a port of the web's `rooms/verify.ts` (v2, with the purpose rules). It recomputes footprints,
//! grid cells, clear zones, reachability and the group rules from the layout and the catalogue alone (it does not
//! use the generator's internals), with a 1e-3 tolerance on float comparisons. The violation texts are the web's, so
//! a parity run can compare them line by line. An empty list means the layout is valid.

use std::collections::BTreeMap;

use super::generate::{Anchor, CatGroup, CatItem, Data, Door, Input, Layout, Place, template_for};

const TOL: f64 = 1e-3;
const WALLMOUNT_DEPTH: f64 = 0.1;
const MIN_ACCESS: f64 = 0.25;
const SPOT_TOL: f64 = 0.0011;
/// A group anchor (or a wall-mounted item's back) is within this distance of the wall or edge it stands on.
const ANCHOR_TOUCH: f64 = 0.08;
/// The strip in front of a wall-mounted item that must stay clear (metres).
const MOUNT_VIEW: f64 = 0.6;
/// Seats face their anchor within this many degrees; spots face their item within the second limit.
const SEAT_FACE_TOL: f64 = 10.0;
const SPOT_FACE_TOL: f64 = 30.0;
const SEAT_IDS: [&str; 3] = ["chair", "armchair", "stool"];
const SEAT_GROUPS: [&str; 4] = ["desk-set", "meeting", "station", "reading-nook"];
/// The rotation that turns a wall item's front to the room centre, per wall (as gen.ts).
const SIDE_ROT: [(Door, f64); 4] = [
    (Door::N, 0.0),
    (Door::E, 270.0),
    (Door::S, 180.0),
    (Door::W, 90.0),
];
const SIDES: [Door; 4] = [Door::N, Door::E, Door::S, Door::W];

/// A number as JavaScript prints it (`String(v)`): `-0` reads as `0`, exponent form outside [1e-6, 1e21).
pub fn js(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    let a = v.abs();
    if !(1e-6..1e21).contains(&a) {
        let s = format!("{v:e}");
        let (m, e) = s.split_once('e').expect("scientific form has an exponent");
        let e: i32 = e.parse().expect("exponent parses");
        return format!("{m}e{}{}", if e < 0 { "-" } else { "+" }, e.abs());
    }
    format!("{v}")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Solid,
    Surface,
    Floor,
    Wall,
}

fn kind_of(cat: &CatItem) -> Kind {
    if cat.place == Place::Surface {
        Kind::Surface
    } else if cat.layer.as_deref() == Some("floor") {
        Kind::Floor
    } else if cat.place == Place::Wallmount {
        Kind::Wall
    } else {
        Kind::Solid
    }
}

/// Front of an item (or spot) at a rotation in degrees: (sin r, cos r) in (x, z).
fn front_of(deg: f64) -> (f64, f64) {
    let r = deg * std::f64::consts::PI / 180.0;
    (r.sin(), r.cos())
}

/// Angle in degrees between two planar vectors; 0 when either has no length.
fn angle_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    let la = a.0.hypot(a.1);
    let lb = b.0.hypot(b.1);
    if la == 0.0 || lb == 0.0 {
        return 0.0;
    }
    let c = ((a.0 * b.0 + a.1 * b.1) / (la * lb)).clamp(-1.0, 1.0);
    c.acos() * 180.0 / std::f64::consts::PI
}

/// Rotate a local (x, z) by rot degrees (the verifier's `rotated`).
fn rotated(x: f64, z: f64, rot: f64) -> (f64, f64) {
    let r = ((rot % 360.0) + 360.0) % 360.0;
    if r == 90.0 {
        (z, -x)
    } else if r == 180.0 {
        (-x, -z)
    } else if r == 270.0 {
        (-z, x)
    } else {
        (x, z)
    }
}

/// The wall side an item's back is against, from its rotation (the inverse of `SIDE_ROT`).
fn side_of_rot(rot: f64) -> Option<Door> {
    SIDE_ROT.iter().find(|(_, r)| *r == rot).map(|(s, _)| *s)
}

fn side_name(d: Door) -> &'static str {
    d.as_str()
}

/// The cell indices of an item's footprint (empty for a surface item or an unknown id).
fn cov_slice(cov: &[Option<Vec<usize>>], k: usize) -> &[usize] {
    cov[k].as_deref().unwrap_or(&[])
}

/// One placed item as the verifier sees it (`VItem` in verify.ts).
struct VItem<'a> {
    k: usize,
    id: &'a str,
    cat: &'a CatItem,
    /// The group instance index, when it names an entry of `layout.groups`.
    g: Option<usize>,
    kind: Kind,
    x: f64,
    z: f64,
    rot: f64,
    host: Option<usize>,
    fw: f64,
    fd: f64,
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
    top: f64,
}

/// A spot the catalogue gives: the item index, and its position and face in the room frame.
struct Expected {
    item: usize,
    x: f64,
    z: f64,
    face: f64,
}

/// Verify a layout against its input, the catalogue and the templates. Returns the violations in the web's order
/// and wording; an empty list means the layout is valid.
pub fn verify(layout: &Layout, input: &Input, data: &Data) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let catalog = &data.catalog;
    let Some(tpl) = template_for(&input.role, &data.roles) else {
        return vec![format!("no template for role \"{}\"", input.role)];
    };

    let cell = catalog.cell.unwrap_or(0.25);
    let w = input.w;
    let d = input.d;
    let door = input.door;
    let cols = (w / cell).floor() as i64;
    let rows = (d / cell).floor() as i64;
    let n_cells = (cols * rows) as usize;
    let cell_x_lo = |i: i64| -w / 2.0 + i as f64 * cell;
    let cell_z_lo = |j: i64| -d / 2.0 + j as f64 * cell;

    // Shell.
    let sh = &layout.shell;
    if sh.door.side != door {
        out.push(format!(
            "shell: door side {}, expected {}",
            side_name(sh.door.side),
            side_name(door)
        ));
    }
    let side_len = if door == Door::N || door == Door::S {
        w
    } else {
        d
    };
    let exp_width = 1.0_f64.min(side_len - 0.5);
    if (sh.door.width - exp_width).abs() > TOL {
        out.push(format!(
            "shell: door width {}, expected {}",
            js(sh.door.width),
            js(exp_width)
        ));
    }
    if sh.door.at.abs() > TOL {
        out.push(format!("shell: door at {}, expected 0", js(sh.door.at)));
    }
    if !sh.walls.is_empty() && sh.walls != SIDES {
        let list: Vec<&str> = sh.walls.iter().map(|s| side_name(*s)).collect();
        out.push(format!(
            "shell: walls [{}] are not all four sides (the door side has stubs)",
            list.join(",")
        ));
    }
    if !layout.dropped.is_empty() {
        out.push(format!(
            "dropped required groups: {}",
            layout.dropped.join(", ")
        ));
    }

    // v2 groups: each entry of layout.groups is a catalogue group.
    let groups: &[String] = &layout.groups;
    let cat_groups: &BTreeMap<String, CatGroup> = &catalog.groups;
    for (gi, gid) in groups.iter().enumerate() {
        if !cat_groups.contains_key(gid) {
            out.push(format!("group {gi}: \"{gid}\" is not a catalogue group"));
        }
    }
    // The walls that exist, or the room edges of an open room (never the door side).
    let edges: Vec<Door> = sh.walls.clone();

    // Door zone and spine (cell centres, as the algorithm defines them).
    let mut ds = vec![false; n_cells];
    let mut door_cells: Vec<usize> = Vec::new();
    let half_width = exp_width / 2.0;
    for j in 0..rows {
        for i in 0..cols {
            let x = -w / 2.0 + (i as f64 + 0.5) * cell;
            let z = -d / 2.0 + (j as f64 + 0.5) * cell;
            let c = (j * cols + i) as usize;
            let (depth, lateral, toward_centre) = match door {
                Door::N => (z + d / 2.0, x, z <= 0.0),
                Door::S => (d / 2.0 - z, x, z >= 0.0),
                Door::E => (w / 2.0 - x, z, x >= 0.0),
                Door::W => (x + w / 2.0, z, x <= 0.0),
            };
            if depth <= 1.0 && lateral.abs() <= half_width {
                ds[c] = true;
                door_cells.push(c);
            } else if lateral.abs() <= 0.25 && toward_centre {
                ds[c] = true;
            }
        }
    }

    // Cells whose open square overlaps the rectangle by more than the tolerance.
    let covered = |x0: f64, x1: f64, z0: f64, z1: f64| -> Vec<usize> {
        let mut res = Vec::new();
        for j in 0..rows {
            let zl = cell_z_lo(j);
            let zh = zl + cell;
            if zh.min(z1) - zl.max(z0) <= TOL {
                continue;
            }
            for i in 0..cols {
                let xl = cell_x_lo(i);
                let xh = xl + cell;
                if xh.min(x1) - xl.max(x0) > TOL {
                    res.push((j * cols + i) as usize);
                }
            }
        }
        res
    };

    // Cells of a point's grid window (a point on a cell edge may belong to either neighbour).
    let window_cells = |x: f64, z: f64| -> Vec<usize> {
        let u = (x + w / 2.0) / cell;
        let v = (z + d / 2.0) / cell;
        let t = TOL / cell;
        let is: Vec<i64> = [(u - t).floor() as i64, (u + t).floor() as i64]
            .iter()
            .map(|&i| i.clamp(0, cols - 1))
            .collect();
        let js: Vec<i64> = [(v - t).floor() as i64, (v + t).floor() as i64]
            .iter()
            .map(|&j| j.clamp(0, rows - 1))
            .collect();
        let mut res: Vec<usize> = Vec::new();
        for &j in &js {
            for &i in &is {
                let c = (j * cols + i) as usize;
                if !res.contains(&c) {
                    res.push(c);
                }
            }
        }
        res
    };

    // Items: footprints, heights and containment.
    let mut vs: Vec<Option<VItem>> = Vec::new();
    let mut cov: Vec<Option<Vec<usize>>> = Vec::new();
    for (k, it) in layout.items.iter().enumerate() {
        if it.n != k {
            out.push(format!("item {k}: n is {}", it.n));
        }
        let g = if it.g < groups.len() {
            Some(it.g)
        } else {
            None
        };
        if g.is_none() {
            out.push(format!(
                "item {k} ({}): group {} is not an index of layout.groups",
                it.id, it.g
            ));
        }
        let Some(cat) = catalog.items.get(&it.id) else {
            out.push(format!("item {k}: unknown id \"{}\"", it.id));
            vs.push(None);
            cov.push(None);
            continue;
        };
        let kind = kind_of(cat);
        let label = format!("item {k} ({})", it.id);
        if ![0.0, 90.0, 180.0, 270.0].contains(&it.rot) {
            out.push(format!("{label}: rot {} is not 0/90/180/270", js(it.rot)));
        }
        if let Some(h) = it.host {
            if h >= k {
                out.push(format!("{label}: host {h} is not an earlier item"));
            }
        }
        let fw = cat.size[0];
        let fd = if kind == Kind::Wall {
            WALLMOUNT_DEPTH
        } else {
            cat.size[2]
        };
        let along = ((it.rot % 180.0) + 180.0) % 180.0 == 0.0;
        let sx = if along { fw } else { fd };
        let sz = if along { fd } else { fw };
        let v = VItem {
            k,
            id: &it.id,
            cat,
            g,
            kind,
            x: it.x,
            z: it.z,
            rot: it.rot,
            host: it.host,
            fw,
            fd,
            x0: it.x - sx / 2.0,
            x1: it.x + sx / 2.0,
            z0: it.z - sz / 2.0,
            z1: it.z + sz / 2.0,
            top: it.y + cat.size[1],
        };
        let host_top_group = it
            .host
            .and_then(|h| vs.get(h).and_then(|o| o.as_ref()))
            .map(|h| (h.k, h.top, h.g, h.x0, h.x1, h.z0, h.z1));
        if kind == Kind::Surface {
            match host_top_group {
                None => out.push(format!("{label}: surface item without a host")),
                Some((hk, htop, hg, hx0, hx1, hz0, hz1)) => {
                    if hg != v.g {
                        out.push(format!(
                            "{label}: host item {hk} is in group {}, not group {}",
                            hg.map_or("-1".to_string(), |x| x.to_string()),
                            v.g.map_or("-1".to_string(), |x| x.to_string())
                        ));
                    }
                    if (it.y - htop).abs() > TOL {
                        out.push(format!("{label}: y {}, host top {}", js(it.y), js(htop)));
                    }
                    if v.x0 < hx0 - TOL || v.x1 > hx1 + TOL || v.z0 < hz0 - TOL || v.z1 > hz1 + TOL
                    {
                        out.push(format!("{label}: footprint not inside its host footprint"));
                    }
                }
            }
            vs.push(Some(v));
            cov.push(None);
            continue;
        }
        let exp_y = if kind == Kind::Wall {
            cat.mount.unwrap_or(0.0)
        } else {
            0.0
        };
        if (it.y - exp_y).abs() > TOL {
            out.push(format!("{label}: y {}, expected {}", js(it.y), js(exp_y)));
        }
        if v.x0 < -w / 2.0 - TOL
            || v.x1 > w / 2.0 + TOL
            || v.z0 < -d / 2.0 - TOL
            || v.z1 > d / 2.0 + TOL
        {
            out.push(format!("{label}: footprint outside the room"));
        }
        cov.push(Some(covered(v.x0, v.x1, v.z0, v.z1)));
        vs.push(Some(v));
    }

    // Occupancy: solids (walk included) occupy; non-walk solids block.
    let mut blk = vec![0_u8; n_cells];
    let mut solids: Vec<usize> = Vec::new();
    for v in vs.iter().flatten() {
        if v.kind != Kind::Solid {
            continue;
        }
        solids.push(v.k);
        if v.cat.walk {
            continue;
        }
        for &c in cov_slice(&cov, v.k) {
            blk[c] = 1;
        }
    }

    // Door zone and spine: no solid, wall or floor item may cover them.
    for v in vs.iter().flatten() {
        if v.kind == Kind::Surface {
            continue;
        }
        if cov_slice(&cov, v.k).iter().any(|&c| ds[c]) {
            out.push(format!(
                "item {} ({}): covers the door zone or spine",
                v.k, v.id
            ));
        }
    }

    // Solid footprints must not overlap each other.
    for (a, &sa) in solids.iter().enumerate() {
        for &sb in &solids[a + 1..] {
            let p = vs[sa].as_ref().expect("a solid is placed");
            let q = vs[sb].as_ref().expect("a solid is placed");
            if p.x1.min(q.x1) - p.x0.max(q.x0) > TOL && p.z1.min(q.z1) - p.z0.max(q.z0) > TOL {
                out.push(format!(
                    "items {} ({}) and {} ({}) overlap",
                    p.k, p.id, q.k, q.id
                ));
            }
        }
    }

    // Clear zones stay free of solids, except those of the same group (a chair in its desk's clear zone).
    for v in vs.iter().flatten() {
        if v.kind != Kind::Solid && v.kind != Kind::Wall {
            continue;
        }
        let (front, side) = match v.cat.clear {
            Some(c) => (c.front, c.side),
            None => (0.0, 0.0),
        };
        if front <= 0.0 {
            continue;
        }
        let lx = v.fw / 2.0 + side;
        let pts = [
            rotated(-lx, v.fd / 2.0, v.rot),
            rotated(lx, v.fd / 2.0, v.rot),
            rotated(lx, v.fd / 2.0 + front, v.rot),
            rotated(-lx, v.fd / 2.0 + front, v.rot),
        ];
        let mnx = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let mxx = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let mnz = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let mxz = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        let clear = covered(v.x + mnx, v.x + mxx, v.z + mnz, v.z + mxz);
        let mut in_clear = vec![false; n_cells];
        for c in clear {
            in_clear[c] = true;
        }
        for &s in &solids {
            let o = vs[s].as_ref().expect("a solid is placed");
            // A group's own items may share its clear zones (a chair in front of its desk).
            if o.k == v.k || o.host == Some(v.k) || o.g == v.g {
                continue;
            }
            if cov_slice(&cov, s).iter().any(|&c| in_clear[c]) {
                out.push(format!(
                    "item {} ({}): clear zone blocked by item {} ({})",
                    v.k, v.id, o.k, o.id
                ));
                break;
            }
        }
    }

    // Z-fighting: two non-floor items with coplanar top faces that overlap.
    let placed: Vec<&VItem> = vs
        .iter()
        .flatten()
        .filter(|v| v.kind != Kind::Floor)
        .collect();
    for (a, p) in placed.iter().enumerate() {
        for q in &placed[a + 1..] {
            if (p.top - q.top).abs() > TOL {
                continue;
            }
            if p.x1.min(q.x1) - p.x0.max(q.x0) > TOL && p.z1.min(q.z1) - p.z0.max(q.z0) > TOL {
                out.push(format!(
                    "items {} ({}) and {} ({}): coplanar tops overlap",
                    p.k, p.id, q.k, q.id
                ));
            }
        }
    }

    // Reachability: BFS over non-blocking cells from the door zone.
    let mut vis = vec![false; n_cells];
    let mut queue: Vec<usize> = Vec::new();
    for &c in &door_cells {
        if blk[c] != 0 {
            continue;
        }
        vis[c] = true;
        queue.push(c);
    }
    let mut h = 0;
    while h < queue.len() {
        let c = queue[h];
        h += 1;
        let i = c as i64 % cols;
        let j = (c as i64 - i) / cols;
        let nb = [
            if i > 0 { c as i64 - 1 } else { -1 },
            if i < cols - 1 { c as i64 + 1 } else { -1 },
            if j > 0 { c as i64 - cols } else { -1 },
            if j < rows - 1 { c as i64 + cols } else { -1 },
        ];
        for n in nb {
            if n < 0 {
                continue;
            }
            let n = n as usize;
            if vis[n] || blk[n] != 0 {
                continue;
            }
            vis[n] = true;
            queue.push(n);
        }
    }
    let reach = |x: f64, z: f64| -> bool { window_cells(x, z).iter().any(|&c| vis[c]) };

    // Access cells of solid and wall items.
    for v in vs.iter().flatten() {
        if v.kind != Kind::Solid && v.kind != Kind::Wall {
            continue;
        }
        if v.cat.walk {
            if !reach(v.x, v.z) {
                out.push(format!(
                    "item {} ({}): centre (walk) unreachable from the door",
                    v.k, v.id
                ));
            }
            continue;
        }
        let front = v.cat.clear.map_or(0.0, |c| c.front);
        let (dx, dz) = rotated(0.0, v.fd / 2.0 + front.max(MIN_ACCESS) / 2.0, v.rot);
        if !reach(v.x + dx, v.z + dz) {
            out.push(format!(
                "item {} ({}): front access unreachable from the door",
                v.k, v.id
            ));
        }
    }

    // Spots: the catalogue's spots, in item order, transformed by each item's pose.
    let mut expected: Vec<Expected> = Vec::new();
    for v in vs.iter().flatten() {
        for s in &v.cat.spots {
            let (dx, dz) = rotated(s.at[0], s.at[1], v.rot);
            expected.push(Expected {
                item: v.k,
                x: v.x + dx,
                z: v.z + dz,
                face: (((s.face + v.rot) % 360.0) + 360.0) % 360.0,
            });
        }
    }
    if expected.len() != layout.spots.len() {
        out.push(format!(
            "spots: {} listed, catalogue gives {}",
            layout.spots.len(),
            expected.len()
        ));
    }
    for (q, e) in expected.iter().enumerate().take(layout.spots.len()) {
        let s = &layout.spots[q];
        if s.item != e.item
            || (s.x - e.x).abs() > SPOT_TOL
            || (s.z - e.z).abs() > SPOT_TOL
            || s.face != e.face
        {
            out.push(format!(
                "spot {q}: item {} at ({}, {}) face {}, catalogue gives item {} at ({:.3}, {:.3}) face {}",
                s.item,
                js(s.x),
                js(s.z),
                js(s.face),
                e.item,
                e.x,
                e.z,
                js(e.face)
            ));
        }
    }
    // A robot spot stays 0.15 off each wall that exists (the room edge where there is none), lies inside the grid,
    // and never sits on a blocking cell (walk items may be stood on).
    let in_walls = |side: Door| sh.walls.contains(&side);
    let spot_x0 = (if in_walls(Door::W) {
        -w / 2.0 + 0.15
    } else {
        -w / 2.0
    }) - TOL;
    let spot_x1 = (if in_walls(Door::E) {
        w / 2.0 - 0.15
    } else {
        w / 2.0
    }) + TOL;
    let spot_z0 = (if in_walls(Door::N) {
        -d / 2.0 + 0.15
    } else {
        -d / 2.0
    }) - TOL;
    let spot_z1 = (if in_walls(Door::S) {
        d / 2.0 - 0.15
    } else {
        d / 2.0
    }) + TOL;
    for s in &layout.spots {
        let owner = vs.get(s.item).and_then(|o| o.as_ref());
        let label = match owner {
            Some(o) => format!(
                "spot of item {} ({}) at ({}, {})",
                s.item,
                o.id,
                js(s.x),
                js(s.z)
            ),
            None => format!("spot of item {} at ({}, {})", s.item, js(s.x), js(s.z)),
        };
        if !(s.x >= spot_x0 && s.x <= spot_x1 && s.z >= spot_z0 && s.z <= spot_z1) {
            out.push(format!(
                "{label}: outside the room (robot spots keep 0.15 from each wall)"
            ));
            continue;
        }
        let gi = ((s.x + w / 2.0) / cell).floor() as i64;
        let gj = ((s.z + d / 2.0) / cell).floor() as i64;
        if gi < 0 || gi >= cols || gj < 0 || gj >= rows {
            out.push(format!(
                "{label}: outside the grid (in the strip beyond the last whole cell)"
            ));
            continue;
        }
        let cells = window_cells(s.x, s.z);
        if cells.iter().all(|&c| blk[c] == 1) {
            out.push(format!(
                "{label}: lies on a blocking cell (inside a solid footprint)"
            ));
            continue;
        }
        let Some(o) = owner else { continue };
        if o.kind != Kind::Solid && o.kind != Kind::Wall {
            continue;
        }
        if !reach(s.x, s.z) {
            out.push(format!("{label}: unreachable from the door"));
        }
    }

    // v2 purpose rules. The first item of each group instance is its anchor (items are in placement order).
    let mut anchor_of: Vec<Option<usize>> = vec![None; groups.len()];
    for v in vs.iter().flatten() {
        if let Some(g) = v.g {
            if anchor_of[g].is_none() {
                anchor_of[g] = Some(v.k);
            }
        }
    }
    let group_of = |v: &VItem| -> Option<&str> { v.g.map(|g| groups[g].as_str()) };
    let def_of =
        |v: &VItem| -> Option<&CatGroup> { v.g.and_then(|g| cat_groups.get(groups[g].as_str())) };
    let gap_of = |v: &VItem, side: Door| -> f64 {
        match side {
            Door::N => v.z0 + d / 2.0,
            Door::S => d / 2.0 - v.z1,
            Door::W => v.x0 + w / 2.0,
            Door::E => w / 2.0 - v.x1,
        }
    };
    let touches = |v: &VItem, side: Door| gap_of(v, side) <= ANCHOR_TOUCH + TOL;
    // The item's back is within reach of the wall on the side its rotation faces away from.
    let against_own_wall = |v: &VItem| -> bool {
        match side_of_rot(v.rot) {
            Some(side) => edges.contains(&side) && touches(v, side),
            None => false,
        }
    };

    // Anchors: a wall anchor backs onto a wall and faces into the room; a corner anchor touches two room edges; a
    // door anchor stands at the door's edge. Wall-mounted items back onto their own wall.
    for v in vs.iter().flatten() {
        let label = format!("item {} ({})", v.k, v.id);
        let def = def_of(v);
        let is_anchor = v.g.is_some_and(|g| anchor_of[g] == Some(v.k));
        if let (true, Some(def)) = (is_anchor, def) {
            if def.anchor == Anchor::Wall && !against_own_wall(v) {
                out.push(format!(
                    "{label}: wall anchor's back is not within {} of the wall on its own side (rot {})",
                    js(ANCHOR_TOUCH),
                    js(v.rot)
                ));
            } else if def.anchor == Anchor::Corner {
                let ns = edges
                    .iter()
                    .any(|&s| (s == Door::N || s == Door::S) && touches(v, s));
                let ew = edges
                    .iter()
                    .any(|&s| (s == Door::E || s == Door::W) && touches(v, s));
                if !(ns && ew) {
                    out.push(format!(
                        "{label}: corner anchor is not within {} of two room edges",
                        js(ANCHOR_TOUCH)
                    ));
                }
            } else if def.anchor == Anchor::Door && !touches(v, door) {
                out.push(format!(
                    "{label}: door anchor is not within {} of the door edge",
                    js(ANCHOR_TOUCH)
                ));
            }
        }
        if v.kind == Kind::Wall && !against_own_wall(v) {
            out.push(format!(
                "{label}: wall-mounted item is not within {} of the wall on its own side (rot {})",
                js(ANCHOR_TOUCH),
                js(v.rot)
            ));
        }
    }

    // Seats sit in a purpose group and face the group's anchor (a meeting chair faces its table).
    for v in vs.iter().flatten() {
        if !SEAT_IDS.contains(&v.id) {
            continue;
        }
        let label = format!("item {} ({})", v.k, v.id);
        let in_purpose = group_of(v).is_some_and(|id| SEAT_GROUPS.contains(&id));
        if !in_purpose {
            out.push(format!(
                "{label}: seat is not in a desk-set, meeting, station or reading-nook group"
            ));
            continue;
        }
        let Some(a) = v.g.and_then(|g| anchor_of[g]) else {
            continue;
        };
        if a == v.k {
            continue;
        }
        let Some(anchor) = vs[a].as_ref() else {
            continue;
        };
        let ang = angle_deg(front_of(v.rot), (anchor.x - v.x, anchor.z - v.z));
        if ang > SEAT_FACE_TOL + TOL {
            out.push(format!(
                "{label}: faces {:.1} degrees off its anchor item {} ({}), limit {}",
                ang,
                anchor.k,
                anchor.id,
                js(SEAT_FACE_TOL)
            ));
        }
    }

    // Relations sit on their side of the anchor, in the anchor's own frame (the offset is turned back by the anchor's
    // rot first): front = +z, beside = -x, beside-other = +x, row = +x along the anchor's row. Tolerance 0.1 on the
    // other axis. Items matched to their catalogue entry by id; a repeated id takes its first entry.
    const REL_TOL: f64 = 0.1;
    for v in vs.iter().flatten() {
        let Some(g) = v.g else { continue };
        if anchor_of[g] == Some(v.k) {
            continue;
        }
        let Some(def) = def_of(v) else { continue };
        let Some(anchor) = anchor_of[g].and_then(|a| vs[a].as_ref()) else {
            continue;
        };
        let Some(entry) = def.items.iter().find(|e| {
            e.id == v.id
                && e.rel.is_some()
                && !matches!(
                    e.rel.as_deref(),
                    Some("around") | Some("mirror") | Some("on")
                )
        }) else {
            continue;
        };
        let (lx, lz) = rotated(v.x - anchor.x, v.z - anchor.z, (360.0 - anchor.rot) % 360.0);
        let rel = entry.rel.as_deref().unwrap_or("");
        let ok = match rel {
            "front" => lz > 0.0 && lx.abs() <= REL_TOL,
            "beside" => lx < 0.0 && lz.abs() <= REL_TOL,
            "beside-other" | "row" => lx > 0.0 && lz.abs() <= REL_TOL,
            _ => true,
        };
        if !ok {
            out.push(format!(
                "item {} ({}): not on the {rel} side of its anchor {} ({}) in the anchor's frame (local {:.3}, {:.3})",
                v.k, v.id, anchor.k, anchor.id, lx, lz
            ));
        }
    }

    // The middle region (|x| <= W/4, |z| <= D/4) belongs to centre groups only.
    for v in vs.iter().flatten() {
        if v.cat.layer.as_deref() == Some("floor") {
            continue;
        }
        if v.x.abs() > w / 4.0 + TOL || v.z.abs() > d / 4.0 + TOL {
            continue;
        }
        if def_of(v).map(|def| def.anchor) != Some(Anchor::Center) {
            out.push(format!(
                "item {} ({}): in the middle of the room but its group is not a centre group",
                v.k, v.id
            ));
        }
    }

    // A wall-mounted item's view strip (from its front face out MOUNT_VIEW, its width plus 0.1 each side) holds no
    // solid that rises above the mount's middle.
    for wm in vs.iter().flatten() {
        if wm.kind != Kind::Wall {
            continue;
        }
        let m = wm.cat.mount.unwrap_or(0.0);
        let h = wm.cat.size[1];
        let half = wm.fw / 2.0 + 0.1;
        let pts = [
            rotated(-half, wm.fd / 2.0, wm.rot),
            rotated(half, wm.fd / 2.0, wm.rot),
            rotated(half, wm.fd / 2.0 + MOUNT_VIEW, wm.rot),
            rotated(-half, wm.fd / 2.0 + MOUNT_VIEW, wm.rot),
        ];
        let sx0 = wm.x + pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let sx1 = wm.x + pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let sz0 = wm.z + pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let sz1 = wm.z + pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        for s in vs.iter().flatten() {
            if s.kind != Kind::Solid {
                continue;
            }
            if s.x0 < sx0 - TOL || s.x1 > sx1 + TOL || s.z0 < sz0 - TOL || s.z1 > sz1 + TOL {
                continue;
            }
            if s.top > m + h / 2.0 + TOL {
                out.push(format!(
                    "item {} ({}): view blocked by item {} ({}), top {} above {}",
                    wm.k,
                    wm.id,
                    s.k,
                    s.id,
                    js(s.top),
                    js(m + h / 2.0)
                ));
            }
        }
    }

    // Spots face their item within SPOT_FACE_TOL (a spot on the item's own centre has no direction to test).
    for s in &layout.spots {
        let Some(owner) = vs.get(s.item).and_then(|o| o.as_ref()) else {
            continue;
        };
        let dx = owner.x - s.x;
        let dz = owner.z - s.z;
        if dx.hypot(dz) <= 0.01 {
            continue;
        }
        let ang = angle_deg(front_of(s.face), (dx, dz));
        if ang > SPOT_FACE_TOL + TOL {
            out.push(format!(
                "spot of item {} ({}) at ({}, {}): faces {:.1} degrees off its item, limit {}",
                s.item,
                owner.id,
                js(s.x),
                js(s.z),
                ang,
                js(SPOT_FACE_TOL)
            ));
        }
    }

    // Required groups present, with the counts the template asks for (per "agent" entries scale with the desk
    // count). The asked-for counts are listed in the template's order, as the web's Map does.
    let agents = input
        .agents
        .unwrap_or_else(|| ((w * d / 6.0).floor() as i64).clamp(1, 4) as usize);
    {
        let mut need: Vec<(String, usize)> = Vec::new();
        for e in &tpl.required {
            let add = if e.per.as_deref() == Some("agent") {
                agents
            } else {
                1
            };
            match need.iter_mut().find(|(g, _)| *g == e.group) {
                Some(slot) => slot.1 += add,
                None => need.push((e.group.clone(), add)),
            }
        }
        for (gid, asked) in need {
            let actual = groups.iter().filter(|x| **x == gid).count();
            if actual < asked {
                out.push(format!(
                    "required group \"{gid}\": {actual} placed, {asked} asked for"
                ));
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::super::generate::{Door as D, Input, data, fixture_cases, generate};
    use super::*;

    /// Every generated layout of the golden cases is checked by the verifier (the golden set predates v2, so this
    /// test checks the verifier against the current generator instead of the golden hashes): a layout with no
    /// dropped group has no violation.
    #[test]
    fn generated_layouts_without_a_drop_verify_clean() {
        let d = data();
        let mut failures = Vec::new();
        for c in fixture_cases() {
            let layout = generate(&c.input, d);
            if !layout.dropped.is_empty() {
                continue;
            }
            let v = verify(&layout, &c.input, d);
            if !v.is_empty() {
                failures.push(format!(
                    "{} {}x{} seed {}: {v:?}",
                    c.input.role, c.input.w, c.input.d, c.input.seed
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} layouts violate:\n{}",
            failures.len(),
            failures[..failures.len().min(5)].join("\n")
        );
    }

    /// A moved robot spot is caught, and a layout whose door side is wrong is reported in the shell line.
    #[test]
    fn moved_spots_and_wrong_door_sides_are_reported() {
        let d = data();
        // A moved robot spot is caught: the verifier reports its position against the catalogue.
        {
            let case = fixture_cases()
                .into_iter()
                .find(|c| c.dropped == 0)
                .expect("a fixture with no drop");
            let layout = generate(&case.input, d);
            assert!(!layout.spots.is_empty(), "the case has robot spots");
            let mut moved = layout.clone();
            moved.spots[0].x = case.input.w / 2.0 + 0.5;
            let v = verify(&moved, &case.input, d);
            assert!(v.iter().any(|m| m.starts_with("spot 0:")), "{v:?}");
        }
        // A layout whose door side is wrong is reported in the shell line.
        {
            let input = Input {
                role: "librarian".to_string(),
                w: 4.0,
                d: 3.5,
                door: D::S,
                seed: 7,
                agents: None,
            };
            let mut layout = generate(&input, d);
            layout.shell.door.side = D::N;
            let v = verify(&layout, &input, d);
            assert!(
                v.iter().any(|m| m == "shell: door side n, expected s"),
                "{v:?}"
            );
        }
    }

    /// Parity with the web's verifier: `APZ_ROOM_DUMP=<file>` (the same JSON-lines dump the generator test reads)
    /// must give the web's violation list for every case, in order and wording. Run with `--ignored`.
    #[test]
    #[ignore = "needs APZ_ROOM_DUMP=<web dump>"]
    fn violations_match_the_web_dump() {
        let path = std::env::var("APZ_ROOM_DUMP").expect("APZ_ROOM_DUMP names the web dump");
        let text = std::fs::read_to_string(path).expect("the dump is readable");
        let dd = data();
        let mut bad = Vec::new();
        let mut total = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let r: serde_json::Value = serde_json::from_str(line).expect("a dump line parses");
            total += 1;
            let input = Input {
                role: r["role"].as_str().unwrap().to_string(),
                w: r["w"].as_f64().unwrap(),
                d: r["d"].as_f64().unwrap(),
                door: Door::parse(r["door"].as_str().unwrap()).unwrap(),
                seed: r["seed"].as_u64().unwrap() as u32,
                agents: r["agents"].as_u64().map(|a| a as usize),
            };
            let layout = generate(&input, dd);
            let got = verify(&layout, &input, dd);
            let want: Vec<String> = r["violations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            if got != want {
                bad.push(format!(
                    "{} {}x{} {} seed {}:\n  rust {got:?}\n  web  {want:?}",
                    input.role,
                    input.w,
                    input.d,
                    input.door.as_str(),
                    input.seed
                ));
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {total} cases differ in violations, first: {:?}",
            bad.len(),
            &bad[..bad.len().min(4)]
        );
        eprintln!("violations_match_the_web_dump: {total} cases match");
    }
}
