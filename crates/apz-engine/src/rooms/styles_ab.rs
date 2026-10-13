//! `floor3d/rooms/styles-a.ts` and `styles-b.ts`: those roles' rooms.

use std::f32::consts::{PI, TAU};

use super::super::geometry::{box_geo, sphere, torus};
use super::super::scene::{Color, Geo, Mat, Obj};
use super::kit::{FloorKind, Room, RoomStyle, WallKind};

/// The style of `role` if this file furnishes it.
pub fn style(role: &str) -> Option<RoomStyle> {
    Some(match role {
        "builder" => RoomStyle { name: "Workshop", size: 4.0, round: false, floor: FloorKind::Planks, wall: WallKind::Brick, build: build_builder },
        "qa" => RoomStyle { name: "Test Lab", size: 3.5, round: false, floor: FloorKind::Checker, wall: WallKind::Glass, build: build_qa },
        "release" => RoomStyle { name: "Launch Pad", size: 4.0, round: false, floor: FloorKind::Hazard, wall: WallKind::Fence, build: build_release },
        "security-reviewer" => RoomStyle { name: "Vault", size: 3.0, round: false, floor: FloorKind::Slate, wall: WallKind::Stone, build: build_security_reviewer },
        "reviewer" => RoomStyle { name: "Review Room", size: 3.0, round: false, floor: FloorKind::Carpet, wall: WallKind::Panel, build: build_reviewer },
        "architect" => RoomStyle { name: "Drafting Studio", size: 3.5, round: true, floor: FloorKind::Blueprint, wall: WallKind::Glass, build: build_architect },
        _ => return None,
    })
}

/// A thin mesh (a torus, a spoke, a beam) placed by its centre, casting shadows.
fn mesh(parent: &Obj, geo: &Geo, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_position(x, y, z);
    m.set_shadows(true, true);
    parent.add(&m);
    m
}

fn build_builder(room: &mut Room) {
    let k = room.k.clone();
    let s = room.s;
    // A long workbench against the back wall, a vice and tools on it, a pegboard above.
    let bench = room.item(-0.6, -s + 0.75, 0.0);
    room.box_(&bench, 2.4, 0.1, 0.7, &k.wood, 0.0, 0.75, 0.0);
    for x in [-1.1, 1.1] {
        room.box_(&bench, 0.1, 0.75, 0.6, &k.dark_wood, x, 0.0, 0.0);
    }
    room.box_(&bench, 2.2, 0.06, 0.5, &k.dark_wood, 0.0, 0.25, 0.0);
    room.box_(&bench, 0.22, 0.16, 0.2, &k.metal, -0.8, 0.85, 0.1); // the vice
    room.box_(&bench, 0.32, 0.05, 0.08, &k.red, 0.2, 0.85, 0.1); // a hammer: head and handle
    room.box_(&bench, 0.05, 0.05, 0.36, &k.wood, 0.2, 0.85, 0.28);
    room.box_(&bench, 1.6, 0.9, 0.05, &k.crate_, 0.0, 1.05, -0.3); // the pegboard
    for (x, y, m) in [(-0.5, 1.5, &k.red), (0.0, 1.3, &k.blue), (0.5, 1.55, &k.yellow)] {
        room.box_(&bench, 0.1, 0.36, 0.04, m, x, y - 0.2, -0.25);
    }

    // A red toolbox, a sawhorse with a plank, and a stack of crates.
    let toolbox = room.item(s - 0.7, -s + 0.8, -0.3);
    room.box_(&toolbox, 0.6, 0.3, 0.35, &k.red, 0.0, 0.0, 0.0);
    room.box_(&toolbox, 0.4, 0.06, 0.06, &k.dark, 0.0, 0.36, 0.0);
    let horse = room.item(-s + 0.9, 0.9, PI / 2.0);
    for x in [-0.35, 0.35] {
        room.box_(&horse, 0.06, 0.5, 0.5, &k.dark_wood, x, 0.0, 0.0).set_rotation(0.2, 0.0, 0.0);
    }
    room.box_(&horse, 1.2, 0.06, 0.3, &k.wood, 0.0, 0.5, 0.0);
    room.crate_(s - 0.6, 0.4, 0.55, 0.0);
    room.crate_(s - 0.55, 1.05, 0.5, 0.3);
    room.crate_(s - 0.6, 0.6, 0.4, 0.0).get().position.y = 0.55;

    // A big turning gear on a stand: the workshop's mascot.
    let stand = room.item(s - 0.6, -0.9, 0.0);
    room.box_(&stand, 0.3, 0.5, 0.3, &k.dark, 0.0, 0.0, 0.0);
    let gear = Obj::group();
    gear.set_position(0.0, 1.0, 0.0);
    stand.add(&gear);
    gear.set_dynamic(true);
    room.cyl(&gear, 0.38, 0.38, 0.12, &k.gold, 0.0, -0.06, 0.0, 24).set_rotation(PI / 2.0, 0.0, 0.0);
    for i in 0..10 {
        let a = (i as f32 / 10.0) * PI * 2.0;
        room.box_(&gear, 0.12, 0.12, 0.12, &k.gold, a.cos() * 0.44, a.sin() * 0.44 - 0.06, 0.0).set_rotation(0.0, 0.0, a);
    }
    let spin = gear.clone();
    room.anim.push(Box::new(move |t: f32| {
        spin.set_rotation(0.0, 0.0, t * 0.8);
    }));

    room.desk(-s + 1.0, s - 1.0, PI / 2.0);
    room.lamp(s - 0.45, s - 0.5);
}

