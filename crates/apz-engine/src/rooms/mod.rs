//! `floor3d/rooms/index.ts`: a district's room. The generator (`generate`, gen.ts) lays out a role's furniture,
//! the builder (`build`, build.ts) builds it from the catalogue, and `build_room` sizes it as the web's `buildRoom`
//! does. The room lab (`lab`, `APZ_ROOM_LAB`) shows rooms on the machines' ground.

/// The generator and the verifier live in `apz-board` (the board layout checks its rooms with their furniture).
pub use apz_board::rooms::{generate, verify};
pub mod build;
pub mod kit;
pub mod lab;
pub mod overlay;

use super::props::Anim;
use super::scene::{Color, Mat, Obj};
use super::world::Colors;
use generate::{Door, Input, Layout, LayoutSpot};
use kit::KitPalette;

/// Walls take this much off a slot's side: the inner size is 2s - WALL_MARGIN (index.ts).
pub const WALL_MARGIN: f64 = 0.15;
/// The largest side a room is built at when its role needs more space (index.ts `MAX_SIDE`).
const MAX_SIDE: f64 = 8.0;

/// The palette the rooms are built with: the sheet and rule are fixed, the rest are the floor's colours.
pub fn kit_palette(c: &Colors) -> KitPalette {
    KitPalette { sheet: Color::hex(0xe9edf7), rule: Color::hex(0xa9b4cc), ink3: c.ink3, signal: c.signal, ok: c.ok, info: c.info }
}

/// The options of a room (`RoomInput` in index.ts).
#[derive(Debug, Clone, Copy)]
pub struct RoomOptions {
    /// The layout seed: a district's `generate::room_seed`, or any u32 in the lab.
    pub seed: u32,
    /// The walkway side.
    pub door: Door,
    /// The inner size in units (x by z).
    pub w: f64,
    pub d: f64,
    /// The desks the room houses; default the generator's cap for the size.
    pub agents: Option<usize>,
}

/// A built room (`BuiltRoomLayout` in index.ts): its meshes, and the layout they were built from.
pub struct BuiltRoom {
    /// Furniture, walls and the name board, in the room's local frame (turn it to face the plaza).
    pub group: Obj,
    /// The floor's top surface material.
    pub floor: Mat,
    /// Monitor materials that light up while the room's agents work.
    pub screens: Vec<Mat>,
    /// The room's animation (the screens' glow), called each frame with the time.
    pub animate: Anim,
    /// Where a robot stands to use each item, in the room's frame (the behaviour engine's stations).
    pub spots: Vec<LayoutSpot>,
    pub layout: Layout,
    /// True when the slot was below the role's minimum: the room is built at the slot size, and a warning names it.
    pub warned: bool,
    /// The size the room was built at (the inner size).
    pub w: f64,
    pub d: f64,
}

/// `buildRoom(role, s, hue, palette, opts)`: a role's room of half-size `s` (the slot's), at the inner size
/// `opts.w` by `opts.d`. The board layout grows a slot's cells to its rooms' minimums (world/board_layout.rs), so a
/// size below the minimum here only means no slot holds the room: it is NOT enlarged (that would overlap the
/// neighbouring slots), its required items may not place, and a warning names the role and the minimum.
pub fn build_room(role: &str, s: f32, hue: f32, palette: &KitPalette, opts: &RoomOptions) -> BuiltRoom {
    let data = generate::data();
    let agents = opts.agents.unwrap_or_else(|| default_agents(opts.w, opts.d)).max(1);
    let need = min_side(role, agents).min(MAX_SIDE);
    // The minimum is a cell size: compare the cell (2 * s), not the room inside its wall margin (index.ts buildRoom).
    let warned = 2.0 * f64::from(s) < need - 1e-9;
    if warned {
        eprintln!(
            "agentplaza-desktop: rooms: {role} slot {s:.2} ({:.2} x {:.2}) is below its minimum {need} ({agents} desks); built at the slot size, not enlarged",
            opts.w, opts.d
        );
    }
    let input = Input { role: role.to_string(), w: opts.w, d: opts.d, door: opts.door, seed: opts.seed, agents: Some(agents) };
    let layout = generate::generate(&input, data);
    let built = build::build_layout(&layout, &data.catalog, hue, palette, opts.w, opts.d, s, &role_name(role));
    BuiltRoom {
        group: built.group,
        floor: built.floor,
        screens: built.screens,
        animate: built.animate,
        spots: built.spots,
        layout,
        warned,
        w: opts.w,
        d: opts.d,
    }
}

/// The generator's default desk count for a size: `clamp(floor(w * d / 6), 1, 4)`.
pub fn default_agents(w: f64, d: f64) -> usize {
    ((w * d / 6.0).floor() as i64).clamp(1, 4) as usize
}

/// The minimum side of a role and desk count, from the table only (`minSideFor` in index.ts, reading
/// assets/rooms/minsize.json). A role the table does not list is 0; a desk count above the table's last entry uses
/// that entry (the 6-agent row); a `null` entry (no side up to 8 qualifies) is infinite. Never computed here.
pub(crate) fn min_side(role: &str, agents: usize) -> f64 {
    let Some(row) = generate::min_size_table().get(role) else { return 0.0 };
    match agents.min(row.len()).checked_sub(1).and_then(|i| row.get(i)) {
        Some(Some(v)) => *v,
        Some(None) => f64::INFINITY,
        None => 0.0,
    }
}

/// The role's label on its room's sign (the web's `roleLabel`).
fn role_name(role: &str) -> String {
    crate::roles::role_label(role)
}

#[cfg(test)]
mod tests {
    use super::min_side;

    /// A role the table does not list has no minimum: 0 (index.ts `minSideFor`).
    #[test]
    fn role_missing_from_the_table_gives_zero() {
        assert_eq!(min_side("no-such-role", 1), 0.0);
        assert_eq!(min_side("no-such-role", 6), 0.0);
        assert_eq!(min_side("no-such-role", 9), 0.0);
    }

    /// Desk counts 1 to 6 give the table's entry for that count (assets/rooms/minsize.json).
    #[test]
    fn agents_one_to_six_give_the_table_value() {
        assert_eq!(min_side("architect", 1), 4.5);
        assert_eq!(min_side("architect", 4), 5.0);
        assert_eq!(min_side("architect", 6), 6.0);
        assert_eq!(min_side("planner", 1), 5.0);
        assert_eq!(min_side("planner", 6), 6.5);
        assert_eq!(min_side("incident", 3), 3.5);
    }

    /// Desk counts above 6 use the 6-agent entry, as index.ts clamps `agents` to the row's length.
    #[test]
    fn agents_above_six_give_the_six_agent_entry() {
        assert_eq!(min_side("architect", 7), 6.0);
        assert_eq!(min_side("architect", 8), 6.0);
        assert_eq!(min_side("orchestrator", 7), 6.5);
        assert_eq!(min_side("orchestrator", 12), 6.5);
    }
}
