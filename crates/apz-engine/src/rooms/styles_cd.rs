//! `floor3d/rooms/styles-c.ts` and `styles-d.ts`: those roles' rooms.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use glam::{EulerRot, Quat, Vec3};

use super::super::geometry::torus;
use super::super::scene::{Color, Kind, Obj};
use super::kit::{FloorKind, Kit, Room, RoomStyle, WallKind};

/// The style of `role` if this file furnishes it.
pub fn style(role: &str) -> Option<RoomStyle> {
    let style = match role {
        "researcher" => RoomStyle { name: "Observatory", size: 4.0, round: true, floor: FloorKind::Stars, wall: WallKind::Stone, build: build_researcher },
        "librarian" => RoomStyle { name: "Library", size: 4.5, round: false, floor: FloorKind::Parquet, wall: WallKind::Panel, build: build_librarian },
        "planner" => RoomStyle { name: "Planning Room", size: 3.5, round: true, floor: FloorKind::Carpet, wall: WallKind::Hedge, build: build_planner },
        "auditor" => RoomStyle { name: "Ledger Office", size: 3.0, round: false, floor: FloorKind::Tiles, wall: WallKind::Brick, build: build_auditor },
        "finance" => RoomStyle { name: "Counting House", size: 3.5, round: false, floor: FloorKind::Marble, wall: WallKind::Stone, build: build_finance },
        "incident" => RoomStyle { name: "Ops Room", size: 3.5, round: false, floor: FloorKind::Hazard, wall: WallKind::Panel, build: build_incident },
        "edge-operator" => RoomStyle { name: "Antenna Yard", size: 4.0, round: true, floor: FloorKind::Gravel, wall: WallKind::Fence, build: build_edge_operator },
        "mayor" => RoomStyle { name: "Town Hall", size: 4.5, round: false, floor: FloorKind::Marble, wall: WallKind::Stone, build: build_mayor },
        "mayor-office" => RoomStyle { name: "Mayor's Office", size: 3.0, round: false, floor: FloorKind::Carpet, wall: WallKind::Panel, build: build_mayor_office },
        _ => return None,
    };
    Some(style)
}

fn build_researcher(room: &mut Room) {
    let k = room.k.clone();
    // A big telescope on a splayed tripod: the whole scope sweeps slowly and the tube nods.
    let scope = room.item(0.0, -2.4, 0.0);
    scope.set_dynamic(true); // the whole scope sweeps; its tube nods
    room.cyl(&scope, 0.2, 0.2, 0.05, &k.dark, 0.0, 0.0, 0.0, 16);
    for i in 0..3 {
        let a = (i as f32 / 3.0) * TAU;
        let leg = Obj::group();
        // three's `rotation.order = 'YXZ'`, as the equivalent quaternion (Node is XYZ-only).
        leg.set_quaternion(Quat::from_euler(EulerRot::YXZ, -a - FRAC_PI_2, 0.32, 0.0));
        leg.set_position(0.0, 1.08, 0.0);
        scope.add(&leg);
        room.box_(&leg, 0.05, 1.12, 0.05, &k.metal, 0.0, -1.12, 0.0);
    }
    let tube = Obj::group();
    tube.set_position(0.0, 1.08, 0.0);
    tube.set_rotation(0.9, 0.0, 0.0);
    scope.add(&tube);
    room.cyl(&tube, 0.12, 0.09, 0.9, &k.white, 0.0, 0.0, 0.0, 16);
    room.cyl(&tube, 0.15, 0.15, 0.06, &k.gold, 0.0, 0.9, 0.0, 16);
    room.anim.push(Box::new(move |t: f32| {
        scope.get().rotation.y = (t * 0.25).sin() * 0.7;
        tube.get().rotation.x = 0.9 + (t * 0.4).sin() * 0.12;
    }));

    // A star-chart board on an easel.
    let chart = room.item(1.5, -2.6, 0.0);
    for x in [-0.5f32, 0.5] {
        room.box_(&chart, 0.06, 0.4, 0.06, &k.dark_wood, x, 0.0, -0.04);
    }
    room.box_(&chart, 1.2, 0.9, 0.05, &k.dark, 0.0, 0.35, 0.0);
    let stars: [(f32, f32); 6] = [(-0.4, 1.1), (-0.1, 1.0), (0.2, 1.15), (0.4, 0.95), (-0.25, 0.8), (0.1, 0.75)];
    for (x, y) in stars {
        room.ball(&chart, 0.035, &k.gold, x, y, 0.04);
    }
    room.box_(&chart, 0.36, 0.012, 0.012, &k.accent_soft, -0.15, 0.92, 0.035).set_rotation(0.0, 0.0, 0.5);
    room.box_(&chart, 0.3, 0.012, 0.012, &k.accent_soft, 0.3, 0.85, 0.035).set_rotation(0.0, 0.0, -0.4);

    // A curved run of bookshelves following the round wall on the left.
    for phi in [-0.9f32, -1.25, -1.6] {
        room.shelf(3.0 * phi.sin(), -3.0 * phi.cos(), -phi, 1.2, 1.6);
    }

    // A globe on a stand that spins on its post, ringed by a brass meridian.
    let globe_stand = room.item(2.4, 0.4, 0.0);
    room.cyl(&globe_stand, 0.3, 0.3, 0.06, &k.dark, 0.0, 0.0, 0.0, 16);
    room.cyl(&globe_stand, 0.04, 0.04, 1.2, &k.gold, 0.0, 0.06, 0.0, 8);
    let ring = Obj::mesh(&torus(0.46, 0.03, 8, 32, TAU), &k.gold);
    ring.set_position(0.0, 1.3, 0.0);
    ring.set_rotation(FRAC_PI_2, 0.0, 0.0);
    ring.set_shadows(true, false);
    globe_stand.add(&ring);
    let globe = Obj::group();
    globe.set_position(0.0, 1.3, 0.0);
    globe_stand.add(&globe);
    globe.set_dynamic(true);
    room.ball(&globe, 0.4, &k.blue, 0.0, 0.0, 0.0);
    room.ball(&globe, 0.16, &k.green, 0.2, 0.1, 0.32);
    room.ball(&globe, 0.13, &k.green, -0.25, -0.12, 0.25);
    room.ball(&globe, 0.12, &k.green, 0.05, -0.2, -0.36);
    room.anim.push(Box::new(move |t: f32| {
        globe.get().rotation.y = t * 0.6;
    }));

    room.desk(-2.0, 1.9, FRAC_PI_2);
    room.lamp(2.1, 1.9);
}