fn build_qa(room: &mut Room) {
    let k = room.k.clone();
    let s = room.s;
    // A lab bench with test tubes in a rack and a bubbling flask.
    let bench = room.item(0.0, -s + 0.7, 0.0);
    room.box_(&bench, 2.2, 0.8, 0.6, &k.white, 0.0, 0.0, 0.0);
    room.box_(&bench, 2.3, 0.06, 0.66, &k.dark, 0.0, 0.8, 0.0);
    let tubes = [&k.green, &k.blue, &k.purple, &k.red, &k.yellow];
    for (i, m) in tubes.iter().enumerate() {
        room.cyl(&bench, 0.045, 0.045, 0.3, m, -0.8 + i as f32 * 0.13, 0.86, 0.0, 10);
    }
    room.box_(&bench, 0.75, 0.05, 0.12, &k.metal, -0.55, 1.0, 0.0);
    room.cyl(&bench, 0.06, 0.2, 0.3, &k.info, 0.6, 0.86, 0.0, 16);
    room.cyl(&bench, 0.05, 0.05, 0.14, &k.info, 0.6, 1.16, 0.0, 10);
    let mut bubbles = Vec::new();
    for _ in 0..3 {
        bubbles.push(room.ball(&bench, 0.04, &k.white, 0.6, 1.3, 0.0));
    }
    for b in &bubbles {
        b.set_dynamic(true); // they drift and shrink
    }
    let bs = bubbles.clone();
    room.anim.push(Box::new(move |t: f32| {
        for (i, b) in bs.iter().enumerate() {
            let p = (t * 0.6 + i as f32 / 3.0) % 1.0;
            b.set_position(0.6 + (p * 9.0 + i as f32).sin() * 0.04, 1.3 + p * 0.5, 0.0);
            b.set_scalar(1.0 - p * 0.7);
        }
    }));

    // A pass/fail traffic light that cycles.
    let light = room.item(s - 0.5, -0.3, 0.0);
    room.cyl(&light, 0.05, 0.05, 1.3, &k.metal, 0.0, 0.0, 0.0, 8);
    room.box_(&light, 0.3, 0.75, 0.25, &k.dark, 0.0, 1.3, 0.0);
    let mut lamps = Vec::new();
    for (i, m) in [&k.glow_red, &k.glow_warm, &k.glow_ok].iter().enumerate() {
        lamps.push(room.ball(&light, 0.08, m, 0.0, 1.88 - i as f32 * 0.22, 0.13));
    }
    for l in &lamps {
        l.set_dynamic(true); // visibility cycles
    }
    let ls = lamps.clone();
    room.anim.push(Box::new(move |t: f32| {
        for (i, l) in ls.iter().enumerate() {
            l.set_visible(((t / 1.5).floor() as i32 % 3) == i as i32);
        }
    }));

    // A test rig: a cube on a pedestal under a scanner arch.
    let rig = room.item(-s + 0.8, 0.2, 0.0);
    room.cyl(&rig, 0.35, 0.4, 0.3, &k.metal, 0.0, 0.0, 0.0, 20);
    let cube = room.box_(&rig, 0.25, 0.25, 0.25, &k.accent, 0.0, 0.42, 0.0);
    cube.set_dynamic(true);
    let spin = cube.clone();
    room.anim.push(Box::new(move |t: f32| {
        spin.set_rotation(0.0, t, 0.0);
    }));
    for x in [-0.35, 0.35] {
        room.box_(&rig, 0.06, 1.0, 0.06, &k.dark, x, 0.3, 0.0);
    }
    room.box_(&rig, 0.76, 0.06, 0.12, &k.dark, 0.0, 1.3, 0.0);
    room.box_(&rig, 0.5, 0.02, 0.06, &k.glow_ok, 0.0, 1.28, 0.0).set_dynamic(true); // its glow colour is animated

    room.desk(s - 1.0, s - 0.9, -PI / 2.0);
    room.plant(-s + 0.5, s - 0.5, 0.8);
}

