//! The navigation debug hooks (`window.__apzBots`, `__apzNavCell`, `__apzNavAudit`, `__apzLifts`, `__apzShove` on the web):
//! the JSON they read from the stage's floor and its board's nav world. Read-only, except `shove` (a robot's position)
//! and `lifts_json` (which takes the lift controllers mutably, as their state is read through `lift_mut`).
//! `__apzBots` and `__apzLifts` return the shapes the retired three.js build returned, so the scripts that read them
//! keep working; every field is filled from the engine's own state.

use serde_json::{json, Value};

use crate::stage::Stage;

/// A Debug name in lowercase (a nav body or a lift phase: `BodyMode::Ground` is "ground").
fn debug_name<T: std::fmt::Debug>(value: T) -> String {
    format!("{value:?}").to_lowercase()
}

/// A number rounded to 3 decimals, as the debug JSON writes positions and headings.
fn round3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

impl Stage {
    /// The floor's agents' robots as the web's `__apzBots` reads them: `{ frames, nav, held, bots }`. `frames` is the
    /// floor's clock at 60 a second (the floor keeps no frame count of its own); `nav` says whether there is a board;
    /// `held` is the work's held flag (false without a bot). Each bot carries its agent, its activity and state, its
    /// position and route goal, the work's debug values for it, and its nav pose and nav cells.
    pub fn bots_json(&self) -> String {
        let nav = self.floor.nav.as_ref();
        let frames = (self.floor.time * 60.0) as u64;
        let held = self.floor.bots.first().is_some_and(|b| self.floor.work.debug_of(b.agent.cid as i64)["held"].as_bool() == Some(true));
        let bots: Vec<Value> = self
            .floor
            .bots
            .iter()
            .map(|bot| {
                let cid = bot.agent.cid;
                let work = self.floor.work.debug_of(cid as i64);
                let pose = nav.and_then(|n| n.pose(cid));
                let path = nav.and_then(|n| n.robot_of(cid).and_then(|id| n.world.path(id)));
                let goal_x = work["target"]["x"].as_f64().unwrap_or(f64::from(bot.x));
                let goal_z = work["target"]["z"].as_f64().unwrap_or(f64::from(bot.y));
                json!({
                    "cid": cid,
                    "role": bot.agent.role,
                    "tier": bot.agent.tier,
                    "status": bot.agent.status,
                    "activity": bot.activity,
                    "state": bot.state.as_ref().map_or_else(|| "idle".to_string(), debug_name),
                    "goalKey": bot.route_goal.map_or_else(String::new, |g| format!("{:.2},{:.2}", g[0], g[1])),
                    "moving": bot.moving,
                    "pos": { "x": round3(bot.x), "y": round3(bot.y), "py": round3(bot.py) },
                    "heading": round3(bot.heading),
                    "work": work,
                    "nav": pose.map_or(Value::Null, |p| {
                        json!({
                            "x": p.x,
                            "y": p.y,
                            "z": p.z,
                            "heading": p.heading,
                            "moving": p.moving,
                            "waiting": p.waiting,
                            "onLift": p.on_lift,
                            "body": debug_name(p.body),
                            "scale": p.scale,
                            "yielding": p.yielding,
                        })
                    }),
                    "navPath": path.map_or(Value::Null, |points| json!(points.len())),
                    "navStart": self.nav_cell_value(f64::from(bot.x), f64::from(bot.y)),
                    "navGoal": self.nav_cell_value(goal_x, goal_z),
                })
            })
            .collect();
        json!({ "frames": frames, "nav": nav.is_some(), "held": held, "bots": bots }).to_string()
    }

    /// One cell of the board's nav grid at (x, z) as `__apzNavCell` reads it, or "null" off the grid or without a board.
    pub fn nav_cell_json(&self, x: f64, z: f64) -> String {
        self.nav_cell_value(x, z).to_string()
    }

    /// The nav cell at (x, z) as the debug JSON writes it, or `Value::Null` off the grid or without a board.
    fn nav_cell_value(&self, x: f64, z: f64) -> Value {
        match self.floor.nav.as_ref().and_then(|n| n.world.debug_cell(x, z)) {
            Some(c) => json!({
                "cell": c.cell,
                "walk": c.walk,
                "blocked": c.blocked,
                "inflated": c.inflated,
                "region": c.region,
                "component": c.component,
                "y": c.y,
            }),
            None => Value::Null,
        }
    }

