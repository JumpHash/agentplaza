//! The engine core: a standalone `bevy_ecs` world and four schedules. Nothing uses it yet; the scenes still run
//! their own loops. See `docs/design/engine-core.md`.
//!
//! * `Startup` runs once, before the first `advance`.
//! * `Fixed` runs at 30 Hz. The ticks owed at a given time are `floor(time_us * 30 / 1e6)`, a pure function of
//!   the total time, so a timeline gives the same ticks however it is split into frames. At most 5 ticks run per
//!   frame; the rest of a longer stall is dropped, so a backlog never becomes a burst.
//! * `Update` runs once per frame, with [`Time`].
//! * `RenderPrep` runs once per frame after `Update`. It propagates [`GlobalTransform`]s.

pub mod components;

use std::collections::HashMap;
use std::time::Duration;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{ExecutorKind, ScheduleLabel};
use glam::Affine3A;

use crate::assets::AssetRegistry;
use crate::stage::Motion;
use components::*;

/// The fixed simulation rate.
pub const FIXED_HZ: u64 = 30;
/// The most fixed ticks one `advance` runs.
pub const MAX_FIXED_PER_FRAME: u64 = 5;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Startup;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fixed;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Update;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderPrep;

/// The frame clock, in seconds. `now` is the host time given to `advance`; `dt` is the time since the last frame
/// (0 on the first); `elapsed` sums `dt` from 0, so it does not depend on the host's origin.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct Time {
    pub now: f64,
    pub dt: f64,
    pub elapsed: f64,
}

/// Set for the fixed tick being run: `tick` counts from 0 for the whole engine, `step` is `1 / FIXED_HZ`.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct FixedTime {
    pub tick: u64,
    pub step: f64,
}

impl Default for FixedTime {
    fn default() -> Self {
        Self {
            tick: 0,
            step: 1.0 / FIXED_HZ as f64,
        }
    }
}

/// The platform's reduced-motion preference. It turns [`Motion::Ambient`] into [`Motion::Rest`].
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReducedMotion(pub bool);

fn schedule(label: impl ScheduleLabel) -> Schedule {
    let mut schedule = Schedule::new(label);
    // Single-threaded: wasm32 has no threads here, and the order must be the same on every host.
    schedule.set_executor_kind(ExecutorKind::SingleThreaded);
    schedule
}

/// The ECS world and its schedules. The host calls [`EngineWorld::advance`] once per frame.
pub struct EngineWorld {
    world: World,
    startup: Schedule,
    fixed: Schedule,
    update: Schedule,
    render_prep: Schedule,
    started: bool,
    /// Fixed ticks run, or dropped, so far.
    fixed_done: u64,
    last_now: Option<Duration>,
}

