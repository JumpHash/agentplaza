//! The robot data and rig checks: every file parses and validates, the species have their bones, the
//! clips evaluate as hand-computed, and each species builds within the draw budget.

use std::collections::BTreeSet;
use std::f64::consts::PI;

use serde_json::{json, Value};

use super::super::robot::{robot_palette, Robot};
use super::super::scene::{Kind, Obj};
use super::clips::{channel_value, eval_clip, wave_at};
use super::files::{registry, ROBOT_FILES};
use super::types::{Channel, Clip, Gait, REQUIRED_BONES};
use super::validate::{validate_clips, validate_index, validate_palette, validate_species, validate_tool};

fn file_text(key: &str) -> &'static str {
    ROBOT_FILES.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or_else(|| panic!("no robot file {key}"))
}

fn rover_gait() -> Gait {
    registry().species["rover"].gait.clone()
}

fn channel(v: Value) -> Channel {
    serde_json::from_value(v).expect("a channel")
}

#[test]
fn every_robot_file_parses_and_validates() {
    let shared: BTreeSet<String> = registry().clips.keys().cloned().collect();
    let mut errors = Vec::new();
    for (key, src) in ROBOT_FILES {
        let v: Value = match serde_json::from_str(src) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{key}: {e}"));
                continue;
            }
        };
        if let Some(id) = key.strip_prefix("species/") {
            errors.extend(validate_species(&v, id, &shared));
        } else if let Some(id) = key.strip_prefix("tools/") {
            errors.extend(validate_tool(&v, id));
        } else if *key == "clips" {
            errors.extend(validate_clips(&v));
        } else if *key == "palette" {
            errors.extend(validate_palette(&v));
        } else if *key == "index" {
            errors.extend(validate_index(&v));
        }
    }
    assert!(errors.is_empty(), "robot files do not validate:\n{}", errors.join("\n"));
    assert!(registry().errors.is_empty(), "the registry has errors:\n{}", registry().errors.join("\n"));
}

#[test]
fn every_species_has_the_required_bones_and_is_listed() {
    let reg = registry();
    assert!(!reg.species.is_empty(), "no valid species");
    for (id, sp) in &reg.species {
        for bone in REQUIRED_BONES {
            assert!(sp.bones.contains_key(*bone), "{id} lacks the bone {bone}");
        }
        assert!(reg.index.species.contains(id), "{id} is not in index.json");
    }
    for (key, _) in ROBOT_FILES.iter().filter(|(k, _)| k.starts_with("species/")) {
        let id = key.strip_prefix("species/").unwrap_or(key);
        assert!(reg.index.species.iter().any(|s| s == id), "{id} has a file but is not in index.json");
    }
}

#[test]
fn clip_evaluation_matches_hand_values() {
    let gait = rover_gait();
    // A sin: base + amp × sin(2π·freq·t).
    let arm = channel(json!({"bone": "armL", "prop": "rot.x", "base": -0.9, "amp": 0.2, "freq": 11.0}));
    let expected = -0.9 + 0.2 * (2.0 * PI * 1.1).sin();
    // The values are f32 in the file (-0.9 is not exact in f32), so the tolerance is 1e-6.
    assert!((channel_value(&arm, 0.1, &gait, 0.0, 0.0) - expected).abs() < 1e-6);

    // The waves at quarter cycles.
    assert_eq!(wave_at("pulse", 0.25, 0.5), 1.0);
    assert_eq!(wave_at("pulse", 0.75, 0.5), 0.0);
    assert_eq!(wave_at("square", 0.25, 0.5), 1.0);
    assert_eq!(wave_at("square", 0.75, 0.5), -1.0);
    assert!((wave_at("saw", 0.25, 0.5) + 0.5).abs() < 1e-12);
    assert!((wave_at("saw", 0.75, 0.5) - 0.5).abs() < 1e-12);
    assert!(wave_at("tri", 0.25, 0.5).abs() < 1e-12);
    assert!((wave_at("tri", 0.5, 0.5) - 1.0).abs() < 1e-12);
    assert_eq!(wave_at("halfsin", 0.75, 0.5), 0.0);
    assert_eq!(wave_at("hold", 0.3, 0.5), 0.0);

    // An in-out envelope: at the middle of a one-shot the sine is at its full amplitude.
    let swell = channel(json!({"bone": "head", "prop": "rot.x", "amp": 0.5, "freq": 0.25, "env": "in-out"}));
    assert!((channel_value(&swell, 1.0, &gait, 1.0, 2.0) - 0.5).abs() < 1e-12);

    // A gait reference: "gait.bob" is the species' bob.
    let bob = channel(json!({"bone": "body", "prop": "pos.y", "amp": "gait.bob", "freq": 1.0}));
    assert!((channel_value(&bob, 0.25, &gait, 0.0, 0.0) - gait.bob as f64).abs() < 1e-6);
}