fn build_librarian(room: &mut Room) {
    let k = room.k.clone();
    let s = room.s;
    // Bookshelves along the back wall and both side walls.
    for x in [-3.0f32, -1.5, 0.0, 1.5, 3.0] {
        room.shelf(x, -s + 0.25, 0.0, 1.2, 1.6);
    }
    for z in [-2.6f32, -1.1, 0.4] {
        room.shelf(-s + 0.25, z, FRAC_PI_2, 1.2, 1.6);
        room.shelf(s - 0.25, z, -FRAC_PI_2, 1.2, 1.6);
    }

    // A rolling ladder leaning on the left-hand shelves.
    let ladder = room.item(-3.0, 0.9, 0.0);
    for z in [-0.27f32, 0.27] {
        let rail = room.box_(&ladder, 0.06, 2.2, 0.06, &k.wood, 0.0, 0.0, z);
        rail.set_rotation(0.0, 0.0, 0.2);
    }
    for y in [0.35f32, 0.7, 1.05, 1.4, 1.75] {
        room.box_(&ladder, 0.05, 0.05, 0.56, &k.wood, -(y - 1.1) * 0.2f32.tan(), y - 0.025, 0.0);
    }

    // A reading corner: a chunky armchair on a round rug, with a floor lamp beside it.
    room.rug(2.2, 2.2, &k.mat(Color::hex(0xc96a5a)), 2.5, 2.0, true);
    let chair = room.item(2.5, 2.2, (-2.5f32).atan2(-2.2));
    room.box_(&chair, 1.0, 0.36, 0.9, &k.accent, 0.0, 0.14, 0.0);
    room.box_(&chair, 0.86, 0.12, 0.8, &k.accent_soft, 0.0, 0.5, 0.02);
    room.box_(&chair, 1.0, 0.8, 0.22, &k.accent, 0.0, 0.14, -0.34);
    for x in [-0.41f32, 0.41] {
        room.box_(&chair, 0.18, 0.38, 0.9, &k.accent, x, 0.14, 0.0);
    }
    for x in [-0.4f32, 0.4] {
        for z in [-0.36f32, 0.36] {
            room.box_(&chair, 0.1, 0.14, 0.1, &k.dark, x, 0.0, z);
        }
    }
    room.lamp(3.6, 1.0);

    // A book trolley whose wheels turn as it rolls.
    let cart = room.item(2.0, 3.4, 0.0);
    room.box_(&cart, 0.7, 0.04, 0.45, &k.wood, 0.0, 0.4, 0.0);
    room.box_(&cart, 0.7, 0.04, 0.45, &k.wood, 0.0, 0.9, 0.0);
    for x in [-0.32f32, 0.32] {
        for z in [-0.2f32, 0.2] {
            room.box_(&cart, 0.04, 0.9, 0.04, &k.metal, x, 0.0, z);
        }
    }
    room.box_(&cart, 0.1, 0.22, 0.3, &k.red, -0.2, 0.44, 0.0);
    room.box_(&cart, 0.1, 0.22, 0.3, &k.blue, -0.08, 0.44, 0.0);
    room.box_(&cart, 0.1, 0.26, 0.3, &k.green, -0.2, 0.94, 0.0);
    room.box_(&cart, 0.1, 0.22, 0.3, &k.yellow, -0.08, 0.94, 0.0);
    let mut wheels: Vec<Obj> = Vec::new();
    for x in [-0.32f32, 0.32] {
        for z in [-0.2f32, 0.2] {
            let w = Obj::group();
            w.set_position(x, 0.07, z);
            cart.add(&w);
            w.set_dynamic(true);
            room.cyl(&w, 0.07, 0.07, 0.04, &k.dark, 0.0, -0.02, 0.0, 10).set_rotation(0.0, 0.0, FRAC_PI_2);
            wheels.push(w);
        }
    }
    room.anim.push(Box::new(move |t: f32| {
        for w in &wheels {
            w.get().rotation.x = t * 3.0;
        }
    }));

    // A card catalogue: a cabinet of little drawers with knobs.
    let cat = room.item(-2.8, 2.6, 0.0);
    room.box_(&cat, 1.0, 1.5, 0.5, &k.dark_wood, 0.0, 0.0, 0.0);
    room.box_(&cat, 1.04, 0.05, 0.54, &k.wood, 0.0, 1.5, 0.0);
    for r in 0..5 {
        for c in 0..3 {
            let x = -0.3 + c as f32 * 0.3;
            let y = r as f32 * 0.27;
            room.box_(&cat, 0.26, 0.2, 0.03, &k.crate_, x, 0.1 + y, 0.265);
            room.ball(&cat, 0.025, &k.gold, x, 0.2 + y, 0.29);
        }
    }

    room.desk(-1.6, 3.6, PI);
}

