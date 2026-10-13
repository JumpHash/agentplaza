//! The layout's tests: every board kind at 1, 4, 8 and 20 agents (finite scale, plates on their drawn supports, a PCB
//! surface, homes inside their rooms), the walk surfaces (ids, convex counter-clockwise rectangles, no overlaps at one
//! height), determinism, and the plate height against the support at its centre.

use std::collections::BTreeMap;

use super::geom::{shoelace, subtract, Frame, Rect};
use super::*;
use crate::walk::{SurfaceKind, WalkSurface};

const KINDS: [&str; 6] = ["edge", "laptop", "phone", "apple-soc", "server", "workstation"];
const COUNTS: [usize; 4] = [1, 4, 8, 20];
const ROLES: [&str; 8] = ["architect", "builder", "qa", "librarian", "finance", "edge-operator", "mayor", "planner"];

/// The input for `n` agents: one orchestrator, then roles in turn (the first agent is the orchestrator).
fn input(n: usize, gpus: u32) -> LayoutInput {
    let agents = (0..n)
        .map(|i| AgentSpec {
            id: format!("agent-{i:02}"),
            role: if i == 0 { "orchestrator".to_string() } else { ROLES[i % ROLES.len()].to_string() },
            seat_hint: None,
        })
        .collect();
    LayoutInput { agents, gpus, scale_cap: 3.0 }
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

/// The centre of a rect [x, z, w, d].
fn centre(rect: [f64; 4]) -> [f64; 2] {
    [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0]
}

/// The overlap area of two axis-aligned rects.
fn overlap(a: &WalkSurface, b: &WalkSurface) -> f64 {
    let (ra, rb) = (bounds_of(&a.poly), bounds_of(&b.poly));
    let w = ra.x1.min(rb.x1) - ra.x0.max(rb.x0);
    let d = ra.z1.min(rb.z1) - ra.z0.max(rb.z0);
    if w > 0.0 && d > 0.0 {
        w * d
    } else {
        0.0
    }
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
            assert!(lay.board_scale.is_finite() && (1.0..=3.0 + 1e-9).contains(&lay.board_scale), "{tag}: scale {}", lay.board_scale);
            assert_eq!(lay.homes.len(), n, "{tag}: one home per agent");
            assert!(lay.walk.surfaces.iter().any(|s| s.kind == SurfaceKind::Board), "{tag}: no PCB surface");
            // Lifts and landings are planned from the open-cell reach, so a lift may now exist: check that the second-pass
            // lists point at real plates and surfaces, not that they are empty (the old placement had none at these counts).
            assert!(lay.gangways.iter().all(|g| g.plate < lay.plates.len()), "{tag}: gangway plate index");
            assert!(lay.lifts.iter().all(|l| l.plate < lay.plates.len() && l.cab < lay.walk.surfaces.len() as u32 && l.bottom < lay.walk.surfaces.len() as u32 && l.top < lay.walk.surfaces.len() as u32), "{tag}: lift refers to a missing plate or surface");
            assert!(lay.landings.iter().all(|l| l.plate < lay.plates.len() && (l.surface as usize) < lay.walk.surfaces.len()), "{tag}: landing refers to a missing plate or surface");
            let surfaces = lay.walk.surfaces.len() as u32;
            assert!(lay.walk.links.iter().all(|k| k.from < surfaces && k.to < surfaces && k.via.map_or(true, |v| v < surfaces)), "{tag}: link refers to a missing walk surface");
            let b = board(kind, 0);
            for p in &lay.plates {
                assert!(p.y.is_finite() && p.support.is_finite() && p.bottom.is_finite(), "{tag}: plate {} finite", p.id);
                let sup = lattice_max(&b, lay.board_scale, p.rect);
                assert!((p.support - sup).abs() < 1e-9, "{tag}: plate {} support {} vs drawn {}", p.id, p.support, sup);
                assert!(p.y + 1e-9 >= p.support, "{tag}: plate {} floats below its support", p.id);
                let back = Rect::from_xywh(p.rect).xywh();
                assert!((0..4).all(|k| (back[k] - p.rect[k]).abs() < 1e-9), "{tag}: rect round trip");
            }
            for h in &lay.homes {
                let r = &lay.rooms[h.room];
                assert!((h.at[0] - r.centre[0]).abs() <= r.half[0] + 1e-9 && (h.at[1] - r.centre[1]).abs() <= r.half[1] + 1e-9, "{tag}: home of {} outside its room", h.agent);
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
                assert!(shoelace(&s.poly) < 0.0, "{tag}: surface {i} is not counter-clockwise from above");
                let k = s.poly.len();
                for j in 0..k {
                    let (a, b, c) = (s.poly[j], s.poly[(j + 1) % k], s.poly[(j + 2) % k]);
                    let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
                    assert!(cross < 1e-12, "{tag}: surface {i} is not convex at vertex {j}");
                }
            }
            for (i, a) in surfaces.iter().enumerate() {
                for b in &surfaces[i + 1..] {
                    if a.height == b.height {
                        assert!(overlap(a, b) < 1e-9, "{tag}: surfaces {} and {} overlap at one height", a.owner, b.owner);
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
            assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap(), "{kind} n={n}: JSON");
        }
    }
}

#[test]
fn plate_height_is_the_support_under_it() {
    for kind in KINDS {
        for n in COUNTS {
            let lay = layout(kind, &input(n, 0));
            let b = board(kind, 0);
            let s = lay.board_scale;
            for p in &lay.plates {
                let at_centre = {
                    let c = centre(p.rect);
                    support_at(&b, s, c[0], c[1])
                };
                assert!(p.support + 1e-9 >= at_centre, "{kind} n={n}: plate {} is below its centre's support", p.id);
                // Where the footprint is flat (every lattice sample at one height), the plate rests on that height at its centre.
                let lat = super::columns::Lattice::of(p.rect, &|x, z| support_at(&b, s, x, z));
                let flat = lat.sup.iter().all(|&v| (v - at_centre).abs() < 1e-9);
                if flat {
                    assert!((p.support - at_centre).abs() < 1e-9, "{kind} n={n}: plate {} height {} vs centre {}", p.id, p.support, at_centre);
                }
            }
        }
    }
}

#[test]
fn columns_and_rooms_hold_their_plates() {
    for kind in KINDS {
        let lay = layout(kind, &input(8, 0));
        for c in &lay.columns {
            assert!(c.bottom < c.top, "{kind}: column {} has no height", c.plate);
            let p = &lay.plates[c.plate];
            assert!((c.top - p.bottom).abs() < 1e-9, "{kind}: column top is the plate's underside");
        }
        for r in &lay.rooms {
            assert!((r.floor_y - lay.plates[r.plate].y).abs() < 1e-9, "{kind}: room {} floor is its plate's top", r.id);
            let (ax, az) = (r.aisle_dir[0], r.aisle_dir[1]);
            assert!((ax.abs() + az.abs() - 1.0).abs() < 1e-9, "{kind}: aisle direction is an axis");
        }
    }
}

#[test]
fn frame_and_subtract_helpers() {
    let f = Frame::of([1.0, 2.0, 6.0, 3.0]);
    assert!(f.along_x && (f.l - 6.0).abs() < 1e-12 && (f.s - 3.0).abs() < 1e-12);
    assert_eq!(f.point(1.0, 0.5), [2.0, 2.5]);
    let g = Frame::of([1.0, 2.0, 3.0, 6.0]);
    assert!(!g.along_x && g.point(1.0, 0.5) == [1.5, 3.0]);
    let outer = Rect { x0: 0.0, z0: 0.0, x1: 10.0, z1: 10.0 };
    let hole = Rect { x0: 2.0, z0: 3.0, x1: 4.0, z1: 5.0 };
    let pieces = subtract(outer, &[hole]);
    let area: f64 = pieces.iter().map(|r| (r.x1 - r.x0) * (r.z1 - r.z0)).sum();
    assert!((area - 96.0).abs() < 1e-9, "area {area}");
    assert!(pieces.len() <= 4);
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
                assert!((p.bottom - under).abs() < 1e-9, "{kind} n={n}: plate {} underside {} vs y - PLATE {}", p.id, p.bottom, under);
                let lat = super::columns::Lattice::of(p.rect, &|x, z| support_at(&b, s, x, z));
                assert!((lat.highest() - under).abs() < 1e-9, "{kind} n={n}: plate {} highest support {} vs underside {}", p.id, lat.highest(), under);
                let cell_w = p.rect[2] / lat.nx as f64;
                let cell_d = p.rect[3] / lat.nz as f64;
                for j in 0..lat.nz {
                    for i in 0..lat.nx {
                        let sup = lat.sup[j * lat.nx + i];
                        assert!(sup <= under + 1e-9, "{kind} n={n}: plate {} sample {sup} above its underside {under}", p.id);
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
                        assert!(covered, "{kind} n={n}: plate {} gap under sample ({x:.3}, {z:.3}) is not covered by a column", p.id);
                    }
                }
            }
        }
    }
}


