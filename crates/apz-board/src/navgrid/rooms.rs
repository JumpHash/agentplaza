//! Rule 7 of the nav grid (nav-grid.ts, "Rule 7: rooms, in order"): each terrace's interior, slab margin and cliff band
//! (7a), door cells (7b), room solids (7c), the door choice (7d) and the spots (7e).

use super::{NAV_RAMP_DEPTH, NavDoor, NavGrid, NavLocalSpot, NavSolid, NavSpot, NavTerrace, Side};
use crate::grid::NAV_COST_OPEN;
use crate::rooms::generate::{Catalog, Door, Layout, Place};

const EPS: f64 = 1e-9;

/// A terrace's frame (nav-grid.ts Frame): the turn, the door side's local normal N and lateral axis T, the outward world
/// normal, the inner half-size along N, and the door's centre and half-width.
struct Frame<'a> {
    t: &'a NavTerrace,
    cos_t: f64,
    sin_t: f64,
    nx: f64,
    nz: f64,
    tx: f64,
    tz: f64,
    nwx: f64,
    nwz: f64,
    h: f64,
    at: f64,
    hw: f64,
}

/// A centre classified against a terrace frame (nav-grid.ts Probe).
struct Probe {
    slab: bool,
    interior: bool,
    gap: f64,
    out: f64,
    lat: f64,
    in_span: bool,
}

/// nav-grid.ts probe.
fn probe(f: &Frame, px: f64, pz: f64) -> Probe {
    let t = f.t;
    let (dx, dz) = (px - t.cx, pz - t.cz);
    let (ax, az) = (dx.abs(), dz.abs());
    let slab = if t.round {
        dx * dx + dz * dz < t.half * t.half
    } else {
        ax < t.half && az < t.half
    };
    let gap = (ax - t.half).max(0.0).hypot((az - t.half).max(0.0));
    let lx = dx * f.cos_t - dz * f.sin_t;
    let lz = dx * f.sin_t + dz * f.cos_t;
    let interior = slab && lx.abs() < t.w / 2.0 && lz.abs() < t.d / 2.0;
    let out = lx * f.nx + lz * f.nz;
    let lat = lx * f.tx + lz * f.tz;
    Probe {
        slab,
        interior,
        gap,
        out,
        lat,
        in_span: (lat - f.at).abs() <= f.hw + EPS,
    }
}

/// The cell k steps outward of cell c along the outward world normal (nav-grid.ts outwardIdx).
fn outward(g: &NavGrid, f: &Frame, c: usize, k: f64) -> Option<usize> {
    let [x, z] = g.centre(c);
    g.cell_at(x + f.nwx * g.cell * k, z + f.nwz * g.cell * k)
}

