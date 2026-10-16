//! Flow fields over the nav grid: one Dijkstra search from a set of goal cells, so that every robot heading for the same
//! spot (a lift landing, the plaza, a room's door) reads its next step from one table instead of running its own A*.
//!
//! The costs and moves are the planner's (plan.rs): the cost of a cell is the cheapest sum of forward moves from it to
//! a goal, where a move is `edge_cost` (the 8-connected step with its jump or drop extra) or a lift portal at its own
//! cost, and only the first portal at a cell is used, as in `plan`.
//!
//! The search runs in reverse. A settled cell d relaxes each neighbour c that has a forward move c -> d, at cost(d) plus
//! that move's cost. The forward move is read from c's own table (`edge_cost(grid, c, k)`), so the one-way drops are
//! followed in their own direction only: a drop is a move downhill from c, and the reverse of it is never taken.
//!
//! Every move costs more than zero, so the step chosen at a cell always leads to a cell with a strictly lower cost, and
//! following the steps from any reachable cell reaches a goal. A goal has cost 0 and no step. A cell with no path to
//! any goal (an island, or the far side of a one-way drop with no other way back) has cost infinity and no step; the
//! planner gives the same answer through its moves, so no separate component test is needed here.
//!
//! The search is deterministic: a binary heap keyed on (cost, cell), a total order on f64 (costs are finite), and
//! ties keep the first relaxation. Build a field once per layout and goal set, then reuse it for every robot.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::grid::{DIRS8, Grid};
use crate::plan::{Edge, edge_cost};

/// The move a robot makes out of a cell on the way to a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// A walk, jump, step down or drop to the neighbour in direction `k` of DIRS8. `edge` is the jump, step-down or drop
    /// kind (see plan::Edge), None for a walk.
    Move { k: usize, edge: Option<Edge> },
    /// A ride through lift portal `p` (an index into `Grid::portals`) to its other landing.
    Portal { p: usize },
}

/// A flow field over one grid: the cost to the nearest goal and the step to take, for every cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    goals: Vec<usize>,
    cost: Vec<f64>,
    step: Vec<Option<Step>>,
}

impl Field {
    /// The goal cells this field was built toward: the walk cells of the given set, sorted, without repeats.
    pub fn goals(&self) -> &[usize] {
        &self.goals
    }

    /// The cost from `cell` to the nearest goal (0 at a goal, f64::INFINITY where no goal can be reached or `cell` is
    /// out of range).
    pub fn cost(&self, cell: usize) -> f64 {
        self.cost.get(cell).copied().unwrap_or(f64::INFINITY)
    }

    /// The step to take out of `cell`: a walk, jump or drop, or a lift ride. None at a goal, where no goal can be
    /// reached, and for a cell out of range.
    pub fn step(&self, cell: usize) -> Option<Step> {
        self.step.get(cell).copied().flatten()
    }

    /// The direction index (into DIRS8) of the step out of `cell`. None at a goal or where no path exists, and also at a
    /// cell whose step is a lift ride: `step` gives that case, and a path is followed with `step`.
    pub fn next(&self, cell: usize) -> Option<usize> {
        match self.step(cell)? {
            Step::Move { k, .. } => Some(k),
            Step::Portal { .. } => None,
        }
    }
}

