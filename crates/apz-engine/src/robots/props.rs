//! The station props (`assets/robots/props/<id>.json`, schema 1), as the web's `robots/props.ts`: furniture a
//! robot works at (a drafting table, a bench, a server rack). A prop is parts on the floor (no bones, the tools'
//! part format) and spots: where a robot stands (`at`, on the floor), which way it faces (`face`, degrees of yaw)
//! and the shared clip it plays there. The props are the `index.props` list; a file with errors is left out, and
//! every error is named by its file path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Value, json};

use super::super::robot::RobotPalette;
use super::super::scene::{Color, Obj};
use super::files::{ROBOT_FILES, registry};
use super::rig::{add_bone, bake_prop, robot_mats};
use super::types::{Part, Vec3};
use super::validate::validate_tool;

/// The accent a prop is baked in (props.ts `buildProp`'s default).
const PROP_ACCENT: &str = "#5b8cff";

/// A prop's spot: where a robot stands, which way it faces, the shared clip it plays.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropSpot {
    /// On the floor, in the prop's frame.
    pub at: Vec3,
    /// Degrees of yaw about the floor's up axis: 0 faces +z, 180 faces -z.
    pub face: f32,
    /// A shared clip (clips.json).
    pub clip: String,
}

/// A validated prop file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prop {
    pub schema: u32,
    pub id: String,
    pub name: String,
    /// The floor the prop takes, in robot units: width (x) and depth (z).
    pub footprint: [f32; 2],
    pub parts: Vec<Part>,
    pub spots: Vec<PropSpot>,
}

/// The valid props, by id, in index order, and every validation error.
pub struct PropRegistry {
    pub props: BTreeMap<String, Prop>,
    pub order: Vec<String>,
    pub errors: Vec<String>,
}

const PROP_KEYS: [&str; 6] = ["schema", "id", "name", "footprint", "parts", "spots"];
const SPOT_KEYS: [&str; 3] = ["at", "face", "clip"];

fn is_num(v: &Value) -> bool {
    v.as_f64().is_some_and(f64::is_finite)
}

/// A prop file's errors, each named by its path (props.ts `validateProp`). Empty when the file is valid.
pub fn validate_prop(file: &Value, name: &str, clips: &BTreeSet<String>) -> Vec<String> {
    let Some(obj) = file.as_object() else {
        return vec![format!("{name}: a JSON object")];
    };
    let mut errs = Vec::new();
    for k in obj.keys() {
        if !PROP_KEYS.contains(&k.as_str()) {
            errs.push(format!("{name}.{k}: unknown field"));
        }
    }
    if obj.get("schema").and_then(Value::as_u64) != Some(1) {
        errs.push(format!("{name}.schema: 1"));
    }
    if obj.get("id").and_then(Value::as_str) != Some(name) {
        errs.push(format!("{name}.id: must match the file name \"{name}\""));
    }
    if !obj
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        errs.push(format!("{name}.name: a name"));
    }
    let footprint_ok = obj
        .get("footprint")
        .and_then(Value::as_array)
        .is_some_and(|fp| {
            fp.len() == 2
                && fp
                    .iter()
                    .all(|n| n.as_f64().is_some_and(|v| v.is_finite() && v > 0.0))
        });
    if !footprint_ok {
        errs.push(format!("{name}.footprint: [width, depth], both above 0"));
    }
    // The parts are the tools' part format (no bone, built at the origin): the tool validator checks them.
    let as_tool = json!({
        "schema": 1, "id": name, "name": name, "grip": "one", "tip": [0, 0, 0],
        "parts": obj.get("parts").cloned().unwrap_or(Value::Null),
    });
    errs.extend(validate_tool(&as_tool, name));
    match obj.get("spots").and_then(Value::as_array) {
        Some(spots) if !spots.is_empty() => {
            for (i, s) in spots.iter().enumerate() {
                let sp = format!("{name}.spots[{i}]");
                let Some(o) = s.as_object() else {
                    errs.push(format!("{sp}: {{ at, face, clip }}"));
                    continue;
                };
                for k in o.keys() {
                    if !SPOT_KEYS.contains(&k.as_str()) {
                        errs.push(format!("{sp}.{k}: unknown field"));
                    }
                }
                let at_ok = o
                    .get("at")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.len() == 3 && a.iter().all(is_num));
                if !at_ok {
                    errs.push(format!("{sp}.at: [x, y, z] on the floor"));
                }
                if !o.get("face").is_some_and(is_num) {
                    errs.push(format!("{sp}.face: degrees of yaw"));
                }
                let clip = o.get("clip").and_then(Value::as_str);
                if !clip.is_some_and(|c| clips.contains(c)) {
                    errs.push(format!(
                        "{sp}.clip: \"{}\" is not a shared clip (clips.json)",
                        clip.unwrap_or("undefined")
                    ));
                }
            }
        }
        _ => errs.push(format!("{name}.spots: a list of at least one spot")),
    }
    errs
}

/// The registry from a file list (the build's `ROBOT_FILES`, or a test's) and the index's prop ids.
fn build_props(ids: &[String], files: &[(&str, &str)], clips: &BTreeSet<String>) -> PropRegistry {
    let mut props = BTreeMap::new();
    let mut order = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        let key = format!("props/{id}");
        let Some((_, src)) = files.iter().find(|(k, _)| *k == key) else {
            errors.push(format!(
                "index.props: \"{id}\" has no file (assets/robots/props/{id}.json)"
            ));
            continue;
        };
        let file: Value = match serde_json::from_str(src) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{key}: {e}"));
                continue;
            }
        };
        let found = validate_prop(&file, id, clips);
        if !found.is_empty() {
            errors.extend(found);
            continue;
        }
        match serde_json::from_value::<Prop>(file) {
            Ok(prop) => {
                order.push(id.clone());
                props.insert(id.clone(), prop);
            }
            Err(e) => errors.push(format!("{key}: {e}")),
        }
    }
    PropRegistry {
        props,
        order,
        errors,
    }
}

static PROPS: OnceLock<PropRegistry> = OnceLock::new();

/// The validated props (built once), in the order index.json lists them. Errors are logged once.
pub fn prop_registry() -> &'static PropRegistry {
    PROPS.get_or_init(|| {
        let reg = registry();
        let clips: BTreeSet<String> = reg.clips.keys().cloned().collect();
        let built = build_props(&reg.index.props, ROBOT_FILES, &clips);
        for e in &built.errors {
            eprintln!("agentplaza-desktop: robots: {e}");
        }
        built
    })
}

/// A prop's meshes, built as a one-bone species (the root) with no eyes (props.ts `buildProp`), and its spots.
pub struct BuiltProp {
    /// The prop's meshes at the origin (the floor's centre). The caller places and turns it.
    pub group: Obj,
    pub spots: Vec<PropSpot>,
}

/// Builds a prop with the robot palette, in `hue`'s body colours.
pub fn build_prop(prop: &Prop, pal: &RobotPalette, hue: f32) -> BuiltProp {
    let accent = Color::css(PROP_ACCENT).unwrap_or(Color::WHITE);
    let geo = bake_prop(&prop.parts, pal, accent);
    let mats = robot_mats(pal, hue, hue);
    let group = Obj::group();
    group.set_name(&format!("prop:{}", prop.id));
    add_bone(&group, &geo, &mats, &mut Vec::new());
    BuiltProp {
        group,
        spots: prop.spots.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prop_the_index_lists_loads_without_errors() {
        let reg = prop_registry();
        assert!(reg.errors.is_empty(), "prop errors: {:?}", reg.errors);
        assert_eq!(reg.order, registry().index.props);
    }
}
