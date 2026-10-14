//! NavWorld's public surface (nav-agents.ts lines 347-389 componentsOf, and 448-780: the constructor, setDrawn, audit,
//! lift, setHome, shove, setGrid, projectWalk, reground, add, remove, has, setGoal, setPriority, step, pose, path,
//! liftState, debugCell).

#![allow(unused_variables)]

use super::follow::clear_stuck;
use super::*;
use crate::nav::drawn::{DRAWN_STEP, bake_drawn_tops};
use crate::nav::height::{BodyMode, body_scale, new_body, step_body};
use crate::nav::plan::snap_walk;
use crate::nav::{NavPose, NavRobotSpec};

/// Invariant 4: the connected component of each walk cell of a grid (nav-agents.ts componentsOf).
pub fn components_of(grid: &NavGrid) -> Vec<i32> {
    let n = grid.walk.len();
    let cols = grid.cols as i64;
    let rows = grid.rows as i64;
    // Portals whose two ends both walk link their cells (both directions).
    let mut links: Vec<Vec<usize>> = vec![Vec::new(); n];
    for p in &grid.portals {
        let (a, b) = (p.a, p.b);
        if grid.walk[a] == 1 && grid.walk[b] == 1 {
            links[a].push(b);
            links[b].push(a);
        }
    }
    const STEPS: [(i64, i64); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
    ];
    let mut comp = vec![-1i32; n];
    let mut next = 0i32;
    let mut stack: Vec<usize> = Vec::new();
    for s in 0..n {
        if grid.walk[s] != 1 || comp[s] >= 0 {
            continue;
        }
        comp[s] = next;
        stack.push(s);
        while let Some(c) = stack.pop() {
            let i = c as i64 % cols;
            let j = c as i64 / cols;
            let mut nbrs: Vec<usize> = links[c].clone();
            for &(di, dj) in STEPS.iter() {
                let ni = i + di;
                let nj = j + dj;
                if ni < 0 || nj < 0 || ni >= cols || nj >= rows {
                    continue;
                }
                // A diagonal step needs both orthogonal cells walkable.
                if di != 0
                    && dj != 0
                    && (grid.walk[(j * cols + ni) as usize] != 1
                        || grid.walk[(nj * cols + i) as usize] != 1)
                {
                    continue;
                }
                nbrs.push((nj * cols + ni) as usize);
            }
            for m in nbrs {
                if grid.walk[m] == 1 && comp[m] < 0 {
                    comp[m] = next;
                    stack.push(m);
                }
            }
        }
        next += 1;
    }
    comp
}

impl NavWorld {
    pub fn new(grid: NavGrid) -> NavWorld {
        let comp = components_of(&grid);
        let mut world = NavWorld {
            grid,
            comp,
            avoidance: BTreeMap::new(),
            robots: BTreeMap::new(),
            lifts: BTreeMap::new(),
            cache: HashMap::new(),
            sim_time: 0.0,
            events: Vec::new(),
            audit: NavAudit::default(),
            sups: None,
            drawn: None,
        };
        world.sync_lifts();
        world
    }

    /// The drawn supports the walk heights bake from (None: the grid's heights, no hover audit).
    pub fn set_drawn(&mut self, sups: Option<Vec<Support>>) {
        self.drawn = sups
            .as_ref()
            .map(|s| bake_drawn_tops(&self.grid, s, DRAWN_STEP));
        self.sups = sups;
    }

    pub fn time(&self) -> f64 {
        self.sim_time
    }

    /// The robots reported unreachable or respawned since the last call (the TS's handlers).
    pub fn take_events(&mut self) -> Vec<NavEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn audit(&self) -> NavAudit {
        self.audit
    }

    /// A lift's cab (the TS's LiftHandle), None for an unknown lift.
    pub fn lift_mut(&mut self, id: usize) -> Option<&mut LiftController> {
        self.lifts.get_mut(&id).map(|l| &mut l.ctrl)
    }