/// Builds the flow field toward `goals` (cell indices). Goals outside the grid and goals that are not walk cells are
/// ignored: no robot can stand on them. The result depends only on the grid and the set of goals, not on their order.
pub fn toward(grid: &Grid, goals: &[usize]) -> Field {
    let n = grid.cols * grid.rows;
    let mut goals: Vec<usize> = goals
        .iter()
        .copied()
        .filter(|&g| g < n && grid.walk[g] == 1)
        .collect();
    goals.sort_unstable();
    goals.dedup();

    let opp = opposites();
    let inbound = portal_inbound(grid, n);
    let mut cost = vec![f64::INFINITY; n];
    let mut step: Vec<Option<Step>> = vec![None; n];
    let mut heap = BinaryHeap::new();
    for &g in &goals {
        cost[g] = 0.0;
        heap.push(Node { cost: 0.0, cell: g });
    }

    while let Some(Node { cost: cd, cell: d }) = heap.pop() {
        if cd > cost[d] {
            continue;
        }
        // Walk, jump and drop moves c -> d: the neighbour c in direction k_back of d, whose forward move back to d is
        // in the opposite direction.
        for (k_back, &k) in opp.iter().enumerate() {
            let Some(c) = grid.neighbour(d, k_back) else {
                continue;
            };
            let Some((to, w, edge)) = edge_cost(grid, c, k) else {
                continue;
            };
            if to == d {
                relax(
                    &mut cost,
                    &mut step,
                    &mut heap,
                    c,
                    cd + w,
                    Step::Move { k, edge },
                );
            }
        }
        // Lift rides c -> d: only from the first portal at c, as the planner does.
        for &(c, p) in &inbound[d] {
            relax(
                &mut cost,
                &mut step,
                &mut heap,
                c,
                cd + grid.portals[p].cost,
                Step::Portal { p },
            );
        }
    }
    Field { goals, cost, step }
}

/// Lowers the cost of `c` to `candidate` when that is strictly less, and records the step out of `c` and the heap entry.
fn relax(
    cost: &mut [f64],
    step: &mut [Option<Step>],
    heap: &mut BinaryHeap<Node>,
    c: usize,
    candidate: f64,
    s: Step,
) {
    if candidate < cost[c] {
        cost[c] = candidate;
        step[c] = Some(s);
        heap.push(Node {
            cost: candidate,
            cell: c,
        });
    }
}

/// The open-set entry of the flow search: the smallest (cost, cell) pops first.
#[derive(Clone, Copy)]
struct Node {
    cost: f64,
    cell: usize,
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Node {}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Node {
    /// Reversed, so that the binary max-heap pops the smallest (cost, cell) first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.cell.cmp(&self.cell))
    }
}

/// For each direction k of DIRS8, the index of the opposite direction.
fn opposites() -> [usize; 8] {
    std::array::from_fn(|k| {
        let (di, dj) = DIRS8[k];
        DIRS8
            .iter()
            .position(|&(a, b)| a == -di && b == -dj)
            .expect("DIRS8 is symmetric")
    })
}

