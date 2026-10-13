//! Goals, the path cache, planning and the stuck timer (nav-agents.ts lines 1137-1313 goalInComponent, snapInComponent,
//! search, crowdPenalty, replanStuck, giveUp, watch; and 1448-1509 plan, finalFaceOf, findCached).
//!
//! Identity: the TS compares `r.refPath !== pts` by object identity. Here a path is a Vec, so every place that assigns a
//! new path (plan, replanStuck, exit) clears `ref_path`, and watch compares by value. A cleared ref never equals a
//! fresh path, which is the same outcome as the TS identity check.

use super::follow::{clear_stuck, progress_made, segment_heading};
use super::*;
use crate::grid::NAV_JUMP_UP;
use crate::nav::plan::{find_path, snap_walk};
use crate::nav::NAV_PATH_TTL;

impl NavWorld {
    /// Invariant 4: the goal, or the nearest walk cell of the start's component to it (None: no such cell).
    pub(super) fn goal_in_component(&self, goal: NavGoal, start: [f64; 2]) -> Option<NavGoal> {
        let grid = &self.grid;
        if let Some(gc) = grid.cell_at(goal.x, goal.z) {
            if grid.walk[gc] == 1 {
                return Some(goal);
            }
        }
        let sc = grid.cell_at(start[0], start[1]);
        let mut comp = sc.map_or(-1, |c| self.comp.get(c).copied().unwrap_or(-1));
        if comp < 0 {
            comp = match snap_walk(grid, goal.x, goal.z) {
                Some(near) => self.comp.get(near).copied().unwrap_or(-1),
                None => -1,
            };
        }
        if comp < 0 {
            return None;
        }
        let cell = self.snap_in_component(goal.x, goal.z, comp)?;
        let c = grid.centre(cell);
        Some(NavGoal { x: c[0], z: c[1], ..goal })
    }

