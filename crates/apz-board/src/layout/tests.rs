//! The layout's tests: every board kind at 1, 4, 8 and 20 agents (finite scale, plates on their drawn supports, a PCB
//! surface, homes inside their rooms), the walk surfaces (ids, convex counter-clockwise rectangles, no overlaps at one
//! height), determinism, and the plate height against the support at its centre.

use std::collections::BTreeMap;

use super::geom::{Rect, shoelace};
use super::*;
use crate::walk::{SurfaceKind, WalkSurface};

const KINDS: [&str; 6] = [
    "edge",
    "laptop",
    "phone",
    "apple-soc",
    "server",
    "workstation",
];
const COUNTS: [usize; 4] = [1, 4, 8, 20];
const ROLES: [&str; 8] = [
    "architect",
    "builder",
    "qa",
    "librarian",
    "finance",
    "edge-operator",
    "mayor",
    "planner",
];

/// The input for `n` agents: one orchestrator, then roles in turn (the first agent is the orchestrator).
fn input(n: usize, gpus: u32) -> LayoutInput {
    let agents = (0..n)
        .map(|i| AgentSpec {
            id: format!("agent-{i:02}"),
            role: if i == 0 {
                "orchestrator".to_string()
            } else {
                ROLES[i % ROLES.len()].to_string()
            },
            seat_hint: None,
        })
        .collect();
    LayoutInput {
        agents,
        gpus,
        scale_cap: 3.0,
    }
}

/// The board at `gpus` with the same params `layout` uses.
fn board(kind: &str, gpus: u32) -> Board {
    let mut params = BTreeMap::new();
    params.insert("gpus".to_string(), f64::from(gpus));
    Board::from_kind(kind, &params)
}

/// The support under (x, z) at world scale `s`, as `layout` reads it.
fn support_at(board: &Board, s: f64, x: f64, z: f64) -> f64 {
    s * board.support_below(x / s, z / s, f64::INFINITY)
}

/// The overlap area of two axis-aligned rects.
fn overlap(a: &WalkSurface, b: &WalkSurface) -> f64 {
    let (ra, rb) = (bounds_of(&a.poly), bounds_of(&b.poly));
    let w = ra.x1.min(rb.x1) - ra.x0.max(rb.x0);
    let d = ra.z1.min(rb.z1) - ra.z0.max(rb.z0);
    if w > 0.0 && d > 0.0 { w * d } else { 0.0 }
}

fn bounds_of(poly: &[[f64; 2]]) -> Rect {
    let xs = poly.iter().map(|p| p[0]);
    let zs = poly.iter().map(|p| p[1]);
    Rect {
        x0: xs.clone().fold(f64::INFINITY, f64::min),
        x1: xs.fold(f64::NEG_INFINITY, f64::max),
        z0: zs.clone().fold(f64::INFINITY, f64::min),
        z1: zs.fold(f64::NEG_INFINITY, f64::max),
    }
}

