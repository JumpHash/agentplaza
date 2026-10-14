//! Avoidance, separation, lifts and respawn (nav-agents.ts lines 856-917 inLiftQueue, dropGoal, respawn; 937-1136
//! avoidState, avoid, yieldTo, rejoin, sideCell, lookAhead, separate, pushPoint; 1413-1438 robotList, liftList,
//! syncLifts, exitOf; 1510-1608 updateLift, surfaceY, pathUsesLift, aboard, ride, stepOff, entryAt, board).

// The TS names a lift slot `L`; kept so each port reads against its source line.
#![allow(non_snake_case)]

use super::follow::{clear_stuck, turn_toward};
use super::*;
use crate::nav::height::{BodyMode, new_body};
use crate::nav::lift::{LiftConfig, LiftTicket};
use crate::nav::plan::snap_walk;
use crate::nav::{NAV_LOOKAHEAD, NAV_RADIUS, NAV_SEPARATION};
use crate::navgrid::PortalKind;

/// The lift-table key of a robot's lift id (-1: none).
fn lift_key(lift_id: i64) -> Option<usize> {
    usize::try_from(lift_id).ok()
}

/// The lift a portal joins, when the portal is of the lift kind.
fn lift_of_portal(grid: &NavGrid, portal: usize) -> Option<usize> {
    let port = grid.portals.get(portal)?;
    if matches!(port.kind, PortalKind::Lift) {
        port.lift
    } else {
        None
    }
}

impl NavWorld {
    /// The robot ids in ascending order (the TS robotList).
    fn robot_order(&self) -> Vec<u32> {
        self.robots.keys().copied().collect()
    }

    pub(super) fn in_lift_queue(&self, id: u32) -> bool {
        let Some(r) = self.robots.get(&id) else {
            return false;
        };
        let Some(pts) = r.path.as_ref() else {
            return false;
        };
        if r.riding {
            return false;
        }
        let grid = &self.grid;
        let is_lift = |portal: usize| {
            grid.portals
                .get(portal)
                .is_some_and(|port| matches!(port.kind, PortalKind::Lift))
        };
        for k in r.next..pts.len().min(r.next + 5) {
            let p = &pts[k];
            if (p.x - r.x).hypot(p.z - r.z) > 2.0 {
                continue;
            }
            let here = is_lift(p.portal as usize);
            let next = pts.get(k + 1).is_some_and(|q| is_lift(q.portal as usize));
            if here || next {
                return true;
            }
        }
        false
    }

    /// A falling robot's walk is over: its goal, path and proposal are dropped and its avoidance is forgotten.
    pub(super) fn drop_goal(&mut self, id: u32) {
        if let Some(r) = self.robots.get_mut(&id) {
            if r.goal.is_none() && r.path.is_none() {
                return;
            }
            r.goal = None;
            r.path = None;
            r.next = 0;
            r.proposal = None;
            r.waiting = false;
            r.moving = false;
            r.v = 0.0;
            clear_stuck(r);
        }
        self.avoidance.remove(&id);
    }

    /// Gravity and heights: the robot goes back to its home (its add position, snapped to a walk cell) with a fresh body that fades in.
    pub(super) fn respawn(&mut self, id: u32) {
        self.drop_goal(id);
        let riding_lift = match self.robots.get(&id) {
            Some(r) if r.riding => Some(r.lift_id),
            _ => None,
        };
        if let Some(lift_id) = riding_lift {
            if let Some(L) = lift_key(lift_id).and_then(|k| self.lifts.get_mut(&k)) {
                L.ctrl.cancel(id);
            }
            if let Some(r) = self.robots.get_mut(&id) {
                r.riding = false;
                r.lift_id = -1;
            }
        }
        let (home_x, home_z) = match self.robots.get(&id) {
            Some(r) => (r.home_x, r.home_z),
            None => return,
        };
        let (mut x, mut z) = (home_x, home_z);
        let off_walk = match self.grid.cell_at(x, z) {
            Some(c) => self.grid.walk[c] != 1,
            None => true,
        };
        if off_walk {
            if let Some(snapped) = snap_walk(&self.grid, x, z) {
                let p = self.grid.centre(snapped);
                x = p[0];
                z = p[1];
            }
        }
        let s = self.surfaces_at(x, z);
        let mut body = new_body(s.walk_top.or(s.floor_top).unwrap_or(0.0));
        body.mode = BodyMode::In;
        if let Some(r) = self.robots.get_mut(&id) {
            r.x = x;
            r.z = z;
            r.heading = 0.0;
            r.v = 0.0;
            r.blocked_since = -1.0;
            r.lift_wait = false;
            r.turning = false;
            r.turn_t = 0.0;
            r.from = NavPoint {
                x,
                y: 0.0,
                z,
                portal: -1,
                edge: None,
            };
            r.body = body;
        }
        self.events.push(NavEvent::Respawn(id));
    }