fn build_planner(room: &mut Room) {
    let k = room.k.clone();
    // A kanban board on two posts: three columns of sticky notes. One note wanders between columns.
    let kb = room.item(0.0, -2.5, 0.0);
    for x in [-0.9f32, 0.9] {
        room.cyl(&kb, 0.04, 0.04, 2.1, &k.dark, x, 0.0, -0.1, 8);
    }
    room.box_(&kb, 2.1, 1.2, 0.06, &k.white, 0.0, 0.9, 0.0);
    let cols: [f32; 3] = [-0.62, 0.0, 0.62];
    let heads = [k.red.clone(), k.yellow.clone(), k.green.clone()];
    for (i, x) in cols.into_iter().enumerate() {
        room.box_(&kb, 0.55, 0.08, 0.03, &heads[i], x, 1.9, 0.04);
    }
    let notes: [(f32, f32, &_); 8] = [
        (-0.62, 1.68, &k.yellow), (-0.62, 1.44, &k.purple), (-0.62, 1.2, &k.blue),
        (0.0, 1.68, &k.green), (0.0, 1.44, &k.gold),
        (0.62, 1.68, &k.red), (0.62, 1.44, &k.blue), (0.62, 1.2, &k.purple),
    ];
    for (x, y, m) in notes {
        room.box_(&kb, 0.42, 0.18, 0.02, m, x, y, 0.05);
    }
    let mover = room.box_(&kb, 0.42, 0.18, 0.02, &k.gold, -0.62, 0.96, 0.06);
    mover.set_dynamic(true);
    let path: [f32; 4] = [-0.62, 0.0, 0.62, 0.0];
    room.anim.push(Box::new(move |t: f32| {
        let p = (t * 0.2) % 4.0;
        let i = p.floor() as usize;
        let f = p - i as f32;
        let a = path[i % 4];
        let b = path[(i + 1) % 4];
        mover.get().position.x = a + (b - a) * f * f * (3.0 - 2.0 * f);
    }));

    // A wall calendar board on a foot and post, its days in a grid with a few marks.
    let cal = room.item(2.75, 0.2, -FRAC_PI_2);
    room.box_(&cal, 0.5, 0.04, 0.3, &k.dark, 0.0, 0.0, -0.05);
    room.cyl(&cal, 0.03, 0.03, 1.0, &k.dark, 0.0, 0.0, -0.05, 8);
    room.box_(&cal, 1.0, 1.2, 0.06, &k.wood, 0.0, 1.0, 0.0);
    room.box_(&cal, 0.88, 1.08, 0.03, &k.paper, 0.0, 1.06, 0.04);
    for r in 0..4 {
        for c in 0..5 {
            room.box_(&cal, 0.15, 0.2, 0.01, &k.white, -0.36 + c as f32 * 0.18, 1.12 + r as f32 * 0.25, 0.06);
        }
    }
    let marks: [(f32, f32, &_); 4] = [(0.0, 1.0, &k.red), (2.0, 3.0, &k.blue), (4.0, 0.0, &k.green), (1.0, 2.0, &k.yellow)];
    for (c, r, m) in marks {
        room.box_(&cal, 0.11, 0.06, 0.01, m, -0.36 + c * 0.18, 1.12 + r * 0.25 + 0.07, 0.08);
    }

    // A round table near the back with three chairs, each turned to face it.
    let table = room.item(-1.9, -1.25, 0.0);
    room.cyl(&table, 0.42, 0.42, 0.05, &k.wood, 0.0, 0.72, 0.0, 20);
    room.cyl(&table, 0.07, 0.2, 0.7, &k.dark_wood, 0.0, 0.0, 0.0, 12);
    room.chair(-1.9, -1.95, 0.0, None);
    room.chair(-2.55, -1.25, FRAC_PI_2, None);
    room.chair(-1.9, -0.35, PI, None);

    // A timeline pole with three milestone flags that wave in the air.
    let pole = room.item(1.9, -1.6, 0.0);
    room.cyl(&pole, 0.3, 0.3, 0.08, &k.dark, 0.0, 0.0, 0.0, 16);
    room.cyl(&pole, 0.04, 0.04, 2.4, &k.metal, 0.0, 0.0, 0.0, 8);
    room.ball(&pole, 0.07, &k.gold, 0.0, 2.45, 0.0);
    let milestones: [(f32, &_); 3] = [(0.9, &k.red), (1.4, &k.blue), (1.9, &k.green)];
    for (y, m) in milestones {
        let flag = Obj::group();
        flag.set_position(0.0, y, 0.0);
        pole.add(&flag);
        flag.set_dynamic(true);
        room.box_(&flag, 0.36, 0.2, 0.02, m, 0.2, -0.1, 0.0);
        room.anim.push(Box::new(move |t: f32| {
            flag.get().rotation.y = (t * 1.5 + y * 2.0).sin() * 0.35;
        }));
    }

    room.desk(-2.2, 1.0, FRAC_PI_2);
}

