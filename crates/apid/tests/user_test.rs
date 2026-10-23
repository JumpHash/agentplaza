//! The `test:user` step on a booted hub (PLAN 19, 8.1). The `ci` recipe gets an owner's version whose one step runs the
//! user tests of a change against a stand-in target, with the gateway's `mock` provider as all three roles (its script is
//! in the scratch directory, read by the hub's process). A seeded bug (Pay leads to an error screen) fails the run: the
//! verifier's re-run confirms the failure, and it is a `fea` finding on the change's review run. The fixed build (Pay
//! leads to the receipt) passes with no finding, and its run's evidence is in the object store.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

use common::{Hub, Scratch};

const WAIT: Duration = Duration::from_secs(60);

/// The planner's plan for both changes: one acceptance scenario (Pay reaches the receipt). The driver presses Pay; the
/// planner reads a failure as a defect.
fn script() -> Value {
    let plan = json!({ "scenarios": [{
        "id": "pay", "title": "Paying reaches the receipt", "origin": "acceptance", "symbol": "cart::pay",
        "steps": [{ "goal": "press Pay", "expect": [{ "present": "Receipt" }] }]
    }] })
    .to_string();
    json!([
        { "when": ["ROLE: planner plan", "SYMBOL: cart-bug"], "turns": [{ "text": plan }] },
        { "when": ["ROLE: planner plan", "SYMBOL: cart-ok"], "turns": [{ "text": plan }] },
        { "when": ["ROLE: planner judge"], "turns": [{ "text": "{\"verdict\": \"defect\", \"reason\": \"the receipt is not shown\"}" }] },
        { "when": ["ROLE: driver tier fast", "GOAL: press Pay"], "turns": [{ "text": "{\"action\": \"click\", \"element\": \"pay\", \"confidence\": 0.9, \"why\": \"Pay is the button\"}" }] }
    ])
}

/// The `ci` recipe's owner's version `version`: one `test:user` step over a shop whose Pay leads to `to`.
fn recipe(version: u32, to: &str) -> String {
    format!(
        r#"id = "ci"
version = {version}

[[steps]]
id = "user"
engine = "test:user"

[steps.user]
planner = "mock/script"
driver = "mock/script"
escalate = "mock/script"
checker = "mock/script"
exploratory = 0
acceptance = ["Paying reaches the receipt"]

[steps.user.stand_in]
start = "cart"

[[steps.user.stand_in.screens]]
id = "cart"
title = "Cart"
elements = [{{ id = "pay", role = "button", label = "Pay" }}]
on = [{{ element = "pay", to = "{to}" }}]

[[steps.user.stand_in.screens]]
id = "receipt"
title = "Receipt"
elements = [{{ id = "home", role = "link", label = "Back to shop" }}]

[[steps.user.stand_in.screens]]
id = "error"
title = "Error"
elements = [{{ id = "retry", role = "button", label = "Retry" }}]
"#
    )
}

/// Fires the `ci` recipe for `change` and waits for its run to end; the run's detail is the answer.
fn fire_and_wait(hub: &Hub, change: &str) -> Value {
    let (code, fired) = hub.call(
        "POST",
        "/v1/recipes/ci/fire",
        Some(json!({ "change": change })),
    );
    assert_eq!(code, 202, "{fired}");
    let path = format!("/v1/recipe-runs/{}", fired["run"].as_str().unwrap());
    hub.wait_for(&path, WAIT, |v| v["state"] != "running")
}

