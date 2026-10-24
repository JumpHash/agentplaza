//! The tool library's own tests: the tree's shape, its coverage of the hub's routes, the seat filter, the search and the
//! prompt map. Hub-state sources (the toolbox, the seats' connectors) need an App and are covered where one exists.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{Access, Catalog, Exec, GROUPS, describe, prompt_map, search, sources, visible};

fn catalog() -> Catalog {
    Catalog::build(None)
}

/// Every route group the hub serves is covered by a catalog entry or is marked internal. A new route group fails this
/// until someone adds its entries here or names it in `sources::INTERNAL` with a reason.
#[test]
fn every_hub_route_group_is_covered_or_internal() {
    let router = crate::api::router();
    let covered: BTreeSet<&str> = sources::CAPABILITIES
        .iter()
        .map(|c| crate::openapi::tag_of(c.pattern))
        .collect();
    let internal: BTreeSet<&str> = sources::INTERNAL.iter().map(|(tag, _)| *tag).collect();
    let mut missing = BTreeSet::new();
    for route in router.routes() {
        let tag = crate::openapi::tag_of(route.pattern);
        if !covered.contains(tag) && !internal.contains(tag) {
            missing.insert(format!("{tag} ({} {})", route.method, route.pattern));
        }
    }
    assert!(
        missing.is_empty(),
        "these route groups have no tool library entries and are not marked internal in \
         tool_library/sources.rs (INTERNAL): {missing:#?}"
    );
}