fn build_auditor(room: &mut Room) {
    let k = room.k.clone();
    // A row of filing cabinets along the back wall, four drawers each, with handles.
    let drawer = k.mat(Color::hex(0xa9b4c4));
    for x in [-2.1f32, -1.5, -0.9, -0.3] {
        let cab = room.item(x, -2.5, 0.0);
        room.box_(&cab, 0.6, 1.25, 0.5, &k.metal, 0.0, 0.0, 0.0);
        for i in 0..4 {
            let y = 0.06 + i as f32 * 0.29;
            room.box_(&cab, 0.54, 0.25, 0.03, &drawer, 0.0, y, 0.265);
            room.box_(&cab, 0.18, 0.03, 0.04, &k.dark, 0.0, y + 0.17, 0.29);
        }
    }

    // A desk on the right, its back to the wall, with stacks of paper and a ledger.
    let desk = room.desk(2.3, -0.9, -FRAC_PI_2);
    room.box_(&desk, 0.24, 0.1, 0.2, &k.paper, 0.25, 0.56, 0.1);
    room.box_(&desk, 0.2, 0.14, 0.18, &k.white, -0.3, 0.56, 0.12);
    room.box_(&desk, 0.22, 0.03, 0.16, &k.blue, 0.28, 0.66, 0.1);

    // A giant magnifying glass on a stand: a gold rim, a lens and a handle, tilting back and forth.
    let mag = room.item(-1.9, 0.6, 0.0);
    room.cyl(&mag, 0.35, 0.35, 0.08, &k.dark, 0.0, 0.0, 0.0, 16);
    room.cyl(&mag, 0.05, 0.05, 1.42, &k.metal, 0.0, 0.08, 0.0, 8);
    let glass = Obj::group();
    glass.set_position(0.0, 1.5, 0.0);
    mag.add(&glass);
    glass.set_dynamic(true);
    let rim = Obj::mesh(&torus(0.5, 0.06, 12, 32, TAU), &k.gold);
    rim.set_shadows(true, false);
    glass.add(&rim);
    room.cyl(&glass, 0.47, 0.47, 0.02, &k.mat(Color::hex(0xbfe6f2)), 0.0, -0.01, 0.0, 32).set_rotation(FRAC_PI_2, 0.0, 0.0);
    let handle = room.box_(&glass, 0.1, 0.7, 0.1, &k.dark_wood, 0.0, 0.0, 0.0);
    handle.set_position(-0.6, -0.6, 0.0);
    handle.set_rotation(0.0, 0.0, (3.0 * PI) / 4.0);
    room.anim.push(Box::new(move |t: f32| {
        glass.get().rotation.x = (t * 1.1).sin() * 0.22;
    }));

    // A table with an abacus: a frame, four rods and beads.
    let tb = room.item(1.9, 1.6, 0.0);
    room.box_(&tb, 0.8, 0.05, 0.5, &k.wood, 0.0, 0.7, 0.0);
    for x in [-0.34f32, 0.34] {
        for z in [-0.2f32, 0.2] {
            room.box_(&tb, 0.05, 0.7, 0.05, &k.dark_wood, x, 0.0, z);
        }
    }
    room.box_(&tb, 0.5, 0.04, 0.08, &k.dark_wood, 0.0, 0.75, 0.0);
    for x in [-0.25f32, 0.25] {
        room.box_(&tb, 0.04, 0.38, 0.08, &k.dark_wood, x, 0.79, 0.0);
    }
    room.box_(&tb, 0.5, 0.04, 0.08, &k.dark_wood, 0.0, 1.13, 0.0);
    for y in [0.87f32, 0.95, 1.03, 1.11] {
        let rod = room.cyl(&tb, 0.008, 0.008, 0.5, &k.metal, 0.0, 0.0, 0.0, 6);
        rod.set_position(0.0, y, 0.0);
        rod.set_rotation(0.0, 0.0, FRAC_PI_2);
        for j in 0..5 {
            let bead = if j % 2 == 1 { &k.red } else { &k.yellow };
            room.ball(&tb, 0.028, bead, -0.2 + j as f32 * 0.1, y, 0.02);
        }
    }

    room.plant(2.3, 2.3, 0.9);
}