    pub fn set_home(&mut self, id: u32, x: f64, z: f64) {
        if let Some(r) = self.robots.get_mut(&id) {
            r.home_x = x;
            r.home_z = z;
        }
    }

    pub fn shove(&mut self, id: u32, x: f64, z: f64) {
        if let Some(r) = self.robots.get_mut(&id) {
            r.x = x;
            r.z = z;
            r.forced = true;
        }
    }

    pub fn set_grid(&mut self, grid: NavGrid) {
        self.grid = grid;
        self.comp = components_of(&self.grid);
        self.drawn = self
            .sups
            .as_ref()
            .map(|s| bake_drawn_tops(&self.grid, s, DRAWN_STEP));
        self.cache.clear();
        self.sync_lifts();
        for id in self.robot_ids() {
            let Some((riding, lift_id)) = self.robots.get(&id).map(|r| (r.riding, r.lift_id))
            else {
                continue;
            };
            if riding {
                if self.lift_key(lift_id).is_some() {
                    let e = self.exit_of(id);
                    self.plan(id, [e.x, e.y, e.z]);
                    if !self.robot_has_path(id) {
                        self.rest_height(id);
                    }
                    continue;
                }
                self.step_off(id);
                self.settle(id);
            }
            // A robot standing off a walk cell of the new grid goes to the nearest one, and its body follows the surface there.
            self.project_walk(id);
            if self.robot_has_goal(id) {
                if let Some(p) = self.robot_pos(id) {
                    self.plan(id, p);
                }
            }
            if !self.robot_has_path(id) {
                self.rest_height(id);
            }
            self.reground(id);
        }
    }

    /// The walk invariant (the commit point and every grid change): a robot's centre is on a walk cell of the current grid. A
    /// centre that is not (a rebuilt grid, a push that landed off the walk) moves to the nearest walk cell at once.
    pub(super) fn project_walk(&mut self, id: u32) {
        let Some((x, z, forced)) = self.robots.get(&id).map(|r| (r.x, r.z, r.forced)) else {
            return;
        };
        if forced {
            return;
        }
        if let Some(c) = self.grid.cell_at(x, z) {
            if self.grid.walk[c] == 1 {
                return;
            }
        }
        let Some(snapped) = snap_walk(&self.grid, x, z) else {
            return;
        };
        let p = self.grid.centre(snapped);
        if let Some(r) = self.robots.get_mut(&id) {
            r.x = p[0];
            r.z = p[1];
        }
        self.audit.projections += 1;
    }

    /// The body follows the surface under the robot now: the body rules with no time passed (a step, not a fade or a fall).
    pub(super) fn reground(&mut self, id: u32) {
        let Some((riding, x, z, floats)) =
            self.robots.get(&id).map(|r| (r.riding, r.x, r.z, r.floats))
        else {
            return;
        };
        if riding {
            return;
        }
        if let Some(r) = self.robots.get_mut(&id) {
            if matches!(r.body.mode, BodyMode::Lift) {
                r.body.mode = BodyMode::Ground;
                r.body.t = 0.0;
            }
        }
        let surfaces = self.surfaces_at(x, z);
        if let Some(r) = self.robots.get_mut(&id) {
            step_body(&mut r.body, surfaces, 0.0, floats);
        }
    }

