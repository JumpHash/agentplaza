//! The robot data and rig checks: every file parses and validates, the species have their bones, the
//! clips evaluate as hand-computed, and each species builds within the draw budget.

use std::collections::BTreeSet;

use serde_json::Value;

use super::super::robot::{Robot, robot_palette};
use super::super::scene::{Kind, Obj};
use super::files::{ROBOT_FILES, registry};
use super::types::REQUIRED_BONES;
use super::validate::{
    validate_clips, validate_index, validate_palette, validate_species, validate_tool,
};

fn file_text(key: &str) -> &'static str {
    ROBOT_FILES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| panic!("no robot file {key}"))
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
    assert!(
        errors.is_empty(),
        "robot files do not validate:\n{}",
        errors.join("\n")
    );
    assert!(
        registry().errors.is_empty(),
        "the registry has errors:\n{}",
        registry().errors.join("\n")
    );
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
    for (key, _) in ROBOT_FILES
        .iter()
        .filter(|(k, _)| k.starts_with("species/"))
    {
        let id = key.strip_prefix("species/").unwrap_or(key);
        assert!(
            reg.index.species.iter().any(|s| s == id),
            "{id} has a file but is not in index.json"
        );
    }
}

#[test]
fn validation_finds_a_parent_cycle() {
    let mut rover: Value =
        serde_json::from_str(file_text("species/rover")).expect("the rover parses");
    rover["bones"]["base"]["parent"] = Value::from("body");
    rover["bones"]["body"]["parent"] = Value::from("base");
    let errs = validate_species(&rover, "rover", &BTreeSet::new());
    assert!(
        errs.iter().any(|e| e.starts_with("rover.bones.base.parent")
            || e.starts_with("rover.bones.body.parent")),
        "{errs:?}"
    );
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
        println!(
            "robots: {id}: {draws} draw calls, {} with a held wrench",
            held
        );
        assert!(
            draws <= 14,
            "{id} builds {draws} draw calls (the budget is 14)"
        );
        assert_eq!(held - draws, 2, "{id}: a held tool adds two draw calls");
    }
    let orchestrator = Robot::new(200.0, true, &palette, 120.0);
    println!(
        "robots: rover as the orchestrator: {} draw calls",
        meshes_under(&orchestrator.group)
    );
}
