//! Following a path and committing moves (nav-agents.ts lines 254-346 and 390-418: wrapAngle, turnToward,
//! segmentHeading, stepHeightOk, sweepCells, onPortalEdge, heightAt, clearStuck, progressMade; 781-855 vertical,
//! surfacesAt, gridSurfacesAt, audit1; 918-936 propose, moveProposal, at; 1314-1412 commit, moveClear, edgesFrom,
//! groundHeight, settle, walkableAt; 1439-1447 restHeight; 1609-1782 follow, passOwnPoints, segmentOk, cornerFactor,
//! arrivalFactor).

#![allow(unused_variables)]

use super::*;
use crate::grid::{NAV_STEP, move_kind};
use crate::nav::drawn::{drawn_top_at, drawn_top_below};
use crate::nav::height::{BodyMode, Surfaces, grounded, step_body};
use crate::nav::lift::LiftTicket;
use crate::nav::{Edge, NAV_RADIUS, NAV_RESPAWN_OUT};
use std::f64::consts::PI;

/// The robot `id`, which the world holds for the whole step (follow only runs for robots in the world).
fn rob(robots: &mut BTreeMap<u32, RobotState>, id: u32) -> &mut RobotState {
    robots
        .get_mut(&id)
        .expect("follow: the robot is in the world")
}

/// A robot with nothing to walk this step.
fn halt(r: &mut RobotState) {
    r.v = 0.0;
    r.moving = false;
}

/// Wraps an angle to (-pi, pi].
pub(super) fn wrap_angle(a: f64) -> f64 {
    let t = (a + PI) % TAU;
    let u = if t < 0.0 { t + TAU } else { t };
    let r = u - PI;
    if r == -PI { PI } else { r }
}

/// The heading moved toward `desired` by at most `max_step` along the shortest signed angle.
pub(super) fn turn_toward(h: f64, desired: f64, max_step: f64) -> f64 {
    let diff = wrap_angle(desired - h);
    if diff.abs() <= max_step {
        return wrap_angle(desired);
    }
    wrap_angle(h + diff.signum() * max_step)
}

/// The heading from a to b (atan2(dx, dz)), None for a zero-length segment.
pub(super) fn segment_heading(a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let dx = b[0] - a[0];
    let dz = b[1] - a[1];
    if dx.hypot(dz) < 1e-9 {
        return None;
    }
    Some(dx.atan2(dz))
}

/// Rule 5 of the sweep: the step from cell a to the touched cell b is level, or a jump or drop the path emits into b.
pub(super) fn step_height_ok(grid: &NavGrid, a: usize, b: usize, edges: &[EdgeCell]) -> bool {
    match move_kind(grid.y[b] - grid.y[a]) {
        None => false,
        Some(0) => true,
        Some(k) => edges.iter().any(|e| e.cell == b && e.kind == i32::from(k)),
    }
}