    pub fn add(&mut self, spec: NavRobotSpec) {
        if self.robots.contains_key(&spec.id) {
            self.remove(spec.id);
        }
        let mut x = spec.x;
        let mut z = spec.z;
        let mut cell = self.grid.cell_at(x, z);
        let r = RobotState {
            id: spec.id,
            x: spec.x,
            y: 0.0,
            z: spec.z,
            heading: 0.0,
            speed: spec.speed,
            turn_rate: spec.turn_rate,
            priority: spec.priority,
            v: 0.0,
            goal: None,
            path: None,
            next: 0,
            from: NavPoint {
                x: spec.x,
                y: 0.0,
                z: spec.z,
                portal: -1,
                edge: None,
            },
            final_face: None,
            riding: false,
            lift_id: -1,
            waiting: false,
            moving: false,
            proposal: None,
            blocked_since: -1.0,
            turning: false,
            turn_t: 0.0,
            lift_wait: false,
            stuck_t: 0.0,
            replan_at: REPLAN_AFTER,
            ref_path: None,
            ref_next: usize::MAX,
            ref_dist: f64::INFINITY,
            body: new_body(0.0),
            home_x: spec.x,
            home_z: spec.z,
            floats: spec.floats,
            forced: false,
        };
        self.robots.insert(spec.id, r);
        if cell.map_or(true, |c| self.grid.walk[c] != 1) {
            if let Some(snapped) = snap_walk(&self.grid, x, z) {
                let c = self.grid.centre(snapped);
                x = c[0];
                z = c[1];
                cell = Some(snapped);
                // A spawn is a placement: it lands on the snapped point at once (lift = true: no sweep).
                self.commit(spec.id, x, z, true, None);
            }
        }
        let y = match cell {
            Some(c) => self.grid.y[c],
            None => 0.0,
        };
        if let Some(r) = self.robots.get_mut(&spec.id) {
            r.y = y;
            r.body = new_body(y);
            r.from = NavPoint {
                x: r.x,
                y,
                z: r.z,
                portal: -1,
                edge: None,
            };
        }
    }

    pub fn remove(&mut self, id: u32) {
        let Some(riding) = self.robots.get(&id).map(|r| r.riding) else {
            return;
        };
        if riding {
            for cab in self.lifts.values_mut() {
                cab.ctrl.cancel(id);
            }
        }
        self.robots.remove(&id);
    }

    pub fn has(&self, id: u32) -> bool {
        self.robots.contains_key(&id)
    }

    /// Unknown ids are a no-op. A goal with no walk cell in reach is given up at once (onUnreachable), the robot stays.
    pub fn set_goal(&mut self, id: u32, goal: NavGoal) {
        let Some(riding) = self.robots.get(&id).map(|r| r.riding) else {
            return;
        };
        if let Some(st) = self.avoidance.get_mut(&id) {
            if st.yielding {
                st.yielding = false;
                st.hold = None;
                st.saved = None;
                st.blocker = -1;
                st.yield_t = 0.0;
                st.clear = 0.0;
            }
        }
        let start = if riding {
            let e = self.exit_of(id);
            [e.x, e.y, e.z]
        } else {
            self.robot_pos(id).unwrap_or([0.0; 3])
        };
        match self.goal_in_component(goal, [start[0], start[2]]) {
            None => {
                if let Some(r) = self.robots.get_mut(&id) {
                    r.goal = None;
                    clear_stuck(r);
                }
                self.plan(id, start);
                if let Some(r) = self.robots.get_mut(&id) {
                    r.waiting = false;
                }
                if !riding {
                    self.rest_height(id);
                }
                self.events.push(NavEvent::Unreachable(id));
            }
            Some(placed) => {
                if let Some(r) = self.robots.get_mut(&id) {
                    r.goal = Some(placed);
                    clear_stuck(r);
                }
                self.plan(id, start);
                if !self.robot_has_path(id) {
                    if let Some(r) = self.robots.get_mut(&id) {
                        r.replan_at = RETRY_AFTER;
                    }
                    self.rest_height(id);
                }
            }
        }
    }

    pub fn set_priority(&mut self, id: u32, priority: u32) {
        if let Some(r) = self.robots.get_mut(&id) {
            r.priority = priority;
        }
    }