#[test]
fn a_test_user_step_confirms_a_seeded_bug_and_passes_the_fixed_build_on_a_booted_hub() {
    let scratch = Scratch::new("user-test");
    let script_path = scratch.0.join("mock-script.json");
    std::fs::write(&script_path, script().to_string()).expect("the mock script is written");
    let hub = Hub::start_with_env(
        &scratch,
        None,
        None,
        None,
        &[(
            "APZ_MOCK_SCRIPT",
            script_path.to_str().expect("a UTF-8 path"),
        )],
    );
    // The hub seeds its `mock` provider (the offline models the scripts answer).
    let (code, body) = hub.call("GET", "/v1/providers/mock", None);
    assert_eq!(code, 200, "{body}");

    // The seeded bug: Pay leads to the error screen. The step fails, and the failure is a confirmed finding.
    let (code, body) = hub.call(
        "PUT",
        "/v1/recipes/ci",
        Some(json!({ "toml": recipe(2, "error") })),
    );
    assert_eq!(code, 200, "{body}");
    let bug = fire_and_wait(&hub, "cart-bug");
    assert_eq!(bug["state"], json!("failed"), "{bug}");
    let check = &bug["checks"][0];
    assert_eq!(check["passed"], json!(false), "{bug}");
    assert!(
        check["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("confirmed: Paying reaches the receipt"),
        "{bug}"
    );
    let evidence = bug["evidence"]
        .as_array()
        .expect("the run keeps its evidence");
    assert!(!evidence.is_empty(), "{bug}");
    for row in evidence {
        for hash in ["tree", "screenshot"] {
            let hex = row[hash].as_str().unwrap_or_default();
            assert_eq!(
                hex.len(),
                64,
                "the {hash} of an observation is an object-store hash: {row}"
            );
        }
    }
    let review = format!("ut-{}", bug["run"].as_str().unwrap());
    let (code, run) = hub.call("GET", &format!("/v1/reviews/{review}"), None);
    assert_eq!(code, 200, "{run}");
    assert_eq!(run["state"], json!("failed"), "{run}");
    let feature = run["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|n| n["def"]["id"] == json!("user")))
        .expect("the user node is on the change's review run");
    let findings = feature["findings"]
        .as_array()
        .expect("the node lists its findings");
    assert_eq!(findings.len(), 1, "{run}");
    assert_eq!(findings[0]["kind"], json!("user-test"), "{run}");
    assert_eq!(findings[0]["verdict"], json!("confirmed"), "{run}");
    assert!(
        findings[0]["proof"]
            .as_str()
            .unwrap_or_default()
            .contains("the re-run failed again"),
        "the verifier's re-run is the proof: {run}"
    );

    // The fixed build: Pay leads to the receipt. The step passes, with no finding.
    let (code, body) = hub.call(
        "PUT",
        "/v1/recipes/ci",
        Some(json!({ "toml": recipe(3, "receipt") })),
    );
    assert_eq!(code, 200, "{body}");
    let fixed = fire_and_wait(&hub, "cart-ok");
    assert_eq!(fixed["state"], json!("passed"), "{fixed}");
    assert_eq!(fixed["checks"][0]["passed"], json!(true), "{fixed}");
    let review = format!("ut-{}", fixed["run"].as_str().unwrap());
    let (code, run) = hub.call("GET", &format!("/v1/reviews/{review}"), None);
    assert_eq!(code, 200, "{run}");
    assert_eq!(run["state"], json!("passed"), "{run}");
}

/// The `ci` recipe's version `version` with a `test:user` step whose target table is `target`.
fn recipe_with_target(version: u32, target: &str) -> String {
    format!(
        r#"id = "ci"
version = {version}

[[steps]]
id = "user"
engine = "test:user"

[steps.user]
planner = "mock/script"
driver = "mock/script"
escalate = "mock/script"
checker = "mock/script"
exploratory = 0
acceptance = ["Paying reaches the receipt"]

{target}
"#
    )
}

#[test]
fn a_web_or_terminal_target_that_cannot_run_says_why_and_fails_the_step() {
    let scratch = Scratch::new("user-test-targets");
    let script_path = scratch.0.join("mock-script.json");
    std::fs::write(&script_path, script().to_string()).expect("the mock script is written");
    let hub = Hub::start_with_env(
        &scratch,
        None,
        None,
        None,
        &[(
            "APZ_MOCK_SCRIPT",
            script_path.to_str().expect("a UTF-8 path"),
        )],
    );

    // A web target reaches the change's lane preview: a change with no preview has none to test, and the step says so.
    let (code, body) = hub.call(
        "PUT",
        "/v1/recipes/ci",
        Some(json!({ "toml": recipe_with_target(4, "[steps.user.web]\npath = \"\"") })),
    );
    assert_eq!(code, 200, "{body}");
    let web = fire_and_wait(&hub, "cart-ok");
    assert_eq!(web["state"], json!("failed"), "{web}");
    let summary = web["checks"][0]["summary"].as_str().unwrap_or_default();
    assert!(summary.starts_with("no target: "), "{web}");

    // A terminal target runs in the lane's test fork: a change that is not on main has no fork, so the step says why.
    let (code, body) = hub.call(
        "PUT",
        "/v1/recipes/ci",
        Some(json!({
            "toml": recipe_with_target(5, "[steps.user.tui]\ncommand = [\"/bin/sh\", \"-c\", \"true\"]")
        })),
    );
    assert_eq!(code, 200, "{body}");
    let terminal = fire_and_wait(&hub, "cart-ok");
    assert_eq!(terminal["state"], json!("failed"), "{terminal}");
    let summary = terminal["checks"][0]["summary"]
        .as_str()
        .unwrap_or_default();
    assert!(summary.starts_with("no target: "), "{terminal}");
}
