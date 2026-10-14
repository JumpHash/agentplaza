//! A board's navigation in the engine (floor.ts's nav world on the web): the nav scene of the board's shared layout
//! (`apz_board::layout::nav_scene`, the grid the layout was accepted on), the robot followers walking it
//! (`apz_board::nav::world::NavWorld`), and the walkways the grid's portals need: a lift at each lift portal, a ramp at
//! each ramp portal, a gangway at each bridged plate. The floor sets the robots' goals; the poses come back here.

use std::collections::HashMap;

use apz_board::layout::{nav_scene, Layout, NavScene};
use apz_board::nav::height::BodyMode as NavBody;
use apz_board::nav::lift::LiftPhase;
use apz_board::nav::world::{NavEvent, NavWorld};
use apz_board::nav::{NavGoal, NavPose, NavRobotSpec};
use apz_board::navgrid::PortalKind;

use super::lift_mesh::{LiftMesh, LiftSpec};
use super::nav_overlay::NavOverlay;
use super::walkway_mesh::{build_gangway, build_ramp, ramp_between, GangwaySpec};
use super::Colors;
use crate::robot::BodyMode;
use crate::scene::{Kind, Obj};

/// The robots' gait on the board (floor.ts's nav robot spec): units per second, radians per second.
const SPEED: f64 = 1.6;
const TURN_RATE: f64 = 6.0;
/// A goal that moved less than this keeps the robot's path.
const GOAL_EPS: f64 = 0.05;

/// The navigation of one board.
pub struct BoardNav {
    pub scene: NavScene,
    pub world: NavWorld,
    /// The walkways' group (lifts, ramps, gangways), under the board's group.
    pub root: Obj,
    lifts: Vec<(usize, LiftMesh)>,
    /// The debug overlay (`APZ_NAV_OVERLAY=1`): the cells, portals and robots' paths and rings, under `root`.
    pub overlay: Option<NavOverlay>,
    /// Robot id by agent cid, and the goal last sent.
    ids: HashMap<u64, u32>,
    goals: HashMap<u32, [f64; 2]>,
    next_id: u32,
}

impl BoardNav {
    /// The navigation of a shared layout for the board's `gpus`, its rooms seeded by `machine` (as the floor builds them).
    pub fn new(layout: &Layout, gpus: u32, machine: &str, colors: &Colors) -> BoardNav {
        let scene = nav_scene(layout, gpus, Some(machine));
        let world = NavWorld::new(scene.grid.clone());
        let root = Obj::group();
        root.set_name("walkways");
        let grid = &scene.grid;
        let mut lifts = Vec::new();
        for l in &grid.lifts {
            let spec = LiftSpec { x: l.x as f32, z: l.z as f32, face: l.face as f32, low: l.low as f32, high: l.high as f32 };
            let mesh = LiftMesh::build(&spec, colors);
            root.add(&mesh.root);
            lifts.push((l.id, mesh));
        }
        for p in grid.portals.iter().filter(|p| p.kind == PortalKind::Ramp) {
            let ([ax, az], [bx, bz]) = (grid.centre(p.a), grid.centre(p.b));
            let spec = ramp_between([ax as f32, az as f32], grid.y[p.a] as f32, [bx as f32, bz as f32], grid.y[p.b] as f32);
            root.add(&build_ramp(&spec, colors));
        }
        let pcb = layout.pcb_top as f32;
        for g in &layout.gangways {
            let Some(plate) = layout.plates.get(g.plate) else { continue };
            let Some(exit) = plate.exit.as_ref() else { continue };
            let spec = GangwaySpec {
                ax: exit.plate_at[0] as f32,
                az: exit.plate_at[1] as f32,
                bx: exit.board_at[0] as f32,
                bz: exit.board_at[1] as f32,
                nx: exit.dir[0] as f32,
                nz: exit.dir[1] as f32,
                bridge: exit.bridge.iter().map(|&[x, z]| [x as f32, z as f32]).collect(),
            };
            let top = layout.slots.get(plate.id).map_or(plate.y, |s| s.top) as f32;
            root.add(&build_gangway(&spec, top, pcb, colors, &|_, _| pcb));
        }
        let overlay = (crate::env_var("APZ_NAV_OVERLAY").as_deref() == Some("1")).then(|| NavOverlay::new(grid));
        if let Some(o) = &overlay {
            root.add(&o.root);
        }
        BoardNav { scene, world, root, lifts, overlay, ids: HashMap::new(), goals: HashMap::new(), next_id: 0 }
    }