    /// The grounding audit of the board (`__apzGroundAudit`, ground.ts on the board scene): every resting part, plate,
    /// column and room that floats over or sinks into what is under it. "null" without a board.
    pub fn ground_audit_json(&self) -> String {
        match self.floor.board.as_ref() {
            Some(board) => {
                let roots = [board.model_obj.clone(), self.floor.district_group.clone()];
                crate::world::ground_audit::ground_audit(&roots).to_json().to_string()
            }
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

    /// The board's lifts as `__apzLifts` reads them: one `{ id, low, high, x, z, yLow, yHigh, at, face, meshes, ctrl }`
    /// per lift of the nav grid that has a controller (`ctrl` is the cab's state); "[]" without a board.
    pub fn lifts_json(&mut self) -> String {
        let Some(nav) = self.floor.nav.as_mut() else { return "[]".to_string() };
        let places = nav.world.grid().lifts.clone();
        let lifts: Vec<Value> = places
            .into_iter()
            .filter_map(|place| {
                let id = place.id;
                let (level, occupant) = nav.world.lift_state(id);
                let meshes = nav.lift_meshes(id);
                let cab = nav.world.lift_mut(id)?.state();
                Some(json!({
                    "id": id,
                    "low": place.low,
                    "high": place.high,
                    "x": place.x,
                    "z": place.z,
                    "yLow": place.low,
                    "yHigh": place.high,
                    "at": [place.x, 0.0, place.z],
                    "face": place.face,
                    "meshes": meshes,
                    "ctrl": {
                        "level": level,
                        "occupant": occupant,
                        "cabY": cab.cab_y,
                        "doorsOpen": cab.doors_open,
                        "doorT": cab.door_t,
                        "phase": debug_name(cab.phase),
                        "stop": cab.stop,
                        "calls": cab.calls,
                    },
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
    use serde_json::{json, Value};

    use crate::stage::Stage;

    #[test]
    fn no_board_gives_empty_hooks() {
        let mut stage = Stage::default();
        let bots: Value = serde_json::from_str(&stage.bots_json()).unwrap();
        assert_eq!(bots["bots"], json!([]));
        assert_eq!(bots["nav"], json!(false));
        assert_eq!(bots["held"], json!(false));
        assert!(bots["frames"].is_u64());
        assert_eq!(stage.nav_cell_json(0.0, 0.0), "null");
        assert_eq!(stage.nav_audit_json(), "null");
        assert_eq!(stage.lifts_json(), "[]");
        assert!(!stage.shove(1, 0.0, 0.0));
        assert_eq!(stage.lift_ride_json(0, 1), r#"{"ok":false,"reason":"no nav or no such lift"}"#);
    }
}

impl Stage {
    /// The shown view's name as the three.js build's `__apzPerf` gave it: `machines` (the fleet), else `floor` (a board).
    pub fn view_name(&self) -> String {
        match &self.view {
            crate::stage::View::Machines => "machines".to_string(),
            crate::stage::View::Machine(_) | crate::stage::View::Plaza => "floor".to_string(),
        }
    }

    /// The shown view's camera as `__apzCamera` reads it: the view (`machines` or `floor`), the eye,
    /// the target, the field of view in degrees, and the floor's built rooms' top.
    pub fn camera_json(&mut self) -> String {
        let view = self.view_name();
        let (w, h) = self.size;
        let aspect = if h > 0.0 { w / h } else { 1.0 };
        let cam = self.active().orbit.camera(aspect);
        json!({
            "view": view,
            "eye": [cam.eye.x, cam.eye.y, cam.eye.z],
            "target": [cam.target.x, cam.target.y, cam.target.z],
            "fov": cam.fov,
            "roomsTop": self.floor.rooms_top,
        })
        .to_string()
    }
}

/// One node of `scene_json`: three's Object3D fields the old `__apzScene` was read by (type, name, visibility, transform,
/// shadow flags, render order, the userData flags), and its children.
fn node_json(o: &crate::scene::Obj) -> Value {
    use crate::scene::Kind;
    let (head, children) = {
        let n = o.get();
        let (ty, extra) = match &n.kind {
            Kind::Group => ("Group", json!({})),
            Kind::Mesh { geo, .. } => ("Mesh", json!({ "vertices": geo.positions.len() })),
            Kind::Instanced { geo, count, .. } => ("Mesh", json!({ "vertices": geo.positions.len(), "isInstancedMesh": true, "count": count })),
            Kind::Points { positions, .. } => ("Points", json!({ "vertices": positions.len() })),
            Kind::PointLight { .. } => ("PointLight", json!({})),
        };
        let mut head = json!({
            "type": ty,
            "name": n.name,
            "visible": n.visible,
            "position": [n.position.x, n.position.y, n.position.z],
            "rotation": [n.rotation.x, n.rotation.y, n.rotation.z],
            "scale": [n.scale.x, n.scale.y, n.scale.z],
            "castShadow": n.cast_shadow,
            "receiveShadow": n.receive_shadow,
            "renderOrder": n.render_order,
            "userData": { "dynamic": n.dynamic, "rest": n.rest, "walk": n.walk, "outline": !n.no_outline },
        });
        if let (Some(h), Some(e)) = (head.as_object_mut(), extra.as_object()) {
            h.extend(e.clone());
        }
        (head, n.children.clone())
    };
    let mut head = head;
    head["children"] = Value::Array(children.iter().map(node_json).collect());
    head
}

impl Stage {
    /// The shown view's scene graph as JSON (`__apzScene`, three's Scene object in the retired build): `{ type: "Scene",
    /// children }`, each node with three's Object3D fields.
    pub fn scene_json(&mut self) -> String {
        let root = self.active().root.clone();
        let children: Vec<Value> = root.get().children.clone().iter().map(node_json).collect();
        json!({ "type": "Scene", "name": "", "children": children }).to_string()
    }
}
