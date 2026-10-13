//! NavWorld's public surface (nav-agents.ts lines 347-389 componentsOf, and 448-780: the constructor, setDrawn, audit,
//! lift, setHome, shove, setGrid, projectWalk, reground, add, remove, has, setGoal, setPriority, step, pose, path,
//! liftState, debugCell).

#![allow(unused_variables)]

use super::*;
use crate::nav::{NavPose, NavRobotSpec};

/// Invariant 4: the connected component of each walk cell of a grid (nav-agents.ts componentsOf).
pub fn components_of(grid: &NavGrid) -> Vec<i32> {
    todo!("componentsOf")
}

impl NavWorld {
    pub fn new(grid: NavGrid) -> NavWorld {
        todo!("constructor")
    }

    /// The drawn supports the walk heights bake from (None: the grid's heights, no hover audit).
    pub fn set_drawn(&mut self, sups: Option<Vec<Support>>) {
        todo!("setDrawn")
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
        todo!("setHome")
    }

    pub fn shove(&mut self, id: u32, x: f64, z: f64) {
        todo!("shove")
    }

    pub fn set_grid(&mut self, grid: NavGrid) {
        todo!("setGrid")
    }

    pub(super) fn project_walk(&mut self, id: u32) {
        todo!("projectWalk")
    }

    pub(super) fn reground(&mut self, id: u32) {
        todo!("reground")
    }

    pub fn add(&mut self, spec: NavRobotSpec) {
        todo!("add")
    }

    pub fn remove(&mut self, id: u32) {
        todo!("remove")
    }

    pub fn has(&self, id: u32) -> bool {
        self.robots.contains_key(&id)
    }

    pub fn set_goal(&mut self, id: u32, goal: NavGoal) {
        todo!("setGoal")
    }

    pub fn set_priority(&mut self, id: u32, priority: u32) {
        todo!("setPriority")
    }

    pub fn step(&mut self, dt: f64) {
        todo!("step")
    }

    pub fn pose(&self, id: u32) -> NavPose {
        todo!("pose")
    }

    /// The robot's remaining path (from its next point), None without one.
    pub fn path(&self, id: u32) -> Option<Vec<NavPoint>> {
        todo!("path")
    }

    /// A lift's level (0 at the bottom stop, 1 at the top) and its first rider (-1: none).
    pub fn lift_state(&self, lift: usize) -> (f64, i64) {
        todo!("liftState")
    }

    pub fn debug_cell(&self, x: f64, z: f64) -> Option<DebugCell> {
        todo!("debugCell")
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
}