    /// Keeps the world's robots to `bots` (cid, x, z): a new cid starts where its bot stands, a gone one is removed.
    pub fn sync(&mut self, bots: &[(u64, f32, f32)]) {
        for &(cid, x, z) in bots {
            if self.ids.contains_key(&cid) {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.ids.insert(cid, id);
            self.world.add(NavRobotSpec { id, x: f64::from(x), z: f64::from(z), speed: SPEED, turn_rate: TURN_RATE, priority: 1, floats: false });
        }
        let gone: Vec<u64> = self.ids.keys().filter(|cid| !bots.iter().any(|b| b.0 == **cid)).copied().collect();
        for cid in gone {
            if let Some(id) = self.ids.remove(&cid) {
                self.world.remove(id);
                self.goals.remove(&id);
            }
        }
    }

    /// Sends a robot's goal when it moved (the path is kept otherwise).
    pub fn goal(&mut self, cid: u64, x: f32, z: f32) {
        let Some(&id) = self.ids.get(&cid) else { return };
        let g = [f64::from(x), f64::from(z)];
        if self.goals.get(&id).is_some_and(|o| (o[0] - g[0]).hypot(o[1] - g[1]) < GOAL_EPS) {
            return;
        }
        self.goals.insert(id, g);
        self.world.set_goal(id, NavGoal { x: g[0], z: g[1], face: None });
    }

    /// One step of the robots and the cabs; the lifts' meshes follow their cabs. A robot the world gave up on drops its
    /// goal, so the floor's next goal is sent again; their cids are returned (the work picks another spot for them).
    pub fn step(&mut self, dt: f32) -> Vec<u64> {
        self.world.step(f64::from(dt));
        if let Some(o) = self.overlay.as_mut() {
            o.update(&self.world);
        }
        let mut gave_up = Vec::new();
        for e in self.world.take_events() {
            let (NavEvent::Unreachable(id) | NavEvent::Respawn(id)) = e;
            self.goals.remove(&id);
            if let (NavEvent::Unreachable(_), Some((&cid, _))) = (e, self.ids.iter().find(|(_, v)| **v == id)) {
                gave_up.push(cid);
            }
        }
        for (id, mesh) in &mut self.lifts {
            if let Some(cab) = self.world.lift_mut(*id) {
                let s = cab.state();
                mesh.set_state(s.cab_y, s.door_t, s.stop, s.phase == LiftPhase::Travel, s.calls);
            }
        }
        gave_up
    }

    /// The pose of an agent's robot.
    pub fn pose(&self, cid: u64) -> Option<NavPose> {
        let id = *self.ids.get(&cid)?;
        self.world.has(id).then(|| self.world.pose(id))
    }

    /// The nav component of the walk cell at (x, z), -1 off the walk (the work's reach: a robot goes only to spots in its
    /// own component).
    pub fn reach(&self) -> Box<dyn Fn(f32, f32) -> i64> {
        let grid = self.world.grid().clone();
        Box::new(move |x, z| grid.cell_at(f64::from(x), f64::from(z)).map_or(-1, |c| i64::from(grid.component[c])))
    }

    /// The robot id of an agent (for the debug hooks).
    pub fn robot_of(&self, cid: u64) -> Option<u32> {
        self.ids.get(&cid).copied()
    }

    /// The mesh nodes under the lift `id`'s mesh group (for the debug hooks); 0 for an unknown lift.
    pub fn lift_meshes(&self, id: usize) -> usize {
        self.lifts.iter().find(|(lift, _)| *lift == id).map_or(0, |(_, mesh)| count_meshes(&mesh.root))
    }
}

/// The mesh nodes (plain or instanced) in a node's subtree, the node itself included.
fn count_meshes(node: &Obj) -> usize {
    let n = node.get();
    let own = usize::from(matches!(n.kind, Kind::Mesh { .. } | Kind::Instanced { .. }));
    own + n.children.iter().map(count_meshes).sum::<usize>()
}

/// The rig's body layer for a nav body (floor.ts maps the same): the air modes show the jump or the fall.
pub fn body_of(mode: NavBody) -> BodyMode {
    match mode {
        NavBody::Crouch => BodyMode::Crouch,
        NavBody::Jump | NavBody::Glide => BodyMode::Air,
        NavBody::Fall | NavBody::Out => BodyMode::Fall,
        NavBody::Land => BodyMode::Land,
        NavBody::Ground | NavBody::Lift | NavBody::In => BodyMode::Ground,
    }
}