/// For each cell d, the lift rides into it as (from, portal): the portal's other end is `from`, and the planner would
/// cross from `from` to `d` (both landings are walk, and the portal is the first one at `from`).
fn portal_inbound(grid: &Grid, n: usize) -> Vec<Vec<(usize, usize)>> {
    let mut inbound = vec![Vec::new(); n];
    for (p, pt) in grid.portals.iter().enumerate() {
        for (from, to) in [(pt.a, pt.b), (pt.b, pt.a)] {
            let usable = from < n && to < n && grid.walk[from] == 1 && grid.walk[to] == 1;
            if usable && grid.portal[from] == p as i32 {
                inbound[to].push((from, p));
            }
        }
    }
    inbound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::build;
    use crate::plan::plan_grid;
    use crate::walk::{LinkKind, Plane, Side, SurfaceKind, WalkLink, WalkSet, WalkSurface};

    /// Costs compare within this tolerance: the planner and the field add the same terms in opposite orders.
    const TOL: f64 = 1e-9;

    fn rect(x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<[f64; 2]> {
        vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]]
    }

    fn surf(id: u32, poly: Vec<[f64; 2]>, height: Plane, sides: Vec<Side>) -> WalkSurface {
        WalkSurface {
            id,
            kind: SurfaceKind::Plate,
            poly,
            height,
            owner: format!("s{id}"),
            sides,
        }
    }

    fn link(from: u32, to: u32, kind: LinkKind, at: [f64; 2]) -> WalkLink {
        WalkLink {
            from,
            to,
            kind,
            via: None,
            at,
        }
    }

    fn open() -> Vec<Side> {
        vec![Side::Open; 4]
    }

    /// The test map (cell size 0.25; surface id = index):
    /// - 0 floor: rect(0, 0, 6, 4) at height 0.
    /// - 1 wall: rect(2, 0, 4, 4) at height 1, walled on every side, standing on the floor. It is an island of its own.
    /// - 2 ledge: rect(0, 4, 6, 8) at height 0.5, joined to the floor by a step (a jump of 0.5) along z = 4.
    /// - 3 high: rect(0, 8, 6, 12) at height 2, reached only by the lift from the floor. Its south side is a drop onto
    ///   the ledge (a fall of 1.5, one way).
    /// - 4 island: rect(8, 8, 10, 10) at height 0, no links.
    fn map() -> WalkSet {
        let mut high_sides = open();
        high_sides[0] = Side::Drop;
        WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 6.0, 4.0), Plane::flat(0.0), open()),
                surf(
                    1,
                    rect(2.0, 0.0, 4.0, 4.0),
                    Plane::flat(1.0),
                    vec![Side::Walled; 4],
                ),
                surf(2, rect(0.0, 4.0, 6.0, 8.0), Plane::flat(0.5), open()),
                surf(3, rect(0.0, 8.0, 6.0, 12.0), Plane::flat(2.0), high_sides),
                surf(4, rect(8.0, 8.0, 10.0, 10.0), Plane::flat(0.0), open()),
            ],
            links: vec![
                link(0, 2, LinkKind::Step, [1.0, 4.0]),
                link(0, 3, LinkKind::Lift, [5.0, 8.0]),
                link(3, 2, LinkKind::Drop, [3.0, 8.0]),
            ],
        }
    }

    fn cell(g: &Grid, x: f64, z: f64) -> usize {
        let c = g.cell_at(x, z).expect("inside the grid");
        assert_eq!(g.walk[c], 1, "({x}, {z}) must be a walk cell for this test");
        c
    }

    /// The cost of the 8-connected A* (plan_grid) between two cells (their centres), or None when it finds no path.
    fn planned(g: &Grid, from: usize, to: usize) -> Option<f64> {
        let a = g.cell_centre(from);
        let b = g.cell_centre(to);
        plan_grid(g, a, b).map(|p| p.cost)
    }

    /// Follows the steps from `start` to a goal, checking that the cost falls at every step and that each step is a
    /// legal move. Returns the cells visited (start first) and the summed cost of the steps.
    fn follow(g: &Grid, f: &Field, start: usize) -> (Vec<usize>, f64) {
        let mut cells = vec![start];
        let mut total = 0.0;
        let mut c = start;
        while f.cost(c) > 0.0 {
            let step = f
                .step(c)
                .expect("a reachable cell that is not a goal has a step");
            let (d, w) = match step {
                Step::Move { k, .. } => {
                    let (d, w, edge) = edge_cost(g, c, k).expect("the step is a legal move");
                    assert_eq!(Some(d), g.neighbour(c, k));
                    assert_eq!(
                        edge,
                        step_edge(step),
                        "the step's edge kind matches the move"
                    );
                    (d, w)
                }
                Step::Portal { p } => {
                    let pt = &g.portals[p];
                    let d = if pt.a == c { pt.b } else { pt.a };
                    (d, pt.cost)
                }
            };
            assert!(f.cost(d) < f.cost(c), "the cost falls from {c} to {d}");
            total += w;
            cells.push(d);
            c = d;
            assert!(cells.len() <= g.cols * g.rows, "the steps do not loop");
        }
        (cells, total)
    }

    fn step_edge(s: Step) -> Option<Edge> {
        match s {
            Step::Move { edge, .. } => edge,
            Step::Portal { .. } => None,
        }
    }

    fn walk_cells(g: &Grid) -> Vec<usize> {
        (0..g.cols * g.rows).filter(|&c| g.walk[c] == 1).collect()
    }

    #[test]
    fn cost_equals_the_planner() {
        // the field cost from several cells to one goal equals the planner path cost
        {
            let g = build(&map());
            let goal = cell(&g, 5.5, 0.5);
            let f = toward(&g, &[goal]);
            let starts = [
                (0.5, 0.5),
                (0.5, 2.0),
                (5.5, 2.0),
                (3.0, 6.0),
                (3.0, 10.0),
                (5.5, 10.5),
                (1.0, 11.0),
            ];
            for (x, z) in starts {
                let s = cell(&g, x, z);
                let want = planned(&g, s, goal)
                    .unwrap_or_else(|| panic!("the planner finds no path from ({x}, {z})"));
                assert!(
                    (f.cost(s) - want).abs() < TOL,
                    "({x}, {z}): field {} vs planner {want}",
                    f.cost(s)
                );
            }
        }

        // the field cost equals the planner for every reachable cell, and is infinite where the planner finds no path
        {
            let g = build(&map());
            let goal = cell(&g, 3.0, 6.0);
            let f = toward(&g, &[goal]);
            for c in walk_cells(&g) {
                match planned(&g, c, goal) {
                    Some(want) => assert!(
                        (f.cost(c) - want).abs() < TOL,
                        "cell {c}: field {} vs planner {want}",
                        f.cost(c)
                    ),
                    None => assert!(
                        f.cost(c).is_infinite(),
                        "cell {c} has no planner path but a finite cost"
                    ),
                }
            }
        }
    }

    #[test]
    fn following_the_field_reaches_the_goal() {
        // following next from every reachable cell reaches the goal with a falling cost, and the summed steps equal the cost
        {
            let g = build(&map());
            let goal = cell(&g, 5.5, 0.5);
            let f = toward(&g, &[goal]);
            let mut reached = 0;
            for c in walk_cells(&g) {
                if !f.cost(c).is_finite() {
                    continue;
                }
                let (cells, total) = follow(&g, &f, c);
                assert_eq!(
                    cells.last(),
                    Some(&goal),
                    "the path from {c} ends at the goal"
                );
                assert!(
                    (total - f.cost(c)).abs() < TOL,
                    "cell {c}: summed steps {total} vs cost {}",
                    f.cost(c)
                );
                reached += 1;
            }
            assert!(reached > 100, "many cells are reachable (got {reached})");
        }

        // one field serves several starts: following it from each start reaches the goal at the planner length
        {
            let g = build(&map());
            let goal = cell(&g, 3.0, 6.0);
            let f = toward(&g, &[goal]);
            for (x, z) in [(0.5, 0.5), (5.5, 2.0), (3.0, 10.0)] {
                let s = cell(&g, x, z);
                let (cells, total) = follow(&g, &f, s);
                assert_eq!(cells.last(), Some(&goal), "from ({x}, {z})");
                assert!((total - planned(&g, s, goal).expect("a path")).abs() < TOL);
            }
        }
    }

    #[test]
    fn an_unreachable_cell_has_infinite_cost_and_no_step() {
        let g = build(&map());
        let goal = cell(&g, 5.5, 0.5);
        let f = toward(&g, &[goal]);
        // The island and the top of the wall plate: walk cells with no link to the goal.
        for (x, z) in [(9.0, 9.0), (3.0, 2.0)] {
            let c = cell(&g, x, z);
            assert!(f.cost(c).is_infinite(), "({x}, {z}) cost {}", f.cost(c));
            assert_eq!(f.next(c), None);
            assert_eq!(f.step(c), None);
            assert_eq!(planned(&g, c, goal), None);
        }
        // Out of range is unreachable too.
        assert!(f.cost(usize::MAX).is_infinite());
        assert_eq!(f.next(usize::MAX), None);
    }

    #[test]
    fn the_drop_is_followed_one_way_and_never_backwards() {
        let g = build(&map());
        // From the high surface, the drop onto the ledge is the short way down to this ledge goal.
        let goal = cell(&g, 3.0, 6.0);
        let f = toward(&g, &[goal]);
        let high = cell(&g, 3.0, 10.0);
        let (cells, total) = follow(&g, &f, high);
        assert_eq!(cells.last(), Some(&goal));
        assert!(cells.len() < 40, "the short way down");
        assert!(
            cells
                .windows(2)
                .any(|w| g.surface[w[0]] == 3 && g.surface[w[1]] == 2),
            "the path crosses from the high surface to the ledge"
        );
        assert!(cells.iter().any(|&c| matches!(
            f.step(c),
            Some(Step::Move {
                edge: Some(Edge::Drop),
                ..
            })
        )));
        assert!((total - planned(&g, high, goal).expect("a path")).abs() < TOL);

        // From the ledge, the drop cannot be climbed back: the way up to the high surface is the lift, and no step on
        // it is a drop.
        let up = cell(&g, 3.0, 10.0);
        let f_up = toward(&g, &[up]);
        let ledge = cell(&g, 3.0, 6.0);
        let (cells, total) = follow(&g, &f_up, ledge);
        assert_eq!(cells.last(), Some(&up));
        // The ledge's own descent to the floor is a StepDown move (a two-way fall above NAV_STEP), but the drop onto the ledge is
        // one way: no step climbs from the ledge straight onto the high surface.
        assert!(
            cells
                .windows(2)
                .all(|w| !(g.surface[w[0]] == 2 && g.surface[w[1]] == 3))
        );
        assert!(
            cells
                .iter()
                .any(|&c| matches!(f_up.step(c), Some(Step::Portal { .. }))),
            "the lift is ridden"
        );
        assert!((total - planned(&g, ledge, up).expect("a path")).abs() < TOL);
    }

    #[test]
    fn the_lift_is_ridden_as_a_portal_step() {
        let g = build(&map());
        let goal = cell(&g, 3.0, 10.0);
        let f = toward(&g, &[goal]);
        let start = cell(&g, 0.5, 0.5);
        let (cells, total) = follow(&g, &f, start);
        assert_eq!(cells.last(), Some(&goal));
        assert!(
            cells
                .iter()
                .any(|&c| matches!(f.step(c), Some(Step::Portal { .. })))
        );
        assert!((total - planned(&g, start, goal).expect("a path")).abs() < TOL);
        // next() has no direction for a ride, so it reports None there.
        let ride = cells
            .iter()
            .copied()
            .find(|&c| matches!(f.step(c), Some(Step::Portal { .. })))
            .unwrap();
        assert_eq!(f.next(ride), None);
    }

    #[test]
    fn building_it_twice_gives_equal_output() {
        let g = build(&map());
        let goal = cell(&g, 5.5, 0.5);
        assert_eq!(toward(&g, &[goal]), toward(&g, &[goal]));
        let other = cell(&g, 3.0, 6.0);
        assert_eq!(
            toward(&g, &[goal, other]),
            toward(&g, &[other, goal]),
            "the order of goals does not matter"
        );
    }

    #[test]
    fn goals_outside_the_grid_and_off_walk_are_ignored() {
        let g = build(&map());
        let goal = cell(&g, 5.5, 0.5);
        // A cell beside the wall plate is blocked (its closed square touches a walled side): no robot can stand there.
        let blocked = g.cell_at(1.9, 2.0).expect("in the grid");
        assert_eq!(g.walk[blocked], 0, "the test cell must be off the walk");
        let f = toward(&g, &[usize::MAX, blocked, goal]);
        assert_eq!(f, toward(&g, &[goal]));
        assert_eq!(f.goals(), &[goal]);
    }
}
