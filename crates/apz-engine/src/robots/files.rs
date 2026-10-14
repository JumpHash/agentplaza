//! The robot files and their registry, as `robots/files.ts`. Every JSON under `assets/robots/` is in
//! `ROBOT_FILES` (build.rs lists them; nothing here names a file). The registry parses and validates the
//! files that `index.json` names. A species or tool with errors is left out (the rover stands in for a
//! species); every error is named by its file path and logged once.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::types::{Clip, ClipsFile, Index, Palette, Species, Tool};
use super::validate::{
    validate_clips, validate_index, validate_palette, validate_species, validate_tool,
};

include!(concat!(env!("OUT_DIR"), "/robot_files.rs"));

/// The validated robot data, built once.
pub struct Registry {
    pub index: Index,
    pub palette: Palette,
    /// The shared clips, by id.
    pub clips: BTreeMap<String, Clip>,
    /// The valid species, by id (the index's order is `index.species`).
    pub species: BTreeMap<String, Species>,
    /// The valid tools and carried items, by id.
    pub tools: BTreeMap<String, Tool>,
    /// Every validation error, named by file path. Empty when every listed file is valid.
    pub errors: Vec<String>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// The validated robot data. The first call builds it and logs its errors.
pub fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let reg = build(ROBOT_FILES);
        for e in &reg.errors {
            eprintln!("agentplaza-desktop: robots: {e}");
        }
        reg
    })
}

/// The text of a robot file by its key (`index`, `species/rover`, `tools/wrench`).
fn text(files: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    files.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// A file's typed value, when its validation passed; otherwise its errors are kept and None is returned.
fn accept<T: DeserializeOwned>(
    key: &str,
    value: &Value,
    mut found: Vec<String>,
    errors: &mut Vec<String>,
) -> Option<T> {
    if !found.is_empty() {
        errors.append(&mut found);
        return None;
    }
    match serde_json::from_value(value.clone()) {
        Ok(v) => Some(v),
        Err(e) => {
            errors.push(format!("{key}: {e}"));
            None
        }
    }
}

/// Builds the registry from a file list (the build's, or a test's).
fn build(files: &[(&'static str, &'static str)]) -> Registry {
    let mut errors = Vec::new();

    let index_src = text(files, "index");
    let index_v = parse("index", index_src, &mut errors);
    let index: Index =
        accept("index", &index_v, validate_index(&index_v), &mut errors).unwrap_or_default();

    let palette_v = parse("palette", text(files, "palette"), &mut errors);
    let palette: Palette = accept(
        "palette",
        &palette_v,
        validate_palette(&palette_v),
        &mut errors,
    )
    .unwrap_or_default();

    let clips_v = parse("clips", text(files, "clips"), &mut errors);
    // The shared ids are the file's keys even when the file has errors (as the web's registry).
    let shared: BTreeSet<String> = clips_v
        .get("clips")
        .and_then(Value::as_object)
        .map(|c| c.keys().cloned().collect())
        .unwrap_or_default();
    let clips: ClipsFile =
        accept("clips", &clips_v, validate_clips(&clips_v), &mut errors).unwrap_or_default();

    let mut species = BTreeMap::new();
    for id in &index.species {
        let key = format!("species/{id}");
        let Some(src) = text(files, &key) else {
            errors.push(format!(
                "index.species: \"{id}\" has no file (assets/robots/species/{id}.json)"
            ));
            continue;
        };
        let Some(v) = parse_src(&key, src, &mut errors) else {
            continue;
        };
        if let Some(sp) =
            accept::<Species>(&key, &v, validate_species(&v, id, &shared), &mut errors)
        {
            species.insert(id.clone(), sp);
        }
    }

    let mut tools = BTreeMap::new();
    for id in &index.tools {
        let key = format!("tools/{id}");
        let Some(src) = text(files, &key) else {
            errors.push(format!(
                "index.tools: \"{id}\" has no file (assets/robots/tools/{id}.json)"
            ));
            continue;
        };
        let Some(v) = parse_src(&key, src, &mut errors) else {
            continue;
        };
        if let Some(tool) = accept::<Tool>(&key, &v, validate_tool(&v, id), &mut errors) {
            tools.insert(id.clone(), tool);
        }
    }

    Registry {
        index,
        palette,
        clips: clips.clips,
        species,
        tools,
        errors,
    }
}

/// A core file parsed as JSON; a missing or unreadable file is an error and a null value.
fn parse(key: &str, src: Option<&str>, errors: &mut Vec<String>) -> Value {
    match src {
        Some(src) => parse_src(key, src, errors).unwrap_or(Value::Null),
        None => {
            errors.push(format!(
                "{key}: no such file (build.rs lists assets/robots)"
            ));
            Value::Null
        }
    }
}

fn parse_src(key: &str, src: &str, errors: &mut Vec<String>) -> Option<Value> {
    match serde_json::from_str(src) {
        Ok(v) => Some(v),
        Err(e) => {
            errors.push(format!("{key}: {e}"));
            None
        }
    }
}

static WARNED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Logs a robot warning once (a missing clip, an unknown tool), as the web's `warnOnce`.
pub fn warn_once(msg: &str) {
    let fresh = WARNED
        .lock()
        .map_or(true, |mut seen| seen.insert(msg.to_string()));
    if fresh {
        eprintln!("agentplaza-desktop: robots: {msg}");
    }
}
