//! The room lab (`APZ_ROOM_LAB`), as the web's `#/room-lab`: a role's room, the twelve seeds of one role side by side
//! (`APZ_ROOM_GRID=1`), or every role at one size, on the machines' ground under the floor's light. Env, all optional
//! except the first:
//!   APZ_ROOM_LAB    a role id, or `all` (every role at the same size)
//!   APZ_ROOM_SEED   the seed, a u32 (default 7); with APZ_ROOM_GRID=1 the seeds seed .. seed+11
//!   APZ_ROOM_SIZE   the inner size WxD in units (default 4x3.5)
//!   APZ_ROOM_DOOR   the walkway side: n | e | s | w (default s)
//!   APZ_ROOM_GRID   1: the twelve seeds side by side (`all` ignores it)
//! Each room is reported on stderr: its items, spots, dropped and skipped items, and the verifier's violations.


use super::super::props::Anim;
use super::super::world::Floor;
use super::generate::{self, Door, Input};
use super::verify::verify;
use super::{build_room, default_agents, kit_palette, BuiltRoom, RoomOptions};

/// The rooms of `all` per row, and the seeds' columns (web: ALL_COLS, SEED_COLS).
const ALL_COLS: usize = 6;
const SEED_COLS: usize = 4;
const SEED_COUNT: u32 = 12;
/// The gaps between rooms along x and z (web: GAP_X, GAP_Z).
const GAP_X: f32 = 2.2;
const GAP_Z: f32 = 2.4;
/// The lab's hue (the web's default).
const HUE: f32 = 200.0;
/// The focus of the camera: the rooms' middle, a little above the ground.
const FOCUS_Y: f32 = 0.7;

/// The rooms on the lab's ground, and their clock.
pub struct Lab {
    anims: Vec<Anim>,
    t: f32,
}

/// One room on the ground: its role, seed and where it stands.
struct Placement {
    role: String,
    seed: u32,
    x: f32,
    z: f32,
}

fn var(name: &str) -> Option<String> {
    crate::env_var(name).filter(|v| !v.is_empty())
}

/// `WxD` in units: two positive numbers joined by an x.
fn parse_size(s: &str) -> Option<(f64, f64)> {
    let (w, d) = s.split_once('x')?;
    let (w, d): (f64, f64) = (w.trim().parse().ok()?, d.trim().parse().ok()?);
    (w > 0.0 && d > 0.0).then_some((w, d))
}

/// The rooms of the lab: every role (`all`), the twelve seeds of one role (`grid`), or the one room, at the origin.
fn placements(role: &str, w: f32, d: f32, seed: u32, grid: bool) -> Vec<Placement> {
    if role == "all" {
        let roles: Vec<&String> = generate::data().roles.roles.keys().collect();
        let rows = roles.len().div_ceil(ALL_COLS);
        return roles
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let (col, row) = (i % ALL_COLS, i / ALL_COLS);
                Placement {
                    role: r.clone(),
                    seed,
                    x: (col as f32 - (ALL_COLS as f32 - 1.0) / 2.0) * (w + GAP_X),
                    z: (row as f32 - (rows as f32 - 1.0) / 2.0) * (d + GAP_Z),
                }
            })
            .collect();
    }
    if grid {
        let rows = (SEED_COUNT as usize).div_ceil(SEED_COLS);
        return (0..SEED_COUNT)
            .map(|i| {
                let (col, row) = (i as usize % SEED_COLS, i as usize / SEED_COLS);
                Placement {
                    role: role.to_string(),
                    seed: seed.wrapping_add(i),
                    x: (col as f32 - (SEED_COLS as f32 - 1.0) / 2.0) * (w + GAP_X),
                    z: (row as f32 - (rows as f32 - 1.0) / 2.0) * (d + GAP_Z),
                }
            })
            .collect();
    }
    vec![Placement { role: role.to_string(), seed, x: 0.0, z: 0.0 }]
}