fn build_finance(room: &mut Room) {
    let k = room.k.clone();
    // Two coin stacks; a coin flips over the taller one.
    let coins_a = room.item(-2.5, -1.8, 0.0);
    for i in 0..6 {
        room.cyl(&coins_a, 0.16, 0.16, 0.05, &k.gold, 0.0, i as f32 * 0.05, 0.0, 20);
    }
    let flip = room.cyl(&coins_a, 0.16, 0.16, 0.05, &k.gold, 0.0, 0.4, 0.0, 20);
    flip.set_dynamic(true);
    room.anim.push(Box::new(move |t: f32| {
        let p = (t * 0.7) % 1.0;
        flip.get().position.y = 0.4 + (p * PI).sin() * 0.25;
        flip.get().rotation.x = p * TAU;
    }));
    let coins_b = room.item(-2.0, -2.7, 0.0);
    for i in 0..3 {
        room.cyl(&coins_b, 0.16, 0.16, 0.05, &k.gold, 0.0, i as f32 * 0.05, 0.0, 20);
    }

    // A pyramid of gold bars: six at the base, four above, one on top.
    let bars = room.item(2.3, -1.9, 0.0);
    for z in [-0.13f32, 0.13] {
        for x in [-0.42f32, 0.0, 0.42] {
            room.box_(&bars, 0.4, 0.14, 0.22, &k.gold, x, 0.0, z);
        }
        for x in [-0.21f32, 0.21] {
            room.box_(&bars, 0.4, 0.14, 0.22, &k.gold, x, 0.14, z);
        }
    }
    room.box_(&bars, 0.4, 0.14, 0.22, &k.gold, 0.0, 0.28, 0.0);

    // A safe whose dial turns.
    let safe = room.item(2.4, -2.9, 0.0);
    room.box_(&safe, 0.9, 0.9, 0.6, &k.dark, 0.0, 0.0, 0.0);
    room.box_(&safe, 0.7, 0.7, 0.03, &k.metal, 0.0, 0.1, 0.31);
    let dial_pivot = Obj::group();
    dial_pivot.set_position(0.0, 0.45, 0.35);
    safe.add(&dial_pivot);
    dial_pivot.set_dynamic(true);
    room.cyl(&dial_pivot, 0.12, 0.12, 0.04, &k.gold, 0.0, 0.0, 0.0, 16).set_rotation(FRAC_PI_2, 0.0, 0.0);
    room.anim.push(Box::new(move |t: f32| {
        dial_pivot.get().rotation.z = t * 0.9;
    }));

    // A teller counter with a brass bar grille along its top.
    let counter = room.item(0.0, -2.85, 0.0);
    room.box_(&counter, 2.4, 0.9, 0.5, &k.dark_wood, 0.0, 0.0, 0.0);
    room.box_(&counter, 2.5, 0.08, 0.6, &k.wood, 0.0, 0.9, 0.0);
    room.box_(&counter, 2.3, 0.06, 0.05, &k.metal, 0.0, 1.5, 0.26);
    for i in 0..=11 {
        room.box_(&counter, 0.04, 0.52, 0.04, &k.metal, -1.1 + i as f32 * 0.2, 0.98, 0.26);
    }

    room.desk(-2.3, 1.2, 0.0);
    room.plant(2.5, 2.4, 0.9);
}

