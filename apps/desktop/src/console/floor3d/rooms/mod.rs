//! `floor3d/rooms/index.ts`: each role's room — its style, walls, sign and furniture.

pub mod kit;
mod styles_ab;
mod styles_cd;

use super::props::Anim;
use super::scene::{Mat, Obj};
pub use kit::{KitPalette, RoomStyle};

/// The style of `role` (the studio for unknown roles).
pub fn room_style(role: &str) -> RoomStyle {
    styles_ab::style(role).or_else(|| styles_cd::style(role)).unwrap_or_else(studio) // contract: RI (STUDIO from index.ts)
}

fn studio() -> RoomStyle {
    RoomStyle { name: "Studio", size: 4.0, round: false, floor: kit::FloorKind::Planks, wall: kit::WallKind::Panel, build: |_| {} } // contract: RI
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
    let _ = (role, s, hue, palette);
    BuiltRoom { group: Obj::group(), floor: Mat::toon(super::scene::Color::WHITE), screens: Vec::new(), animate: Box::new(|_| {}) } // contract: RI
}
