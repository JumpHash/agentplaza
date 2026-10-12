//! `floor3d/rooms/index.ts`: each role's room — its style, walls, sign and furniture.

pub mod kit;
mod styles_ab;
mod styles_cd;

use std::f32::consts::PI;

use super::geometry;
use super::props::Anim;
use super::scene::{Color, Mat, Obj};
use super::textures::{canvas_texture, Rng};
use super::toon;
use kit::{floor_texture, make_kit, wall_look, FloorKind, Room, WallKind};
pub use kit::{KitPalette, RoomStyle};

/// The style of `role` (the studio for unknown roles).
pub fn room_style(role: &str) -> RoomStyle {
    styles_ab::style(role).or_else(|| styles_cd::style(role)).unwrap_or_else(studio)
}

/// A plain studio: desks and plants, for a role without a room of its own (`STUDIO`).
fn studio() -> RoomStyle {
    RoomStyle {
        name: "Studio",
        size: 3.0,
        round: false,
        floor: FloorKind::Tiles,
        wall: WallKind::Panel,
        build: |room: &mut Room| {
            let s = room.s;
            room.desk(-s + 1.2, -s + 1.0, 0.0);
            room.desk(s - 1.2, -s + 1.0, 0.0);
            room.plant(-s + 0.5, s - 0.6, 0.9);
            room.plant(s - 0.5, s - 0.6, 0.9);
            room.lamp(s - 0.45, 0.0);
        },
    }
}

/// A built room (`BuiltRoom`).
pub struct BuiltRoom {
    /// Furniture, walls and sign in the room's local frame (turn it to face the plaza).
    pub group: Obj,
    pub floor: Mat,
    /// Monitor materials that light up while the room's agents work.
    pub screens: Vec<Mat>,
    pub animate: Anim,
}

/// `buildRoom(role, s, hue, palette)`: half-size `s`, hue in degrees.
pub fn build_room(role: &str, s: f32, hue: f32, palette: &KitPalette) -> BuiltRoom {
    let style = room_style(role);
    let g = Obj::group();
    let kit = make_kit(hue, palette);
    let seed = (hue * 131.0 + role.len() as f32) as u32;
    let mut room = Room::new(g.clone(), s, hue, kit, Rng::new(seed));
    walls(&mut room, &style);
    sign(&mut room, style.name, style.round);
    (style.build)(&mut room);
    toon::outline(&g, 0.12);

    // Tinted toward the night's blue, so a room's floor reads as lit by the plaza's moonlight.
    let floor = Mat::toon(Color::hex(0xb4bde0));
    floor.get().map = Some(floor_texture(style.floor, hue, s));
    let screen = room.k.screen.clone();
    let glow_ok = room.k.glow_ok.clone();
    let glow_info = room.k.glow_info.clone();
    let (ok, info) = (palette.ok, palette.info);
    let mut anims = std::mem::take(&mut room.anim);
    let animate: Anim = Box::new(move |t: f32| {
        glow_ok.get().color = ok.scale(0.4 + 0.6 * (t * 2.7).sin().max(0.0));
        glow_info.get().color = info.scale(0.4 + 0.6 * (t * 3.9 + 1.0).sin().max(0.0));
        for a in anims.iter_mut() {
            a(t);
        }
    });
    BuiltRoom { group: g, floor, screens: vec![screen], animate }
}

/// Walls on the back half: square rooms get a back wall and two half side walls; round ones an arc.
fn walls(room: &mut Room, style: &RoomStyle) {
    let Some((mat, h, thick)) = wall_look(style.wall, room.hue) else { return };
    let s = room.s;
    let cap = style.wall != WallKind::Hedge && style.wall != WallKind::Glass;
    let seg = |room: &mut Room, x: f32, z: f32, len: f32, rot: f32| {
        let it = room.item(x, z, rot);
        room.box_(&it, len, h, thick, &mat, 0.0, 0.0, 0.0);
        // A cap rail in the darker wood, the cartoon trim.
        if cap {
            let dark = room.k.dark_wood.clone();
            room.box_(&it, len + 0.04, 0.06, thick + 0.06, &dark, 0.0, h, 0.0);
        }
    };
    if style.round {
        let n: u32 = 9;
        let r = s - 0.15;
        for i in 0..n {
            let a = PI + (i as f32 + 0.5) * (PI / n as f32); // the back half, z < 0
            let len = 2.0 * r * (PI / (2.0 * n as f32)).sin() + 0.05;
            seg(room, a.cos() * r, a.sin() * r, len, -a - PI / 2.0);
        }
    } else {
        seg(room, 0.0, -s + thick / 2.0 + 0.05, 2.0 * s - 0.1, 0.0);
        for side in [-1.0_f32, 1.0] {
            let len = s * 0.9;
            seg(room, side * (s - thick / 2.0 - 0.05), -s + len / 2.0 + 0.05, len, PI / 2.0);
        }
    }
}

/// The room's name on a painted board at the middle of the back wall.
fn sign(room: &mut Room, name: &str, round: bool) {
    let tex = canvas_texture(&format!("roomsign:{name}"), 512, 128, 1.0, |g, w, h, _| {
        g.set_fill("#fff6e0");
        g.fill_rect(0.0, 0.0, w, h);
        g.set_stroke("#2b2420");
        g.set_line_width(10.0);
        g.stroke_rect(5.0, 5.0, w - 10.0, h - 10.0);
        g.set_fill("#2b2420");
        g.set_font("700 64px ui-rounded, \"Comic Sans MS\", system-ui, sans-serif");
        g.set_text_align("center");
        g.set_text_baseline("middle");
        g.fill_text(name, w / 2.0, h / 2.0 + 4.0);
    });
    let face = Mat::toon(Color::WHITE);
    face.get().map = Some(tex);
    let z = -room.s + if round { 0.45 } else { 0.3 };
    let it = room.item(0.0, z, 0.0);
    let dark = room.k.dark_wood.clone();
    room.cyl(&it, 0.04, 0.04, 1.5, &dark, -0.8, 0.0, 0.0, 8);
    room.cyl(&it, 0.04, 0.04, 1.5, &dark, 0.8, 0.0, 0.0, 8);
    // The board's wood body, then the painted face (the TS box's +z material) a plane proud of it.
    let wood = room.k.wood.clone();
    let body = Obj::mesh(&geometry::box_geo(1.9, 0.48, 0.06), &wood);
    body.set_position(0.0, 1.42, 0.02).set_shadows(true, false);
    it.add(&body);
    let board = Obj::mesh(&geometry::plane(1.9, 0.48, 1, 1), &face);
    board.set_position(0.0, 1.42, 0.052).set_shadows(true, false);
    it.add(&board);
}
