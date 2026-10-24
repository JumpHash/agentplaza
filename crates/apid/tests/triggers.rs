//! Triggers on a scratch hub (PLAN 14.6, 5.2): a labelled GitHub issue, delivered to the connector's webhook by the test
//! itself, starts one run of the recipe, however many deliveries of it come in the window. The deliveries are signed as
//! GitHub signs them (HMAC-SHA256 of the body under the webhook's secret); a wrong token, a wrong signature and other
//! actions are refused or ignored. The schedules' times (a cron expression, an interval, the engine watch) are tested
//! with a fake clock in `crates/apid/src/triggers/mod.rs`; this file checks that the hub's routes keep them.

mod common;

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use common::{Hub, Scratch};

/// The window the trigger waits: long enough that the test's deliveries all land in it, short enough to wait out.
const WINDOW_MS: u64 = 2_000;
const SECRET: &str = "webhook-test-secret";

/// GitHub's signature of a body under `secret`.
fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

/// A `labeled` delivery of issue 7 of `acme/app`, with `label`.
fn labelled(label: &str) -> Vec<u8> {
    json!({
        "action": "labeled",
        "label": { "name": label },
        "issue": { "number": 7, "title": "Crash on start" },
        "repository": { "full_name": "acme/app" },
    })
    .to_string()
    .into_bytes()
}

fn runs(hub: &Hub) -> Vec<Value> {
    let (code, body) = hub.call("GET", "/v1/recipe-runs", None);
    assert_eq!(code, 200, "{body}");
    body["runs"].as_array().cloned().unwrap_or_default()
}

#[test]
fn three_deliveries_of_a_labelled_issue_in_the_window_start_one_run() {
    let scratch = Scratch::new("triggers");
    let hub = Hub::start(&scratch, None, None);

    // The recipe's trigger: an issue labelled `agent` on GitHub, debounced for a window.
    let (code, trigger) = hub.call(
        "POST",
        "/v1/recipes/intake/triggers",
        Some(json!({
            "source": "connector:github",
            "event": "issue.labelled",
            "label": "agent",
            "window_ms": WINDOW_MS,
        })),
    );
    assert_eq!(code, 201, "{trigger}");
    assert_eq!(trigger["action"], "run");

    // The connector's webhook: a token in the vault (shown once in the path) and the App's signing secret.
    let (code, hook) = hub.call(
        "POST",
        "/v1/connectors/github/webhook",
        Some(json!({ "secret": SECRET })),
    );
    assert_eq!(code, 201, "{hook}");
    let path = hook["path"].as_str().expect("the webhook path").to_string();
    assert!(path.starts_with("/v1/hooks/github/"), "{path}");

    // A wrong token is not found; a wrong signature is refused; neither starts anything.
    let wrong = path.rsplit_once('/').unwrap().0.to_string() + "/not-the-token";
    let (code, _) = hub.send_with_headers(
        "POST",
        &wrong,
        &[("x-github-event", "issues")],
        &labelled("agent"),
    );
    assert_eq!(code, 404);
    let (code, _) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("x-github-event", "issues"),
            (
                "x-hub-signature-256",
                &signature("another-secret", &labelled("agent")),
            ),
        ],
        &labelled("agent"),
    );
    assert_eq!(code, 401);
    assert!(runs(&hub).is_empty(), "a refused delivery starts no run");

    // Three signed deliveries of the same labelled issue, inside the window.
    for _ in 0..3 {
        let body = labelled("agent");
        let sig = signature(SECRET, &body);
        let (code, reply) = hub.send_with_headers(
            "POST",
            &path,
            &[("x-github-event", "issues"), ("x-hub-signature-256", &sig)],
            &body,
        );
        assert_eq!(code, 202, "{reply}");
        assert_eq!(reply["triggers"], 1, "{reply}");
    }

    // An action that is not `labeled` is acknowledged and ignored, and a label the trigger does not name matches nothing.
    let opened = json!({ "action": "opened", "issue": { "number": 7 }, "repository": { "full_name": "acme/app" } })
        .to_string()
        .into_bytes();
    let sig = signature(SECRET, &opened);
    let (code, reply) = hub.send_with_headers(
        "POST",
        &path,
        &[("x-github-event", "issues"), ("x-hub-signature-256", &sig)],
        &opened,
    );
    assert_eq!(code, 202);
    assert!(reply["ignored"].is_string(), "{reply}");
    let other = labelled("bug");
    let sig = signature(SECRET, &other);
    let (_, reply) = hub.send_with_headers(
        "POST",
        &path,
        &[("x-github-event", "issues"), ("x-hub-signature-256", &sig)],
        &other,
    );
    assert_eq!(reply["triggers"], 0, "{reply}");

    // The window closes and the trigger starts one run of the recipe for the issue.
    let listed = hub.wait_for("/v1/recipe-runs", Duration::from_secs(20), |v| {
        v["runs"].as_array().is_some_and(|r| !r.is_empty())
    });
    let started = listed["runs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        started.len(),
        1,
        "three deliveries in the window are one run: {listed}"
    );
    assert_eq!(started[0]["recipe"], "intake");
    assert_eq!(started[0]["trigger"], "connector:github");
    assert_eq!(started[0]["subject"], "acme/app#7");

    // A later delivery of the same item waits: its run is still going.
    let body = labelled("agent");
    let sig = signature(SECRET, &body);
    hub.send_with_headers(
        "POST",
        &path,
        &[("x-github-event", "issues"), ("x-hub-signature-256", &sig)],
        &body,
    );
    std::thread::sleep(Duration::from_millis(WINDOW_MS + 4_000));
    assert_eq!(runs(&hub).len(), 1, "one run per item and recipe at a time");

    // The owner's hand starts a run of the same recipe at once.
    let (code, run) = hub.call("POST", "/v1/recipes/intake/fire", Some(json!({})));
    assert_eq!(code, 202, "{run}");
    assert_eq!(run["trigger"], "owner");
    assert_eq!(runs(&hub).len(), 2);
}

