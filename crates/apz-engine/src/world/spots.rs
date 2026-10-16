// The places the behaviour engine sends robots to, in the floor's world frame: x and z on the plaza (the robot's own
// position is group.position (x, y, z) with z = the floor's y). Region 0 is the board; a room's region is its id
// (regions count from 1 in the order the rooms are built). Pure: no three.js, no DOM.
//
// A spot's face is a heading in radians in the floor's own convention: 0 faces +z (the robot's forward), +x is
// atan2's first argument, the same as the floor's robot heading. Room spots are authored in degrees in the room's frame
// (rooms.md): they are turned to the world by the room's yaw. Rules: docs/design/robots.md ("Places").

use std::collections::HashMap;
use std::f32::consts::PI;

use super::District;
use super::behaviour::SpotIn;
use super::board_layout::{BoardDistrict, BoardLayout, TraceGraph, board_height_at};
use crate::hardware::{Model, Params, PartBox, part_boxes};
use crate::rooms::generate::{LayoutItem, LayoutSpot};

/// A robot's footprint radius (the floor's ROBOT_R): how far a station stands off a part.
pub const CLEAR: f32 = 0.55;
/// The grid of free points on the board (anywhere spots).
const ANY_STEP: f32 = 2.5;
/// A part this far above the PCB is a step or taller (nav.md: the step height is 0.25).
const STEP_HEIGHT: f32 = 0.25;

/// A room's station, as rooms build it (LayoutSpot in rooms/gen.ts): in the room's own frame, the face in degrees.
pub type RoomSpot = LayoutSpot;

/// A furniture item of a room layout: `n` is the number the spots name, `id` the catalogue id (a prop id).
pub type RoomItem = LayoutItem;

/// A room as the engine sees it: its region, its district and its stations in the world frame.
pub struct RoomFrame {
    pub region: i64,
    pub d: BoardDistrict,
    pub spots: Vec<SpotIn>,
}

/// The key of a district: the same across layout rebuilds that keep the district (role and place).
pub fn district_key(d: &District) -> String {
    format!("{}|{},{}", d.role, d.cx, d.cy)
}

/// Whether (x, z) lies in a district's cell, grown by `margin` (the cell test the floor's roomAt uses).
fn in_cell(d: &BoardDistrict, x: f32, z: f32, margin: f32) -> bool {
    (x - d.district.cx).abs() <= d.w / 2.0 + margin
        && (z - d.district.cy).abs() <= d.d / 2.0 + margin
}

/// A heading from (dx, dz): atan2's first argument is dx, the second dz (the TS `Math.atan2(dx, dz)`).
fn face(dx: f32, dz: f32) -> f64 {
    (dx as f64).atan2(dz as f64)
}

/// The region of the point (x, z): the room whose cell holds it, else 0 (the board).
pub fn region_of<'a>(rooms: impl IntoIterator<Item = &'a RoomFrame>, x: f32, z: f32) -> i64 {
    for r in rooms {
        if in_cell(&r.d, x, z, 0.0) {
            return r.region;
        }
    }
    0
}

/// A room's stations in the world frame: `turn` is the room group's yaw (rotation.y), the room's origin is its
/// district's centre.
pub fn room_spots(
    region: i64,
    d: &BoardDistrict,
    turn: f32,
    spots: &[RoomSpot],
    items: &[RoomItem],
) -> Vec<SpotIn> {
    let idx: HashMap<usize, &str> = items.iter().map(|it| (it.n, it.id.as_str())).collect();
    let turn = turn as f64;
    let (c, s) = (turn.cos(), turn.sin());
    let (cx, cy) = (d.district.cx as f64, d.district.cy as f64);
    spots
        .iter()
        .enumerate()
        .map(|(i, sp)| SpotIn {
            key: format!("room{region}:{i}"),
            kind: format!("prop:{}", idx.get(&sp.item).copied().unwrap_or("item")),
            region,
            x: cx + sp.x * c + sp.z * s,
            z: cy - sp.x * s + sp.z * c,
            face: sp.face.to_radians() + turn,
            clip: sp.clip.clone(),
        })
        .collect()
}

/// One desk per agent: its console slot in its room, facing the room's centre (the desk's own face is not authored).
/// `homes` maps each agent to its district's index in `districts` and its (x, z); `region_for` gives a district's
/// region. Agents are emitted in cid order.
pub fn desk_spots(
    homes: &HashMap<u64, (usize, f32, f32)>,
    districts: &[BoardDistrict],
    region_for: impl Fn(&District) -> i64,
) -> Vec<SpotIn> {
    let mut cids: Vec<&u64> = homes.keys().collect();
    cids.sort();
    cids.into_iter()
        .map(|cid| {
            let (di, x, z) = homes[cid];
            let d = &districts[di].district;
            SpotIn {
                key: format!("desk:{cid}"),
                kind: "desk".into(),
                region: region_for(d),
                x: x as f64,
                z: z as f64,
                face: face(d.cx - x, d.cy - z),
                clip: Some("type".into()),
            }
        })
        .collect()
}