/// Applies rule 7 for every terrace, in order: sets the cells' region, height and blocked flags, pushes each terrace's
/// `door_cells` entry and its spots onto the grid, and returns each terrace's door cells (7b, the exempt cells of rule 8).
pub(super) fn apply(g: &mut NavGrid, terraces: &[NavTerrace], plate_of: &[i32]) -> Vec<Vec<usize>> {
    let (cols, rows, cell, x0, z0) = (g.cols, g.rows, g.cell, g.x0, g.z0);
    let mut all_doors = Vec::with_capacity(terraces.len());
    for t in terraces {
        let ((nx, nz), (tx, tz)) = match t.door.side {
            Side::N => ((0.0, -1.0), (1.0, 0.0)),
            Side::E => ((1.0, 0.0), (0.0, 1.0)),
            Side::S => ((0.0, 1.0), (1.0, 0.0)),
            Side::W => ((-1.0, 0.0), (0.0, 1.0)),
        };
        let (cos_t, sin_t) = (t.turn.cos(), t.turn.sin());
        let f = Frame {
            t,
            cos_t,
            sin_t,
            nx,
            nz,
            tx,
            tz,
            nwx: nx * cos_t + nz * sin_t,
            nwz: -nx * sin_t + nz * cos_t,
            h: f64::abs(nx) * (t.w / 2.0) + f64::abs(nz) * (t.d / 2.0),
            at: t.door.at,
            hw: t.door.width / 2.0,
        };

        // 7a: interior, slab margin and band.
        let mut slab = Vec::new();
        for j in 0..rows {
            for i in 0..cols {
                let c = j * cols + i;
                if g.off[c] == 1 {
                    continue;
                }
                let [px, pz] = g.centre(c);
                let p = probe(&f, px, pz);
                if p.slab {
                    slab.push(c);
                    if p.interior {
                        g.region[c] = t.id;
                        g.y[c] = t.y;
                        g.blocked[c] = 0;
                        g.cost[c] = NAV_COST_OPEN;
                    } else {
                        g.blocked[c] = 1;
                    }
                } else if p.gap < NAV_RAMP_DEPTH && plate_of[c] == 0 {
                    g.blocked[c] = 1;
                }
            }
        }

        // 7b: door cells (the innermost interior row and the margin on the door side, within the span).
        let mut doors = Vec::new();
        for &c in &slab {
            let [px, pz] = g.centre(c);
            let p = probe(&f, px, pz);
            if !p.in_span {
                continue;
            }
            let is_door = if p.interior {
                p.out >= f.h - cell
            } else {
                p.out >= f.h
            };
            if !is_door {
                continue;
            }
            g.blocked[c] = 0;
            g.region[c] = t.id;
            g.y[c] = t.y;
            doors.push(c);
        }

        // 7c: room solids block the slab cells their footprints overlap (the local bbox of the cell's four corners).
        for s in &t.solids {
            let (fx0, fx1, fz0, fz1) = (
                s.x - s.sx / 2.0,
                s.x + s.sx / 2.0,
                s.z - s.sz / 2.0,
                s.z + s.sz / 2.0,
            );
            for &c in &slab {
                let (i, j) = (c % cols, c / cols);
                let (xa, xb) = (x0 + i as f64 * cell, x0 + (i + 1) as f64 * cell);
                let (za, zb) = (z0 + j as f64 * cell, z0 + (j + 1) as f64 * cell);
                let (mut mnx, mut mxx, mut mnz, mut mxz) = (
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                );
                for x in [xa, xb] {
                    for z in [za, zb] {
                        let (dx, dz) = (x - t.cx, z - t.cz);
                        let lx = dx * cos_t - dz * sin_t;
                        let lz = dx * sin_t + dz * cos_t;
                        mnx = mnx.min(lx);
                        mxx = mxx.max(lx);
                        mnz = mnz.min(lz);
                        mxz = mxz.max(lz);
                    }
                }
                if mnx < fx1 && mxx > fx0 && mnz < fz1 && mxz > fz0 {
                    g.blocked[c] = 1;
                }
            }
        }

        // 7d: the door: the open door cells, nearest the middle first. The outside cell of a door is the first cell
        // outward of it (within 4 cells) that lies outside the slab. The door is the first whose outside cell is an open
        // plate cell; if none, the first.
        let outside_of = |g: &NavGrid, d: usize| -> Option<usize> {
            for k in 1..=4 {
                let o = outward(g, &f, d, k as f64)?;
                let [px, pz] = g.centre(o);
                if !probe(&f, px, pz).slab {
                    return Some(o);
                }
                if g.blocked[o] != 0 {
                    return None;
                }
            }
            None
        };
        let mut order: Vec<(f64, usize)> = doors
            .iter()
            .filter(|&&d| g.blocked[d] == 0)
            .map(|&d| {
                let [px, pz] = g.centre(d);
                ((probe(&f, px, pz).lat - f.at).abs(), d)
            })
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut chosen = (None, None);
        for &(_, d) in &order {
            if let Some(o) = outside_of(g, d).filter(|&o| g.blocked[o] == 0 && plate_of[o] != 0) {
                chosen = (Some(d), Some(o));
                break;
            }
        }
        if let (None, Some(&(_, d))) = (chosen.0, order.first()) {
            chosen = (Some(d), outside_of(g, d));
        }
        g.door_cells.push(chosen);

        // 7e: spots, in world space.
        for (k, sp) in t.spots.iter().enumerate() {
            let wx = t.cx + sp.x * cos_t + sp.z * sin_t;
            let wz = t.cz - sp.x * sin_t + sp.z * cos_t;
            let cell_at = g.cell_at(wx, wz);
            g.spots.push(NavSpot {
                terrace: t.id,
                index: k,
                x: wx,
                z: wz,
                face: (sp.face * std::f64::consts::PI) / 180.0 + t.turn,
                cell: cell_at,
            });
        }
        all_doors.push(doors);
    }
    all_doors
}

