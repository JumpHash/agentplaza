//! The room lab (`APZ_ROOM_LAB`), as the web's `#/room-lab`: a role's room, the twelve seeds of one role side by side
//! (`APZ_ROOM_GRID=1`), or every role at one size, on the machines' ground under the floor's light. Env, all optional
//! except the first:
//!   APZ_ROOM_LAB    a role id, or `all` (every role at the same size)
//!   APZ_ROOM_SEED   the seed, a u32 (default 7); with APZ_ROOM_GRID=1 the seeds seed .. seed+11
//!   APZ_ROOM_SIZE   the inner size WxD in units (default 4x3.5)
//!   APZ_ROOM_DOOR   the walkway side: n | e | s | w (default s)
//!   APZ_ROOM_GRID   1: the twelve seeds side by side (`all` ignores it)
//!   APZ_ROOM_CAM    obl (default, an oblique view) | top (looking straight down, the rooms fitted)
//!   APZ_ROOM_DEBUG  1: the debug overlay (the grid, the solid and walk cells, the reservations, the spots and the
//!                   violations' markers; see `overlay`)
//! Each room is reported on stderr: its items, spots, dropped and skipped items, and the verifier's violations.
//! `Lab::info` is the readout the page shows (the violations, the size, the door, and each room's line).

use super::super::props::Anim;
use super::super::world::Floor;
use super::generate::{self, Door, Input};
use super::overlay::{Overlay, overlay};
use super::verify::verify;
use super::{BuiltRoom, RoomOptions, build_room, default_agents, kit_palette};
use serde_json::{Value, json};

/// The rooms of `all` per row, and the seeds' columns (web: ALL_COLS, SEED_COLS).
const ALL_COLS: usize = 6;
const SEED_COLS: usize = 4;
const SEED_COUNT: u32 = 12;
/// The gaps between rooms along x and z (web: GAP_X, GAP_Z).
const GAP_X: f32 = 2.2;
const GAP_Z: f32 = 2.4;
/// The lab's hue (the web's default).
const HUE: f32 = 200.0;

/// The rooms on the lab's ground, and their clock.
pub struct Lab {
    anims: Vec<Anim>,
    t: f32,
    info: Value,
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
        let roles: Vec<&String> = generate::role_order().iter().collect();
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
    vec![Placement {
        role: role.to_string(),
        seed,
        x: 0.0,
        z: 0.0,
    }]
}

/// The rooms, when `APZ_ROOM_LAB` asks for them: they join the floor's root, and the camera frames them. None
/// otherwise (the floor is then the machines' own view).
pub fn build(floor: &mut Floor) -> Option<Lab> {
    let want = var("APZ_ROOM_LAB")?;
    let (w, d) = var("APZ_ROOM_SIZE")
        .and_then(|s| parse_size(&s))
        .unwrap_or((4.0, 3.5));
    let seed = var("APZ_ROOM_SEED")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(7);
    let door = var("APZ_ROOM_DOOR")
        .and_then(|s| Door::parse(&s))
        .unwrap_or(Door::S);
    let grid = var("APZ_ROOM_GRID").is_some_and(|g| g == "1");
    let top = var("APZ_ROOM_CAM").as_deref() == Some("top");
    let debug = var("APZ_ROOM_DEBUG").is_some_and(|g| g == "1");

    // The web room lab's stage: the pale blue, its lights, no plaza scenery.
    crate::robots::lab::pale_stage(
        floor,
        crate::scene::Color::hex(crate::robots::lab::DEFAULT_BACKGROUND),
    );
    let palette = kit_palette(&floor.colors);
    // The slot's half-size: the inner size plus the wall margin (web room-lab).
    let half = (w.max(d) / 2.0 + 0.075) as f32;
    let data = generate::data();
    let mut anims: Vec<Anim> = Vec::new();
    let (mut reach_x, mut reach_z) = (0.0_f32, 0.0_f32);
    let (mut total, mut warnings) = (0usize, 0usize);
    let mut lines: Vec<String> = Vec::new();
    // room-lab.ts's window.__roomLab: the roles shown and each room's layout, by its key (the role, or `seed:N` in the grid).
    let mut layouts = serde_json::Map::new();
    let mut roles: Vec<String> = Vec::new();
    let placed = placements(&want, w as f32, d as f32, seed, grid);
    let count = placed.len();
    for p in &placed {
        let opts = RoomOptions {
            seed: p.seed,
            door,
            w,
            d,
            agents: None,
        };
        let built: BuiltRoom = build_room(&p.role, half, HUE, &palette, &opts);
        let input = Input {
            role: p.role.clone(),
            w: built.w,
            d: built.d,
            door,
            seed: p.seed,
            agents: Some(default_agents(w, d)),
        };
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
            if built.warned {
                " (below its minimum)"
            } else {
                ""
            }
        );
        for v in &violations {
            eprintln!("agentplaza-desktop: room lab:   {v}");
        }
        total += violations.len();
        warnings += usize::from(built.warned);
        lines.push(room_line(
            &label_of(&want, p),
            &built.layout,
            violations.len(),
        ));
        let key = if grid && want != "all" {
            format!("seed:{}", p.seed)
        } else {
            p.role.clone()
        };
        layouts.insert(
            key,
            serde_json::to_value(&built.layout).unwrap_or(Value::Null),
        );
        if !roles.contains(&p.role) {
            roles.push(p.role.clone());
        }
        lines.extend(violations.iter().map(|v| format!("    {v}")));
        built.group.set_position(p.x, 0.0, p.z);
        floor.root.add(&built.group);
        floor.root.add(&caption(
            &label_of(&want, p),
            built.w as f32,
            built.d as f32,
            p.x,
            p.z,
        ));
        if debug {
            let o = Overlay {
                layout: &built.layout,
                violations: &violations,
                catalog: &data.catalog,
                w: built.w,
                d: built.d,
                door,
            };
            let g = overlay(&o);
            g.set_position(p.x, 0.0, p.z);
            floor.root.add(&g);
        }
        reach_x = reach_x.max(p.x.abs() + built.w as f32 / 2.0 + 1.0);
        reach_z = reach_z.max(p.z.abs() + built.d as f32 / 2.0 + 1.0);
        anims.push(built.animate);
    }
    // room-lab.ts's ground and its key light's shadow camera: a shadow-only plane 6 wider and deeper than the rooms, and
    // a shadow square 2 past them (the engine's is square, so the larger half).
    crate::robots::lab::shadow_ground(
        floor,
        &crate::geometry::plane(2.0 * reach_x + 6.0, 2.0 * reach_z + 6.0, 1, 1),
    );
    floor.world.shadow_extent = reach_x.max(reach_z) + 2.0;
    floor.world.shadow_near = 1.0;
    floor.world.shadow_far = 80.0;
    floor.statics_dirty = true; // the rooms' static parts are baked
    // room-lab.ts frame: the span (the larger of the lab's width and depth) times 1.45 plus 4, from (0.55, 0.85, 0.9)
    // or straight down, at the origin. Its camera's field was 30 degrees and the orbit's is FOV_DEG, so the distance
    // is scaled by tan 15 / tan(FOV_DEG / 2) to frame the same.
    let span = (2.0 * reach_x).max(2.0 * reach_z);
    let field = 15f32.to_radians().tan() / (crate::controls::FOV_DEG / 2.0).to_radians().tan();
    let dist = (span * 1.45 + 4.0) * field;
    let eye = if top {
        glam::Vec3::new(0.0, dist, 0.001)
    } else {
        glam::Vec3::new(0.55, 0.85, 0.9).normalize() * dist
    };
    floor.orbit.look(eye, glam::Vec3::ZERO);
    // room-lab.ts's OrbitControls kept three's defaults: no polar or distance limits (the top view looks straight down).
    floor.orbit.unlimited(0.0, f32::INFINITY);
    let counts = if count == 1 {
        String::new()
    } else {
        format!("{count} rooms · ")
    };
    let small = if warnings > 0 {
        format!(" · {warnings} below minimum")
    } else {
        String::new()
    };
    let debug_tag = if debug { " · debug" } else { "" };
    let summary = format!(
        "{counts}violations {total}{small} · {w} × {d} · door {}{debug_tag}",
        door.as_str()
    );
    let info = json!({ "kind": "rooms", "summary": summary, "violations": total, "rooms": count, "below_minimum": warnings, "lines": lines, "roles": roles, "layouts": layouts });
    Some(Lab {
        anims,
        t: 0.0,
        info,
    })
}

