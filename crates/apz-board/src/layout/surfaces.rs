//! The walk surfaces and links of a layout (docs/design/walk-surfaces.md), assembled in one place from the layout's
//! geometry: the PCB's pieces, the plates' pieces (cut by their rooms and their landings), the room floors, and each
//! plate's mouth (mouth.rs). Surface ids are the indices of the pieces, in that order.
//!
//! Links: two flat surfaces of one height that share a boundary are joined by a Step (the PCB, the plates, the landings,
//! the cabs and the bridges). A room joins the plate piece at its door by a Door. A ramp joins the plate and the board by
//! a Ramp at each end. A lift joins its bottom and top landings by a Lift through the cab. Sides: a side is Open where a
//! link or a neighbour joins it; otherwise a board, cab or room wall is Walled, and a plate, landing, bridge or ramp edge
//! is Drop (a free edge that does not block the grid; no link leaves it).

use crate::walk::{LinkKind, Plane, Side, SurfaceKind, WalkLink, WalkSet, WalkSurface};

use super::geom::{subtract, Rect};
use super::mouth::{Mouth, MouthIds, EDGE};

/// A plate's geometry: its id (the plate index), its footprint and its walking top.
#[derive(Debug, Clone)]
pub struct PlateGeo {
    pub id: usize,
    pub rect: Rect,
    pub y: f64,
}

/// A room's geometry: its id, its plate, its floor rect and height, its door point and the outward direction of its door.
#[derive(Debug, Clone)]
pub struct RoomGeo {
    pub id: usize,
    pub plate: usize,
    pub rect: Rect,
    pub floor_y: f64,
    pub door_at: [f64; 2],
    pub face: [f64; 2],
}

/// The inputs of the walk: the PCB, the plates, the rooms, the mouths (each with its plate) and the PCB's holes from tall
/// parts.
pub struct Assembly<'a> {
    pub pcb: Rect,
    pub pcb_top: f64,
    pub tall: &'a [Rect],
    pub plates: &'a [PlateGeo],
    pub rooms: &'a [RoomGeo],
    /// Each mouth with the plate it serves.
    pub mouths: &'a [(usize, Mouth)],
}

/// The walk and the ids of each mouth's pieces (`MouthIds`, in the order of `Assembly::mouths`).
pub struct Assembled {
    pub walk: WalkSet,
    pub mouths: Vec<MouthIds>,
    /// The owner of each surface is `plate:<id>`, `room:<id>`, `board`, `ramp:<plate>`, `cab:<plate>`, `landing:<plate>:<bottom|top>`
    /// or `bridge:<plate>`. The acceptance reads the owners of reached surfaces.
    pub owners: Vec<String>,
}

/// A piece under construction: its rect, kind and plane, its owner, its plate and room (when any), and the sides a link
/// joins (`joined`, by side index).
struct Piece {
    rect: Rect,
    kind: SurfaceKind,
    plane: Plane,
    owner: String,
    plate: Option<usize>,
    room: Option<usize>,
    joined: [bool; 4],
}

/// The side segment of a rect: side k runs from its poly point k to point k+1 (0 west, 1 +z, 2 east, 3 -z).
fn side_segment(r: &Rect, k: usize) -> ([f64; 2], [f64; 2]) {
    match k {
        0 => ([r.x0, r.z0], [r.x0, r.z1]),
        1 => ([r.x0, r.z1], [r.x1, r.z1]),
        2 => ([r.x1, r.z1], [r.x1, r.z0]),
        _ => ([r.x1, r.z0], [r.x0, r.z0]),
    }
}

/// The outward normal of side k.
fn side_normal(k: usize) -> [f64; 2] {
    [[-1.0, 0.0], [0.0, 1.0], [1.0, 0.0], [0.0, -1.0]][k]
}

/// Does a point lie on side k of a rect (within a tolerance)?
fn on_side(r: &Rect, k: usize, p: [f64; 2]) -> bool {
    let ([ax, az], [bx, bz]) = side_segment(r, k);
    let eps = 1e-6;
    if ax == bx {
        (p[1] >= az.min(bz) - eps) && (p[1] <= az.max(bz) + eps) && (p[0] - ax).abs() <= eps
    } else {
        (p[0] >= ax.min(bx) - eps) && (p[0] <= ax.max(bx) + eps) && (p[1] - az).abs() <= eps
    }
}