fn build_release(room: &mut Room) {
    let k = room.k.clone();
    // A rocket on a round pad: white body, signal nose, three fins and a porthole. It hovers.
    let pad = room.item(-2.0, -2.2, 0.0);
    room.cyl(&pad, 0.85, 0.85, 0.12, &k.dark, 0.0, 0.0, 0.0, 32);
    room.cyl(&pad, 0.66, 0.66, 0.02, &k.signal, 0.0, 0.12, 0.0, 32);
    let rocket = Obj::group();
    rocket.set_position(0.0, 0.2, 0.0);
    pad.add(&rocket);
    rocket.set_dynamic(true);
    room.cyl(&rocket, 0.24, 0.24, 1.5, &k.white, 0.0, 0.0, 0.0, 20);
    room.cone(&rocket, 0.24, 0.55, &k.signal, 0.0, 1.5, 0.0, 20);
    for i in 0..3 {
        let a = (i as f32 / 3.0) * PI * 2.0;
        room.box_(&rocket, 0.18, 0.5, 0.04, &k.signal, a.cos() * 0.22, 0.02, a.sin() * 0.22).set_rotation(0.0, -a, 0.0);
    }
    room.ball(&rocket, 0.11, &k.gold, 0.0, 0.95, 0.22);
    room.ball(&rocket, 0.08, &k.info, 0.0, 0.95, 0.27);
    let hover = rocket.clone();
    room.anim.push(Box::new(move |t: f32| {
        hover.set_position(0.0, 0.2 + 0.04 * (t * 1.3).sin(), 0.0);
    }));

    // A lattice gantry tower beside the pad, with an access arm to the rocket and a blinking beacon.
    let gantry = room.item(-3.3, -2.2, 0.0);
    for (x, z) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] {
        room.box_(&gantry, 0.06, 3.2, 0.06, &k.dark, x, 0.0, z);
    }
    for y in [0.4, 1.2, 2.0, 2.8] {
        for z in [-0.25, 0.25] {
            room.box_(&gantry, 0.5, 0.05, 0.05, &k.metal, 0.0, y, z);
        }
        for x in [-0.25, 0.25] {
            room.box_(&gantry, 0.05, 0.05, 0.5, &k.metal, x, y, 0.0);
        }
    }
    let diag = 0.8_f32.atan2(0.5);
    for i in 0..3 {
        let tilt = if i % 2 == 1 { -diag } else { diag };
        room.box_(&gantry, 0.94, 0.04, 0.04, &k.metal, 0.0, 0.8 + i as f32 * 0.8, 0.25).set_rotation(0.0, 0.0, tilt);
    }
    room.box_(&gantry, 0.8, 0.06, 0.12, &k.signal, 0.65, 2.2, 0.0);
    let beacon = room.ball(&gantry, 0.09, &k.glow_red, 0.0, 3.3, 0.0);
    beacon.set_dynamic(true);
    room.anim.push(Box::new(move |t: f32| {
        beacon.set_visible((t * 5.0).sin() > 0.0);
    }));

    // The control desk, traffic cones at the front-left, crates along both sides.
    room.desk(2.6, 1.6, -PI / 2.0);
    for (x, z) in [(-2.5, 2.2), (-1.6, 2.7), (-3.0, 3.1)] {
        let c = room.item(x, z, 0.0);
        room.cyl(&c, 0.17, 0.17, 0.05, &k.dark, 0.0, 0.0, 0.0, 12);
        room.cone(&c, 0.13, 0.42, &k.signal, 0.0, 0.05, 0.0, 12);
        room.cyl(&c, 0.075, 0.09, 0.07, &k.white, 0.0, 0.2, 0.0, 12);
    }
    room.crate_(-3.1, 0.6, 0.5, 0.0);
    room.crate_(-3.1, 0.6, 0.4, 0.0).get().position.y = 0.5;
    room.crate_(-3.1, 1.25, 0.45, 0.2);
    room.crate_(3.1, -1.2, 0.55, 0.0);
    room.crate_(3.1, -1.2, 0.42, 0.0).get().position.y = 0.55;
    room.crate_(3.1, -0.3, 0.5, 0.3);
}