#[test]
fn rate_channels_accumulate_and_are_not_blended() {
    let gait = rover_gait();
    let clip: Clip = serde_json::from_value(json!({
        "loop": true,
        "channels": [
            {"bone": "head", "prop": "rot.z", "rate": 2.0},
            {"bone": "head", "prop": "rot.x", "base": 0.5}
        ]
    }))
    .expect("a clip");
    let mut seen = Vec::new();
    eval_clip(&clip, 0.0, &gait, 0.0, |ch, v| seen.push((ch.prop.clone(), v)));
    assert_eq!(seen, vec![("rot.x".to_string(), 0.5)]);
}

#[test]
fn validation_names_the_path_of_each_error() {
    let mut rover: Value = serde_json::from_str(file_text("species/rover")).expect("the rover parses");
    rover["bones"].as_object_mut().expect("bones").remove("head");
    rover["parts"][0]["bone"] = Value::from("nope");
    let errs = validate_species(&rover, "rover", &BTreeSet::new());
    assert!(errs.iter().any(|e| e == "rover.bones: the required bone \"head\" is missing"), "{errs:?}");
    assert!(errs.iter().any(|e| e.starts_with("rover.parts[0].bone: \"nope\"")), "{errs:?}");
}

#[test]
fn validation_finds_a_parent_cycle() {
    let mut rover: Value = serde_json::from_str(file_text("species/rover")).expect("the rover parses");
    rover["bones"]["base"]["parent"] = Value::from("body");
    rover["bones"]["body"]["parent"] = Value::from("base");
    let errs = validate_species(&rover, "rover", &BTreeSet::new());
    assert!(errs.iter().any(|e| e.starts_with("rover.bones.base.parent") || e.starts_with("rover.bones.body.parent")), "{errs:?}");
}

/// The meshes a robot adds, counted as the web counts its draw calls: each mesh node, and each ink hull.
fn meshes_under(root: &Obj) -> usize {
    let mut n = 0;
    root.traverse(&mut |o: &Obj| {
        let node = o.get();
        if matches!(node.kind, Kind::Mesh { .. }) {
            n += 1;
        }
        if node.hull.is_some() {
            n += 1;
        }
    });
    n
}

#[test]
fn each_species_builds_within_the_draw_budget() {
    let palette = robot_palette();
    for id in ["rover", "hover", "tv", "can", "kitty"] {
        let mut robot = Robot::with_variant(200.0, false, &palette, 120.0, id);
        let draws = meshes_under(&robot.group);
        robot.hold(Some("wrench"));
        let held = meshes_under(&robot.group);
        println!("robots: {id}: {draws} draw calls, {} with a held wrench", held);
        assert!(draws <= 14, "{id} builds {draws} draw calls (the budget is 14)");
        assert_eq!(held - draws, 2, "{id}: a held tool adds two draw calls");
    }
    let orchestrator = Robot::new(200.0, true, &palette, 120.0);
    println!("robots: rover as the orchestrator: {} draw calls", meshes_under(&orchestrator.group));
}
