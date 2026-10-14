//! The navigation scene of a layout (nav-scene.ts): each room's plan (the values its room is built with), the terraces,
//! and the nav grid over the board, its plates with their landings, and the rooms with their furniture. The layout's
//! acceptance builds it (with no machine), and so does the engine for a finished layout, so the grid the robots walk is
//! the grid the layout was accepted on.

use std::collections::BTreeMap;

use super::aisle::{Exit, ExitKind};
use super::{Door, Layout, Room};
use crate::Board;
use crate::hardware::{Library, Model};
use crate::navgrid::{
    self, LandingKind, NavBoard, NavBox, NavGrid, NavPlate, NavTerrace, NavTrace, PlateLanding,
};
use crate::rooms::generate::{self, room_seed};

/// The room plan of one room (nav-scene.ts RoomPlan): its turn and door (the layout's), its layout seed, its desk count
/// and its inner size.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomPlan {
    pub turn: f64,
    pub door: generate::Door,
    pub seed: u32,
    pub agents: usize,
    pub w: f64,
    pub d: f64,
}

/// A layout's navigation scene: the grid, the terraces (room i is terrace i + 1) and the room plans.
#[derive(Debug, Clone, PartialEq)]
pub struct NavScene {
    pub grid: NavGrid,
    pub terraces: Vec<NavTerrace>,
    pub plans: Vec<RoomPlan>,
}

/// The room plan of room `index` (nav-scene.ts roomPlan): the seed of `<machine>/<role>/<index>` (no machine: ""), at
/// least one desk, and the inner side 2r - 0.15.
pub fn room_plan(room: &Room, index: usize, machine: Option<&str>) -> RoomPlan {
    let inner = 2.0 * room.r - 0.15;
    let door = match room.door {
        Door::N => generate::Door::N,
        Door::S => generate::Door::S,
        Door::E => generate::Door::E,
        Door::W => generate::Door::W,
    };
    RoomPlan {
        turn: room.turn,
        door,
        seed: room_seed(machine.unwrap_or(""), &room.role, index),
        agents: room.members.len().max(1),
        w: inner,
        d: inner,
    }
}

/// The navigation scene of a finished layout for the board's `gpus` (nav-scene.ts navScene): `machine` seeds the
/// rooms as the floor builds them.
pub fn nav_scene(layout: &Layout, gpus: u32, machine: Option<&str>) -> NavScene {
    let kind = layout.board_kind.as_str();
    let params = BTreeMap::from([("gpus".to_string(), f64::from(gpus))]);
    let board = Board::from_kind(kind, &params);
    let model = Library::get()
        .model(&format!("board/{kind}"))
        .unwrap_or_else(|| panic!("apz-board: no board kind \"{kind}\""));
    let s = layout.board_scale;
    let scale3 = |v: [f64; 3]| [v[0] * s, v[1] * s, v[2] * s];
    let parts: Vec<([f64; 3], [f64; 3])> = board
        .part_boxes()
        .iter()
        .map(|it| (scale3(it.bounds.min), scale3(it.bounds.max)))
        .collect();
    let size = [model.size[0] * s, model.size[2] * s];
    let plates: Vec<([f64; 4], f64, &Exit)> = layout
        .plates
        .iter()
        .filter_map(|p| Some((p.rect, layout.slots.get(p.id)?.top, p.exit.as_ref()?)))
        .collect();
    scene(
        size,
        layout.pcb_top,
        &parts,
        traces_of(model, s),
        &plates,
        &layout.rooms,
        machine,
    )
}

/// The board's copper traces at the world scale `s`.
pub(super) fn traces_of(model: &Model, s: f64) -> Vec<NavTrace> {
    model
        .traces
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|t| NavTrace {
            path: t.path.iter().map(|&[x, z]| [x * s, z * s]).collect(),
            width: t.width * s,
        })
        .collect()
}

/// The scene of a board of `size` (x, z) at `pcb_top` with its part boxes and traces, the plates `(rect, top, exit)`
/// (region ids after the rooms') and the rooms, each a terrace with the furniture its plan lays out.
pub(super) fn scene(
    size: [f64; 2],
    pcb_top: f64,
    parts: &[([f64; 3], [f64; 3])],
    traces: Vec<NavTrace>,
    plates: &[([f64; 4], f64, &Exit)],
    rooms: &[Room],
    machine: Option<&str>,
) -> NavScene {
    let first = rooms.len() as i32 + 1;
    let plates = plates
        .iter()
        .enumerate()
        .map(|(k, &([x0, z0, w, d], top, e))| {
            let kind = match e.kind {
                ExitKind::Ramp => LandingKind::Ramp,
                ExitKind::Lift => LandingKind::Lift,
                ExitKind::Gangway => LandingKind::Gangway,
            };
            let landing = PlateLanding {
                ax: e.plate_at[0],
                az: e.plate_at[1],
                bx: e.board_at[0],
                bz: e.board_at[1],
                kind,
                nx: e.dir[0],
                nz: e.dir[1],
                bridge: e.bridge.clone(),
            };
            NavPlate {
                id: first + k as i32,
                x0,
                z0,
                w,
                d,
                y: top,
                landing: Some(landing),
            }
        })
        .collect();
    let board = NavBoard {
        size: [size[0], 0.0, size[1]],
        pcb_top,
        boxes: parts.iter().map(|&(lo, hi)| NavBox { lo, hi }).collect(),
        traces,
        plates,
    };
    let data = generate::data();
    let mut plans = Vec::with_capacity(rooms.len());
    let terraces: Vec<NavTerrace> = rooms
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let plan = room_plan(r, i, machine);
            let input = generate::Input {
                role: r.role.clone(),
                w: plan.w,
                d: plan.d,
                door: plan.door,
                seed: plan.seed,
                agents: Some(plan.agents),
            };
            let (door, solids, spots) =
                navgrid::furniture(&generate::generate(&input, data), &data.catalog);
            let t = NavTerrace {
                id: i as i32 + 1,
                cx: r.centre[0],
                cz: r.centre[1],
                half: r.half[0].min(r.half[1]),
                hw: r.half[0],
                hd: r.half[1],
                round: r.round,
                y: r.floor_y,
                turn: r.turn,
                w: plan.w,
                d: plan.d,
                door,
                solids,
                spots,
            };
            plans.push(plan);
            t
        })
        .collect();
    NavScene {
        grid: navgrid::build(&board, &terraces),
        terraces,
        plans,
    }
}