#[test]
fn every_board_and_count_lays_out() {
    for kind in KINDS {
        for n in COUNTS {
            let lay = layout(kind, &input(n, 0));
            let tag = format!("{kind} n={n}");
            assert!(
                lay.board_scale.is_finite() && (1.0..=3.0 + 1e-9).contains(&lay.board_scale),
                "{tag}: scale {}",
                lay.board_scale
            );
            assert_eq!(lay.homes.len(), n, "{tag}: one home per agent");
            assert!(
                lay.walk
                    .surfaces
                    .iter()
                    .any(|s| s.kind == SurfaceKind::Board),
                "{tag}: no PCB surface"
            );
            // Lifts and landings are planned from the open-cell reach, so a lift may now exist: check that the second-pass
            // lists point at real plates and surfaces, not that they are empty (the old placement had none at these counts).
            assert!(
                lay.gangways.iter().all(|g| g.plate < lay.plates.len()),
                "{tag}: gangway plate index"
            );
            assert!(
                lay.lifts.iter().all(|l| l.plate < lay.plates.len()
                    && l.cab < lay.walk.surfaces.len() as u32
                    && l.bottom < lay.walk.surfaces.len() as u32
                    && l.top < lay.walk.surfaces.len() as u32),
                "{tag}: lift refers to a missing plate or surface"
            );
            assert!(
                lay.landings.iter().all(|l| l.plate < lay.plates.len()
                    && (l.surface as usize) < lay.walk.surfaces.len()),
                "{tag}: landing refers to a missing plate or surface"
            );
            let surfaces = lay.walk.surfaces.len() as u32;
            assert!(
                lay.walk.links.iter().all(|k| k.from < surfaces
                    && k.to < surfaces
                    && k.via.is_none_or(|v| v < surfaces)),
                "{tag}: link refers to a missing walk surface"
            );
            let b = board(kind, 0);
            for p in &lay.plates {
                assert!(
                    p.y.is_finite() && p.support.is_finite() && p.bottom.is_finite(),
                    "{tag}: plate {} finite",
                    p.id
                );
                let sup = lattice_max(&b, lay.board_scale, p.rect);
                assert!(
                    (p.support - sup).abs() < 1e-9,
                    "{tag}: plate {} support {} vs drawn {}",
                    p.id,
                    p.support,
                    sup
                );
                assert!(
                    p.y + 1e-9 >= p.support,
                    "{tag}: plate {} floats below its support",
                    p.id
                );
                let back = Rect::from_xywh(p.rect).xywh();
                assert!(
                    (0..4).all(|k| (back[k] - p.rect[k]).abs() < 1e-9),
                    "{tag}: rect round trip"
                );
            }
            for h in &lay.homes {
                let r = &lay.rooms[h.room];
                assert!(
                    (h.at[0] - r.centre[0]).abs() <= r.half[0] + 1e-9
                        && (h.at[1] - r.centre[1]).abs() <= r.half[1] + 1e-9,
                    "{tag}: home of {} outside its room",
                    h.agent
                );
            }
            let seated: usize = lay.rooms.iter().map(|r| r.members.len()).sum();
            assert_eq!(seated, n, "{tag}: every agent is in a room");
        }
    }
}