    /// The walk cell of `component` nearest (x, z): rings out to SNAP_COMPONENT_RINGS, then the whole grid.
    pub(super) fn snap_in_component(&self, x: f64, z: f64, component: i32) -> Option<usize> {
        let grid = &self.grid;
        let ci = ((x - grid.x0) / grid.cell).floor() as i64;
        let cj = ((z - grid.z0) / grid.cell).floor() as i64;
        let cols = grid.cols as i64;
        let rows = grid.rows as i64;
        let in_comp = |n: usize| grid.walk.get(n) == Some(&1) && self.comp.get(n) == Some(&component);
        let d2 = |n: usize| {
            let dx = grid.x0 + ((n % grid.cols) as f64 + 0.5) * grid.cell - x;
            let dz = grid.z0 + ((n / grid.cols) as f64 + 0.5) * grid.cell - z;
            dx * dx + dz * dz
        };
        for ring in 0..=SNAP_COMPONENT_RINGS {
            let mut best: Option<usize> = None;
            let mut best_d = f64::INFINITY;
            for j in (cj - ring)..=(cj + ring) {
                if j < 0 || j >= rows {
                    continue;
                }
                for i in (ci - ring)..=(ci + ring) {
                    if i < 0 || i >= cols {
                        continue;
                    }
                    if (i - ci).abs().max((j - cj).abs()) != ring {
                        continue;
                    }
                    let n = (j * cols + i) as usize;
                    if !in_comp(n) {
                        continue;
                    }
                    let d = d2(n);
                    if d < best_d || (d == best_d && best.is_some_and(|b| n < b)) {
                        best = Some(n);
                        best_d = d;
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        }
        let mut best: Option<usize> = None;
        let mut best_d = f64::INFINITY;
        for n in 0..grid.walk.len() {
            if !in_comp(n) {
                continue;
            }
            let d = d2(n);
            if d < best_d || (d == best_d && best.is_some_and(|b| n < b)) {
                best = Some(n);
                best_d = d;
            }
        }
        best
    }

    /// Invariant 4: the path between two points, or None when either cannot be snapped or they are in two components.
    pub(super) fn search(&mut self, from: [f64; 2], to: [f64; 2], penalty: Option<&[f64]>) -> Option<NavPath> {
        let grid = &self.grid;
        let s = snap_walk(grid, from[0], from[1])?;
        let g = snap_walk(grid, to[0], to[1])?;
        if self.comp.get(s) != self.comp.get(g) {
            return None;
        }
        // A lift is taken only for a change of level: between two points within NAV_JUMP_UP of each other, the plan first
        // avoids lifts (a ramp or a walk), and only when that finds no way does it allow them.
        if (grid.y[g] - grid.y[s]).abs() <= NAV_JUMP_UP {
            if let Some(level) = find_path(grid, from, to, penalty, true) {
                return Some(level);
            }
        }
        find_path(grid, from, to, penalty, false)
    }

    /// Invariant 2: CROWD_PENALTY on the cells within CROWD_RADIUS of every other robot.
    pub(super) fn crowd_penalty(&self, id: u32) -> Vec<f64> {
        let grid = &self.grid;
        let mut pen = vec![0.0; grid.cost.len()];
        let k = (CROWD_RADIUS / grid.cell).ceil() as i64;
        let cols = grid.cols as i64;
        let rows = grid.rows as i64;
        for o in self.robots.values() {
            if o.id == id {
                continue;
            }
            let Some(c) = grid.cell_at(o.x, o.z) else { continue };
            let ci = c as i64 % cols;
            let cj = c as i64 / cols;
            for dj in -k..=k {
                for di in -k..=k {
                    let i = ci + di;
                    let j = cj + dj;
                    if i < 0 || j < 0 || i >= cols || j >= rows {
                        continue;
                    }
                    let cx = grid.x0 + (i as f64 + 0.5) * grid.cell;
                    let cz = grid.z0 + (j as f64 + 0.5) * grid.cell;
                    if (cx - o.x).hypot(cz - o.z) > CROWD_RADIUS {
                        continue;
                    }
                    if let Some(p) = pen.get_mut((j * cols + i) as usize) {
                        *p += CROWD_PENALTY;
                    }
                }
            }
        }
        pen
    }

    /// Invariant 2: replans from the robot's cell to its goal, with the crowd penalty; false when no plan was found.
    pub(super) fn replan_stuck(&mut self, id: u32) -> bool {
        let Some((goal, x, z)) = self.robots.get(&id).and_then(|r| r.goal.clone().map(|g| (g, r.x, r.z))) else {
            return false;
        };
        let pen = self.crowd_penalty(id);
        let Some(p) = self.search([x, z], [goal.x, goal.z], Some(pen.as_slice())) else {
            return false;
        };
        let face = final_face_of(&goal, &p.points);
        let Some(r) = self.robots.get_mut(&id) else { return false };
        r.path = Some(p.points);
        r.ref_path = None;
        r.next = 0;
        r.from = NavPoint { x: r.x, y: r.y, z: r.z, portal: -1, edge: None };
        r.final_face = face;
        r.proposal = None;
        true
    }

    /// Invariant 2: the goal is cleared, the robot stops, and the handler is told.
    pub(super) fn give_up(&mut self, id: u32) {
        if let Some(r) = self.robots.get_mut(&id) {
            r.goal = None;
            r.path = None;
            r.final_face = None;
            r.proposal = None;
            r.v = 0.0;
            r.moving = false;
            r.waiting = false;
            clear_stuck(r);
        }
        self.events.push(NavEvent::Unreachable(id));
    }

    /// Invariant 2: the stuck timer, after the commit of each step. Progress is the distance to the current waypoint
    /// shrinking by PROGRESS_STEP, or a waypoint crossed. Legitimate waits stop the clock: a finished path (not counted at
    /// all), a lift wait, a yield (invariant 3) and turning in place (up to TURN_WAIT s). Otherwise the robot replans after
    /// REPLAN_AFTER s without progress, retries a failed plan after RETRY_AFTER s, and gives up after GIVE_UP_AFTER s.
    pub(super) fn watch(&mut self, id: u32, dt: f64) {
        let Some(r) = self.robots.get(&id) else { return };
        let ended = matches!(&r.path, Some(p) if r.next >= p.len());
        if r.riding || r.goal.is_none() || ended {
            if let Some(r) = self.robots.get_mut(&id) {
                clear_stuck(r);
            }
            return;
        }
        let lift_wait = r.lift_wait;
        let yielding = self.avoidance.get(&id).is_some_and(|a| a.yielding);
        if yielding || lift_wait || self.in_lift_queue(id) {
            return;
        }
        let Some(r) = self.robots.get_mut(&id) else { return };
        if r.turning && r.v == 0.0 {
            r.turn_t += dt;
            if r.turn_t <= TURN_WAIT {
                return;
            }
        } else {
            r.turn_t = 0.0;
        }
        // The distance to the current waypoint; None when there is no path (the TS's `pts && target`).
        let dist = r.path.as_ref().and_then(|p| p.get(r.next)).map(|t| (t.x - r.x).hypot(t.z - r.z));
        if let Some(dist) = dist {
            if r.ref_path != r.path || r.ref_next != r.next {
                // A new path, or a waypoint reached (its distance has shrunk on the way: that was the progress). Crossing a
                // point the robot already stands on is not progress: a replan starts on the robot's own cell.
                r.ref_path = r.path.clone();
                r.ref_next = r.next;
                r.ref_dist = dist;
                return;
            }
            if dist <= r.ref_dist - PROGRESS_STEP {
                r.ref_dist = dist;
                progress_made(r);
                return;
            }
        }
        r.stuck_t += dt;
        let due = r.stuck_t >= r.replan_at;
        if due {
            let ok = self.replan_stuck(id);
            let delay = if ok { REPLAN_AFTER } else { RETRY_AFTER };
            if let Some(r) = self.robots.get_mut(&id) {
                r.replan_at = r.stuck_t + delay;
            }
        }
        if self.robots.get(&id).is_some_and(|r| r.stuck_t >= GIVE_UP_AFTER) {
            self.give_up(id);
        }
    }

    /// Rule 2: plans the robot's path to its goal from `start` (x, y, z).
    pub(super) fn plan(&mut self, id: u32, start: [f64; 3]) {
        let Some((riding, goal)) = self.robots.get(&id).map(|r| (r.riding, r.goal.clone())) else { return };
        let exit = if riding { Some(self.exit_of(id)) } else { None };
        let pts = match &goal {
            Some(g) => self.find_cached([start[0], start[2]], g.clone()),
            None => None,
        };
        let Some(r) = self.robots.get_mut(&id) else { return };
        r.next = 0;
        r.proposal = None;
        if let (Some(g), Some(pts)) = (&goal, pts) {
            if !pts.is_empty() {
                r.ref_path = None;
                r.path = Some(pts);
                r.from = NavPoint { x: start[0], y: start[1], z: start[2], portal: -1, edge: None };
                r.final_face = final_face_of(g, r.path.as_deref().unwrap_or(&[]));
                return;
            }
        }
        r.final_face = None;
        if let Some(e) = exit {
            r.ref_path = None;
            r.path = Some(vec![e]);
            return;
        }
        r.path = None;
        r.v = 0.0;
        r.moving = false;
    }

    /// Rule 1: a path from the cache, or a new plan cached while fresh (NAV_PATH_TTL).
    pub(super) fn find_cached(&mut self, start: [f64; 2], goal: NavGoal) -> Option<Vec<NavPoint>> {
        let to = [goal.x, goal.z];
        let sc = self.grid.cell_at(start[0], start[1]);
        let gc = self.grid.cell_at(goal.x, goal.z);
        let key = match (sc, gc) {
            (Some(s), Some(g)) => (s, g),
            // Off the grid: planned without the cache.
            _ => return self.search(start, to, None).map(|p| p.points),
        };
        if let Some((path, made)) = self.cache.get(&key) {
            if self.sim_time - *made < NAV_PATH_TTL {
                // The cached path was made from another start in the same cell: its first point becomes this start.
                let mut pts = path.points.clone();
                if let Some(first) = pts.first_mut() {
                    first.x = start[0];
                    first.z = start[1];
                }
                return Some(pts);
            }
        }
        match self.search(start, to, None) {
            Some(p) => {
                let pts = p.points.clone();
                self.cache.insert(key, (p, self.sim_time));
                Some(pts)
            }
            None => {
                self.cache.remove(&key);
                None
            }
        }
    }
}

/// Rule 2: the final face: the goal's, else the heading of the last segment (None for a one-point path).
pub(super) fn final_face_of(goal: &NavGoal, pts: &[NavPoint]) -> Option<f64> {
    if let Some(face) = goal.face {
        return Some(face);
    }
    let n = pts.len();
    if n < 2 {
        return None;
    }
    segment_heading([pts[n - 2].x, pts[n - 2].z], [pts[n - 1].x, pts[n - 1].z])
}