    pub(super) fn avoid_state(&mut self, id: u32) -> &mut AvoidState {
        self.avoidance.entry(id).or_insert_with(|| AvoidState {
            blocker: -1,
            ..AvoidState::default()
        })
    }

    /// Rule 10: avoidance with the look-ahead.
    pub(super) fn avoid(&mut self, dt: f64) {
        for id in self.robot_order() {
            self.avoid_state(id);
            let Some(r) = self.robots.get(&id) else {
                continue;
            };
            let (riding, lifting, rx, rz, rprio) = (
                r.riding,
                r.proposal.is_some_and(|p| p.lift),
                r.x,
                r.z,
                r.priority,
            );
            if riding {
                self.avoid_state(id).hold = None;
                continue;
            }
            if lifting {
                continue;
            }
            // A rider is a fixed obstacle (it never yields), so a robot that is not riding yields to it.
            let others: Vec<u32> = self
                .robot_order()
                .into_iter()
                .filter(|&o| o != id)
                .collect();
            if self.avoid_state(id).yielding {
                let blocker = self.avoid_state(id).blocker;
                let b: Option<(u32, f64, f64)> = u32::try_from(blocker)
                    .ok()
                    .and_then(|bid| self.robots.get(&bid))
                    .map(|o| (o.id, o.x, o.z));
                let yield_t = {
                    let st = self.avoid_state(id);
                    st.yield_t += dt;
                    st.yield_t
                };
                let far = match b {
                    None => true,
                    Some((_, bx, bz)) => (bx - rx).hypot(bz - rz) >= 2.0 * NAV_RADIUS,
                };
                let clear = {
                    let st = self.avoid_state(id);
                    st.clear = if far { st.clear + dt } else { 0.0 };
                    st.clear
                };
                // Two robots yielding to each other: only the higher id keeps yielding, so the lower id proceeds.
                let both_yield = b.is_some_and(|(bid, _, _)| {
                    id < bid && self.avoidance.get(&bid).is_some_and(|s| s.yielding)
                });
                let time_up = yield_t >= YIELD_MAX;
                if clear >= CLEAR_AFTER || b.is_none() || both_yield || time_up {
                    if let (true, Some((bid, _, _))) = (time_up, b) {
                        let until = self.sim_time + NO_YIELD_AFTER;
                        self.avoid_state(id).no_yield.insert(bid, until);
                    }
                    self.rejoin(id);
                    continue;
                }
                let hold = self.avoid_state(id).hold;
                if let Some(h) = hold {
                    self.propose(id, h[0], h[1], false, None);
                    if let Some(r) = self.robots.get_mut(&id) {
                        r.v = 0.0;
                        r.waiting = true;
                        r.moving = false;
                    }
                }
                continue;
            }
            let ahead = self.look_ahead(id);
            let mut rival: Option<u32> = None;
            for &o in &others {
                let Some(ro) = self.robots.get(&o) else {
                    continue;
                };
                if !same_level(self, id, o) {
                    continue;
                }
                let (ox, oz) = (ro.x, ro.z);
                let no_yield_until = self
                    .avoid_state(id)
                    .no_yield
                    .get(&o)
                    .copied()
                    .unwrap_or(f64::NEG_INFINITY);
                if no_yield_until > self.sim_time {
                    continue;
                }
                if (ox - ahead[0]).hypot(oz - ahead[1]) < 2.0 * NAV_RADIUS
                    || (ox - rx).hypot(oz - rz) < 2.0 * NAV_RADIUS
                {
                    rival = Some(o);
                    break;
                }
            }
            if let Some(rv) = rival {
                // A parked robot in the way steps aside instead (and comes back once this one has passed): yielding to it
                // would only stall this robot until it gave up, since a parked robot never moves on.
                let rival_yields = self.avoidance.get(&rv).is_some_and(|a| a.yielding);
                if !rival_yields
                    && self.robots.get(&id).is_some_and(|r| !parked(r))
                    && self.robots.get(&rv).is_some_and(parked)
                    && self.step_aside(rv, id)
                {
                    continue;
                }
                let Some(o) = self.robots.get(&rv) else {
                    continue;
                };
                let (rriding, rpri) = (o.riding, o.priority);
                if rriding || rprio < rpri || (rprio == rpri && id > rv) {
                    self.yield_to(id, rv, &others);
                }
            }
        }
    }

