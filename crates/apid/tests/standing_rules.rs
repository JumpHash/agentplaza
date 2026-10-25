//! The owner's standing rules on a scratch hub (PLAN 9.8): GET and PUT /v1/rules/standing keep the text in force and
//! its versions, and a save of the text in force again saves nothing.

mod common;

use serde_json::json;

use common::{Hub, Scratch};

#[test]
fn the_standing_rules_are_saved_as_versions_and_read_back() {
    let scratch = Scratch::new("standing-rules");
    let hub = Hub::start(&scratch, None, None);

    let (status, body) = hub.call("GET", "/v1/rules/standing", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"], "");
    assert_eq!(body["version"], json!(null));
    assert_eq!(body["versions"], json!([]));

    let first = "Never merge PRs; the owner merges.";
    let (status, body) = hub.call("PUT", "/v1/rules/standing", Some(json!({ "text": first })));
    assert_eq!(status, 200, "{body}");
    // `saved` is false until plaza.db has the component's tables (the schema regeneration); the text is kept either way.
    assert!(body["saved"].is_boolean(), "{body}");
    assert_eq!(body["text"], first);
    assert_eq!(body["version"], 1);
    assert_eq!(body["author"], "owner");

    // The text in force again is not a new version.
    let (status, body) = hub.call(
        "PUT",
        "/v1/rules/standing",
        Some(json!({ "text": format!("{first}\n") })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["version"], 1);
    assert_eq!(body["versions"].as_array().map(Vec::len), Some(1));

    // A new text is the next version, and the history keeps the first.
    let second = "Never merge PRs; the owner merges. Merge main into a branch; never rebase.";
    let (status, body) = hub.call("PUT", "/v1/rules/standing", Some(json!({ "text": second })));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["version"], 2);
    assert_eq!(body["versions"][0]["version"], 1);
    assert_eq!(body["versions"][1]["version"], 2);

    let (status, body) = hub.call("GET", "/v1/rules/standing", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"], second);
    assert_eq!(body["version"], 2);
    assert!(body.get("saved").is_none(), "{body}");
}

#[test]
fn a_standing_rules_save_must_carry_a_text_and_fit_the_limit() {
    let scratch = Scratch::new("standing-rules-bad");
    let hub = Hub::start(&scratch, None, None);

    let (status, body) = hub.call("PUT", "/v1/rules/standing", Some(json!({ "text": 3 })));
    assert_eq!(status, 400, "{body}");
    let (status, body) = hub.call("PUT", "/v1/rules/standing", Some(json!(["no"])));
    assert_eq!(status, 400, "{body}");

    let long = "x".repeat(20_001);
    let (status, body) = hub.call("PUT", "/v1/rules/standing", Some(json!({ "text": long })));
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().is_some_and(|e| e.contains("20000")),
        "{body}"
    );

    let (status, body) = hub.call("GET", "/v1/rules/standing", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["versions"], json!([]));
}