impl Default for EngineWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineWorld {
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(Time::default());
        world.insert_resource(FixedTime::default());
        world.insert_resource(ReducedMotion::default());
        world.insert_resource(AssetRegistry::default());
        let mut render_prep = schedule(RenderPrep);
        render_prep.add_systems(propagate_transforms);
        Self {
            world,
            startup: schedule(Startup),
            fixed: schedule(Fixed),
            update: schedule(Update),
            render_prep,
            started: false,
            fixed_done: 0,
            last_now: None,
        }
    }

    /// Runs the frame. `now` is the host clock since any fixed origin; it must not go backwards (if it does, no
    /// time is owed and `dt` is 0).
    pub fn advance(&mut self, now: Duration) {
        if !self.started {
            self.startup.run(&mut self.world);
            self.started = true;
        }

        let owed = (now.as_micros() * u128::from(FIXED_HZ) / 1_000_000) as u64;
        let due = owed.saturating_sub(self.fixed_done).min(MAX_FIXED_PER_FRAME);
        for _ in 0..due {
            self.world.resource_mut::<FixedTime>().tick = self.fixed_done;
            self.fixed.run(&mut self.world);
            self.fixed_done += 1;
        }
        // Whatever the cap cut off is dropped, not carried into the next frame.
        self.fixed_done = self.fixed_done.max(owed);

        let dt = self
            .last_now
            .map_or(0.0, |prev| now.saturating_sub(prev).as_secs_f64());
        self.last_now = Some(now);
        let elapsed = self.world.resource::<Time>().elapsed + dt;
        *self.world.resource_mut::<Time>() = Time {
            now: now.as_secs_f64(),
            dt,
            elapsed,
        };

        self.update.run(&mut self.world);
        self.render_prep.run(&mut self.world);
    }

    /// What the host should do with its frame clock. Any `Active` entity gives [`Motion::Active`]; otherwise any
    /// `Ambient` entity gives [`Motion::Ambient`] (or [`Motion::Rest`] under reduced motion); otherwise Rest.
    pub fn motion(&mut self) -> Motion {
        if self
            .world
            .query_filtered::<Entity, With<Active>>()
            .iter(&self.world)
            .next()
            .is_some()
        {
            return Motion::Active;
        }
        let ambient = self
            .world
            .query_filtered::<Entity, With<Ambient>>()
            .iter(&self.world)
            .next()
            .is_some();
        if ambient && !self.world.resource::<ReducedMotion>().0 {
            Motion::Ambient
        } else {
            Motion::Rest
        }
    }

    pub fn set_reduced_motion(&mut self, on: bool) {
        self.world.resource_mut::<ReducedMotion>().0 = on;
    }

    /// Spawns a root entity with a transform.
    pub fn spawn_node(&mut self, transform: Transform) -> Entity {
        self.world.spawn((transform, GlobalTransform::default())).id()
    }

    /// Spawns an entity under `parent`.
    pub fn spawn_child(&mut self, parent: Entity, transform: Transform) -> Entity {
        self.world
            .spawn((transform, GlobalTransform::default(), Parent(parent)))
            .id()
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn startup_mut(&mut self) -> &mut Schedule {
        &mut self.startup
    }

    pub fn fixed_mut(&mut self) -> &mut Schedule {
        &mut self.fixed
    }

    pub fn update_mut(&mut self) -> &mut Schedule {
        &mut self.update
    }
}

/// Computes every [`GlobalTransform`] from its [`Transform`] and its chain of [`Parent`]s. A cycle is cut where
/// it closes, so the system always terminates.
fn propagate_transforms(
    locals: Query<(Entity, &Transform, Option<&Parent>)>,
    mut globals: Query<(Entity, &mut GlobalTransform)>,
) {
    let local: HashMap<Entity, (Affine3A, Option<Entity>)> = locals
        .iter()
        .map(|(e, t, p)| (e, (t.matrix(), p.map(|p| p.0))))
        .collect();
    let mut resolved: HashMap<Entity, Affine3A> = HashMap::with_capacity(local.len());
    for &e in local.keys() {
        resolve(e, &local, &mut resolved);
    }
    for (e, mut global) in globals.iter_mut() {
        if let Some(m) = resolved.get(&e) {
            global.0 = *m;
        }
    }
}

