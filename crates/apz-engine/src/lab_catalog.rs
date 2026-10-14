//! The labs' option lists, from what the engine embeds: the robot species, their clips, the tools and the carried items,
//! the props, the effect kinds, the room roles and the hardware models. The glue's `lab_catalog` returns it as JSON, and
//! the web's lab controls list their options from it, so the page carries none of the asset JSON.

use serde_json::{json, Map, Value};

use crate::hardware;
use crate::robots::files::registry;
use crate::robots::props::prop_registry;
use crate::robots::types::FX_KINDS;
use crate::rooms::generate;

/// The catalog as JSON (see `catalog`).
pub fn catalog_json() -> String {
    catalog().to_string()
}

/// The catalog: `species` (the index's order), `clips` (the shared clip ids, sorted), `speciesClips` (each species' own
/// clip ids, sorted), `tools` (the index's order), `carry` (the tools whose grip is carry), `props` (the index's order),
/// `fx` (the effect kinds), `roles` (the room roles, sorted), `models` (the hardware model and part ids, sorted) and
/// `params` (each model's params and their defaults, without the GPUs, which have their own control).
pub fn catalog() -> Value {
    let reg = registry();
    let index = &reg.index;
    let species_clips: Map<String, Value> = index
        .species
        .iter()
        .map(|id| {
            let own: Vec<&String> = reg.species.get(id).map(|s| s.clips.keys().collect()).unwrap_or_default();
            (id.clone(), json!(own))
        })
        .collect();
    let carry: Vec<&String> = index
        .tools
        .iter()
        .filter(|id| reg.tools.get(*id).is_some_and(|t| t.grip == "carry"))
        .collect();
    let roles: Vec<&String> = generate::role_order().iter().collect();
    let mut models: Vec<&str> = hardware::MODELS
        .iter()
        .chain(hardware::PARTS.iter())
        .map(|(name, _)| *name)
        .collect();
    models.sort_unstable();
    let params: Map<String, Value> = models
        .iter()
        .map(|name| {
            let values: Map<String, Value> = hardware::model(name)
                .map(|m| {
                    m.params
                        .iter()
                        .filter(|(k, _)| k.as_str() != "gpus")
                        .map(|(k, v)| (k.clone(), json!(v)))
                        .collect()
                })
                .unwrap_or_default();
            (name.to_string(), Value::Object(values))
        })
        .collect();
    json!({
        "species": index.species,
        "clips": reg.clips.keys().collect::<Vec<_>>(),
        "speciesClips": Value::Object(species_clips),
        "tools": index.tools,
        "carry": carry,
        "props": prop_registry().order,
        "fx": FX_KINDS,
        "roles": roles,
        "models": models,
        "params": Value::Object(params),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_the_species_and_the_models() {
        let cat: Value = serde_json::from_str(&catalog_json()).expect("the catalog is JSON");
        let species: Vec<&str> = cat["species"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(species.contains(&"rover") && species.len() > 20, "{species:?}");
        assert_eq!(species, registry().index.species.iter().map(String::as_str).collect::<Vec<_>>());
        let models: Vec<&str> = cat["models"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            models.contains(&"board/workstation") && models.contains(&"exterior/workstation"),
            "{models:?}"
        );
        for station in ["workstation/mac-mini", "workstation/windows-laptop", "workstation/linux-tower", "workstation/vm-rack"] {
            assert!(models.contains(&station), "the lab lists {station}: {models:?}");
        }
        assert!(cat["params"]["board/workstation"].as_object().is_some());
        assert!(cat["speciesClips"]["rover"].as_array().is_some());
        assert!(cat["clips"].as_array().unwrap().iter().any(|c| c == "type"));
        assert_eq!(cat["fx"].as_array().unwrap().len(), FX_KINDS.len());
        assert!(cat["roles"].as_array().unwrap().iter().any(|r| r == "architect"));
        assert!(cat["carry"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| reg_tool_is_carry(t.as_str().unwrap())));
    }

    fn reg_tool_is_carry(id: &str) -> bool {
        registry().tools.get(id).is_some_and(|t| t.grip == "carry")
    }
}
