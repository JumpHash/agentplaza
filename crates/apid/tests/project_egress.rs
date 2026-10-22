//! A Mac node's project proxy asks the hub as the project's party (`apz_egress_rules::project_party`). The hub knows
//! the project by that CID and decides its connections by the project's state, as it decides its agents'; a party no
//! project holds is refused. Each test starts its own hub; the proxy's questions carry the internal token.

mod common;

use apz_egress_rules::project_party;
use serde_json::json;

use common::{Hub, Scratch, text};

#[test]
fn a_project_party_is_decided_by_its_project_and_an_unknown_party_is_refused() {
    let scratch = Scratch::new("project-egress");
    let hub = Hub::start(&scratch, None, None);
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Mac", "goal": "Build on the Mac" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let party = u64::from(project_party(&pid));
    let ask = |cid: u64| {
        hub.internal(
            "/internal/egress/check",
            json!({ "cid": cid, "host": "example.com", "port": 443 }),
        )
    };

    // The project works: its party may reach the host.
    let (status, answer) = ask(party);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["allow"], true, "{answer}");

    // The owner pauses the project: its party is held, with the project's reason.
    let (status, body) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{body}");
    let (_, answer) = ask(party);
    assert_eq!(answer["allow"], false, "{answer}");
    assert!(text(&answer["reason"]).contains("paused"), "{answer}");

    // Resumed, it reaches the host again.
    let (status, body) = hub.call("POST", &format!("/v1/projects/{pid}/resume"), None);
    assert_eq!(status, 200, "{body}");
    let (_, answer) = ask(party);
    assert_eq!(answer["allow"], true, "{answer}");

    // A party no project holds is refused as an unknown agent, whatever it asks.
    let stranger = u64::from(project_party("pr-nobody"));
    let (status, answer) = ask(stranger);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["allow"], false, "{answer}");
    assert!(
        text(&answer["reason"]).contains("unknown agent"),
        "{answer}"
    );
}