#[test]
fn walk_surfaces_are_indexed_convex_and_counter_clockwise() {
    for kind in KINDS {
        for n in COUNTS {
            let lay = layout(kind, &input(n, 0));
            let tag = format!("{kind} n={n}");
            let surfaces = &lay.walk.surfaces;
            for (i, s) in surfaces.iter().enumerate() {
                assert_eq!(s.id as usize, i, "{tag}: id is the index");
                assert_eq!(s.sides.len(), s.poly.len(), "{tag}: one side per edge");
                assert!(s.poly.len() >= 3, "{tag}: polygon");
                assert!(
                    shoelace(&s.poly) < 0.0,
                    "{tag}: surface {i} is not counter-clockwise from above"
                );
                let k = s.poly.len();
                for j in 0..k {
                    let (a, b, c) = (s.poly[j], s.poly[(j + 1) % k], s.poly[(j + 2) % k]);
                    let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
                    assert!(
                        cross < 1e-12,
                        "{tag}: surface {i} is not convex at vertex {j}"
                    );
                }
            }
            for (i, a) in surfaces.iter().enumerate() {
                for b in &surfaces[i + 1..] {
                    if a.height == b.height {
                        assert!(
                            overlap(a, b) < 1e-9,
                            "{tag}: surfaces {} and {} overlap at one height",
                            a.owner,
                            b.owner
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn layout_is_deterministic() {
    for kind in KINDS {
        for n in COUNTS {
            let a = layout(kind, &input(n, 0));
            let b = layout(kind, &input(n, 0));
            assert_eq!(a, b, "{kind} n={n}");
            assert_eq!(
                serde_json::to_string(&a).unwrap(),
                serde_json::to_string(&b).unwrap(),
                "{kind} n={n}: JSON"
            );
        }
    }
}

/// The highest support over the plate lattice (columns::SUPPORT_CELL centres), at the world scale `s`.
fn lattice_max(board: &Board, s: f64, rect: [f64; 4]) -> f64 {
    let lat = super::columns::Lattice::of(rect, &|x, z| support_at(board, s, x, z));
    lat.highest()
}

/// The plate rule (the user's decision): a plate rests on the highest support sample under its footprint, plus PLATE.
/// Every support sample under the footprint is at or below the underside, and every gap between a sample and the
/// underside is covered by a column that stands on that sample's block.
#[test]
fn plate_rests_on_highest_support() {
    for kind in KINDS {
        for n in [1usize, 8] {
            let lay = layout(kind, &input(n, 0));
            let b = board(kind, 0);
            let s = lay.board_scale;
            for p in &lay.plates {
                let under = p.y - PLATE;
                assert!(
                    (p.bottom - under).abs() < 1e-9,
                    "{kind} n={n}: plate {} underside {} vs y - PLATE {}",
                    p.id,
                    p.bottom,
                    under
                );
                let lat = super::columns::Lattice::of(p.rect, &|x, z| support_at(&b, s, x, z));
                assert!(
                    (lat.highest() - under).abs() < 1e-9,
                    "{kind} n={n}: plate {} highest support {} vs underside {}",
                    p.id,
                    lat.highest(),
                    under
                );
                let cell_w = p.rect[2] / lat.nx as f64;
                let cell_d = p.rect[3] / lat.nz as f64;
                for j in 0..lat.nz {
                    for i in 0..lat.nx {
                        let sup = lat.sup[j * lat.nx + i];
                        assert!(
                            sup <= under + 1e-9,
                            "{kind} n={n}: plate {} sample {sup} above its underside {under}",
                            p.id
                        );
                        if under - sup <= super::columns::GROUND_TOL + 1e-9 {
                            continue;
                        }
                        let x = p.rect[0] + (i as f64 + 0.5) * cell_w;
                        let z = p.rect[1] + (j as f64 + 0.5) * cell_d;
                        let covered = lay.columns.iter().any(|c| {
                            c.plate == p.id
                                && c.rect[0] - 1e-9 <= x
                                && x <= c.rect[0] + c.rect[2] + 1e-9
                                && c.rect[1] - 1e-9 <= z
                                && z <= c.rect[1] + c.rect[3] + 1e-9
                                // A column's foot is its block's highest sample, so it may sit within GROUND_TOL above this one (the generator's flat-block rule).
                                && c.bottom <= sup + super::columns::GROUND_TOL + 1e-9
                                && (c.top - under).abs() < 1e-9
                        });
                        assert!(
                            covered,
                            "{kind} n={n}: plate {} gap under sample ({x:.3}, {z:.3}) is not covered by a column",
                            p.id
                        );
                    }
                }
            }
        }
    }
}

/// The text of a caught panic's payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic".to_string())
}

/// The GPU-fitted boards: the workstation and the server at 1, 2 and 4 GPUs (the `gpu-pcie` parts the board model adds),
/// every board count. The GPU cards are tall parts on the board, so they change which plates are flat and which aisles
/// are clear. Release only, like the timing test; each case prints `TIMING kind gpus=g n ms seated|failed(reason)`.
#[test]
#[cfg_attr(debug_assertions, ignore = "layout timing is asserted in release only")]
fn gpu_boards_seat_every_agent() {
    let failed = seat_cases(&[1]);
    assert!(
        failed.is_empty(),
        "agents do not seat on the one-GPU boards: {failed:?}"
    );
}

/// Two and four GPUs: the web cannot lay these out either (nothing within the scale cap), so they are not a port test yet.
#[test]
#[ignore = "the web cannot lay these out either (nothing within scale cap 3); a design change, not a port"]
fn gpu_boards_with_two_or_four_gpus_seat_every_agent() {
    let failed = seat_cases(&[2, 4]);
    assert!(
        failed.is_empty(),
        "agents do not seat on the multi-GPU boards: {failed:?}"
    );
}

/// Lays out each (board, GPU count, agent count) case, prints its TIMING line, and returns the cases that panic.
fn seat_cases(gpu_counts: &[u32]) -> Vec<String> {
    let mut failed = Vec::new();
    for kind in ["workstation", "server"] {
        for &gpus in gpu_counts {
            for n in COUNTS {
                let t = std::time::Instant::now();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    layout(kind, &input(n, gpus))
                }));
                let secs = t.elapsed().as_secs_f64();
                let status = match &outcome {
                    Ok(_) => "seated".to_string(),
                    Err(payload) => format!("failed({})", panic_text(payload.as_ref())),
                };
                eprintln!(
                    "TIMING {kind} gpus={gpus} n={n} {:.0} {status}",
                    secs * 1000.0
                );
                if outcome.is_err() {
                    failed.push(format!("{kind} gpus={gpus} n={n}"));
                }
            }
        }
    }
    failed
}

#[test]
fn the_nav_scene_of_a_layout_reaches_every_plate_and_door() {
    for kind in [
        "edge",
        "laptop",
        "phone",
        "workstation",
        "server",
        "apple-soc",
    ] {
        let lay = layout(kind, &input(4, 0));
        let scene = super::nav_scene(&lay, 0, None);
        let c = crate::navgrid::connectivity(&scene.grid);
        assert!(
            c.unreached.is_empty() && c.unconnected.is_empty(),
            "{kind}: unreached {:?}, unconnected {:?}",
            c.unreached,
            c.unconnected
        );
        assert_eq!(scene.plans.len(), lay.rooms.len());
    }
}