fn build_security_reviewer(room: &mut Room) {
    let k = room.k.clone();
    let s = room.s;
    // A round vault door set into the back wall; its spoked wheel slowly turns.
    let vault = room.item(0.0, -s + 0.25, 0.0);
    room.cyl(&vault, 0.95, 0.95, 0.24, &k.mat(Color::hex(0x7a8696)), 0.0, 1.0, 0.0, 40).set_rotation(PI / 2.0, 0.0, 0.0);
    let rim = torus(0.95, 0.07, 10, 40, TAU);
    mesh(&vault, &rim, &k.gold, 0.0, 1.12, 0.13);
    for i in 0..8 {
        let a = (i as f32 / 8.0) * PI * 2.0;
        room.ball(&vault, 0.045, &k.dark, a.cos() * 0.78, 1.12 + a.sin() * 0.78, 0.14);
    }
    let wheel = Obj::group();
    wheel.set_position(0.0, 1.12, 0.17);
    vault.add(&wheel);
    wheel.set_dynamic(true);
    let hub = torus(0.42, 0.045, 8, 28, TAU);
    mesh(&wheel, &hub, &k.gold, 0.0, 0.0, 0.0);
    for i in 0..3 {
        let spoke = box_geo(0.06, 0.84, 0.05);
        mesh(&wheel, &spoke, &k.gold, 0.0, 0.0, 0.0).set_rotation(0.0, 0.0, i as f32 * PI / 3.0);
    }
    let knob = sphere(0.09, 12, 10);
    mesh(&wheel, &knob, &k.dark, 0.0, 0.0, 0.03);
    let turn = wheel.clone();
    room.anim.push(Box::new(move |t: f32| {
        turn.set_rotation(0.0, 0.0, t * 0.5);
    }));

    // A CCTV camera on a pole in the back-right corner; its head pans slowly.
    let pole = room.item(2.4, -2.4, 0.0);
    room.cyl(&pole, 0.05, 0.05, 2.0, &k.metal, 0.0, 0.0, 0.0, 8);
    let head = Obj::group();
    head.set_position(0.0, 2.0, 0.0);
    pole.add(&head);
    head.set_dynamic(true);
    room.box_(&head, 0.26, 0.2, 0.42, &k.dark, 0.0, -0.1, 0.1);
    room.cyl(&head, 0.07, 0.07, 0.06, &k.info, 0.0, -0.03, 0.32, 12).set_rotation(PI / 2.0, 0.0, 0.0);
    room.ball(&head, 0.035, &k.glow_red, 0.09, 0.12, 0.2);
    let pan = head.clone();
    room.anim.push(Box::new(move |t: f32| {
        pan.set_rotation(0.0, 0.9 * (t * 0.5).sin(), 0.0);
    }));

    // A row of lockboxes along the left wall.
    for (z, m) in [(-2.3, &k.red), (-1.75, &k.blue), (-1.2, &k.gold)] {
        let lb = room.item(-2.3, z, 0.0);
        room.box_(&lb, 0.5, 0.36, 0.42, m, 0.0, 0.0, 0.0);
        room.box_(&lb, 0.05, 0.04, 0.2, &k.dark, 0.27, 0.24, 0.0);
        room.ball(&lb, 0.05, &k.white, 0.27, 0.26, 0.0);
    }

    // A laser fence across the front: two posts and two red beams. The beams are light, not walls.
    for x in [-2.6, 2.6] {
        let post = room.item(x, 1.8, 0.0);
        room.cyl(&post, 0.05, 0.06, 1.0, &k.metal, 0.0, 0.0, 0.0, 8);
        room.ball(&post, 0.07, &k.glow_red, 0.0, 1.02, 0.0);
    }
    for y in [0.35, 0.8] {
        let beam = box_geo(5.2, 0.03, 0.03);
        mesh(&room.g, &beam, &k.glow_red, 0.0, y, 1.8).set_walk();
    }

    room.desk(2.2, 0.4, -PI / 2.0);
}

