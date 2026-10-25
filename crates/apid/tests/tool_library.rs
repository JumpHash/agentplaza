//! The tool library's owner routes on a scratch hub (PLAN p13): the root lists the groups and what the library leaves
//! out, a group lists its children, a search finds the rollback and the Kubernetes undo, and an entry answers with its
//! schema and the seats that may use it. The agents' `tools_*` calls run inside a session and are covered by the unit
//! tests of `tool_library` and the harness's rewrite tests.

mod common;

use common::{Hub, Scratch};

#[test]
fn the_root_lists_the_groups_and_names_what_it_leaves_out() {
    let scratch = Scratch::new("tool-library-root");
    let hub = Hub::start(&scratch, None, None);
    let (status, root) = hub.call("GET", "/v1/tool-library", None);
    assert_eq!(status, 200, "{root}");
    assert_eq!(root["path"], "");
    let groups: Vec<&str> = root["children"]
        .as_array()
        .expect("children")
        .iter()
        .map(|child| child["path"].as_str().unwrap_or_default())
        .collect();
    for top in ["code", "deploy", "releases", "connectors", "lab", "recipes"] {
        assert!(
            groups.contains(&top),
            "{top} is a top-level group: {groups:?}"
        );
    }
    let internal: Vec<&str> = root["internal"]
        .as_array()
        .expect("internal")
        .iter()
        .map(|item| item["group"].as_str().unwrap_or_default())
        .collect();
    assert!(
        internal.contains(&"vault"),
        "the vault is left out and says so: {internal:?}"
    );
}

#[test]
fn a_group_lists_its_children_and_an_unknown_path_is_refused() {
    let scratch = Scratch::new("tool-library-group");
    let hub = Hub::start(&scratch, None, None);
    let (status, deploy) = hub.call("GET", "/v1/tool-library?path=deploy", None);
    assert_eq!(status, 200, "{deploy}");
    let paths: Vec<&str> = deploy["children"]
        .as_array()
        .expect("children")
        .iter()
        .map(|child| child["path"].as_str().unwrap_or_default())
        .collect();
    assert!(paths.contains(&"deploy.rollback"), "{paths:?}");
    let (missing, _) = hub.call("GET", "/v1/tool-library?path=no.such.group", None);
    assert_eq!(missing, 404);
}

#[test]
fn search_finds_the_rollback_and_the_kubernetes_undo() {
    let scratch = Scratch::new("tool-library-search");
    let hub = Hub::start(&scratch, None, None);
    let (status, found) = hub.call("GET", "/v1/tool-library/search?q=rollback", None);
    assert_eq!(status, 200, "{found}");
    let paths: Vec<&str> = found["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["path"].as_str().unwrap_or_default())
        .collect();
    assert!(paths.contains(&"deploy.rollback"), "{paths:?}");
    assert!(paths.contains(&"deploy.kubernetes.undo"), "{paths:?}");
}

#[test]
fn an_entry_answers_with_its_schema_and_the_seats_that_may_use_it() {
    let scratch = Scratch::new("tool-library-entry");
    let hub = Hub::start(&scratch, None, None);
    let (status, entry) = hub.call("GET", "/v1/tool-library/entry?path=deploy.rollback", None);
    assert_eq!(status, 200, "{entry}");
    assert_eq!(entry["path"], "deploy.rollback");
    assert_eq!(entry["input_schema"]["type"], "object");
    assert_eq!(entry["access"], "operator");
    assert_eq!(entry["approval"], true);
    let seats = entry["seats"].as_array().expect("seats");
    assert!(
        seats
            .iter()
            .any(|seat| seat["seat"] == "orchestrator" && seat["allowed"] == true),
        "the orchestrator may use it: {seats:?}"
    );
    let (missing, _) = hub.call("GET", "/v1/tool-library/entry?path=no.such.tool", None);
    assert_eq!(missing, 404);
}
