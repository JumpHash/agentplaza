//! The layout's planning grid: the board-only nav grid (navgrid.rs with no terraces, as board-layout.ts layoutAtScale
//! builds grid0 from navBoardOf), as the `Grid` the aisle planning reads (its walk, blocked, inflated and component).

use crate::grid::{Grid, NAV_COST_OPEN};
use crate::navgrid::{self, NavBoard, NavBox, NavPlate};

/// The board-only grid of a board of `size` (x, z) with its PCB at `pcb_top`, its part boxes `(lo, hi)` and the
/// candidate plates `(rect [x, z, w, d], top)` (region ids 1, 2, ... in order).
pub fn board_grid(
    size: [f64; 2],
    pcb_top: f64,
    boxes: &[([f64; 3], [f64; 3])],
    plates: &[([f64; 4], f64)],
) -> Grid {
    let board = NavBoard {
        size: [size[0], 0.0, size[1]],
        pcb_top,
        boxes: boxes.iter().map(|&(lo, hi)| NavBox { lo, hi }).collect(),
        traces: Vec::new(),
        plates: plates
            .iter()
            .enumerate()
            .map(|(k, &([x0, z0, w, d], y))| NavPlate {
                id: k as i32 + 1,
                x0,
                z0,
                w,
                d,
                y,
                landing: None,
            })
            .collect(),
    };
    let g = navgrid::build(&board, &[]);
    let n = g.cols * g.rows;
    let main = navgrid::main_component(&g);
    Grid {
        cell: g.cell,
        cols: g.cols,
        rows: g.rows,
        x0: g.x0,
        z0: g.z0,
        walk: g.walk,
        blocked: g.blocked,
        inflated: g.inflated,
        surface: vec![-1; n],
        y: g.y,
        cost: vec![NAV_COST_OPEN; n],
        component: g.component,
        portal: vec![-1; n],
        moves: vec![0; n],
        seam: vec![0; n],
        drops: Vec::new(),
        portals: Vec::new(),
        main,
        surfaces: Vec::new(),
        fine: Default::default(),
    }
}
