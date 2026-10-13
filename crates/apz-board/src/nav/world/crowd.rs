//! Avoidance, separation, lifts and respawn (nav-agents.ts lines 856-917 inLiftQueue, dropGoal, respawn; 937-1136
//! avoidState, avoid, yieldTo, rejoin, sideCell, lookAhead, separate, pushPoint; 1413-1438 robotList, liftList,
//! syncLifts, exitOf; 1510-1608 updateLift, surfaceY, pathUsesLift, aboard, ride, stepOff, entryAt, board).

#![allow(unused_variables)]

use super::*;

impl NavWorld {
    pub(super) fn in_lift_queue(&self, id: u32) -> bool {
        todo!("inLiftQueue")
    }

    pub(super) fn drop_goal(&mut self, id: u32) {
        todo!("dropGoal")
    }

    /// Gravity and heights: the robot respawns at home (its walk dropped), and the event is reported.
    pub(super) fn respawn(&mut self, id: u32) {
        todo!("respawn")
    }

    pub(super) fn avoid_state(&mut self, id: u32) -> &mut AvoidState {
        self.avoidance.entry(id).or_insert_with(|| AvoidState { blocker: -1, ..AvoidState::default() })
    }

    /// Rule 10: avoidance with the look-ahead.
    pub(super) fn avoid(&mut self, dt: f64) {
        todo!("avoid")
    }

    pub(super) fn yield_to(&mut self, id: u32, rival: u32, others: &[u32]) {
        todo!("yieldTo")
    }

    pub(super) fn rejoin(&mut self, id: u32) {
        todo!("rejoin")
    }

    pub(super) fn side_cell(&self, id: u32, others: &[u32], remaining: &[NavPoint]) -> Option<[f64; 2]> {
        todo!("sideCell")
    }

    pub(super) fn look_ahead(&self, id: u32) -> [f64; 2] {
        todo!("lookAhead")
    }

    /// Rule 12: separation of the proposals.
    pub(super) fn separate(&mut self) {
        todo!("separate")
    }

    pub(super) fn push_point(&self, p: [f64; 2], dx: f64, dz: f64, amount: f64) -> Option<[f64; 2]> {
        todo!("pushPoint")
    }

    /// Rule 9: the lift slots of the grid's lifts, keeping the cabs of lifts that still exist.
    pub(super) fn sync_lifts(&mut self) {
        todo!("syncLifts")
    }

    /// The step-off point of a riding robot.
    pub(super) fn exit_of(&self, id: u32) -> NavPoint {
        todo!("exitOf")
    }

    pub(super) fn update_lift(&mut self, lift: usize, dt: f64) {
        todo!("updateLift")
    }

    pub(super) fn surface_y(&self, lift: usize) -> f64 {
        todo!("surfaceY")
    }

    pub(super) fn path_uses_lift(&self, id: u32, lift: usize) -> bool {
        todo!("pathUsesLift")
    }

    pub(super) fn aboard(&self, lift: usize, id: u32) -> bool {
        todo!("aboard")
    }

    /// Rule 4: a riding robot rides, or steps off.
    pub(super) fn ride(&mut self, id: u32, dt: f64) {
        todo!("ride")
    }

    pub(super) fn step_off(&mut self, id: u32) {
        todo!("stepOff")
    }

    /// Rule 4: the lift entry at path point k: its lift and the stop the robot needs (0 the board, 1 the plate).
    pub(super) fn entry_at(&self, pts: &[NavPoint], k: usize) -> Option<(usize, usize)> {
        todo!("entryAt")
    }

    /// Rule 4: boarding at path point k.
    pub(super) fn board(&mut self, id: u32, lift: usize, k: usize) {
        todo!("board")
    }
}