    pub fn step(&mut self, dt: f64) {
        let h = if dt > 0.0 { dt.min(MAX_DT) } else { 0.0 };
        self.sim_time += h;
        for lift in self.lift_ids() {
            self.update_lift(lift, h);
        }
        let ids = self.robot_ids();
        for &id in &ids {
            let Some((riding, lift_id)) = self.robots.get(&id).map(|r| (r.riding, r.lift_id))
            else {
                continue;
            };
            if riding {
                match self.lift_key(lift_id) {
                    Some(lift) if self.aboard(lift, id) => self.ride(id, h),
                    Some(lift) => {
                        if let Some(cab) = self.lifts.get_mut(&lift) {
                            cab.ctrl.alight(id);
                        }
                        self.step_off(id);
                    }
                    None => self.step_off(id),
                }
                continue;
            }
            self.follow(id, h);
        }
        self.avoid(h);
        self.separate();
        for &id in &ids {
            self.settle(id);
        }
        for &id in &ids {
            self.watch(id, h);
        }
        for &id in &ids {
            self.vertical(id, h);
        }
        for &id in &ids {
            self.audit1(id);
        }
    }

    /// The pose of robot `id`. An unknown id returns a STUB, not a robot: the origin, not moving, not on a lift. Check has(id)
    /// before reading a pose that must be real.
    pub fn pose(&self, id: u32) -> NavPose {
        let Some(r) = self.robots.get(&id) else {
            return NavPose {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                heading: 0.0,
                moving: false,
                waiting: false,
                on_lift: false,
                body: BodyMode::Ground,
                scale: 1.0,
                yielding: false,
            };
        };
        NavPose {
            x: r.x,
            y: r.body.y,
            z: r.z,
            heading: r.heading,
            moving: r.moving,
            waiting: r.waiting,
            on_lift: r.riding,
            body: r.body.mode,
            scale: body_scale(&r.body),
            yielding: self.avoidance.get(&id).map_or(false, |a| a.yielding),
        }
    }

    /// The robot's remaining path (from its next point), None without one.
    pub fn path(&self, id: u32) -> Option<Vec<NavPoint>> {
        let r = self.robots.get(&id)?;
        let pts = r.path.as_ref()?;
        Some(pts[r.next.min(pts.len())..].to_vec())
    }

    /// A lift's level (0 at the bottom stop, 1 at the top) and its first rider (-1: none).
    pub fn lift_state(&self, lift: usize) -> (f64, i64) {
        let Some(slot) = self.lifts.get(&lift) else {
            return (0.0, -1);
        };
        let s = slot.ctrl.state();
        let level = (s.cab_y as f64 - slot.def.low) / (slot.def.high - slot.def.low).max(1e-6);
        let occupant = s.boarding.first().map_or(-1, |&id| id as i64);
        (level, occupant)
    }

    pub fn debug_cell(&self, x: f64, z: f64) -> Option<DebugCell> {
        let c = self.grid.cell_at(x, z)?;
        Some(DebugCell {
            cell: c,
            walk: self.grid.walk[c] == 1,
            blocked: self.grid.blocked[c] == 1,
            inflated: self.grid.inflated[c] == 1,
            region: self.grid.region[c],
            component: self.comp.get(c).copied().unwrap_or(-1),
            y: self.grid.y[c],
        })
    }

    /// The grid the robots walk.
    pub fn grid(&self) -> &NavGrid {
        &self.grid
    }

    /// The robots' ids, ascending.
    pub fn robot_ids(&self) -> Vec<u32> {
        self.robots.keys().copied().collect()
    }

    /// The lifts' ids, ascending.
    pub fn lift_ids(&self) -> Vec<usize> {
        self.lifts.keys().copied().collect()
    }

    /// The key of lift `id` when such a lift exists (a robot's lift_id is -1 when it has none).
    fn lift_key(&self, id: i64) -> Option<usize> {
        usize::try_from(id)
            .ok()
            .filter(|l| self.lifts.contains_key(l))
    }

    fn robot_has_path(&self, id: u32) -> bool {
        self.robots.get(&id).map_or(false, |r| r.path.is_some())
    }

    fn robot_has_goal(&self, id: u32) -> bool {
        self.robots.get(&id).map_or(false, |r| r.goal.is_some())
    }

    /// A robot's (x, y, z) as the TS's {x, y, z} start point.
    fn robot_pos(&self, id: u32) -> Option<[f64; 3]> {
        self.robots.get(&id).map(|r| [r.x, r.y, r.z])
    }
}