fn resolve(
    e: Entity,
    local: &HashMap<Entity, (Affine3A, Option<Entity>)>,
    resolved: &mut HashMap<Entity, Affine3A>,
) -> Affine3A {
    if let Some(m) = resolved.get(&e) {
        return *m;
    }
    let Some(&(m, parent)) = local.get(&e) else {
        return Affine3A::IDENTITY;
    };
    // Provisional value first, so a cycle returns it instead of recursing forever.
    resolved.insert(e, m);
    let global = match parent {
        Some(p) if p != e && local.contains_key(&p) => resolve(p, local, resolved) * m,
        _ => m,
    };
    resolved.insert(e, global);
    global
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[derive(Resource, Debug, Default)]
    struct Sim {
        ticks: u64,
        digest: u64,
    }

    /// A fixed system whose state depends on the tick index, so both the count and the order matter.
    fn sim_step(mut sim: ResMut<Sim>, t: Res<FixedTime>) {
        sim.ticks += 1;
        sim.digest = sim.digest.wrapping_mul(31).wrapping_add(t.tick + 1);
    }

    fn sim_engine() -> EngineWorld {
        let mut engine = EngineWorld::new();
        engine.world_mut().insert_resource(Sim::default());
        engine.fixed_mut().add_systems(sim_step);
        engine
    }

    /// Advances to `total_ms` in steps of `step_ms` and returns the fixed tick count and the state digest.
    fn run_in_steps(step_ms: u64, total_ms: u64) -> (u64, u64) {
        let mut engine = sim_engine();
        let mut t = 0;
        while t < total_ms {
            t += step_ms;
            engine.advance(Duration::from_millis(t));
        }
        let sim = engine.world().resource::<Sim>();
        (sim.ticks, sim.digest)
    }

    #[test]
    fn fixed_steps_do_not_depend_on_frame_length() {
        // 2310 ms is a multiple of both 7 and 33, so both timelines end at the same instant.
        let (ticks_7, digest_7) = run_in_steps(7, 2310);
        let (ticks_33, digest_33) = run_in_steps(33, 2310);
        assert_eq!(ticks_7, 69);
        assert_eq!(ticks_7, ticks_33);
        assert_eq!(digest_7, digest_33);
        assert_ne!(digest_7, 0);
    }

    #[test]
    fn a_long_stall_runs_at_most_five_fixed_ticks() {
        let mut engine = sim_engine();
        engine.advance(Duration::from_secs(10));
        assert_eq!(engine.world().resource::<Sim>().ticks, MAX_FIXED_PER_FRAME);
        // The backlog is dropped, not spent on the next frames.
        engine.advance(Duration::from_millis(10_010));
        assert_eq!(engine.world().resource::<Sim>().ticks, MAX_FIXED_PER_FRAME);
    }

    #[test]
    fn time_resource_tracks_dt_and_elapsed() {
        let mut engine = EngineWorld::new();
        engine.advance(Duration::from_millis(1000));
        let t = *engine.world().resource::<Time>();
        assert_eq!((t.dt, t.elapsed), (0.0, 0.0));
        engine.advance(Duration::from_millis(1007));
        let t = *engine.world().resource::<Time>();
        assert!((t.dt - 0.007).abs() < 1e-12);
        assert!((t.elapsed - 0.007).abs() < 1e-12);
        assert!((t.now - 1.007).abs() < 1e-12);
    }

    #[test]
    fn motion_follows_the_markers() {
        let mut engine = EngineWorld::new();
        assert!(matches!(engine.motion(), Motion::Rest));

        let ambient = engine.world_mut().spawn(Ambient).id();
        assert!(matches!(engine.motion(), Motion::Ambient));

        engine.set_reduced_motion(true);
        assert!(matches!(engine.motion(), Motion::Rest));
        engine.set_reduced_motion(false);

        let active = engine.world_mut().spawn(Active).id();
        assert!(matches!(engine.motion(), Motion::Active));

        // Reduced motion holds only the ambient tier; an active thing still moves.
        engine.set_reduced_motion(true);
        assert!(matches!(engine.motion(), Motion::Active));
        assert!(engine.world_mut().despawn(active));
        assert!(matches!(engine.motion(), Motion::Rest));
        engine.set_reduced_motion(false);
        assert!(matches!(engine.motion(), Motion::Ambient));

        assert!(engine.world_mut().despawn(ambient));
        assert!(matches!(engine.motion(), Motion::Rest));
    }

    #[test]
    fn transforms_propagate_through_three_levels() {
        let mut engine = EngineWorld::new();
        let root = engine.spawn_node(Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)));
        let mid = engine.spawn_child(
            root,
            Transform {
                translation: Vec3::new(0.0, 2.0, 0.0),
                scale: Vec3::splat(2.0),
                ..Transform::default()
            },
        );
        let leaf = engine.spawn_child(mid, Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)));
        engine.advance(Duration::ZERO);

        // The mid node's scale applies to its child's offset: (1,2,0) + 2 * (0,0,3).
        let expected = Vec3::new(1.0, 2.0, 6.0);
        let got = Vec3::from(engine.world().get::<GlobalTransform>(leaf).unwrap().0.translation);
        assert!((got - expected).abs().max_element() < 1e-5, "leaf at {got:?}");
        let mid_at = Vec3::from(engine.world().get::<GlobalTransform>(mid).unwrap().0.translation);
        assert!((mid_at - Vec3::new(1.0, 2.0, 0.0)).abs().max_element() < 1e-5);
    }

    #[test]
    fn a_parent_cycle_terminates() {
        let mut engine = EngineWorld::new();
        let a = engine.spawn_node(Transform::default());
        let b = engine.spawn_child(a, Transform::default());
        engine.world_mut().entity_mut(a).insert(Parent(b));
        engine.advance(Duration::ZERO);
    }
}