/// Whether a point is free for a robot to stand on the board: off every room's cell, on the PCB top, off the tall parts.
fn standable(layout: &BoardLayout, boxes: &[PartBox], x: f32, z: f32) -> bool {
    if layout.districts.iter().any(|d| in_cell(d, x, z, CLEAR)) {
        return false;
    }
    if (board_height_at(x, z, layout) - layout.pcb_top).abs() > 1e-6 {
        return false;
    }
    for b in boxes {
        if b.hi[1] <= layout.pcb_top + STEP_HEIGHT {
            continue; // decals and flat parts: walkable
        }
        if x > b.lo[0] - CLEAR && x < b.hi[0] + CLEAR && z > b.lo[2] - CLEAR && z < b.hi[2] + CLEAR
        {
            return false;
        }
    }
    true
}

/// The node after `i` on the walkway: the first edge that touches `i` (the TS `graph.edges[i][0]`, whose edges run
/// both ways), as its other end.
fn first_neighbour(graph: &TraceGraph, i: usize) -> Option<usize> {
    graph.edges.iter().find_map(|&(a, b)| {
        if a == i {
            Some(b)
        } else if b == i {
            Some(a)
        } else {
            None
        }
    })
}

/// A board spot on region 0 with no clip.
fn board_spot(key: String, kind: &str, x: f32, z: f32, face: f64) -> SpotIn {
    SpotIn {
        key,
        kind: kind.into(),
        region: 0,
        x: x as f64,
        z: z as f64,
        face,
        clip: None,
    }
}

/// The board's own spots: components (on the edges of each slot, facing it; a slot under a room has none, since the
/// room stands there), commons (a ring round the atom), docks (one per room, at its rim), traces (the walkway points) and
/// anywhere (a grid of free points). All on region 0.
pub fn board_spots(
    model: &Model,
    params: &Params,
    graph: &TraceGraph,
    layout: &BoardLayout,
) -> Vec<SpotIn> {
    let mut out = Vec::new();
    let boxes = part_boxes(model, params);
    // Components: four stations per slot, one off each edge, facing the slot's centre.
    for slot in &model.slots {
        let [x0, z0, w, d] = slot.rect;
        let cx = x0 + w / 2.0;
        let cz = z0 + d / 2.0;
        let edge = [
            (cx, z0 - CLEAR),
            (cx, z0 + d + CLEAR),
            (x0 - CLEAR, cz),
            (x0 + w + CLEAR, cz),
        ];
        for (k, (x, z)) in edge.into_iter().enumerate() {
            if layout.districts.iter().any(|r| in_cell(r, x, z, 0.0)) {
                continue;
            }
            out.push(board_spot(
                format!("slot:{}:{k}", slot.id),
                &format!("component:{}", slot.kind),
                x,
                z,
                face(cx - x, cz - z),
            ));
        }
    }
    // Commons: two rings round the atom (its benches and the meeting table stand on them).
    let [ax, _, az] = layout.atom;
    for i in 0..8usize {
        for r in [3.0f32, 4.5] {
            let a = i as f32 * PI / 4.0;
            let x = ax + a.cos() * r;
            let z = az + a.sin() * r;
            if !standable(layout, &boxes, x, z) {
                continue;
            }
            out.push(board_spot(
                format!("commons:{i}:{r}"),
                "commons",
                x,
                z,
                face(ax - x, az - z),
            ));
        }
    }
    // Docks: the first free point round each room's rim (the charging pad).
    for (i, d) in layout.districts.iter().enumerate() {
        for k in 0..8usize {
            let a = k as f32 * PI / 4.0;
            let x = d.district.cx + a.cos() * (d.district.r + 0.6);
            let z = d.district.cy + a.sin() * (d.district.r + 0.6);
            if !standable(layout, &boxes, x, z) {
                continue;
            }
            out.push(board_spot(
                format!("dock:{i}"),
                "dock",
                x,
                z,
                face(d.district.cx - x, d.district.cy - z),
            ));
            break;
        }
    }
    // Traces: the walkway points, facing the next point of the walkway.
    for (i, &[x, z]) in graph.nodes.iter().enumerate() {
        if layout.districts.iter().any(|r| in_cell(r, x, z, 0.3)) {
            continue;
        }
        let f = match first_neighbour(graph, i) {
            Some(next) => {
                let [nx, nz] = graph.nodes[next];
                face(nx - x, nz - z)
            }
            None => 0.0,
        };
        out.push(board_spot(format!("trace:{i}"), "trace", x, z, f));
    }
    // Anywhere: a grid of free points over the board's footprint.
    let b = &layout.bounds;
    for ix in 0usize.. {
        if ix as f32 * ANY_STEP >= b.w {
            break;
        }
        for iz in 0usize.. {
            if iz as f32 * ANY_STEP >= b.d {
                break;
            }
            let x = b.cx - b.w / 2.0 + ANY_STEP / 2.0 + ix as f32 * ANY_STEP;
            let z = b.cz - b.d / 2.0 + ANY_STEP / 2.0 + iz as f32 * ANY_STEP;
            if !standable(layout, &boxes, x, z) {
                continue;
            }
            out.push(board_spot(format!("any:{ix},{iz}"), "anywhere", x, z, 0.0));
        }
    }
    out
}