/// Invariant 1: the cells a move touches (supercover, Amanatides-Woo, the X axis first at a corner), start excluded;
/// None when the move leaves the grid.
pub(super) fn sweep_cells(
    grid: &NavGrid,
    x0: f64,
    z0: f64,
    x1: f64,
    z1: f64,
) -> Option<Vec<usize>> {
    let mut cells = Vec::new();
    let s = grid.cell;
    let mut i = ((x0 - grid.x0) / s).floor() as i64;
    let mut j = ((z0 - grid.z0) / s).floor() as i64;
    let ti = ((x1 - grid.x0) / s).floor() as i64;
    let tj = ((z1 - grid.z0) / s).floor() as i64;
    let mut mx = (ti - i).abs();
    let mut mz = (tj - j).abs();
    let si: i64 = if ti >= i { 1 } else { -1 };
    let sj: i64 = if tj >= j { 1 } else { -1 };
    let dx = x1 - x0;
    let dz = z1 - z0;
    let tdx = if dx != 0.0 {
        s / dx.abs()
    } else {
        f64::INFINITY
    };
    let tdz = if dz != 0.0 {
        s / dz.abs()
    } else {
        f64::INFINITY
    };
    let mut tx = if dx > 0.0 {
        (grid.x0 + (i + 1) as f64 * s - x0) / dx
    } else if dx < 0.0 {
        (grid.x0 + i as f64 * s - x0) / dx
    } else {
        f64::INFINITY
    };
    let mut tz = if dz > 0.0 {
        (grid.z0 + (j + 1) as f64 * s - z0) / dz
    } else if dz < 0.0 {
        (grid.z0 + j as f64 * s - z0) / dz
    } else {
        f64::INFINITY
    };
    while mx > 0 || mz > 0 {
        if mz == 0 || (mx > 0 && tx <= tz) {
            i += si;
            mx -= 1;
            tx += tdx;
        } else {
            j += sj;
            mz -= 1;
            tz += tdz;
        }
        if i < 0 || j < 0 || i >= grid.cols as i64 || j >= grid.rows as i64 {
            return None;
        }
        let c = j as usize * grid.cols + i as usize;
        if grid.walk[c] != 1 {
            return None;
        }
        cells.push(c);
    }
    Some(cells)
}

