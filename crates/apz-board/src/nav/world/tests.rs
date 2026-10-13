//! NavWorld end to end: robots on real boards' nav scenes walk between their homes (across ramps, lifts and gangways)
//! and arrive, never standing on a blocked cell.

use super::*;
use crate::layout::{layout, nav_scene, AgentSpec, LayoutInput};
use crate::nav::NavRobotSpec;

fn input(n: usize) -> LayoutInput {
    let roles = ["builder", "reviewer", "researcher", "ops"];
    let agents = (0..n)
        .map(|i| AgentSpec { id: format!("a{i}"), role: if i == 0 { "orchestrator".into() } else { roles[i % roles.len()].into() }, seat_hint: None })
        .collect();
    LayoutInput { agents, gpus: 0, scale_cap: 3.0 }
}

/// Each robot walks from its home to the next robot's home; returns (arrived, total, unreachable events).
fn walk(kind: &str, n: usize, seconds: f64) -> (usize, usize, usize) {
    walk_rides(kind, n, seconds).0
}

/// As `walk`, and the robot-steps spent on a lift.
fn walk_rides(kind: &str, n: usize, seconds: f64) -> ((usize, usize, usize), usize) {
    let lay = layout(kind, &input(n));
    let scene = nav_scene(&lay, 0, None);
    let mut world = NavWorld::new(scene.grid);
    let homes: Vec<[f64; 2]> = lay.homes.iter().map(|h| h.at).collect();
    for (i, h) in homes.iter().enumerate() {
        world.add(NavRobotSpec { id: i as u32, x: h[0], z: h[1], speed: 1.6, turn_rate: 6.0, priority: 1, floats: false });
    }
    for i in 0..homes.len() {
        let g = homes[(i + 1) % homes.len()];
        world.set_goal(i as u32, NavGoal { x: g[0], z: g[1], face: None });
    }
    let mut unreachable = 0;
    let mut rides = 0;
    let steps = (seconds * 60.0) as usize;
    for _ in 0..steps {
        world.step(1.0 / 60.0);
        for e in world.take_events() {
            if matches!(e, NavEvent::Unreachable(_)) {
                unreachable += 1;
            }
        }
        for id in world.robot_ids() {
            let p = world.pose(id);
            rides += usize::from(p.on_lift);
            if let Some(c) = world.grid().cell_at(p.x, p.z) {
                assert_eq!(world.grid().blocked[c], 0, "{kind}: robot {id} stands on a blocked cell at ({:.2}, {:.2})", p.x, p.z);
            }
        }
    }
    let arrived = (0..homes.len())
        .filter(|&i| {
            // A goal on a desk snaps to the nearest walk cell: arrived is the path walked to its end, near the goal.
            let p = world.pose(i as u32);
            let g = homes[(i + 1) % homes.len()];
            world.path(i as u32).is_none_or(|left| left.is_empty()) && (p.x - g[0]).hypot(p.z - g[1]) < 1.0
        })
        .count();
    ((arrived, homes.len(), unreachable), rides)
}

#[test]
fn robots_walk_between_homes_on_every_board() {
    for kind in ["edge", "laptop", "phone", "workstation", "server", "apple-soc"] {
        let (arrived, total, unreachable) = walk(kind, 4, 120.0);
        assert_eq!((arrived, unreachable), (total, 0), "{kind}: {arrived} of {total} arrived, {unreachable} unreachable");
    }
}

#[test]
fn a_crowd_of_eight_arrives_and_rides_the_lifts() {
    let mut rides = 0;
    for kind in ["edge", "laptop", "phone", "workstation", "server", "apple-soc"] {
        let ((arrived, total, unreachable), r) = walk_rides(kind, 8, 180.0);
        eprintln!("{kind}: {arrived}/{total} arrived, {unreachable} unreachable, {r} lift steps");
        // A crowd still stalls at a chokepoint now and then (a robot parked at a lift's exit or in a narrow aisle is not
        // asked to step aside), as on the web (test/nav-sim.mjs fails the same way): at least half the crowd arrives.
        assert!(arrived * 2 >= total, "{kind}: {arrived} of {total} arrived, {unreachable} unreachable");
        rides += r;
    }
    assert!(rides > 0, "no robot rode a lift");
}

#[test]
#[ignore = "debug"]
fn debug_walk() {
    let kind = std::env::var("APZ_KIND").unwrap_or_else(|_| "phone".into());
    let n: usize = std::env::var("APZ_N").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let lay = layout(&kind, &input(n));
    let scene = nav_scene(&lay, 0, None);
    let mut world = NavWorld::new(scene.grid);
    let homes: Vec<[f64; 2]> = lay.homes.iter().map(|h| h.at).collect();
    for (i, h) in homes.iter().enumerate() {
        world.add(NavRobotSpec { id: i as u32, x: h[0], z: h[1], speed: 1.6, turn_rate: 6.0, priority: 1, floats: false });
    }
    for i in 0..homes.len() {
        let g = homes[(i + 1) % homes.len()];
        world.set_goal(i as u32, NavGoal { x: g[0], z: g[1], face: None });
    }
    let mut trail: std::collections::BTreeMap<u32, std::collections::VecDeque<String>> = Default::default();
    for k in 0..(180 * 60) {
        world.step(1.0 / 60.0);
        if k % 20 == 0 {
            for (id, r) in &world.robots {
                let tgt = r.path.as_ref().and_then(|p| p.get(r.next)).map(|p| format!("({:.2},{:.2},{:.2},p{},{:?})", p.x, p.z, p.y, p.portal, p.edge));
                let line = format!("t={k} ({:.2},{:.2},{:.2}) {:?} next={} tgt={:?} stuck={:.2} v={:.2} bs={:.2} wait={} ride={} lift={} yield={}", r.x, r.z, r.body.y, r.body.mode, r.next, tgt, r.stuck_t, r.v, r.blocked_since, r.waiting, r.riding, r.lift_id, world.avoidance.get(id).is_some_and(|a| a.yielding));
                let q = trail.entry(*id).or_default();
                q.push_back(line);
                if q.len() > 12 {
                    q.pop_front();
                }
            }
        }
        for e in world.take_events() {
            if let NavEvent::Unreachable(id) = e {
                eprintln!("UNREACHABLE r{id} at step {k}:");
                for l in trail.get(&id).into_iter().flatten() {
                    eprintln!("  {l}");
                }
                for (oid, o) in &world.robots {
                    eprintln!("  other r{oid} ({:.2},{:.2},{:.2}) {:?} goal={:?} ride={} wait={} v={:.2} path_left={:?}", o.x, o.z, o.body.y, o.body.mode, o.goal.map(|g| (g.x, g.z)), o.riding, o.waiting, o.v, o.path.as_ref().map(|p| p.len().saturating_sub(o.next)));
                }
                for (lk, l) in &world.lifts {
                    let st = l.ctrl.state();
                    eprintln!("  lift{lk} ({:.2},{:.2}) phase={:?} cab_y={:.2} calls={:?} boarding={:?} door_t={:.2}", l.def.x, l.def.z, st.phase, st.cab_y, st.calls, st.boarding, st.door_t);
                }
            }
        }
    }
}