fn build_reviewer(room: &mut Room) {
    let k = room.k.clone();
    let s = room.s;
    // A whiteboard on the back wall with a few marker strokes.
    let board = room.item(0.0, -s + 0.12, 0.0);
    room.box_(&board, 2.0, 1.2, 0.06, &k.white, 0.0, 0.9, 0.0);
    room.box_(&board, 2.08, 0.06, 0.08, &k.metal, 0.0, 2.1, 0.0);
    room.box_(&board, 2.1, 0.05, 0.1, &k.metal, 0.0, 0.85, 0.02);
    for x in [-1.02, 1.02] {
        room.box_(&board, 0.06, 1.25, 0.08, &k.metal, x, 0.9, 0.0);
    }
    for (x, y, w, a, m) in [
        (-0.6, 1.6, 0.5, 0.15, &k.blue), (-0.5, 1.3, 0.7, -0.05, &k.red), (0.35, 1.7, 0.4, 0.4, &k.green),
        (0.1, 1.1, 0.9, 0.0, &k.blue), (-0.25, 0.95, 0.4, 0.2, &k.red),
    ] {
        room.box_(&board, w, 0.035, 0.02, m, x, y, 0.04).set_rotation(0.0, 0.0, a);
    }

    // A giant rubber stamp: a wooden handle and a red base. It stamps up and down on a paper pad.
    let stand = room.item(-1.9, -1.9, 0.0);
    room.box_(&stand, 0.9, 0.8, 0.7, &k.dark_wood, 0.0, 0.0, 0.0);
    room.box_(&stand, 0.7, 0.06, 0.5, &k.paper, 0.0, 0.8, 0.0);
    room.box_(&stand, 0.68, 0.06, 0.48, &k.white, 0.0, 0.86, 0.0);
    let stamp = Obj::group();
    stamp.set_position(0.0, 0.92, 0.0);
    stand.add(&stamp);
    stamp.set_dynamic(true);
    room.box_(&stamp, 0.6, 0.12, 0.42, &k.red, 0.0, 0.0, 0.0);
    room.cyl(&stamp, 0.07, 0.09, 0.5, &k.wood, 0.0, 0.12, 0.0, 12);
    room.ball(&stamp, 0.11, &k.gold, 0.0, 0.66, 0.0);
    let press = stamp.clone();
    room.anim.push(Box::new(move |t: f32| {
        press.set_position(0.0, 0.92 + 0.5 * (t * 1.8).sin().abs(), 0.0);
    }));

    // A lectern on the right, with a book on its slope.
    let lectern = room.item(1.9, -1.0, 0.0);
    room.box_(&lectern, 0.5, 1.0, 0.4, &k.dark_wood, 0.0, 0.0, 0.0);
    room.box_(&lectern, 0.6, 0.06, 0.5, &k.wood, 0.0, 1.0, 0.02).set_rotation(-0.35, 0.0, 0.0);
    room.box_(&lectern, 0.4, 0.03, 0.3, &k.paper, 0.0, 1.1, 0.02);

    // Two chairs for the audience, a desk, and a plant.
    room.chair(1.7, 1.9, PI, None);
    room.chair(2.3, 1.9, PI, None);
    room.desk(-2.2, 1.0, PI / 2.0);
    room.plant(-2.4, 2.3, 0.9);
}