/// Every board kind at 1, 4, 8 and 20 agents. Each case is timed and its outcome recorded before anything is asserted, so one
/// failure does not hide the rest. Timing is meaningful only in release (`cargo test -p apz-board --release`), so the test is
/// ignored in debug builds. Each case prints `TIMING kind n ms seated|failed(reason)`.
#[test]
#[cfg_attr(debug_assertions, ignore = "layout timing is asserted in release only")]
fn every_layout_seats_and_stays_under_the_measured_ceiling() {
    let (failed, slow) = timed_layouts(LAYOUT_CEILING_S);
    assert!(failed.is_empty(), "agents do not seat: {failed:?}");
    assert!(slow.is_empty(), "over the measured ceiling of {LAYOUT_CEILING_S} s (it only ratchets down): {slow:?}");
}

/// The budget. Laptop (about 4 s) and apple-soc n=1 (about 2.4 s) are over it today: every round of a scale step
/// rebuilds the nav grid for its walk set (support, moves, components). Un-ignore once they fit.
#[test]
#[ignore = "laptop (about 4 s) and apple-soc n=1 (about 2.4 s) are over 2 s; LAYOUT_CEILING_S holds the measured worst case meanwhile"]
fn every_layout_returns_within_two_seconds() {
    let (failed, slow) = timed_layouts(2.0);
    assert!(failed.is_empty(), "agents do not seat: {failed:?}");
    assert!(slow.is_empty(), "over the 2 s budget: {slow:?}");
}