/// The free side k of a PCB piece: Walled against a tall part (a wall) or the board's edge, and Drop against a plate or a
/// mouth, where the cliff does not block the cells beside it (the TS terraces are walkable; only parts block).
pub(super) fn pcb_free_side(rect: &Rect, k: usize, pcb: &Rect, walls: &[Rect]) -> Side {
    let n = side_normal(k);
    let mid = side_mid(rect, k);
    let probe = [mid[0] + n[0] * 1e-6, mid[1] + n[1] * 1e-6];
    let outside = !contains(pcb, probe);
    let wall = walls.iter().any(|w| w.x0 < probe[0] && probe[0] < w.x1 && w.z0 < probe[1] && probe[1] < w.z1);
    if outside || wall {
        Side::Walled
    } else {
        Side::Drop
    }
}

/// Does a rect contain a point (closed, within a tolerance)?
fn contains(r: &Rect, p: [f64; 2]) -> bool {
    let eps = 1e-9;
    p[0] >= r.x0 - eps && p[0] <= r.x1 + eps && p[1] >= r.z0 - eps && p[1] <= r.z1 + eps
}

/// Do two rects share a boundary segment of positive length? Returns the side of `a`, the side of `b`, and the midpoint.
pub(super) fn shared_side(a: &Rect, b: &Rect) -> Option<(usize, usize, [f64; 2])> {
    let eps = 1e-9;
    let zo = a.z1.min(b.z1) - a.z0.max(b.z0);
    let xo = a.x1.min(b.x1) - a.x0.max(b.x0);
    let zm = (a.z0.max(b.z0) + a.z1.min(b.z1)) / 2.0;
    let xm = (a.x0.max(b.x0) + a.x1.min(b.x1)) / 2.0;
    if zo > eps {
        if (a.x1 - b.x0).abs() < eps {
            return Some((2, 0, [a.x1, zm]));
        }
        if (a.x0 - b.x1).abs() < eps {
            return Some((0, 2, [a.x0, zm]));
        }
    }
    if xo > eps {
        if (a.z1 - b.z0).abs() < eps {
            return Some((1, 3, [xm, a.z1]));
        }
        if (a.z0 - b.z1).abs() < eps {
            return Some((3, 1, [xm, a.z0]));
        }
    }
    None
}

/// The first piece of a kind and plate whose rect contains a point.
fn find(pieces: &[Piece], kind: SurfaceKind, plate: Option<usize>, p: [f64; 2]) -> Option<usize> {
    pieces.iter().position(|q| q.kind == kind && (plate.is_none() || q.plate == plate) && contains(&q.rect, p))
}

/// Marks the sides of a piece that a point on them joins.
fn mark(piece: &mut Piece, p: [f64; 2]) {
    for k in 0..4 {
        if on_side(&piece.rect, k, p) {
            piece.joined[k] = true;
        }
    }
}

/// A piece thinner than this in either direction is float residue where two cut edges meet within rounding (the plan
/// is in metres), not floor. Its signed area is below the rounding of the shoelace sum at the plan's coordinates, so
/// its winding cannot be read; it is dropped rather than emitted.
const SLIVER: f64 = 1e-6;

fn sliver(r: &Rect) -> bool {
    r.x1 - r.x0 < SLIVER || r.z1 - r.z0 < SLIVER
}

/// Is a flat, walkable kind joined to its neighbours by Step (at the same height)?
fn flat_kind(k: SurfaceKind) -> bool {
    matches!(k, SurfaceKind::Board | SurfaceKind::Plate | SurfaceKind::Landing | SurfaceKind::Cab | SurfaceKind::Bridge)
}