fn build_architect(room: &mut Room) {
    let k = room.k.clone();
    // A tilted drafting table, with a lamp arm rising from its left edge.
    let table = room.item(-1.9, -1.5, 0.0);
    let top = Obj::group();
    top.set_position(0.0, 0.85, 0.0);
    top.set_rotation(0.35, 0.0, 0.0);
    table.add(&top);
    room.box_(&top, 1.0, 0.05, 0.7, &k.wood, 0.0, -0.05, 0.0);
    for x in [-0.42, 0.42] {
        for z in [-0.25, 0.25] {
            room.box_(&table, 0.06, 0.82, 0.06, &k.dark_wood, x, 0.0, z);
        }
    }
    room.cyl(&table, 0.03, 0.03, 1.4, &k.metal, -0.52, 0.0, -0.3, 8);
    let arm = Obj::group();
    arm.set_position(-0.52, 1.4, -0.3);
    table.add(&arm);
    arm.set_dynamic(true);
    room.box_(&arm, 0.04, 0.04, 0.45, &k.metal, 0.0, -0.02, 0.22);
    room.cone(&arm, 0.14, 0.16, &k.signal, 0.0, -0.16, 0.45, 16).set_rotation(PI, 0.0, 0.0);
    room.ball(&arm, 0.05, &k.glow_warm, 0.0, -0.1, 0.45);
    let swing = arm.clone();
    room.anim.push(Box::new(move |t: f32| {
        swing.set_rotation(0.0, 0.15 * (t * 0.7).sin(), 0.0);
    }));
    room.chair(-1.9, -0.5, PI, None);

    // An easel with a painted canvas, at the front-left.
    let easel = room.item(-2.0, 1.5, 0.0);
    for x in [-0.3, 0.3] {
        room.box_(&easel, 0.05, 1.6, 0.05, &k.dark_wood, x, 0.0, 0.2).set_rotation(-0.2, 0.0, 0.0);
    }
    room.box_(&easel, 0.7, 0.9, 0.04, &k.paper, 0.0, 0.7, -0.05);
    room.box_(&easel, 0.3, 0.22, 0.02, &k.purple, -0.1, 0.95, -0.03);
    room.ball(&easel, 0.07, &k.yellow, 0.15, 1.2, -0.03);

    // A scale model city on a green board: stacked boxes, a tower with a cone roof, and a tiny crane that turns.
    let city = room.item(1.8, -1.7, 0.0);
    room.box_(&city, 0.9, 0.08, 0.7, &k.leaf, 0.0, 0.0, 0.0);
    room.box_(&city, 0.2, 0.16, 0.18, &k.blue, -0.25, 0.08, -0.15);
    room.box_(&city, 0.16, 0.14, 0.16, &k.white, -0.25, 0.24, -0.15);
    room.box_(&city, 0.18, 0.12, 0.2, &k.yellow, -0.2, 0.08, 0.15);
    room.cone(&city, 0.13, 0.12, &k.red, -0.2, 0.2, 0.15, 4);
    room.cyl(&city, 0.07, 0.07, 0.45, &k.white, 0.1, 0.08, -0.1, 12);
    room.cone(&city, 0.1, 0.2, &k.red, 0.1, 0.53, -0.1, 12);
    let crane = Obj::group();
    crane.set_position(0.22, 0.08, 0.14);
    city.add(&crane);
    crane.set_dynamic(true);
    room.cyl(&crane, 0.04, 0.04, 0.3, &k.gold, 0.0, 0.0, 0.0, 8);
    room.box_(&crane, 0.36, 0.03, 0.03, &k.gold, 0.0, 0.3, 0.0);
    room.box_(&crane, 0.02, 0.1, 0.02, &k.dark, 0.15, 0.2, 0.0);
    let turn = crane.clone();
    room.anim.push(Box::new(move |t: f32| {
        turn.set_rotation(0.0, t * 0.7, 0.0);
    }));

    // A bin of rolled plans, beside the desk.
    let bin = room.item(2.5, -0.7, 0.0);
    room.cyl(&bin, 0.22, 0.18, 0.5, &k.mat(Color::hex(0x6a7f96)), 0.0, 0.0, 0.0, 16);
    room.cyl(&bin, 0.04, 0.04, 0.42, &k.paper, -0.08, 0.3, 0.02, 8).set_rotation(0.0, 0.0, 0.1);
    room.cyl(&bin, 0.04, 0.04, 0.42, &k.yellow, 0.05, 0.3, -0.05, 8).set_rotation(0.0, 0.0, -0.12);
    room.cyl(&bin, 0.04, 0.04, 0.38, &k.paper, 0.02, 0.3, 0.08, 8).set_rotation(0.0, 0.0, 0.05);

    room.desk(2.2, 0.6, -PI / 2.0);
}