/// The rooms, when `APZ_ROOM_LAB` asks for them: they join the floor's root, and the camera frames them. None
/// otherwise (the floor is then the machines' own view).
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let want = var("APZ_ROOM_LAB")?;
    let (w, d) = var("APZ_ROOM_SIZE").and_then(|s| parse_size(&s)).unwrap_or((4.0, 3.5));
    let seed = var("APZ_ROOM_SEED").and_then(|s| s.parse::<u32>().ok()).unwrap_or(7);
    let door = var("APZ_ROOM_DOOR").and_then(|s| Door::parse(&s)).unwrap_or(Door::S);
    let grid = var("APZ_ROOM_GRID").is_some_and(|g| g == "1");

    // The web room lab's stage: the pale blue, its lights, no plaza scenery.
    crate::robots::lab::pale_stage(floor, crate::scene::Color::hex(crate::robots::lab::DEFAULT_BACKGROUND));
    let palette = kit_palette(&floor.colors);
    // The slot's half-size: the inner size plus the wall margin (web room-lab).
    let half = (w.max(d) / 2.0 + 0.075) as f32;
    let data = generate::data();
    let mut anims: Vec<Anim> = Vec::new();
    let (mut reach_x, mut reach_z) = (0.0_f32, 0.0_f32);
    for p in placements(&want, w as f32, d as f32, seed, grid) {
        let opts = RoomOptions { seed: p.seed, door, w, d, agents: None };
        let built: BuiltRoom = build_room(&p.role, half, HUE, &palette, &opts);
        let input = Input { role: p.role.clone(), w: built.w, d: built.d, door, seed: p.seed, agents: Some(default_agents(w, d)) };
        let violations = verify(&built.layout, &input, data);
        eprintln!(
            "agentplaza-desktop: room lab: {} seed {}: {} items, {} spots, dropped [{}], skipped [{}], {} violations{}",
            p.role,
            p.seed,
            built.layout.items.len(),
            built.spots.len(),
            built.layout.dropped.join(", "),
            built.layout.skipped.join(", "),
            violations.len(),
            if built.warned { " (below its minimum)" } else { "" }
        );
        for v in &violations {
            eprintln!("agentplaza-desktop: room lab:   {v}");
        }
        built.group.set_position(p.x, 0.0, p.z);
        floor.root.add(&built.group);
        reach_x = reach_x.max(p.x.abs() + built.w as f32 / 2.0 + 1.0);
        reach_z = reach_z.max(p.z.abs() + built.d as f32 / 2.0 + 1.0);
        anims.push(built.animate);
    }
    floor.statics_dirty = true; // the rooms' static parts are baked
    // The whole lab in view, from the floor's own camera angle.
    let radius = (reach_x * reach_x + reach_z * reach_z).sqrt() + 0.5;
    floor.fit_sphere([0.0, FOCUS_Y, 0.0], radius, 1.5);
    Some(Lab { anims, t: 0.0 })
}

impl Lab {
    /// Sets the rooms' animation to the ambient clock in seconds: the screens' glow is a function of it.
    pub fn step(&mut self, clock: f32) {
        self.t = clock;
        let t = clock;
        for a in &mut self.anims {
            a(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::generate::data;
    use super::*;
    use crate::world::Colors;

    #[test]
    fn the_size_reads_as_w_by_d() {
        assert_eq!(parse_size("4x3.5"), Some((4.0, 3.5)));
        assert_eq!(parse_size("4"), None);
        assert_eq!(parse_size("0x3"), None);
    }

    #[test]
    fn the_grid_is_twelve_seeds_of_one_role() {
        let grid = placements("architect", 4.0, 3.5, 7, true);
        assert_eq!(grid.iter().map(|p| p.seed).collect::<Vec<_>>(), (7..19).collect::<Vec<u32>>());
        assert_eq!(placements("architect", 4.0, 3.5, 7, false).len(), 1);
    }

    #[test]
    fn every_role_builds_a_lab_room_with_a_fixed_seed() {
        let palette = kit_palette(&Colors::default());
        let opts = RoomOptions { seed: 7, door: Door::S, w: 4.0, d: 3.5, agents: None };
        for role in data().roles.roles.keys() {
            let built = build_room(role, 2.1, HUE, &palette, &opts);
            assert!(!built.layout.items.is_empty(), "{role} has items");
            assert_eq!((built.w, built.d), (4.0, 3.5));
        }
    }
}
