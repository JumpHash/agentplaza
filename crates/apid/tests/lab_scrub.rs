//! The scrubbed production copy of a lab run over the hub's routes (PLAN 22.5, 10.13 round 2): a run whose source has an
//! unruled email column is refused with the scan's counts and no copy; with the project's rule the copy is kept for the
//! run, and it is gone when the run ends. The source's URL is the vault's (`connector:<source>`), never in the lab.

mod common;

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use common::{Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(30);
const RULE: &str = "[[column]]\ntable = \"users\"\ncolumn = \"email\"\naction = \"fake\"\n";

/// A production snapshot: users with emails, and nothing else personal.
fn snapshot(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("the snapshot opens");
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);
         INSERT INTO users VALUES (1, 'ana.keller@example.com'), (2, 'bo@example.org');",
    )
    .expect("the snapshot is seeded");
}

/// Lands `files` on the repository's main (the hub's own Weft).
fn land(hub: &Hub, repo: &str, title: &str, files: &[(&str, &str)]) {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|(path, text)| json!({ "path": path, "text": text }))
        .collect();
    let (status, body) = hub.call(
        "POST",
        &format!("/v1/weft/repos/{repo}/edits"),
        Some(json!({ "title": title, "files": files })),
    );
    assert!(
        status == 200 || status == 201,
        "{status} {body}\n{}",
        hub.log()
    );
}

#[test]
fn an_unruled_email_column_refuses_the_run_and_with_the_rule_the_copy_lasts_as_long_as_the_run() {
    let scratch = Scratch::new("lab-scrub");
    let hub = Hub::start(&scratch, None, None);
    hub.wait_for("/v1/weft", WAIT, |w| w["running"] == true);

    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Scrubbed", "goal": "Its production data", "kind": "code" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let project = hub.wait_for(&format!("/v1/projects/{pid}"), WAIT, |p| {
        p["repo"].is_string()
    });
    let repo = text(&project["repo"]);

    // The source's URL is in the vault, bound to its connector: the hub reads it, the lab never does.
    let snap = scratch.0.join("production-snapshot.sqlite");
    snapshot(&snap);
    let (status, body) = hub.call(
        "POST",
        "/v1/vault/secrets",
        Some(json!({
            "name": "SCRUB_SOURCE_URL",
            "kind": "generic",
            "label": "the production snapshot",
            "value": format!("sqlite:{}", snap.display()),
            "scope": { "kind": "plaza" },
            "bindings": ["connector:prod-db"],
        })),
    );
    assert!((200..300).contains(&status), "{status} {body}");

    let (status, run) = hub.call(
        "POST",
        "/v1/test-lab/runs",
        Some(json!({"project": pid, "channel": "stable", "cpus": 1, "memory_mib": 512})),
    );
    assert_eq!(status, 201, "{run}");
    let id = text(&run["id"]);
    let copy_path = hub
        .state_dir()
        .join("lab-runs")
        .join(&id)
        .join("scrub")
        .join("production.db");

    // No rules on main: the email column is unruled, the scan stops the copy and nothing is kept.
    land(
        &hub,
        &repo,
        "The project's first change",
        &[("README.md", "scrubbed")],
    );
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/scrub"),
        Some(json!({ "source": "prod-db" })),
    );
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refused["state"], "refused");
    let found = refused["report"]["findings"]
        .as_array()
        .expect("the findings");
    assert!(
        found.iter().any(|f| f["table"] == "users"
            && f["column"] == "email"
            && f["kind"] == "email"
            && f["samples"] == 2),
        "{refused}"
    );
    assert!(
        !refused.to_string().contains("example.com"),
        "the report names no value: {refused}"
    );
    assert!(!copy_path.exists(), "a refused copy keeps nothing");

    // The rule lands on main: the same run's copy passes the scan and is kept.
    land(
        &hub,
        &repo,
        "The project's scrubbing rules",
        &[(".plaza/scrub.toml", RULE)],
    );
    let (status, copied) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/scrub"),
        Some(json!({ "source": "prod-db" })),
    );
    assert_eq!(status, 201, "{copied}");
    assert_eq!(copied["state"], "copied");
    assert!(copy_path.exists(), "the copy is kept for the run");
    let kept = rusqlite::Connection::open(&copy_path).expect("the copy opens");
    let original: i64 = kept
        .query_row(
            "SELECT count(*) FROM users WHERE email = 'ana.keller@example.com'",
            [],
            |r| r.get(0),
        )
        .expect("the query runs");
    assert_eq!(original, 0, "the email is faked");

    // What the scans found is kept with the run, as counts.
    let (status, listed) = hub.call("GET", &format!("/v1/test-lab/runs/{id}/scrub"), None);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(
        listed["copies"].as_array().map(Vec::len),
        Some(2),
        "{listed}"
    );
    assert!(!listed.to_string().contains("example.com"), "{listed}");

    // The run ends: its copies go with it.
    let (status, _) = hub.call("POST", &format!("/v1/test-lab/runs/{id}/cancel"), None);
    assert_eq!(status, 200);
    assert!(!copy_path.exists(), "the copy is gone when the run ends");
    assert!(!copy_path.parent().expect("the copy's directory").exists());

    // A run that is done takes no copy.
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/scrub"),
        Some(json!({ "source": "prod-db" })),
    );
    assert_eq!(status, 409);
}