#[test]
fn internal_groups_name_real_routes_and_are_not_also_covered() {
    let router = crate::api::router();
    let tags: BTreeSet<&str> = router
        .routes()
        .map(|route| crate::openapi::tag_of(route.pattern))
        .collect();
    let covered: BTreeSet<&str> = sources::CAPABILITIES
        .iter()
        .map(|c| crate::openapi::tag_of(c.pattern))
        .collect();
    let mut problems = Vec::new();
    for (tag, reason) in sources::INTERNAL {
        if reason.is_empty() {
            problems.push(format!("internal group {tag} needs a reason"));
        }
        if !tags.contains(tag) {
            problems.push(format!("internal group {tag} serves no route: remove it"));
        }
        if covered.contains(tag) {
            problems.push(format!("group {tag} has entries: take it out of INTERNAL"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_capability_is_a_served_route() {
    let router = crate::api::router();
    let served: BTreeSet<(String, &str)> = router
        .routes()
        .map(|route| (route.method.to_string(), route.pattern))
        .collect();
    let missing: Vec<String> = sources::CAPABILITIES
        .iter()
        .filter(|cap| !served.contains(&(cap.method.to_string(), cap.pattern)))
        .map(|cap| format!("{} {} {}", cap.path, cap.method, cap.pattern))
        .collect();
    assert!(
        missing.is_empty(),
        "capabilities the router does not serve: {missing:#?}"
    );
}

#[test]
fn every_hub_tool_name_has_a_path() {
    let named: BTreeSet<&str> = sources::HOST.iter().map(|(name, _)| *name).collect();
    for name in crate::tools::host_names() {
        assert!(
            named.contains(name),
            "hub tool {name} has no path in tool_library/sources.rs (HOST)"
        );
    }
}

#[test]
fn every_entry_path_is_unique_across_sources() {
    let mut seen = BTreeSet::new();
    for source in super::all_sources() {
        for entry in source.entries(None) {
            assert!(
                seen.insert(entry.path.clone()),
                "two sources add {}",
                entry.path
            );
        }
    }
}

#[test]
fn every_group_has_a_summary_and_top_level_groups_are_named() {
    let catalog = catalog();
    for entry in catalog.entries() {
        let segments: Vec<&str> = entry.path.split('.').collect();
        for n in 1..segments.len() {
            let group = segments[..n].join(".");
            let summary = catalog
                .group(&group)
                .unwrap_or_else(|| panic!("no group {group}"));
            assert!(!summary.is_empty(), "group {group} has no summary");
        }
        assert!(
            entry.summary.len() <= 170,
            "{} has a long summary: {}",
            entry.path,
            entry.summary
        );
        assert!(
            !entry.description.is_empty(),
            "{} has no description",
            entry.path
        );
    }
    for top in catalog
        .entries()
        .iter()
        .filter_map(|e| e.path.split('.').next())
        .collect::<BTreeSet<_>>()
    {
        assert!(
            GROUPS.iter().any(|g| g.path == top),
            "top-level group {top} needs a summary in GROUPS"
        );
    }
}

#[test]
fn the_catalog_has_entries_from_every_built_in_source() {
    let catalog = catalog();
    let has = |path: &str| catalog.entry(path).is_some();
    // The harness's in-VM tools.
    assert!(has("code.read") && has("code.edit") && has("shell.run"));
    // The hub's own tools.
    assert!(has("memory.search") && has("journal.note"));
    assert!(has("tasks.browse") && has("weft.status"));
    // Connector actions, by connector.
    assert!(
        has("connectors.github.repos"),
        "the GitHub connector's actions"
    );
    assert!(has("deploy.kubernetes.undo"), "the Kubernetes undo");
    // Deploy, releases, the lab, recipes and the others.
    assert!(has("deploy.promote") && has("deploy.rollback") && has("deploy.run"));
    assert!(has("releases.candidate.cut") && has("releases.publish"));
    assert!(has("lab.runs.start") && has("lab.stages.performance.read"));
    assert!(has("qualify.report") && has("qualify.run"));
    assert!(has("recipes.run"));
    assert!(
        catalog
            .entries()
            .iter()
            .any(|e| e.path.starts_with("recipes.library."))
    );
    assert!(
        has("incidents.list")
            && has("upgrades.run")
            && has("cves.report")
            && has("evals.leaderboard")
    );
    assert!(has("environments.list"));
    // The recipe catalog entries are held text.
    assert!(
        catalog
            .entries()
            .iter()
            .filter(|e| e.path.starts_with("recipes.library."))
            .all(|e| matches!(e.exec, Exec::Text(_)))
    );
}

#[test]
fn the_root_and_a_nested_node_list_their_children() {
    let catalog = catalog();
    let root: Vec<String> = catalog
        .children(None, None, "")
        .into_iter()
        .map(|n| n.path)
        .collect();
    assert!(root.contains(&"code".to_string()), "{root:?}");
    assert!(root.contains(&"deploy".to_string()));
    assert!(root.contains(&"connectors".to_string()));
    let deploy = catalog.children(None, None, "deploy");
    let paths: Vec<&str> = deploy.iter().map(|n| n.path.as_str()).collect();
    assert!(
        paths.contains(&"deploy.promote")
            && paths.contains(&"deploy.rollback")
            && paths.contains(&"deploy.run")
    );
    assert!(deploy.iter().all(|n| !n.summary.is_empty()));
    let connectors = catalog.children(None, None, "connectors");
    assert!(
        connectors
            .iter()
            .any(|n| n.path == "connectors.github" && n.kind == "group")
    );
}

#[test]
fn describe_returns_the_schema_and_the_policy_of_an_entry() {
    let value = describe(None, None, "deploy.rollback").expect("the entry exists");
    assert_eq!(value["path"], "deploy.rollback");
    assert_eq!(value["runs"], "hub");
    assert_eq!(value["input_schema"]["type"], "object");
    let required: Vec<&str> = value["input_schema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(required, ["id", "env"]);
    assert_eq!(value["access"], "operator");
    let group = describe(None, None, "deploy").expect("the group exists");
    assert_eq!(group["kind"], "group");
    assert!(describe(None, None, "no.such.thing").is_err());
}

#[test]
fn search_finds_the_rollback_and_the_kubernetes_undo() {
    let hits: Vec<String> = search(None, None, "rollback", "")
        .into_iter()
        .map(|(entry, _)| entry.path)
        .collect();
    assert!(hits.contains(&"deploy.rollback".to_string()), "{hits:?}");
    assert!(
        hits.contains(&"deploy.kubernetes.undo".to_string()),
        "{hits:?}"
    );
    let scoped: Vec<String> = search(None, None, "rollback", "connectors")
        .into_iter()
        .map(|(entry, _)| entry.path)
        .collect();
    assert!(scoped.iter().all(|p| p.starts_with("connectors.")));
    assert!(search(None, None, "zzqxq", "").is_empty());
}

#[test]
fn a_seat_without_permission_does_not_see_an_operator_entry_and_is_told_why() {
    let catalog = catalog();
    let entry = catalog.entry("deploy.rollback").expect("the entry exists");
    assert_eq!(entry.access, Access::Operator);
    let why = visible(None, "builder", entry).expect_err("a builder may not roll back");
    assert!(why.contains("orchestrator"), "{why}");
    assert!(visible(None, crate::settings::ORCHESTRATOR_ROLE, entry).is_ok());
    let described = describe(None, Some("builder"), "deploy.rollback").expect("it describes");
    assert_eq!(described["available"], false);
    assert!(
        described["reason"]
            .as_str()
            .unwrap()
            .contains("orchestrator")
    );
    let seen: Vec<String> = catalog
        .children(None, Some("builder"), "deploy")
        .into_iter()
        .map(|n| n.path)
        .collect();
    assert!(
        seen.iter()
            .all(|p| !["deploy.run", "deploy.rollback", "deploy.promote"].contains(&p.as_str())),
        "a builder sees no operator deploy entries: {seen:?}"
    );
    let search_hits = search(None, Some("builder"), "rollback", "");
    assert!(search_hits.iter().all(|(e, _)| e.path != "deploy.rollback"));
}

#[test]
fn an_entry_that_runs_in_the_vm_is_the_harness_s_to_run() {
    let catalog = catalog();
    let read = catalog.entry("code.read").expect("the read tool");
    assert_eq!(read.exec, Exec::Guest("read".to_string()));
    assert_eq!(read.runs, super::Runs::Guest);
}

#[test]
fn the_prompt_map_is_stable_and_names_every_top_level_group() {
    let first = prompt_map();
    assert_eq!(first, prompt_map(), "the same bytes on every turn");
    assert!(first.starts_with("## Tool library"));
    for group in GROUPS.iter().filter(|g| !g.path.contains('.')) {
        assert!(
            first.contains(&format!("- {}: ", group.path)),
            "{} is missing",
            group.path
        );
    }
    assert!(
        first.contains("tools_list")
            && first.contains("tools_search")
            && first.contains("tools_describe")
    );
    assert!(
        first.len() < 6000,
        "the map is compact: {} bytes",
        first.len()
    );
}
