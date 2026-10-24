//! A continuous release, end to end on a scratch hub with Weft: a project that releases continuously (its
//! `release_continuous` setting, 22.3) lands a change, and a draft release holding that change appears, cut by the
//! release recipe as the landing happens. The landing is the owner's file drop, which is no session's work, so the draft
//! does not depend on a task: each landing on main of such a project is released.

mod common;

use std::time::Duration;

use serde_json::json;

use common::{Hub, Scratch, text};

/// How long a step waits for the hub (scaled by the slack of a slow run).
const WAIT: Duration = Duration::from_secs(40);

#[test]
fn a_continuous_project_lands_a_change_and_a_draft_release_holding_it_appears() {
    let scratch = Scratch::new("continuous-release");
    let hub = Hub::start(&scratch, None, None);
    let weft = hub.wait_for("/v1/weft", WAIT, |w| w["running"] == true);
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());

    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Continuous", "goal": "Release every landing" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, saved) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "release_continuous": true })),
    );
    assert_eq!(status, 200, "{saved}\n{}", hub.log());

    let (_, before) = hub.call("GET", &format!("/v1/projects/{pid}/releases"), None);
    assert_eq!(before["releases"], json!([]), "{before}");

    // The owner drops a file: it lands in the repository as one change.
    let (status, up) = hub.send_bytes(
        "PUT",
        &format!("/v1/projects/{pid}/uploads"),
        b"# Notes\n\nFirst landing.\n",
        "application/octet-stream",
    );
    assert_eq!(status, 201, "{up}");
    let (status, added) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/files"),
        Some(json!({ "folder": "docs", "files": [{ "path": "notes.md", "hash": up["hash"] }] })),
    );
    assert_eq!(status, 200, "{added}\n{}", hub.log());

    // The draft appears: the release recipe cut it on the landing, holding the landed change.
    let listed = hub.wait_for(&format!("/v1/projects/{pid}/releases"), WAIT, |r| {
        r["releases"].as_array().is_some_and(|r| !r.is_empty())
    });
    let releases = listed["releases"].as_array().cloned().unwrap_or_default();
    assert_eq!(releases.len(), 1, "{listed}\n{}", hub.log());
    let draft = &releases[0];
    assert_eq!(draft["state"], "draft", "{draft}");
    assert_eq!(
        draft["changes"].as_array().map(Vec::len),
        Some(1),
        "{draft}"
    );
    assert!(!text(&draft["version"]).is_empty(), "{draft}");
}