/// A room's name in the readout: its role for the `all` lab, else its role and seed.
fn label_of(want: &str, p: &Placement) -> String {
    if want == "all" {
        p.role.clone()
    } else {
        format!("{} seed {}", p.role, p.seed)
    }
}

/// One room's line of the readout: its items, spots, the skipped and dropped preferred and required groups, and its
/// violations (the count).
/// room-lab.ts caption: the room's label in dark text on the ground in front of it (+z, the side the oblique camera
/// looks from), as wide as the room (at least 2.4 units).
fn caption(text: &str, w: f32, d: f32, x: f32, z: f32) -> crate::scene::Obj {
    use crate::scene::{Mat, Material, Obj};
    let mut cv = crate::canvas::Canvas::new(768, 80);
    cv.set_fill("#1f2937");
    cv.set_font("600 40px system-ui, sans-serif");
    cv.set_text_baseline("middle");
    cv.fill_text(text, 8.0, 42.0);
    let width = w.max(2.4);
    let mat = Mat::new(Material {
        map: Some(cv.to_texture()),
        transparent: true,
        depth_write: false,
        shading: crate::scene::Shading::Basic,
        ..Material::default()
    });
    let mesh = Obj::mesh(
        &crate::geometry::plane(width, width * 80.0 / 768.0, 1, 1),
        &mat,
    );
    mesh.set_rotation(-std::f32::consts::FRAC_PI_2, 0.0, 0.0)
        .set_position(x, 0.02, z + d / 2.0 + 0.55);
    mesh
}

fn room_line(label: &str, layout: &generate::Layout, violations: usize) -> String {
    let dropped = if layout.dropped.is_empty() {
        String::new()
    } else {
        format!(" dropped {}", layout.dropped.join(", "))
    };
    let skipped = if layout.skipped.is_empty() {
        String::new()
    } else {
        format!(" skipped {}", layout.skipped.join(", "))
    };
    format!(
        "{label}: items {}, spots {}{skipped}{dropped}, violations {violations}",
        layout.items.len(),
        layout.spots.len()
    )
}

impl Lab {
    /// The readout the page shows: the summary line, the violations, the rooms and each room's line (JSON).
    pub fn info(&self) -> &Value {
        &self.info
    }

    /// Sets the rooms' animation to the ambient clock in seconds: the screens' glow is a function of it.
    pub fn step(&mut self, clock: f32) {
        self.t = clock;
        let t = clock;
        for a in &mut self.anims {
            a(t);
        }
    }
}