fn build_incident(room: &mut Room) {
    let k = room.k.clone();
    // A wall of six monitors on the back wall, held up by two posts.
    let wall = room.item(0.0, -3.3, 0.0);
    room.box_(&wall, 2.6, 1.2, 0.12, &k.dark, 0.0, 0.7, 0.0);
    for x in [-1.1f32, 1.1] {
        room.box_(&wall, 0.1, 0.7, 0.1, &k.metal, x, 0.0, 0.0);
    }
    for y in [0.9f32, 1.4] {
        for x in [-0.82f32, 0.0, 0.82] {
            room.box_(&wall, 0.7, 0.4, 0.03, &k.screen, x, y, 0.075).set_dynamic(true);
        }
    }

    // A rotating red siren with a flashing red light.
    let siren = room.item(2.4, -2.4, 0.0);
    room.box_(&siren, 0.5, 0.08, 0.5, &k.dark, 0.0, 0.0, 0.0);
    room.cyl(&siren, 0.04, 0.04, 0.9, &k.metal, 0.0, 0.08, 0.0, 8);
    let head = Obj::group();
    head.set_position(0.0, 0.95, 0.0);
    siren.add(&head);
    head.set_dynamic(true); // spins; its light rides along
    room.ball(&head, 0.22, &k.red, 0.0, 0.0, 0.0).set_scale(1.0, 0.6, 1.0);
    room.box_(&head, 0.1, 0.1, 0.1, &k.glow_red, 0.2, -0.02, 0.0);
    let light = Obj::point_light(Color::hex(0xff4a3a), 0.8, 4.0, 2.0);
    light.set_position(0.0, 0.05, 0.0);
    head.add(&light);
    room.anim.push(Box::new(move |t: f32| {
        head.get().rotation.y = t * 3.0;
        if let Kind::PointLight { intensity, .. } = &mut light.get().kind {
            *intensity = 0.2 + 0.8 * (t * 3.0).sin().max(0.0);
        }
    }));

    // Two fire extinguishers in the back corner.
    for (x, z) in [(-2.8f32, -2.7f32), (-2.3, -2.8)] {
        let ext = room.item(x, z, 0.0);
        room.cyl(&ext, 0.1, 0.1, 0.5, &k.red, 0.0, 0.0, 0.0, 14);
        room.cyl(&ext, 0.105, 0.105, 0.08, &k.white, 0.0, 0.2, 0.0, 14);
        room.cyl(&ext, 0.04, 0.05, 0.08, &k.dark, 0.0, 0.5, 0.0, 8);
        room.box_(&ext, 0.04, 0.22, 0.04, &k.dark, 0.13, 0.25, 0.0);
    }

    // A desk with a red phone on it.
    let desk = room.desk(-2.2, 1.0, 0.0);
    room.box_(&desk, 0.22, 0.06, 0.16, &k.red, 0.3, 0.56, 0.1);
    room.box_(&desk, 0.26, 0.04, 0.05, &k.red, 0.3, 0.62, 0.1);

    // A whiteboard on the left wall, with a few marker scribbles.
    let board = room.item(-3.3, 0.0, FRAC_PI_2);
    room.box_(&board, 1.5, 1.0, 0.04, &k.metal, 0.0, 0.8, -0.02);
    room.box_(&board, 1.4, 0.9, 0.05, &k.white, 0.0, 0.85, 0.0);
    room.box_(&board, 0.5, 0.04, 0.02, &k.red, -0.25, 1.3, 0.03);
    room.box_(&board, 0.8, 0.04, 0.02, &k.blue, 0.1, 1.1, 0.03);
    room.box_(&board, 0.4, 0.04, 0.02, &k.green, -0.3, 0.95, 0.03);

    room.plant(2.6, 2.6, 0.9);
}

