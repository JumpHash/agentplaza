//! A board room's resting slab (the web's `restingSlab` in board-rooms.ts): a plate under the room's
//! top, lowered onto whatever the room stands on, with columns down to the ground or to the support
//! below. The slab is split into cells that are then sampled for their support heights.

use apz_board::nav::drawn::{GROUND_TOL, Support, top_below};

use crate::geometry::{box_geo, cylinder};
use crate::scene::{Mat, Obj};

/// The web's PLATE: the thickness of a resting slab's plate under its top.
pub const PLATE: f32 = 0.2;
/// The web's COLUMN_CELL: the grid cell a slab is first cut into, and the smallest column.
const COLUMN_CELL: f32 = 0.5;

/// A room's slab (the web's `Slab`): its centre and half-sizes, whether it is round, its top, and its role.
pub struct Slab {
    pub cx: f32,
    pub cz: f32,
    pub hw: f32,
    pub hd: f32,
    pub round: bool,
    pub top: f32,
    pub role: String,
}

/// A leaf cell of the split (the web's `leaves` entry): its centre and size, and its highest support.
struct Leaf {
    x: f32,
    z: f32,
    sx: f32,
    sz: f32,
    sup: f32,
}

/// The web's restingSlab: builds a room's plate and its columns as one group. The plate sits under the
/// top, at the lowest of the highest supports (or `top - PLATE` when it is higher); columns hang from it
/// to every leaf whose support is below the plate.
pub fn resting_slab(s: &Slab, support: Option<&Support>, mat: &Mat) -> Obj {
    let g = Obj::group();
    let nx = ((2.0 * s.hw) / COLUMN_CELL).ceil().max(1.0) as usize;
    let nz = ((2.0 * s.hd) / COLUMN_CELL).ceil().max(1.0) as usize;
    let csx = (2.0 * s.hw) / nx as f32;
    let csz = (2.0 * s.hd) / nz as f32;
    let mut leaves: Vec<Leaf> = Vec::new();
    for i in 0..nx {
        for j in 0..nz {
            let x = s.cx - s.hw + (i as f32 + 0.5) * csx;
            let z = s.cz - s.hd + (j as f32 + 0.5) * csz;
            split(s, support, x, z, csx, csz, &mut leaves);
        }
    }
    let highest = leaves.iter().fold(0.0_f32, |m, c| m.max(c.sup));
    let under = highest.min(s.top - PLATE);
    let thick = s.top - under;
    let body = if s.round {
        cylinder(s.hw, s.hw, thick, 48)
    } else {
        box_geo(2.0 * s.hw, thick, 2.0 * s.hd)
    };
    let slab = Obj::mesh(&body, mat);
    slab.set_position(s.cx, under + thick / 2.0, s.cz);
    slab.set_shadows(true, true);
    slab.get().rest = true;
    slab.set_name(&format!("plate {}", s.role));
    g.add(&slab);
    for c in &leaves {
        if f64::from(c.sup) >= f64::from(under) - GROUND_TOL {
            continue;
        }
        let col = Obj::mesh(&box_geo(c.sx - 0.04, under - c.sup, c.sz - 0.04), mat);
        col.set_position(c.x, (under + c.sup) / 2.0, c.z);
        col.set_shadows(true, true);
        col.get().rest = true;
        col.set_name(&format!("column {}", s.role));
        g.add(&col);
    }
    g
}

/// The web's `split` (a recursive inner function in restingSlab): a cell is kept as a leaf when its
/// five sample heights agree within GROUND_TOL or it is at the column cell, and is otherwise cut into
/// four quarters. A round slab drops the cells whose centre lies outside its radius.
fn split(
    s: &Slab,
    support: Option<&Support>,
    x: f32,
    z: f32,
    sx: f32,
    sz: f32,
    leaves: &mut Vec<Leaf>,
) {
    if s.round && (x - s.cx).hypot(z - s.cz) > s.hw {
        return;
    }
    let k = (sx.min(sz) - 0.04) * 0.4;
    let pts = [
        (x, z),
        (x - k, z - k),
        (x + k, z - k),
        (x - k, z + k),
        (x + k, z + k),
    ];
    let heights: Vec<f64> = pts
        .iter()
        .map(|&(px, pz)| top_below(support, f64::from(px), f64::from(pz), f64::from(s.top)))
        .collect();
    let hi = heights.iter().fold(f64::NEG_INFINITY, |m, &h| m.max(h));
    let lo = heights.iter().fold(f64::INFINITY, |m, &h| m.min(h));
    if hi - lo > GROUND_TOL && sx.max(sz) > COLUMN_CELL + 1e-9 {
        let qx = sx / 4.0;
        let qz = sz / 4.0;
        for (dx, dz) in [(-qx, -qz), (qx, -qz), (-qx, qz), (qx, qz)] {
            split(s, support, x + dx, z + dz, sx / 2.0, sz / 2.0, leaves);
        }
        return;
    }
    leaves.push(Leaf {
        x,
        z,
        sx,
        sz,
        sup: hi as f32,
    });
}
