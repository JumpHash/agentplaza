//! The labs' option lists, from what the engine embeds: the robot species, their clips, the tools and the carried items,
//! the props, the effect kinds and the machine kit. The glue's `lab_catalog` returns it as JSON, and
//! the web's lab controls list their options from it, so the page carries none of the asset JSON.

use serde_json::{Map, Value, json};

use crate::kit;
use crate::robots::files::registry;
use crate::robots::props::prop_registry;
use crate::robots::types::FX_KINDS;

/// The catalog as JSON (see `catalog`).
pub fn catalog_json() -> String {
    catalog().to_string()
}

/// The catalog: `species` (the index's order), `clips` (the shared clip ids, sorted), `speciesClips` (each species' own
/// clip ids, sorted), `tools` (the index's order), `carry` (the tools whose grip is carry), `props` (the index's order),
/// `fx` (the effect kinds), `models` (the machine kit's model ids, in the kit's order), `styles` (the machines' styles)
/// and `lods` (the kit's levels of detail).
pub fn catalog() -> Value {
    let reg = registry();
    let index = &reg.index;
    let species_clips: Map<String, Value> = index
        .species
        .iter()
        .map(|id| {
            let own: Vec<&String> = reg
                .species
                .get(id)
                .map(|s| s.clips.keys().collect())
                .unwrap_or_default();
            (id.clone(), json!(own))
        })
        .collect();
    let carry: Vec<&String> = index
        .tools
        .iter()
        .filter(|id| reg.tools.get(*id).is_some_and(|t| t.grip == "carry"))
        .collect();
    let models: Vec<&str> = kit::models().iter().map(|m| m.id.as_str()).collect();
    json!({
        "species": index.species,
        "clips": reg.clips.keys().collect::<Vec<_>>(),
        "speciesClips": Value::Object(species_clips),
        "tools": index.tools,
        "carry": carry,
        "props": prop_registry().order,
        "fx": FX_KINDS,
        "models": models,
        "styles": ["ink", "blueprint", "night"],
        "lods": 3,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_the_species_and_the_models() {
        let cat: Value = serde_json::from_str(&catalog_json()).expect("the catalog is JSON");
        let species: Vec<&str> = cat["species"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            species.contains(&"rover") && species.len() > 20,
            "{species:?}"
        );
        assert_eq!(
            species,
            registry()
                .index
                .species
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        let models: Vec<&str> = cat["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(models.len(), 10, "{models:?}");
        for id in [
            "hub", "server", "rack", "gpu", "tower", "edge", "mini", "studio", "laptop", "phone",
        ] {
            assert!(models.contains(&id), "the lab lists {id}: {models:?}");
        }
        assert_eq!(cat["styles"].as_array().unwrap().len(), 3);
        assert!(cat["speciesClips"]["rover"].as_array().is_some());
        assert!(cat["clips"].as_array().unwrap().iter().any(|c| c == "type"));
        assert_eq!(cat["fx"].as_array().unwrap().len(), FX_KINDS.len());
        assert!(
            cat["carry"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| reg_tool_is_carry(t.as_str().unwrap()))
        );
    }

    fn reg_tool_is_carry(id: &str) -> bool {
        registry().tools.get(id).is_some_and(|t| t.grip == "carry")
    }
}