/// `at(i, c)`: corner `c` of lattice level `i` (the mast tapers as it climbs).
fn mast_at(i: usize, c: usize) -> [f32; 3] {
    const CORNERS: [(f32, f32); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let (sx, sz) = CORNERS[c];
    let w = 0.3 - i as f32 * 0.035;
    [sx * w, i as f32 * 0.5, sz * w]
}

/// `strut(a, b)`: a thin box spanning two lattice points.
fn mast_strut(room: &mut Room, k: &Kit, mast: &Obj, a: [f32; 3], b: [f32; 3]) {
    let dir = Vec3::new(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    let len = dir.length();
    let m = room.box_(
        mast,
        0.035,
        len,
        0.035,
        &k.metal,
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0 - len / 2.0,
        (a[2] + b[2]) / 2.0,
    );
    m.set_quaternion(Quat::from_rotation_arc(Vec3::Y, dir.normalize()));
}

fn build_edge_operator(room: &mut Room) {
    let k = room.k.clone();
    // A lattice radio mast: six rings of struts, with a dish on top that turns and a blinking tip light.
    let mast = room.item(-2.0, -1.7, 0.0);
    for i in 0..6 {
        for c in 0..4 {
            mast_strut(room, &k, &mast, mast_at(i, c), mast_at(i, (c + 1) % 4));
            if i < 5 {
                mast_strut(room, &k, &mast, mast_at(i, c), mast_at(i + 1, c));
                mast_strut(room, &k, &mast, mast_at(i, c), mast_at(i + 1, (c + 1) % 4));
            }
        }
    }
    let arm = Obj::group();
    arm.set_position(0.0, 2.5, 0.0);
    mast.add(&arm);
    arm.set_dynamic(true);
    room.cyl(&arm, 0.5, 0.08, 0.16, &k.white, 0.0, -0.08, 0.15, 24).set_rotation(FRAC_PI_2, 0.0, 0.0);
    room.box_(&arm, 0.05, 0.05, 0.2, &k.metal, 0.0, -0.025, 0.07);
    let tip = room.ball(&mast, 0.07, &k.glow_red, 0.0, 3.1, 0.0);
    tip.set_dynamic(true);
    room.anim.push(Box::new(move |t: f32| {
        arm.get().rotation.y = t * 0.5;
    }));
    room.anim.push(Box::new(move |t: f32| {
        tip.get().visible = (t * 4.0).sin() > 0.0;
    }));

    // Cable spools lying on their sides.
    for (x, z) in [(2.2f32, 1.2f32), (1.5, 2.3)] {
        let spool = room.item(x, z, 0.0);
        let drum = room.cyl(&spool, 0.2, 0.2, 0.4, &k.yellow, 0.0, 0.0, 0.0, 16);
        drum.set_rotation(0.0, 0.0, FRAC_PI_2);
        drum.set_position(0.0, 0.3, 0.0);
        for fx in [-0.22f32, 0.22] {
            let flange = room.cyl(&spool, 0.3, 0.3, 0.04, &k.dark, 0.0, 0.0, 0.0, 16);
            flange.set_rotation(0.0, 0.0, FRAC_PI_2);
            flange.set_position(fx, 0.3, 0.0);
        }
    }

    // Two solar panels tilted toward the front, on legs.
    for (x, z) in [(2.3f32, -1.6f32), (2.3, -0.3)] {
        let panel = room.item(x, z, 0.0);
        room.cyl(&panel, 0.03, 0.03, 0.6, &k.metal, -0.35, 0.0, 0.0, 6);
        room.cyl(&panel, 0.03, 0.03, 0.6, &k.metal, 0.35, 0.0, 0.0, 6);
        let tilt = Obj::group();
        tilt.set_position(0.0, 0.6, 0.0);
        tilt.set_rotation(0.5, 0.0, 0.0);
        panel.add(&tilt);
        room.box_(&tilt, 0.8, 0.04, 0.5, &k.blue, 0.0, -0.02, 0.0);
        room.box_(&tilt, 0.02, 0.05, 0.5, &k.white, 0.0, -0.02, 0.0);
    }

    // A small equipment hut with a red roof and a lit window.
    let hut = room.item(-2.1, 1.3, 0.0);
    room.box_(&hut, 0.8, 0.7, 0.8, &k.white, 0.0, 0.0, 0.0);
    room.cone(&hut, 0.65, 0.5, &k.red, 0.0, 0.7, 0.0, 4).set_rotation(0.0, FRAC_PI_4, 0.0);
    room.box_(&hut, 0.25, 0.42, 0.02, &k.dark_wood, 0.0, 0.0, 0.41);
    room.box_(&hut, 0.22, 0.2, 0.02, &k.glow_warm, -0.2, 0.42, 0.41);

    // The operator's desk, facing the fence.
    room.desk(0.0, 2.5, PI);
}

fn build_mayor(room: &mut Room) {
    let k = room.k.clone();
    // A row of five columns along the back wall, each with a base and a capital.
    for x in [-3.2f32, -1.6, 0.0, 1.6, 3.2] {
        let col = room.item(x, -3.9, 0.0);
        room.box_(&col, 0.6, 0.12, 0.6, &k.white, 0.0, 0.0, 0.0);
        room.cyl(&col, 0.2, 0.22, 2.0, &k.white, 0.0, 0.12, 0.0, 16);
        room.box_(&col, 0.6, 0.14, 0.6, &k.white, 0.0, 2.12, 0.0);
    }

    // The podium with its microphone, and the town seal on its front.
    let podium = room.item(0.0, -2.7, 0.0);
    room.box_(&podium, 0.9, 1.0, 0.5, &k.dark_wood, 0.0, 0.0, 0.0);
    room.box_(&podium, 1.0, 0.08, 0.6, &k.wood, 0.0, 1.0, 0.0);
    room.box_(&podium, 0.5, 0.4, 0.02, &k.gold, 0.0, 0.3, 0.26);
    room.cyl(&podium, 0.015, 0.015, 0.4, &k.metal, 0.25, 1.08, 0.05, 6);
    room.ball(&podium, 0.06, &k.dark, 0.25, 1.52, 0.05);

    // A flag on a pole by the left wall, its cloth rippling in the wind.
    let flag = room.item(-3.6, -2.4, 0.0);
    room.cyl(&flag, 0.03, 0.03, 2.6, &k.metal, 0.0, 0.0, 0.0, 8);
    room.ball(&flag, 0.06, &k.gold, 0.0, 2.62, 0.0);
    let wave = Obj::group();
    wave.set_position(0.0, 2.22, 0.0);
    flag.add(&wave);
    wave.set_dynamic(true); // the cloth rides along
    let cloth = room.box_(&wave, 0.7, 0.45, 0.02, &k.blue, 0.35, -0.225, 0.0);
    room.anim.push(Box::new(move |t: f32| {
        wave.get().rotation.y = (t * 3.0).sin() * 0.3;
        cloth.get().rotation.z = (t * 6.0).sin() * 0.1;
        cloth.get().scale.y = 1.0 + 0.08 * (t * 6.0).sin();
    }));

    // A red carpet from the podium to the front.
    room.rug(1.2, 6.6, &k.red, 0.0, 0.9, false);

    // Two benches along the side walls, backs to the walls.
    for (x, rot) in [(-3.6f32, FRAC_PI_2), (3.6, -FRAC_PI_2)] {
        let bench = room.item(x, 0.6, rot);
        for lx in [-0.7f32, 0.7] {
            room.box_(&bench, 0.08, 0.42, 0.45, &k.dark_wood, lx, 0.0, 0.0);
        }
        room.box_(&bench, 1.6, 0.08, 0.5, &k.wood, 0.0, 0.42, 0.0);
        room.box_(&bench, 1.6, 0.42, 0.06, &k.wood, 0.0, 0.5, -0.22);
    }

    room.desk(2.4, 2.4, PI);
    room.plant(-3.3, 2.9, 1.0);
}

fn build_mayor_office(room: &mut Room) {
    let k = room.k.clone();
    // A wide executive desk against the back wall, with a monitor.
    let desk = room.item(0.0, -2.3, 0.0);
    room.box_(&desk, 2.0, 0.07, 0.8, &k.wood, 0.0, 0.72, 0.0);
    for x in [-0.65f32, 0.65] {
        room.box_(&desk, 0.5, 0.72, 0.7, &k.dark_wood, x, 0.0, 0.0);
    }
    room.box_(&desk, 0.28, 0.02, 0.18, &k.metal, 0.0, 0.79, -0.25);
    room.box_(&desk, 0.07, 0.14, 0.05, &k.metal, 0.0, 0.81, -0.25);
    room.box_(&desk, 0.6, 0.4, 0.04, &k.screen, 0.0, 0.95, -0.25).set_dynamic(true);

    // A tall armchair in the corner, angled toward the room.
    let chair = room.item(-2.1, -2.1, FRAC_PI_4);
    room.box_(&chair, 0.6, 0.05, 0.6, &k.dark_wood, 0.0, 0.0, 0.0);
    room.box_(&chair, 0.7, 0.3, 0.7, &k.accent, 0.0, 0.05, 0.0);
    room.box_(&chair, 0.12, 0.45, 0.7, &k.accent, -0.29, 0.05, 0.0);
    room.box_(&chair, 0.12, 0.45, 0.7, &k.accent, 0.29, 0.05, 0.0);
    room.box_(&chair, 0.7, 1.1, 0.14, &k.accent, 0.0, 0.35, -0.28);

    // A bookshelf on the right wall, its books facing the room.
    room.shelf(2.75, -0.9, -FRAC_PI_2, 1.2, 1.6);

    // A flag on a pole in the back-right corner.
    let flag = room.item(2.3, -2.5, 0.0);
    room.cyl(&flag, 0.12, 0.12, 0.05, &k.gold, 0.0, 0.0, 0.0, 16);
    room.cyl(&flag, 0.03, 0.03, 2.2, &k.metal, 0.0, 0.0, 0.0, 8);
    room.box_(&flag, 0.5, 0.36, 0.02, &k.blue, 0.27, 1.7, 0.0);
    room.ball(&flag, 0.05, &k.gold, 0.0, 2.23, 0.0);

    // A framed portrait on the back wall, above the desk.
    let portrait = room.item(0.0, -2.94, 0.0);
    room.box_(&portrait, 0.6, 0.7, 0.04, &k.gold, 0.0, 1.45, 0.0);
    room.box_(&portrait, 0.5, 0.6, 0.02, &k.accent_soft, 0.0, 1.5, 0.02);
    room.box_(&portrait, 0.3, 0.18, 0.03, &k.blue, 0.0, 1.45, 0.05);
    room.ball(&portrait, 0.1, &k.clay, 0.0, 1.68, 0.05);

    // A round rug at the room's centre.
    room.rug(1.8, 1.8, &k.purple, 0.0, 0.9, true);

    room.plant(-2.4, 2.2, 0.9);
}