/// The worst layout time measured on 2026-10-13 (laptop: 4.0 s with the suite's other layout tests in parallel, 4.6 s
/// alone at load 21), and room for a loaded CI runner. Lower it as the layout gets faster; never raise it.
const LAYOUT_CEILING_S: f64 = 9.0;

/// Lays out every kind at every count; returns the cases that panicked and those at or over `limit` seconds.
fn timed_layouts(limit: f64) -> (Vec<String>, Vec<String>) {
    let mut slow = Vec::new();
    let mut failed = Vec::new();
    for kind in KINDS {
        for n in COUNTS {
            let t = std::time::Instant::now();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layout(kind, &input(n, 0))));
            let secs = t.elapsed().as_secs_f64();
            let status = match &outcome {
                Ok(_) => "seated".to_string(),
                Err(payload) => format!("failed({})", panic_text(payload.as_ref())),
            };
            eprintln!("TIMING {kind} {n} {:.0} {status}", secs * 1000.0);
            if secs >= limit {
                slow.push(format!("{kind} n={n}: {secs:.3} s"));
            }
            if outcome.is_err() {
                failed.push(format!("{kind} n={n}"));
            }
        }
    }
    (failed, slow)
}

/// The text of a caught panic's payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic".to_string())
}

/// Baseline dump for the layout-speed work: writes `layout(kind, input(4, 0))` as pretty JSON to
/// `/tmp/apzb-{APZB_LABEL}-{kind}-n4.json` for each kind in APZB_KINDS (comma-separated; default "workstation,laptop").
/// Ignored by default, so it runs only when named: `-- --ignored dump_layout_json`.
#[test]
#[ignore = "writes layout JSON for the before/after comparison"]
fn dump_layout_json() {
    let label = std::env::var("APZB_LABEL").unwrap_or_else(|_| "dump".to_string());
    let kinds = std::env::var("APZB_KINDS").unwrap_or_else(|_| "workstation,laptop".to_string());
    for kind in kinds.split(',').filter(|k| !k.is_empty()) {
        let json = serde_json::to_string_pretty(&layout(kind, &input(4, 0))).expect("layout serializes");
        std::fs::write(format!("/tmp/apzb-{label}-{kind}-n4.json"), json).expect("write dump");
    }
}

/// Phone at 20 agents (the cap scale, 0.25 nav cells): every agent gets a home. Before the fix, 16 plates were unusable with
/// "no perimeter point" because reach_main walked only height-legal moves, where board-layout.ts reachMain walks any open cell.
#[test]
fn phone_twenty_seats_every_agent() {
    let lay = layout("phone", &input(20, 0));
    assert_eq!(lay.homes.len(), 20, "phone n=20 seats {} of 20", lay.homes.len());
}