#[test]
fn a_schedule_is_a_cron_expression_or_an_interval_and_a_bad_one_is_refused() {
    let scratch = Scratch::new("triggers-schedule");
    let hub = Hub::start(&scratch, None, None);

    let (code, daily) = hub.call(
        "POST",
        "/v1/recipes/intake/triggers",
        Some(json!({ "cron": "30 9 * * 1-5" })),
    );
    assert_eq!(code, 201, "{daily}");
    assert_eq!(daily["source"], "schedule");
    assert!(
        daily["next_ms"].as_u64().is_some(),
        "a schedule has its next fire: {daily}"
    );

    let (code, every) = hub.call(
        "POST",
        "/v1/recipes/intake/triggers",
        Some(json!({ "every_minutes": 15 })),
    );
    assert_eq!(code, 201, "{every}");

    let (code, _) = hub.call(
        "POST",
        "/v1/recipes/intake/triggers",
        Some(json!({ "cron": "61 9 * * *" })),
    );
    assert_eq!(code, 400);
    let (code, _) = hub.call(
        "POST",
        "/v1/recipes/intake/triggers",
        Some(json!({ "cron": "30 9 * * *", "event": "issue.labelled", "source": "connector:github" })),
    );
    assert_eq!(code, 400, "a trigger is a schedule or an event, not both");

    // A recipe whose steps wait for a task cannot be started by a trigger.
    let (code, _) = hub.call(
        "POST",
        "/v1/recipes/change/triggers",
        Some(json!({ "every_minutes": 15 })),
    );
    assert_eq!(code, 409);

    let (code, listed) = hub.call("GET", "/v1/recipes/intake/triggers", None);
    assert_eq!(code, 200);
    assert_eq!(
        listed["triggers"].as_array().map(Vec::len),
        Some(2),
        "{listed}"
    );

    // The engine watch and the full suite are schedules the scheduler owns: the watch's record is listed by the hub.
    let (_, watch) = hub.call("GET", "/v1/recipes/engine-watch/triggers", None);
    let watch = watch["triggers"].as_array().cloned().unwrap_or_default();
    assert_eq!(watch.len(), 1, "{watch:?}");
    assert_eq!(watch[0]["every_minutes"], 1440);
    assert_eq!(watch[0]["action"], "engine_watch");
}

/// A code project and its repo on the hub's own Weft (the hub starts it), once Weft answers.
fn code_project(hub: &Hub) -> (String, String) {
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Landings", "goal": "A repo to land changes on", "kind": "code" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = project["id"]
        .as_str()
        .expect("the project's id")
        .to_string();
    let project = hub.wait_for(
        &format!("/v1/projects/{pid}"),
        Duration::from_secs(30),
        |p| p["repo"].is_string(),
    );
    let repo = project["repo"]
        .as_str()
        .expect("the project's repo")
        .to_string();
    (pid, repo)
}

/// The owner's edit of one file, which lands at once on the repo's main: the change's id.
fn land_edit(hub: &Hub, repo: &str, title: &str, path: &str) -> String {
    let (status, body) = hub.call(
        "POST",
        &format!("/v1/weft/repos/{repo}/edits"),
        Some(json!({ "title": title, "files": [{ "path": path, "text": "hello\n" }] })),
    );
    assert!(
        status == 200 || status == 201,
        "{status} {body}\n{}",
        hub.log()
    );
    body["change"]
        .as_str()
        .expect("the landed change")
        .to_string()
}

#[test]
fn a_landing_on_the_hubs_weft_starts_one_run_of_the_recipe_whose_trigger_names_it() {
    let scratch = Scratch::new("triggers-landing");
    let hub = Hub::start(&scratch, None, None);
    let (pid, repo) = code_project(&hub);
    let (code, trigger) = hub.call(
        "POST",
        "/v1/recipes/intake/triggers",
        Some(json!({
            "source": "weft",
            "event": "change.landed",
            "project": pid,
            "window_ms": 0,
        })),
    );
    assert_eq!(code, 201, "{trigger}");
    // The hub reads Weft's event feed from the moment Weft runs: let the feed connect before the landing.
    std::thread::sleep(Duration::from_secs(3));

    let first = land_edit(&hub, &repo, "Land the readme", "README.md");
    let listed = hub.wait_for("/v1/recipe-runs", Duration::from_secs(30), |v| {
        v["runs"].as_array().is_some_and(|r| !r.is_empty())
    });
    let started = listed["runs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        started.len(),
        1,
        "one landing, one run: {listed}\n{}",
        hub.log()
    );
    assert_eq!(started[0]["trigger"], "weft");
    assert_eq!(started[0]["project"], pid.as_str());
    assert_eq!(started[0]["subject"], format!("{repo}#{first}").as_str());

    // The same landing is taken once: after the window and the feed's next passes, still one run.
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(runs(&hub).len(), 1, "{}", hub.log());

    // A second landing is a second event: one more run, for its own change.
    let second = land_edit(&hub, &repo, "Land the licence", "LICENSE");
    let listed = hub.wait_for("/v1/recipe-runs", Duration::from_secs(30), |v| {
        v["runs"].as_array().is_some_and(|r| r.len() >= 2)
    });
    let started = listed["runs"].as_array().cloned().unwrap_or_default();
    assert_eq!(started.len(), 2, "{listed}\n{}", hub.log());
    assert!(
        started
            .iter()
            .any(|r| r["subject"] == format!("{repo}#{second}").as_str()),
        "{listed}"
    );
}