    /// A parked robot `id` steps aside for `walker`: to the nearest free walk cell clear of the walker's remaining path,
    /// as a yield whose saved goal is the robot's stand, so the rejoin walks it back once the walker is clear. False
    /// when there is no such cell (the walker then yields as before).
    pub(super) fn step_aside(&mut self, id: u32, walker: u32) -> bool {
        let Some(w) = self.robots.get(&walker) else {
            return false;
        };
        let remaining: Vec<NavPoint> = w
            .path
            .as_ref()
            .and_then(|p| p.get(w.next..))
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let others: Vec<u32> = self
            .robot_order()
            .into_iter()
            .filter(|&o| o != id)
            .collect();
        let Some(side) = self.side_cell(id, &others, &remaining) else {
            return false;
        };
        let Some(r) = self.robots.get(&id) else {
            return false;
        };
        // Back to its own goal (where it stands, its path being done), or to this spot when it has none.
        let stand = r.goal.unwrap_or(NavGoal {
            x: r.x,
            z: r.z,
            face: r.final_face.or(Some(r.heading)),
        });
        let (rx, ry, rz) = (r.x, r.y, r.z);
        {
            let st = self.avoid_state(id);
            st.yielding = true;
            st.blocker = walker as i64;
            st.clear = 0.0;
            st.yield_t = 0.0;
            st.saved = Some(stand);
            st.hold = None;
        }
        if let Some(r) = self.robots.get_mut(&id) {
            r.goal = Some(NavGoal {
                x: side[0],
                z: side[1],
                face: None,
            });
        }
        self.plan(id, [rx, ry, rz]);
        true
    }

    /// Starts a yield: a side-step (its path is planned to the side cell) or a hold on the spot when there is none.
    pub(super) fn yield_to(&mut self, id: u32, rival: u32, others: &[u32]) {
        let Some(r) = self.robots.get(&id) else {
            return;
        };
        let (rx, ry, rz) = (r.x, r.y, r.z);
        let own: Vec<NavPoint> = r
            .path
            .as_ref()
            .and_then(|p| p.get(r.next..))
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let saved = r.goal.clone();
        // Clear of the rival's way too (the web's rule is the yielder's own path only): a side-step onto the cells the
        // rival walks next leaves both stuck at a chokepoint. In a corridor with no such cell, the yielder backs off
        // along its own path, clear of the rival's alone.
        let theirs: Vec<NavPoint> = self
            .robots
            .get(&rival)
            .and_then(|w| {
                w.path
                    .as_ref()
                    .and_then(|p| p.get(w.next..))
                    .map(|s| s.to_vec())
            })
            .unwrap_or_default();
        let both: Vec<NavPoint> = own.iter().chain(&theirs).copied().collect();
        let side = self
            .side_cell(id, others, &both)
            .or_else(|| self.side_cell(id, others, &theirs));
        {
            let st = self.avoid_state(id);
            st.yielding = true;
            st.blocker = rival as i64;
            st.clear = 0.0;
            st.yield_t = 0.0;
            st.saved = saved;
        }
        match side {
            Some(s) => {
                self.avoid_state(id).hold = None;
                if let Some(r) = self.robots.get_mut(&id) {
                    r.goal = Some(NavGoal {
                        x: s[0],
                        z: s[1],
                        face: None,
                    });
                }
                self.plan(id, [rx, ry, rz]);
            }
            None => {
                self.avoid_state(id).hold = Some([rx, rz]);
                self.propose(id, rx, rz, false, None);
                if let Some(r) = self.robots.get_mut(&id) {
                    r.v = 0.0;
                    r.waiting = true;
                    r.moving = false;
                }
            }
        }
    }

    /// Ends a yield: the saved goal is restored and the plan is made from here.
    pub(super) fn rejoin(&mut self, id: u32) {
        let saved = {
            let st = self.avoid_state(id);
            st.yielding = false;
            st.hold = None;
            st.clear = 0.0;
            st.yield_t = 0.0;
            st.blocker = -1;
            st.saved.take()
        };
        let start = match self.robots.get_mut(&id) {
            Some(r) => {
                r.goal = saved;
                [r.x, r.y, r.z]
            }
            None => return,
        };
        self.plan(id, start);
    }