/// Invariant 1: the move lies on a portal edge (its ends within PORTAL_EDGE of the two cell centres).
pub(super) fn on_portal_edge(grid: &NavGrid, x0: f64, z0: f64, x1: f64, z1: f64) -> bool {
    // Both ends on the segment between the portal's two cell centres (within PORTAL_EDGE). The web asks for the ends at
    // the two centres, which a walk that crosses in partial steps never meets: its robots stall at the cell boundary of
    // every ramp. A move along the portal's own segment is the transition, whatever its steps.
    let on = |px: f64, pz: f64, a: [f64; 2], b: [f64; 2]| {
        let (vx, vz) = (b[0] - a[0], b[1] - a[1]);
        let len2 = vx * vx + vz * vz;
        let t = if len2 > 0.0 {
            (((px - a[0]) * vx + (pz - a[1]) * vz) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (a[0] + t * vx - px).hypot(a[1] + t * vz - pz) <= PORTAL_EDGE
    };
    grid.portals.iter().any(|p| {
        let a = grid.centre(p.a);
        let b = grid.centre(p.b);
        on(x0, z0, a, b) && on(x1, z1, a, b)
    })
}

/// Rule 3f: the height along a flagged edge toward point `next`, from `from` ([x, y, z]); None when the target is not an
/// edge.
pub(super) fn height_at(
    pts: &[NavPoint],
    next: usize,
    from: [f64; 3],
    x: f64,
    z: f64,
) -> Option<f64> {
    let last = pts.last()?;
    let Some(t) = pts.get(next) else {
        return Some(last.y);
    };
    let dx = t.x - from[0];
    let dz = t.z - from[2];
    let len2 = dx * dx + dz * dz;
    let mut u = 0.0;
    if len2 > 1e-12 {
        u = (((x - from[0]) * dx + (z - from[2]) * dz) / len2)
            .max(0.0)
            .min(1.0);
    }
    Some(from[1] + (t.y - from[1]) * u)
}

/// Invariant 2: the stuck timer back to its start (no goal, a new goal, a finished path, a riding robot).
pub(super) fn clear_stuck(r: &mut RobotState) {
    r.stuck_t = 0.0;
    r.turn_t = 0.0;
    r.replan_at = REPLAN_AFTER;
    r.ref_path = None;
    // The TS's -1 (no reference index) is the largest usize.
    r.ref_next = usize::MAX;
    r.ref_dist = f64::INFINITY;
}

/// Invariant 2: progress (the timer restarts, the next replan is REPLAN_AFTER s away).
pub(super) fn progress_made(r: &mut RobotState) {
    r.stuck_t = 0.0;
    r.replan_at = REPLAN_AFTER;
}

impl NavWorld {
    /// Gravity and heights: steps the robot's body on the surface under its centre, after the commit.
    pub(super) fn vertical(&mut self, id: u32, dt: f64) {
        let (riding, x, z, floats) = {
            let r = &self.robots[&id];
            (r.riding, r.x, r.z, r.floats)
        };
        if riding {
            let r = rob(&mut self.robots, id);
            r.body.mode = BodyMode::Lift;
            r.body.y = r.y;
            r.body.t = 0.0;
            return;
        }
        let s = self.surfaces_at(x, z);
        let r = rob(&mut self.robots, id);
        if r.body.mode == BodyMode::Lift {
            r.body.mode = BodyMode::Ground;
            r.body.t = 0.0;
        }
        step_body(&mut r.body, s, dt, floats);
        r.forced = false;
        let out = r.body.mode == BodyMode::Out;
        if out {
            self.drop_goal(id);
            if self.robots[&id].body.t >= NAV_RESPAWN_OUT {
                self.respawn(id);
            }
        }
    }

    /// The surfaces under (x, z). The footprint's support is the highest surface under its four corners (NAV_RADIUS), taken
    /// over the cells that are not blocked, off the board or cut off (a cliff or a part is not a floor for a corner); the
    /// centre must be a walk cell for the ground (walkTop), else the robot is airborne (walkTop null). Off the board: none.
    pub(super) fn surfaces_at(&self, x: f64, z: f64) -> Surfaces {
        let s = self.grid_surfaces_at(x, z);
        let Some(drawn) = self.drawn.as_ref() else {
            return s;
        };
        if s.walk_top.is_none() {
            return s;
        }
        // The feet stand on the drawn top under the centre (the walk heights bake, drawn-tops.ts); the grid's height where none is drawn.
        let h = drawn_top_at(drawn, x, z);
        if h.is_nan() {
            s
        } else {
            Surfaces {
                walk_top: Some(h),
                floor_top: s.floor_top,
            }
        }
    }

    /// The surfaces from the grid's heights alone (the centre cell and the footprint corners).
    pub(super) fn grid_surfaces_at(&self, x: f64, z: f64) -> Surfaces {
        let grid = &self.grid;
        let Some(c) = grid.cell_at(x, z) else {
            return Surfaces {
                walk_top: None,
                floor_top: None,
            };
        };
        if grid.off[c] == 1 {
            return Surfaces {
                walk_top: None,
                floor_top: None,
            };
        }
        let centre = grid.y[c];
        let mut top = centre;
        for &[dx, dz] in FOOTPRINT.iter() {
            let Some(k) = grid.cell_at(x + dx * NAV_RADIUS, z + dz * NAV_RADIUS) else {
                continue;
            };
            if grid.off[k] == 1 || grid.blocked[k] == 1 {
                continue;
            }
            // A corner over a surface more than NAV_STEP above the centre is a cliff beside the walk, not a step: it is no floor.
            let y = grid.y[k];
            if y - centre <= NAV_STEP {
                top = top.max(y);
            }
        }
        Surfaces {
            walk_top: if grid.walk[c] == 1 { Some(top) } else { None },
            floor_top: Some(top),
        }
    }

    /// The ground audit of one robot (after its body step): a grounded body's feet are on its support, exactly.
    /// Not yet called by the step (the caller is in another part of the world).
    pub(super) fn audit1(&mut self, id: u32) {
        let (riding, x, z, body, floats) = {
            let r = &self.robots[&id];
            (r.riding, r.x, r.z, r.body, r.floats)
        };
        if riding {
            return;
        }
        self.audit.frames += 1;
        if !grounded(&body) {
            return;
        }
        let body_y = body.y;
        // onWall: the centre is off a walk cell, or the feet are above the grid's walk surface there (the grid's own heights).
        let grid_top = self.grid_surfaces_at(x, z).walk_top;
        let on_wall = match grid_top {
            None => true,
            Some(g) => body_y - g > AUDIT_EPS,
        };
        if on_wall {
            self.audit.on_wall += 1;
        }
        if let (Some(d), Some(_)) = (self.drawn.as_ref(), grid_top) {
            if drawn_top_at(d, x, z).is_nan() {
                self.audit.fallback += 1;
            }
        }
        let sup = self.surfaces_at(x, z).walk_top;
        match sup {
            None => self.audit.floating += 1,
            Some(s) if body_y - s > AUDIT_EPS => self.audit.floating += 1,
            Some(s) if s - body_y > AUDIT_EPS => self.audit.sinking += 1,
            _ => {}
        }
        // hover: the feet are more than HOVER_EPS over the exact drawn top under the centre (a floating species flies: exempt).
        let Some(sups) = self.sups.as_ref() else {
            return;
        };
        if floats {
            return;
        }
        let exact = drawn_top_below(sups, x, z, body_y + AUDIT_EPS);
        if body_y - exact <= HOVER_EPS {
            return;
        }
        self.audit.hover += 1;
        let under = match grid_top {
            None => 0.0,
            Some(g) => drawn_top_below(sups, x, z, g),
        };
        let undrawn = match grid_top {
            None => true,
            Some(g) => (under - g).abs() > HOVER_EPS,
        };
        if undrawn {
            self.audit.hover_undrawn += 1;
        }
    }

    pub(super) fn propose(&mut self, id: u32, x: f64, z: f64, lift: bool, walk: Option<Walk>) {
        rob(&mut self.robots, id).proposal = Some(Proposal { x, z, lift, walk });
    }

    /// Rule A: moves the proposal (a push keeps the path progress of a followed proposal), or makes one.
    pub(super) fn move_proposal(&mut self, id: u32, x: f64, z: f64) {
        let p = rob(&mut self.robots, id).proposal.get_or_insert(Proposal {
            x,
            z,
            lift: false,
            walk: None,
        });
        p.x = x;
        p.z = z;
    }

    /// The robot's proposed point this step, else its position.
    pub(super) fn at(&self, id: u32) -> [f64; 2] {
        let r = &self.robots[&id];
        match r.proposal {
            Some(p) => [p.x, p.z],
            None => [r.x, r.z],
        }
    }

    /// Invariant 1: moves the robot to (x, z) when the move is clear (or x only, or z only), else records the block.
    pub(super) fn commit(&mut self, id: u32, x: f64, z: f64, lift: bool, walk: Option<Walk>) {
        let (rx, rz, rnext) = {
            let r = &self.robots[&id];
            (r.x, r.z, r.next)
        };
        let upto = walk.map_or(rnext, |w| w.next);
        let options = [(x, z, true), (x, rz, false), (rx, z, false)];
        let chosen = options
            .into_iter()
            .find(|&(ox, oz, _)| lift || self.move_clear(id, ox, oz, upto));
        let Some((cx, cz, full)) = chosen else {
            let now = self.sim_time;
            let r = rob(&mut self.robots, id);
            halt(r);
            if r.blocked_since < 0.0 {
                r.blocked_since = now;
            }
            // The robot stays on its point, but its height follows the ground (or the edge it now heads for).
            if !lift {
                self.project_walk(id);
                self.ground_height(id);
            }
            return;
        };
        let r = rob(&mut self.robots, id);
        r.blocked_since = -1.0;
        r.x = cx;
        r.z = cz;
        if !lift && full {
            if let Some(w) = walk {
                r.next = w.next;
                r.from = w.from;
            }
        }
        if !lift {
            self.project_walk(id);
            self.ground_height(id);
        }
    }

    /// Invariant 1 (rule 5): the move from the robot to (x1, z1) is clear when its centre line sweeps walk cells and every
    /// height step between consecutive cells is level, or a jump or drop edge the path emits (the edges from the next point
    /// to `upto`); or the move is a portal transition.
    pub(super) fn move_clear(&self, id: u32, x1: f64, z1: f64, upto: usize) -> bool {
        let (rx, rz) = {
            let r = &self.robots[&id];
            (r.x, r.z)
        };
        if let Some(cells) = sweep_cells(&self.grid, rx, rz, x1, z1) {
            let edges = self.edges_from(id, upto);
            let mut prev = self.grid.cell_at(rx, rz);
            let mut ok = true;
            for c in cells {
                if let Some(p) = prev {
                    if !step_height_ok(&self.grid, p, c, &edges) {
                        ok = false;
                        break;
                    }
                }
                prev = Some(c);
            }
            if ok {
                return true;
            }
        }
        on_portal_edge(&self.grid, rx, rz, x1, z1)
    }

    /// The jump and drop edges of r's path from its next point to `upto` (at least the next point): each cell entered.
    /// An edge point off the grid is left out (no cell can match it).
    pub(super) fn edges_from(&self, id: u32, upto: usize) -> Vec<EdgeCell> {
        let r = &self.robots[&id];
        let mut out = Vec::new();
        let Some(pts) = r.path.as_deref() else {
            return out;
        };
        if pts.is_empty() {
            return out;
        }
        let last = (pts.len() - 1).min(r.next.max(upto));
        for k in r.next..=last {
            let p = pts[k];
            let kind = match p.edge {
                Some(Edge::Jump) => 1,
                Some(Edge::Drop) => 2,
                None => continue,
            };
            if let Some(cell) = self.grid.cell_at(p.x, p.z) {
                out.push(EdgeCell { cell, kind });
            }
        }
        out
    }

    /// Rule 3f (commit): a walking robot stands on the ground under its centre (grid.y of its cell). A jump or drop changes
    /// the height at the cell boundary, so the pose never sits between two levels (a robot on the top of a drop is still
    /// on the top's ground). Off the grid, the height follows the path.
    pub(super) fn ground_height(&mut self, id: u32) {
        let (x, z) = {
            let r = &self.robots[&id];
            (r.x, r.z)
        };
        let y = match self.grid.cell_at(x, z) {
            Some(c) => Some(self.grid.y[c]),
            None => {
                let r = &self.robots[&id];
                r.path
                    .as_deref()
                    .and_then(|pts| height_at(pts, r.next, [r.from.x, r.from.y, r.from.z], x, z))
            }
        };
        if let Some(y) = y {
            rob(&mut self.robots, id).y = y;
        }
    }

    /// Commits the robot's proposal (if any), as the step's one commit for the robot.
    pub(super) fn settle(&mut self, id: u32) {
        let p = rob(&mut self.robots, id).proposal.take();
        let (x, z, lift, walk) = match p {
            Some(p) => (p.x, p.z, p.lift, p.walk),
            None => {
                let r = &self.robots[&id];
                (r.x, r.z, false, None)
            }
        };
        self.commit(id, x, z, lift, walk);
    }

    pub(super) fn walkable_at(&self, x: f64, z: f64) -> bool {
        self.grid
            .cell_at(x, z)
            .is_some_and(|c| self.grid.walk[c] == 1)
    }

    pub(super) fn rest_height(&mut self, id: u32) {
        let (x, z) = {
            let r = &self.robots[&id];
            (r.x, r.z)
        };
        if let Some(c) = self.grid.cell_at(x, z) {
            let y = self.grid.y[c];
            rob(&mut self.robots, id).y = y;
        }
    }

    /// Rule 3: follows the path for one step (proposes; does not move the robot).
    pub(super) fn follow(&mut self, id: u32, dt: f64) {
        {
            let r = rob(&mut self.robots, id);
            r.waiting = false;
            r.lift_wait = false;
            r.turning = false;
            r.proposal = None;
        }
        // A request is live until the robot has LEFT: a robot whose path no longer uses that lift (a new goal, a replan, a fall)
        // cancels it here. Waiting, holding and yielding keep it: nothing times a request out (lift-ctrl.ts).
        for lid in self.lifts.keys().copied().collect::<Vec<usize>>() {
            let ticket = self.lifts.get(&lid).map(|s| s.ctrl.ticket(id));
            if matches!(ticket, Some(LiftTicket::Queued | LiftTicket::Boarding))
                && !self.path_uses_lift(id, lid)
            {
                if let Some(s) = self.lifts.get_mut(&lid) {
                    s.ctrl.cancel(id);
                }
            }
        }
        if self.avoidance.get(&id).is_some_and(|a| a.hold.is_some()) {
            let r = rob(&mut self.robots, id);
            halt(r);
            r.waiting = true;
            return;
        }
        if self.robots[&id].path.is_none() {
            halt(rob(&mut self.robots, id));
            return;
        }
        self.pass_own_points(id);
        if !self.segment_ok(id) {
            // Rule 13: the walk to the next point is not on plannable cells of this region; replan from here.
            let (x, y, z) = {
                let r = &self.robots[&id];
                (r.x, r.y, r.z)
            };
            self.plan(id, [x, y, z]);
            if self.robots[&id].path.is_none() {
                halt(rob(&mut self.robots, id));
                return;
            }
            self.pass_own_points(id);
        }
        // A local copy of the path: the walk below reads it while it calls the lift and the board.
        let pts: Vec<NavPoint> = self.robots[&id].path.clone().unwrap_or_default();
        let n = pts.len();

        let (rx, rz, next0, from0, mut heading, turn_rate, speed, final_face, mut v) = {
            let r = &self.robots[&id];
            (
                r.x,
                r.z,
                r.next,
                r.from,
                r.heading,
                r.turn_rate,
                r.speed,
                r.final_face,
                r.v,
            )
        };
        let used = next0 >= n;
        let (mut x, mut z) = (rx, rz);
        let (mut next, mut from) = (next0, from0);

        // 3a, 3b: turn first.
        let desired = match pts.get(next0) {
            Some(t) => (t.x - rx).atan2(t.z - rz),
            None => final_face.unwrap_or(heading),
        };
        heading = turn_toward(heading, desired, turn_rate * dt);
        let turning = wrap_angle(desired - heading).abs() > WALK_ANGLE;

        // 3b, 3c, 3d: turning in place (after the turn, more than WALK_ANGLE off the target) the robot does not move this
        // step (v is 0). Otherwise the speed moves toward its target; a finished path has target 0.
        if turning {
            v = 0.0;
        } else {
            let v_target = if used {
                0.0
            } else {
                speed * self.corner_factor(id) * self.arrival_factor(id)
            };
            if v < v_target {
                v = v_target.min(v + ACCEL * speed * dt);
            } else {
                v = v_target.max(v - DECEL * speed * dt);
            }
        }

        // 3e: walk along the points on local copies; the lift entries are crossed under rule 4.
        let mut budget = v * dt;
        let mut at_lift = false;
        while next < n {
            if let Some((lid, need)) = self.entry_at(&pts, next) {
                let (low, high) = {
                    let s = &self.lifts[&lid];
                    (s.def.low, s.def.high)
                };
                let (from_y, to_y) = if need == 0 { (low, high) } else { (high, low) };
                // LiftController.request takes (id, fromY, toY); the handle takes (fromY, toY, id). This call is the controller's.
                if let Some(s) = self.lifts.get_mut(&lid) {
                    if matches!(s.ctrl.ticket(id), LiftTicket::None) {
                        s.ctrl.request(id, from_y as f32, to_y as f32);
                    }
                }
                let boarding = matches!(
                    self.lifts.get(&lid).map(|s| s.ctrl.ticket(id)),
                    Some(LiftTicket::Boarding)
                );
                if boarding && self.lifts.get_mut(&lid).is_some_and(|s| s.ctrl.board(id)) {
                    let r = rob(&mut self.robots, id);
                    r.heading = heading;
                    r.turning = turning;
                    r.v = v;
                    self.board(id, lid, next);
                    return;
                }
                at_lift = true;
                break;
            }
            let Some(&p) = pts.get(next) else {
                break;
            };
            let dx = p.x - x;
            let dz = p.z - z;
            let d = dx.hypot(dz);
            if d > budget {
                if d > 0.0 {
                    let f = budget / d;
                    x += dx * f;
                    z += dz * f;
                }
                break;
            }
            x = p.x;
            z = p.z;
            budget -= d;
            from = p;
            next += 1;
        }
        let r = rob(&mut self.robots, id);
        r.heading = heading;
        r.turning = turning;
        if at_lift {
            r.waiting = true;
            r.lift_wait = true;
            v = 0.0;
        }
        if next >= n && v < WALK_SPEED {
            v = 0.0;
        }
        r.v = v;
        r.moving = v > WALK_SPEED;
        self.propose(id, x, z, false, Some(Walk { next, from }));
    }

    /// Crosses the points the robot already stands on (no movement), except a lift entry, which the walk decides.
    pub(super) fn pass_own_points(&mut self, id: u32) {
        loop {
            let r = &self.robots[&id];
            let Some(pts) = r.path.as_deref() else {
                return;
            };
            let k = r.next;
            let Some(&p) = pts.get(k) else {
                return;
            };
            if (p.x - r.x).hypot(p.z - r.z) > 1e-9 || self.entry_at(pts, k).is_some() {
                return;
            }
            let r = rob(&mut self.robots, id);
            r.from = p;
            r.next += 1;
        }
    }

    /// Rule 13: the walk from the robot to its next point samples plannable cells at the planner's spacing (cell / 4, end
    /// included) of the robot's region; a portal point may lead to another region. The robot's own sample is skipped when
    /// its cell is not plannable (it is already off the path and may walk back onto it).
    pub(super) fn segment_ok(&self, id: u32) -> bool {
        let r = &self.robots[&id];
        let Some(pts) = r.path.as_deref() else {
            return true;
        };
        let Some(&q) = pts.get(r.next) else {
            return true;
        };
        let grid = &self.grid;
        let c0 = grid.cell_at(r.x, r.z);
        let own = c0.is_some_and(|c| grid.walk[c] == 1);
        let region = c0.map_or(-2, |c| grid.region[c]);
        let dx = q.x - r.x;
        let dz = q.z - r.z;
        let n = (dx.hypot(dz) / (grid.cell / 4.0)).ceil().max(1.0) as usize;
        for k in (if own { 0 } else { 1 })..=n {
            let (x, z) = if k == n {
                (q.x, q.z)
            } else {
                let f = k as f64 / n as f64;
                (r.x + dx * f, r.z + dz * f)
            };
            let Some(c) = grid.cell_at(x, z) else {
                return false;
            };
            if grid.walk[c] != 1 {
                return false;
            }
            if q.portal < 0 && q.edge.is_none() && grid.region[c] != region {
                return false;
            }
        }
        true
    }

    /// Rule 3c: slow-down for the turn ahead.
    pub(super) fn corner_factor(&self, id: u32) -> f64 {
        let r = &self.robots[&id];
        let Some(pts) = r.path.as_deref() else {
            return 1.0;
        };
        let (Some(t), Some(after)) = (pts.get(r.next), pts.get(r.next + 1)) else {
            return 1.0;
        };
        let (Some(cur), Some(nxt)) = (
            segment_heading([r.from.x, r.from.z], [t.x, t.z]),
            segment_heading([t.x, t.z], [after.x, after.z]),
        ) else {
            return 1.0;
        };
        let angle = wrap_angle(nxt - cur).abs();
        1.0 - (CORNER_LOSS * angle) / PI
    }

    /// Rule 3c: easing into the final point.
    pub(super) fn arrival_factor(&self, id: u32) -> f64 {
        let r = &self.robots[&id];
        let Some(pts) = r.path.as_deref() else {
            return 1.0;
        };
        if r.next + 1 != pts.len() {
            return 1.0;
        }
        let Some(t) = pts.get(r.next) else {
            return 1.0;
        };
        let dist = (t.x - r.x).hypot(t.z - r.z);
        (dist / EASE_RADIUS).max(ARRIVE_MIN).min(1.0)
    }
}
