//! The navigation debug hooks (`window.__apzBots`, `__apzNavCell`, `__apzNavAudit`, `__apzLifts`, `__apzShove` on the web):
//! the JSON they read from the stage's floor and its board's nav world. Read-only, except `shove` (a robot's position)
//! and `lifts_json` (which takes the lift controllers mutably, as their state is read through `lift_mut`).

use serde_json::{json, Value};

use crate::stage::Stage;

/// A Debug name in lowercase (a nav body or a lift phase: `BodyMode::Ground` is "ground").
fn debug_name<T: std::fmt::Debug>(value: T) -> String {
    format!("{value:?}").to_lowercase()
}

impl Stage {
    /// The floor's agents' robots as the web's `__apzBots` reads them. The three.js-only fields (activity, goalKey, work,
    /// navStart, navGoal) are null here.
    pub fn bots_json(&self) -> String {
        let nav = self.floor.nav.as_ref();
        let bots: Vec<Value> = self
            .floor
            .bots
            .iter()
            .map(|bot| {
                let cid = bot.agent.cid;
                let pose = nav.and_then(|n| n.pose(cid));
                let path = nav.and_then(|n| n.robot_of(cid).and_then(|id| n.world.path(id)));
                json!({
                    "cid": cid,
                    "role": bot.agent.role,
                    "tier": bot.agent.tier,
                    "status": bot.agent.status,
                    "x": bot.x,
                    "y": bot.py,
                    "z": bot.y,
                    "heading": bot.heading,
                    "moving": bot.moving,
                    "onLift": pose.is_some_and(|p| p.on_lift),
                    "waiting": pose.is_some_and(|p| p.waiting),
                    "body": pose.map_or_else(|| "ground".to_string(), |p| debug_name(p.body)),
                    "navPath": path.map(|points| points.iter().map(|p| [p.x, p.z]).collect::<Vec<_>>()),
                    "activity": null,
                    "goalKey": null,
                    "work": null,
                    "navStart": null,
                    "navGoal": null,
                })
            })
            .collect();
        Value::Array(bots).to_string()
    }

    /// One cell of the board's nav grid at (x, z) as `__apzNavCell` reads it, or "null" off the grid or without a board.
    pub fn nav_cell_json(&self, x: f64, z: f64) -> String {
        match self.floor.nav.as_ref().and_then(|n| n.world.debug_cell(x, z)) {
            Some(c) => json!({
                "cell": c.cell,
                "walk": c.walk,
                "blocked": c.blocked,
                "inflated": c.inflated,
                "region": c.region,
                "component": c.component,
                "y": c.y,
            })
            .to_string(),
            None => "null".to_string(),
        }
    }

    /// The board's ground audit as `__apzNavAudit` reads it, or "null" without a board.
    pub fn nav_audit_json(&self) -> String {
        match self.floor.nav.as_ref() {
            Some(n) => {
                let a = n.world.audit();
                json!({
                    "frames": a.frames,
                    "floating": a.floating,
                    "sinking": a.sinking,
                    "onWall": a.on_wall,
                    "hover": a.hover,
                    "hoverUndrawn": a.hover_undrawn,
                    "fallback": a.fallback,
                    "projections": a.projections,
                })
                .to_string()
            }
            None => "null".to_string(),
        }
    }

    /// The board's lifts as `__apzLifts` reads them; "[]" without a board.
    pub fn lifts_json(&mut self) -> String {
        let Some(nav) = self.floor.nav.as_mut() else { return "[]".to_string() };
        let ids = nav.world.lift_ids();
        let lifts: Vec<Value> = ids
            .into_iter()
            .filter_map(|id| {
                let (level, occupant) = nav.world.lift_state(id);
                let cab = nav.world.lift_mut(id)?.state();
                Some(json!({
                    "id": id,
                    "level": level,
                    "occupant": occupant,
                    "cabY": cab.cab_y,
                    "doorsOpen": cab.doors_open,
                    "doorT": cab.door_t,
                    "phase": debug_name(cab.phase),
                    "stop": cab.stop,
                    "calls": cab.calls,
                }))
            })
            .collect();
        Value::Array(lifts).to_string()
    }

    /// Sends the `n` robots nearest lift `index` on a trip through it (`__apzLiftRide`), as JSON: `{ok, goal: {x, z},
    /// robots}` (the cids sent), or `{ok: false, reason}`.
    pub fn lift_ride_json(&mut self, index: i64, n: i64) -> String {
        let n = usize::try_from(n.max(0)).unwrap_or(0);
        let outcome = match usize::try_from(index) {
            Ok(i) => self.floor.lift_ride(i, n),
            Err(_) => Err("no nav or no such lift".to_string()),
        };
        match outcome {
            Ok(ride) => json!({ "ok": true, "goal": { "x": ride.goal.0, "z": ride.goal.1 }, "robots": ride.robots }).to_string(),
            Err(reason) => json!({ "ok": false, "reason": reason }).to_string(),
        }
    }

    /// Moves an agent's robot to (x, z) on the board (`__apzShove`). False without a board or a robot for the agent.
    pub fn shove(&mut self, cid: u64, x: f64, z: f64) -> bool {
        let Some(nav) = self.floor.nav.as_mut() else { return false };
        let Some(id) = nav.robot_of(cid) else { return false };
        nav.world.shove(id, x, z);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::stage::Stage;

    #[test]
    fn no_board_gives_empty_hooks() {
        let mut stage = Stage::default();
        assert_eq!(stage.bots_json(), "[]");
        assert_eq!(stage.nav_cell_json(0.0, 0.0), "null");
        assert_eq!(stage.nav_audit_json(), "null");
        assert_eq!(stage.lifts_json(), "[]");
        assert!(!stage.shove(1, 0.0, 0.0));
        assert_eq!(stage.lift_ride_json(0, 1), r#"{"ok":false,"reason":"no nav or no such lift"}"#);
    }
}