    /// Rule 10: the nearest walkable cell (rings out to 6 cells) clear of the others and of the path's own cells.
    pub(super) fn side_cell(
        &self,
        id: u32,
        others: &[u32],
        remaining: &[NavPoint],
    ) -> Option<[f64; 2]> {
        let r = self.robots.get(&id)?;
        let grid = &self.grid;
        let c0 = grid.cell_at(r.x, r.z)?;
        let cols = grid.cols as i64;
        let rows = grid.rows as i64;
        let ci = c0 as i64 % cols;
        let cj = c0 as i64 / cols;
        let path_cells: Vec<i64> = remaining
            .iter()
            .filter_map(|p| grid.cell_at(p.x, p.z))
            .map(|c| c as i64)
            .collect();
        let other_pos: Vec<(f64, f64)> = others
            .iter()
            .filter_map(|o| self.robots.get(o))
            .map(|o| (o.x, o.z))
            .collect();
        for ring in 0..=6i64 {
            let mut best: Option<[f64; 2]> = None;
            let mut best_d = f64::INFINITY;
            let mut best_idx: i64 = -1;
            for dj in -ring..=ring {
                for di in -ring..=ring {
                    if di.abs().max(dj.abs()) != ring {
                        continue;
                    }
                    let i = ci + di;
                    let j = cj + dj;
                    if i < 0 || j < 0 || i >= cols || j >= rows {
                        continue;
                    }
                    let idx = j * cols + i;
                    if grid.walk[idx as usize] != 1 {
                        continue;
                    }
                    if path_cells
                        .iter()
                        .any(|&pc| ((pc % cols) - i).abs() <= 1 && ((pc / cols) - j).abs() <= 1)
                    {
                        continue;
                    }
                    let x = grid.x0 + (i as f64 + 0.5) * grid.cell;
                    let z = grid.z0 + (j as f64 + 0.5) * grid.cell;
                    if other_pos
                        .iter()
                        .any(|&(ox, oz)| (ox - x).hypot(oz - z) < 2.0 * NAV_RADIUS)
                    {
                        continue;
                    }
                    let d2 = (x - r.x) * (x - r.x) + (z - r.z) * (z - r.z);
                    if d2 < best_d || (d2 == best_d && idx < best_idx) {
                        best = Some([x, z]);
                        best_d = d2;
                        best_idx = idx;
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        }
        None
    }

    /// The point NAV_LOOKAHEAD units ahead along the robot's remaining path (the last point if the path is shorter).
    pub(super) fn look_ahead(&self, id: u32) -> [f64; 2] {
        let Some(r) = self.robots.get(&id) else {
            return [0.0, 0.0];
        };
        let Some(path) = r.path.as_ref() else {
            return [r.x, r.z];
        };
        let (mut px, mut pz, mut acc) = (r.x, r.z, 0.0);
        for p in path.get(r.next..).unwrap_or(&[]) {
            let seg = (p.x - px).hypot(p.z - pz);
            if seg > 0.0 && acc + seg >= NAV_LOOKAHEAD {
                let t = (NAV_LOOKAHEAD - acc) / seg;
                return [px + (p.x - px) * t, pz + (p.z - pz) * t];
            }
            acc += seg;
            px = p.x;
            pz = p.z;
        }
        [px, pz]
    }

    /// Rule 12: separation of the proposals.
    pub(super) fn separate(&mut self) {
        let ids = self.robot_order();
        // A rider, or a robot on a lift proposal, is fixed: only the other robot of its pair is pushed.
        let fixed = |w: &NavWorld, id: u32| {
            w.robots
                .get(&id)
                .is_some_and(|r| r.riding || r.proposal.is_some_and(|p| p.lift))
        };
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let (a, b) = (ids[i], ids[j]);
                let (fa, fb) = (fixed(self, a), fixed(self, b));
                if fa && fb || !same_level(self, a, b) {
                    continue;
                }
                let pa = self.at(a);
                let pb = self.at(b);
                let d = (pb[0] - pa[0]).hypot(pb[1] - pa[1]);
                if d >= NAV_SEPARATION {
                    continue;
                }
                let (mut nx, mut nz) = (1.0, 0.0);
                if d > 1e-9 {
                    nx = (pb[0] - pa[0]) / d;
                    nz = (pb[1] - pa[1]) / d;
                }
                let push = NAV_SEPARATION - d;
                let push_a = if fa {
                    None
                } else {
                    self.push_point(pa, -nx, -nz, push)
                };
                let push_b = if fb {
                    None
                } else {
                    self.push_point(pb, nx, nz, push)
                };
                if let Some(p) = push_a {
                    self.move_proposal(a, p[0], p[1]);
                }
                if let Some(p) = push_b {
                    self.move_proposal(b, p[0], p[1]);
                }
            }
        }
    }

    /// Rule 12: the full push from p along (dx, dz) when its point is walkable, else half of it, else none.
    pub(super) fn push_point(
        &self,
        p: [f64; 2],
        dx: f64,
        dz: f64,
        amount: f64,
    ) -> Option<[f64; 2]> {
        for f in [1.0, 0.5] {
            let x = p[0] + dx * amount * f;
            let z = p[1] + dz * amount * f;
            if self.walkable_at(x, z) {
                return Some([x, z]);
            }
        }
        None
    }

    /// Rule 9: the lift slots of the grid's lifts, keeping the cabs of lifts that still exist.
    pub(super) fn sync_lifts(&mut self) {
        let mut old = std::mem::take(&mut self.lifts);
        let mut next = BTreeMap::new();
        for def in &self.grid.lifts {
            let kept = old
                .remove(&def.id)
                .filter(|s| s.def.low == def.low && s.def.high == def.high);
            let ctrl = match kept {
                Some(s) => s.ctrl,
                None => LiftController::new(def.low as f32, def.high as f32, LiftConfig::default())
                    .expect("lift shaft"),
            };
            next.insert(
                def.id,
                LiftSlot {
                    def: def.clone(),
                    ctrl,
                },
            );
        }
        self.lifts = next;
    }

    /// The step-off point of a riding robot (path[0]); for any other robot, its own position.
    pub(super) fn exit_of(&self, id: u32) -> NavPoint {
        let Some(r) = self.robots.get(&id) else {
            return NavPoint {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                portal: -1,
                edge: None,
            };
        };
        match r.path.as_ref().and_then(|p| p.first()) {
            Some(first) => first.clone(),
            None => NavPoint {
                x: r.x,
                y: r.y,
                z: r.z,
                portal: -1,
                edge: None,
            },
        }
    }

    pub(super) fn update_lift(&mut self, lift: usize, dt: f64) {
        if let Some(L) = self.lifts.get_mut(&lift) {
            L.ctrl.update(dt as f32);
        }
    }

    pub(super) fn surface_y(&self, lift: usize) -> f64 {
        self.lifts
            .get(&lift)
            .map_or(0.0, |L| L.ctrl.state().cab_y as f64)
    }

    /// Rule 4: does the robot's remaining path use the lift `id` (a portal of that lift, at or after its next point)?
    pub(super) fn path_uses_lift(&self, id: u32, lift: usize) -> bool {
        let Some(r) = self.robots.get(&id) else {
            return false;
        };
        let Some(path) = r.path.as_ref() else {
            return false;
        };
        path.get(r.next..).unwrap_or(&[]).iter().any(|p| {
            self.grid
                .portals
                .get(p.portal as usize)
                .is_some_and(|port| {
                    matches!(port.kind, PortalKind::Lift) && port.lift == Some(lift)
                })
        })
    }

    /// Rule 4: a rider is on the cab until its ticket is arrived with the doors open (then it steps off).
    pub(super) fn aboard(&self, lift: usize, id: u32) -> bool {
        let Some(L) = self.lifts.get(&lift) else {
            return false;
        };
        match L.ctrl.ticket(id) {
            LiftTicket::Riding => true,
            LiftTicket::Arrived => L.ctrl.state().door_t < 1.0,
            _ => false,
        }
    }

    /// Rule 4, riding: the robot proposes the platform centre (lift) and turns toward its step-off point.
    pub(super) fn ride(&mut self, id: u32, dt: f64) {
        let Some(key) = self.robots.get(&id).and_then(|r| lift_key(r.lift_id)) else {
            return;
        };
        let Some((lx, lz)) = self.lifts.get(&key).map(|L| (L.def.x, L.def.z)) else {
            return;
        };
        self.propose(id, lx, lz, true, None);
        let y = self.surface_y(key);
        let exit = self.exit_of(id);
        let target = (exit.x - lx).atan2(exit.z - lz);
        if let Some(r) = self.robots.get_mut(&id) {
            r.y = y;
            r.v = 0.0;
            r.waiting = false;
            r.moving = false;
            r.heading = turn_toward(r.heading, target, r.turn_rate * dt);
        }
    }

    /// Rule 4, step-off: the robot proposes the step-off point (lift), the lift is released, and the robot walks from the
    /// next step. Called from setGrid, the proposal is committed at once.
    pub(super) fn step_off(&mut self, id: u32) {
        let exit = self.exit_of(id);
        if let Some(key) = self.robots.get(&id).and_then(|r| lift_key(r.lift_id)) {
            if let Some(L) = self.lifts.get_mut(&key) {
                L.ctrl.cancel(id);
            }
        }
        if let Some(r) = self.robots.get_mut(&id) {
            r.riding = false;
            r.lift_id = -1;
            r.turning = false;
            r.lift_wait = false;
            r.y = exit.y;
            r.from = NavPoint {
                x: exit.x,
                y: exit.y,
                z: exit.z,
                portal: -1,
                edge: None,
            };
            r.next = 1;
        }
        self.propose(id, exit.x, exit.z, true, None);
    }

    /// Rule 4, entry: the lift that the path enters at point k, and the level the robot needs (see the header).
    /// Returns the lift's key and the need (0 the board, the low stop; 1 the plate, the high stop).
    pub(super) fn entry_at(&self, pts: &[NavPoint], k: usize) -> Option<(usize, usize)> {
        let p = pts.get(k)?;
        let q = pts.get(k + 1)?;
        let grid = &self.grid;
        let front = grid.cell_at(p.x, p.z);
        if let Some(lk) = lift_of_portal(grid, p.portal as usize) {
            if let Some(L) = self.lifts.get(&lk) {
                if front == Some(L.def.front) {
                    return Some((lk, 1));
                }
            }
        }
        if let Some(lk) = lift_of_portal(grid, q.portal as usize) {
            if let Some(L) = self.lifts.get(&lk) {
                // The portal joins the lift's two landings: the front (the platform's cell on the board) and the door (the plate's
                // edge). A robot standing at either boards the cab at its own level: the board side needs the low end, the plate
                // side the high end. The path's next point is the step-off.
                let b = grid.cell_at(q.x, q.z);
                if front == Some(L.def.front) && b == Some(L.def.door) {
                    return Some((lk, 0));
                }
                if front == Some(L.def.door) && b == Some(L.def.front) {
                    return Some((lk, 1));
                }
            }
        }
        None
    }

    /// Rule 4, boarding at point k (the front cell). The robot's path continues from the step-off point.
    pub(super) fn board(&mut self, id: u32, lift: usize, k: usize) {
        let Some(L) = self.lifts.get(&lift) else {
            return;
        };
        let (lx, lz) = (L.def.x, L.def.z);
        let Some(r) = self.robots.get_mut(&id) else {
            return;
        };
        r.riding = true;
        r.lift_id = lift as i64;
        let path = r.path.take().unwrap_or_default();
        r.path = Some(path.get(k + 1..).map(|s| s.to_vec()).unwrap_or_default());
        r.next = 0;
        r.turning = false;
        self.propose(id, lx, lz, true, None);
        let y = self.surface_y(lift);
        if let Some(r) = self.robots.get_mut(&id) {
            r.y = y;
            r.v = 0.0;
            r.waiting = false;
            r.moving = false;
        }
    }
}

/// A robot standing with nothing to do: not riding or waiting for a lift, and either without a goal or with its path
/// walked to the end and stopped (a yielding robot is checked by the caller).
fn parked(r: &RobotState) -> bool {
    // A goal without a path is a plan that failed and will be retried: that robot is not done.
    let done = match (&r.goal, &r.path) {
        (None, _) => true,
        (Some(_), Some(p)) => r.next >= p.len(),
        (Some(_), None) => false,
    };
    !r.riding
        && !r.waiting
        && !r.lift_wait
        && r.proposal.is_none_or(|p| !p.lift)
        && done
        && r.v < WALK_SPEED
}

/// Whether two robots stand at one level: their bodies within a jump of each other. A robot at another height (a rider in
/// the shaft below a waiting robot, one on a plate over the board) is not in the other's way, so neither avoidance nor
/// separation pairs them (the web pairs them, and a robot waiting at a lift door is pushed off it by the rider below).
fn same_level(w: &NavWorld, a: u32, b: u32) -> bool {
    match (w.robots.get(&a), w.robots.get(&b)) {
        (Some(ra), Some(rb)) => (ra.body.y - rb.body.y).abs() <= crate::grid::NAV_JUMP_UP,
        _ => false,
    }
}