/// Assembles the walk of a layout: its surfaces, links and the mouth ids.
pub fn assemble(a: &Assembly) -> Assembled {
    let mut pieces: Vec<Piece> = Vec::new();
    let flat = |y: f64| Plane::flat(y);

    // The PCB: the board less the plates, the tall parts, the ramps and the cabs and queues of the lifts.
    let mut pcb_holes: Vec<Rect> = a.plates.iter().map(|p| p.rect).collect();
    pcb_holes.extend_from_slice(a.tall);
    for (_, m) in a.mouths {
        match m {
            Mouth::Ramp { rect, .. } => pcb_holes.push(*rect),
            Mouth::Lift { cab, bottom, .. } => {
                pcb_holes.push(*cab);
                pcb_holes.push(*bottom);
            }
        }
    }
    for rect in subtract(a.pcb, &pcb_holes).into_iter().filter(|r| !sliver(r)) {
        pieces.push(Piece { rect, kind: SurfaceKind::Board, plane: flat(a.pcb_top), owner: "board".to_string(), plate: None, room: None, joined: [false; 4] });
    }

    // The plates: each less its rooms and its lift's top landing.
    for p in a.plates {
        let mut holes: Vec<Rect> = a.rooms.iter().filter(|r| r.plate == p.id).map(|r| r.rect).collect();
        holes.extend(a.mouths.iter().filter_map(|(_, m)| match m {
            Mouth::Lift { top, top_kind: SurfaceKind::Landing, .. } if contains(&p.rect, [(top.x0 + top.x1) / 2.0, (top.z0 + top.z1) / 2.0]) => Some(*top),
            _ => None,
        }));
        for rect in subtract(p.rect, &holes).into_iter().filter(|r| !sliver(r)) {
            pieces.push(Piece { rect, kind: SurfaceKind::Plate, plane: flat(p.y), owner: format!("plate:{}", p.id), plate: Some(p.id), room: None, joined: [false; 4] });
        }
    }

    // The room floors.
    for r in a.rooms {
        pieces.push(Piece { rect: r.rect, kind: SurfaceKind::RoomFloor, plane: flat(r.floor_y), owner: format!("room:{}", r.id), plate: Some(r.plate), room: Some(r.id), joined: [false; 4] });
    }

    // The mouths' pieces, in the order of the mouths. Each mouth's plate is the plate its footprint was planned on.
    let mut ids: Vec<MouthIds> = Vec::with_capacity(a.mouths.len());
    for (plate, m) in a.mouths {
        let plate = *plate;
        match m {
            Mouth::Ramp { rect, plane, .. } => {
                let ramp = pieces.len() as u32;
                pieces.push(Piece { rect: *rect, kind: SurfaceKind::Ramp, plane: *plane, owner: format!("ramp:{plate}"), plate: Some(plate), room: None, joined: [false; 4] });
                ids.push(MouthIds { plate, ramp: Some(ramp), cab: None, bottom: None, top: None });
            }
            Mouth::Lift { cab, bottom, top, top_kind, bottom_y, top_y, .. } => {
                let cab_id = pieces.len() as u32;
                pieces.push(Piece { rect: *cab, kind: SurfaceKind::Cab, plane: flat(*bottom_y), owner: format!("cab:{plate}"), plate: Some(plate), room: None, joined: [false; 4] });
                let bottom_id = pieces.len() as u32;
                pieces.push(Piece { rect: *bottom, kind: SurfaceKind::Landing, plane: flat(*bottom_y), owner: format!("landing:{plate}:bottom"), plate: Some(plate), room: None, joined: [false; 4] });
                let (top_owner, top_kind) = if *top_kind == SurfaceKind::Bridge { (format!("bridge:{plate}"), SurfaceKind::Bridge) } else { (format!("landing:{plate}:top"), SurfaceKind::Landing) };
                let top_id = pieces.len() as u32;
                pieces.push(Piece { rect: *top, kind: top_kind, plane: flat(*top_y), owner: top_owner, plate: Some(plate), room: None, joined: [false; 4] });
                ids.push(MouthIds { plate, ramp: None, cab: Some(cab_id), bottom: Some(bottom_id), top: Some(top_id) });
            }
        }
    }

    // Links. Adjacent flat surfaces of one height: Step at their shared boundary.
    let mut links: Vec<WalkLink> = Vec::new();
    for i in 0..pieces.len() {
        for j in i + 1..pieces.len() {
            let (p, q) = (&pieces[i], &pieces[j]);
            if !flat_kind(p.kind) || !flat_kind(q.kind) || p.plane != q.plane {
                continue;
            }
            if let Some((sa, sb, at)) = shared_side(&p.rect, &q.rect) {
                pieces[i].joined[sa] = true;
                pieces[j].joined[sb] = true;
                links.push(WalkLink { from: i as u32, to: j as u32, kind: LinkKind::Step, via: None, at });
            }
        }
    }

    // Room doors: the room floor joins the plate piece in front of its door.
    for r in a.rooms {
        let probe = [r.door_at[0] + r.face[0] * EDGE, r.door_at[1] + r.face[1] * EDGE];
        let Some(room) = pieces.iter().position(|q| q.room == Some(r.id)) else { continue };
        let Some(plate_piece) = find(&pieces, SurfaceKind::Plate, Some(r.plate), probe) else { continue };
        mark(&mut pieces[room], r.door_at);
        mark(&mut pieces[plate_piece], r.door_at);
        links.push(WalkLink { from: room as u32, to: plate_piece as u32, kind: LinkKind::Door, via: None, at: r.door_at });
    }

    // Mouths: a ramp joins the plate at its edge and the board at its far end; a lift joins its bottom and top through the
    // cab; a gangway's bridge joins the plate by adjacency (above) and its top is the bridge.
    for (k, (plate, m)) in a.mouths.iter().enumerate() {
        let plate = *plate;
        match m {
            Mouth::Ramp { edge, far, inside, beyond, .. } => {
                let ramp = ids[k].ramp.expect("ramp id") as usize;
                if let Some(pp) = find(&pieces, SurfaceKind::Plate, Some(plate), *inside) {
                    mark(&mut pieces[ramp], *edge);
                    mark(&mut pieces[pp], *edge);
                    links.push(WalkLink { from: ramp as u32, to: pp as u32, kind: LinkKind::Ramp, via: None, at: *edge });
                }
                if let Some(bp) = find(&pieces, SurfaceKind::Board, None, *beyond) {
                    mark(&mut pieces[ramp], *far);
                    mark(&mut pieces[bp], *far);
                    links.push(WalkLink { from: ramp as u32, to: bp as u32, kind: LinkKind::Ramp, via: None, at: *far });
                }
            }
            Mouth::Lift { edge, .. } => {
                let id = &ids[k];
                let (bottom, top, cab) = (id.bottom.expect("bottom"), id.top.expect("top"), id.cab.expect("cab"));
                mark(&mut pieces[top as usize], *edge);
                links.push(WalkLink { from: bottom, to: top, kind: LinkKind::Lift, via: Some(cab), at: *edge });
            }
        }
    }

    // Sides: Open where joined; otherwise the kind's free edge (a wall, or a Drop that blocks nothing).
    let room_rects: Vec<(usize, Rect)> = a.rooms.iter().map(|r| (r.plate, r.rect)).collect();
    let mut surfaces: Vec<WalkSurface> = Vec::with_capacity(pieces.len());
    let mut owners: Vec<String> = Vec::with_capacity(pieces.len());
    for (id, p) in pieces.iter().enumerate() {
        let mut sides = Vec::with_capacity(4);
        for k in 0..4 {
            let side = if p.joined[k] {
                Side::Open
            } else {
                match p.kind {
                    SurfaceKind::Board => pcb_free_side(&p.rect, k, &a.pcb, a.tall),
                    SurfaceKind::Cab | SurfaceKind::RoomFloor | SurfaceKind::Stair => Side::Walled,
                    SurfaceKind::Plate => {
                        // A plate's edge against a room is the room's wall; any other free edge is a fall.
                        let n = side_normal(k);
                        let mid = side_mid(&p.rect, k);
                        let probe = [mid[0] + n[0] * 1e-6, mid[1] + n[1] * 1e-6];
                        let wall = room_rects.iter().any(|(plate, rr)| Some(*plate) == p.plate && rr.x0 < probe[0] && probe[0] < rr.x1 && rr.z0 < probe[1] && probe[1] < rr.z1);
                        if wall {
                            Side::Walled
                        } else {
                            Side::Drop
                        }
                    }
                    SurfaceKind::Landing | SurfaceKind::Bridge | SurfaceKind::Ramp => Side::Drop,
                }
            };
            sides.push(side);
        }
        surfaces.push(WalkSurface { id: id as u32, kind: p.kind, poly: p.rect.poly(), height: p.plane, owner: p.owner.clone(), sides });
        owners.push(p.owner.clone());
    }
    Assembled { walk: WalkSet { surfaces, links }, mouths: ids, owners }
}

/// The midpoint of side k of a rect.
fn side_mid(r: &Rect, k: usize) -> [f64; 2] {
    let (a, b) = side_segment(r, k);
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}
