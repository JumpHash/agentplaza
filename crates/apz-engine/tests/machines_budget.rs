//! The Machines view's budgets, on the CPU side (no GPU needed): the 15-machine fixture and a 128-machine fleet built
//! from it. What the renderer is handed per frame (triangles, line segments and draw calls) is counted in every build;
//! the time to build the grid and to step one frame is asserted only without debug assertions (release, as Linux x86_64
//! tests), scaled by the slack below.

use std::time::{Duration, Instant};

use apz_engine::fleet::{FleetResponse, lod_for};
use apz_engine::scene::{DrawKind, Frame};
use apz_engine::stage::Stage;

/// The stage size the budgets are measured at.
const W: f32 = 1600.0;
const H: f32 = 900.0;

/// The 15-machine fixture.
fn fixture() -> FleetResponse {
    serde_json::from_str(include_str!("../testdata/floor-15.json")).expect("the fixture parses")
}

/// A fleet of `n` machines: the fixture's machines repeated, each copy with its own id and name.
fn fleet_of(n: usize) -> FleetResponse {
    let base = fixture();
    let mut fleet = FleetResponse::default();
    for i in 0..n {
        let mut m = base.machines[i % base.machines.len()].clone();
        m.id = format!("{}-{i}", m.id);
        m.name = format!("{}-{i}", m.name);
        if m.role == "hub" && i >= base.machines.len() {
            m.role = "node".into(); // one hub
        }
        fleet.machines.push(m);
    }
    fleet
}

/// What one frame hands the renderer: triangles, line segments and draw calls (static draws and GPU-animated batches).
struct Load {
    triangles: usize,
    segments: usize,
    draws: usize,
}

fn load_of(frame: &Frame) -> Load {
    let mut load = Load {
        triangles: 0,
        segments: 0,
        draws: frame.statics.len() + frame.draws.len() + frame.anim_batches.len(),
    };
    for d in frame
        .statics
        .iter()
        .chain(&frame.draws)
        .chain(frame.anim_batches.iter().map(|b| &b.draw))
    {
        match d.kind {
            DrawKind::Mesh => load.triangles += d.geo.indices.len() / 3,
            DrawKind::Lines => load.segments += d.geo.indices.len() / 2,
            _ => {}
        }
    }
    load
}

/// The time budgets' multiplier: `APZ_PERF_SLACK` when set and positive; else 4 under CI (shared, loaded runners) and
/// 1 on a developer's machine (as `apz-metrics`'s budget test).
fn perf_slack() -> f64 {
    match std::env::var("APZ_PERF_SLACK")
        .ok()
        .filter(|text| !text.trim().is_empty())
    {
        Some(text) => match text.trim().parse::<f64>() {
            Ok(slack) if slack.is_finite() && slack > 0.0 => slack,
            _ => panic!("APZ_PERF_SLACK must be a positive number, got {text:?}"),
        },
        None if std::env::var_os("CI").is_some() => 4.0,
        None => 1.0,
    }
}

/// Builds the Machines view of `fleet` at the budget size and steps it a few frames. Returns the build time, the
/// slowest of the steps after the first, and the last frame's load.
fn measure(fleet: FleetResponse) -> (Duration, Duration, Load) {
    let mut stage = Stage::default();
    stage.resize(W, H);
    let t0 = Instant::now();
    stage.apply_fleet(fleet);
    let mut now = Instant::now();
    stage.tick(now); // the first frame bakes the statics: part of the build
    let build = t0.elapsed();
    let mut slowest = Duration::ZERO;
    for _ in 0..30 {
        now += Duration::from_millis(16);
        let t = Instant::now();
        stage.tick(now);
        slowest = slowest.max(t.elapsed());
    }
    (build, slowest, load_of(&stage.frame))
}

#[test]
fn the_level_of_detail_follows_the_fleet_size() {
    assert_eq!(lod_for(15), 0);
    assert_eq!(lod_for(32), 0);
    assert_eq!(lod_for(33), 1);
    assert_eq!(lod_for(96), 1);
    assert_eq!(lod_for(128), 2);
}

#[test]
fn the_fifteen_machine_fixture_stays_inside_its_frame_budget() {
    let (build, step, load) = measure(fixture());
    eprintln!(
        "15 machines: build {build:?}, step {step:?}, {} triangles, {} segments, {} draws",
        load.triangles, load.segments, load.draws
    );
    assert!(load.triangles <= 120_000, "{} triangles", load.triangles);
    assert!(load.segments <= 120_000, "{} line segments", load.segments);
    assert!(load.draws <= 160, "{} draws", load.draws);
    if cfg!(not(debug_assertions)) {
        let slack = perf_slack();
        assert!(build.as_secs_f64() * 1e3 <= 80.0 * slack, "build {build:?}");
        assert!(step.as_secs_f64() * 1e3 <= 2.0 * slack, "step {step:?}");
    }
}

#[test]
fn a_128_machine_fleet_stays_inside_its_frame_budget() {
    let (build, step, load) = measure(fleet_of(128));
    eprintln!(
        "128 machines: build {build:?}, step {step:?}, {} triangles, {} segments, {} draws",
        load.triangles, load.segments, load.draws
    );
    assert!(load.triangles <= 200_000, "{} triangles", load.triangles);
    assert!(load.segments <= 200_000, "{} line segments", load.segments);
    // Two load bars a machine are dynamic draws; the rest merge into a few batches.
    assert!(load.draws <= 300, "{} draws", load.draws);
    if cfg!(not(debug_assertions)) {
        let slack = perf_slack();
        assert!(
            build.as_secs_f64() * 1e3 <= 150.0 * slack,
            "build {build:?}"
        );
        assert!(step.as_secs_f64() * 1e3 <= 4.0 * slack, "step {step:?}");
    }
}