/// A room layout's nav inputs: its door (layout.shell.door), its solids (rule 7c's catalogue filter: placed neither on
/// a surface nor on a wall, not on the floor layer, not walkable; the size swapped when the item is turned a quarter) and
/// its spots.
pub fn furniture(
    layout: &Layout,
    catalog: &Catalog,
) -> (NavDoor, Vec<NavSolid>, Vec<NavLocalSpot>) {
    let d = &layout.shell.door;
    let side = match d.side {
        Door::N => Side::N,
        Door::E => Side::E,
        Door::S => Side::S,
        Door::W => Side::W,
    };
    let door = NavDoor {
        side,
        at: d.at,
        width: d.width,
    };
    let solids = layout
        .items
        .iter()
        .filter_map(|it| {
            let cat = catalog.items.get(&it.id)?;
            if matches!(cat.place, Place::Surface | Place::Wallmount)
                || cat.layer.as_deref() == Some("floor")
                || cat.walk
            {
                return None;
            }
            let along = it.rot % 180.0 == 0.0;
            let (sx, sz) = if along {
                (cat.size[0], cat.size[2])
            } else {
                (cat.size[2], cat.size[0])
            };
            Some(NavSolid {
                x: it.x,
                z: it.z,
                sx,
                sz,
            })
        })
        .collect();
    let spots = layout
        .spots
        .iter()
        .map(|s| NavLocalSpot {
            x: s.x,
            z: s.z,
            face: s.face,
        })
        .collect();
    (door, solids, spots)
}

#[cfg(test)]
mod tests {
    use super::super::{NavBoard, NavTerrace, build};
    use super::*;

    fn terrace(solids: Vec<NavSolid>) -> NavTerrace {
        NavTerrace {
            id: 1,
            cx: 0.0,
            cz: 0.0,
            half: 3.0,
            hw: 3.0,
            hd: 3.0,
            round: false,
            y: 1.0,
            turn: 0.0,
            w: 4.0,
            d: 4.0,
            door: NavDoor {
                side: Side::S,
                at: 0.0,
                width: 1.0,
            },
            solids,
            spots: Vec::new(),
        }
    }

    fn board() -> NavBoard {
        NavBoard {
            size: [40.0, 1.0, 40.0],
            pcb_top: 0.4,
            boxes: Vec::new(),
            traces: Vec::new(),
            plates: Vec::new(),
        }
    }

    #[test]
    fn build_places_terraces_and_solids() {
        // A terrace has its interior band and its door.
        {
            let g = build(&board(), &[terrace(Vec::new())]);
            let centre = g.cell_at(0.1, 0.1).unwrap();
            assert_eq!((g.region[centre], g.y[centre]), (1, 1.0));
            assert_eq!(
                g.blocked[g.cell_at(3.6, 0.1).unwrap()],
                1,
                "the cliff band is blocked"
            );
            assert!(g.door_cells[0].0.is_some());
        }
        // A solid blocks its cells.
        {
            let g = build(
                &board(),
                &[terrace(vec![NavSolid {
                    x: 0.0,
                    z: 0.0,
                    sx: 1.0,
                    sz: 1.0,
                }])],
            );
            assert_eq!(g.blocked[g.cell_at(0.1, 0.1).unwrap()], 1);
        }
    }
}
