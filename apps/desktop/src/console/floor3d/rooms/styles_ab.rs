//! `floor3d/rooms/styles-a.ts` and `styles-b.ts`: those roles' rooms.

use super::kit::RoomStyle;

/// The style of `role` if this file furnishes it.
pub fn style(role: &str) -> Option<RoomStyle> {
    let _ = role;
    None // contract: SAB
}
