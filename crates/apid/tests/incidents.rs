//! Incidents on a scratch hub (PLAN 24.1, 12.1): a Sentry alert arrives at the hub's webhook, is checked by its signature,
//! is matched to a project by its service, and runs the incident recipe. A deploy before the alert is rolled back through
//! the deploy path to a fake Cloudflare; the triage reads the latest event from a fake Sentry API; the postmortem is written
//! to the project's docs in Weft, and its lessons are proposed into memory. Without a correlating deploy the alert opens a
//! fix task with its spec. A bad signature is refused, a duplicate is folded, and a resolved alert closes its incident.
//! The other sources' webhooks open and close incidents through their bearer secrets.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(60);
const CF_TOKEN: &str = "cf-test-token-incidents";
const SENTRY_TOKEN: &str = "sentry-test-token-incidents";
const SENTRY_SECRET: &str = "sentry-test-secret";
const SENTRY_ALERT: &[u8] = include_bytes!("../src/incidents/fixtures/sentry_issue_created.json");
const SENTRY_RESOLVED: &[u8] =
    include_bytes!("../src/incidents/fixtures/sentry_issue_resolved.json");
const SNS_V2: &str = include_str!("../src/incidents/fixtures/sns/notification_v2.json");
const LOKI_TOKEN: &str = "loki-test-token-incidents";

/// The fake Cloudflare's state: the version each Workers script serves at 100%, and the ids it hands out.
#[derive(Default)]
struct Cdn {
    next: AtomicU64,
    live: Mutex<BTreeMap<String, String>>,
}

/// The Cloudflare calls the deploy makes, answered as Workers' versions and percentage deployments.
fn cloudflare(cdn: &Cdn, method: &str, path: &str, body: &str) -> (u16, String) {
    let Some(rest) = path.split("/workers/scripts/").nth(1) else {
        return refuse(&format!("{method} {path} is not expected"));
    };
    let (script, tail) = rest.split_once('/').unwrap_or((rest, ""));
    match (method, tail) {
        ("GET", "deployments") => {
            let live = cdn.live.lock().unwrap().get(script).cloned();
            let deployments = match live {
                Some(id) => json!([{ "versions": [{ "version_id": id, "percentage": 100 }] }]),
                None => json!([]),
            };
            ok(json!({ "success": true, "result": { "deployments": deployments } }))
        }
        ("POST", "versions") => {
            let n = cdn.next.fetch_add(1, Ordering::SeqCst) + 1;
            ok(json!({ "success": true, "result": { "id": format!("v{n}") } }))
        }
        ("POST", "deployments") => {
            let split: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let full = split["versions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|v| v["percentage"] == json!(100))
                .and_then(|v| v["version_id"].as_str())
                .map(str::to_string);
            if let Some(full) = full {
                cdn.live.lock().unwrap().insert(script.to_string(), full);
            }
            ok(json!({ "success": true, "result": {} }))
        }
        _ => refuse(&format!("{method} {path} is not expected")),
    }
}

/// The Loki query API: one log line for the service the alerts name, whatever the query.
fn loki(path: &str) -> (u16, String) {
    if !path.starts_with("/loki/api/v1/query_range") {
        return (404, json!({ "status": "error" }).to_string());
    }
    ok(json!({
        "status": "success",
        "data": { "resultType": "streams", "result": [{
            "stream": { "service": "site-web" },
            "values": [["1792835900000000000", "placeOrder: stock row missing for sku 42"]]
        }] }
    }))
}

/// The Sentry API: the latest event of issue 4521187, with its exception.
fn sentry(path: &str) -> (u16, String) {
    if path != "/api/0/issues/4521187/events/latest/" {
        return (404, json!({ "detail": "not found" }).to_string());
    }
    ok(json!({
        "eventID": "e1",
        "title": "TypeError: Cannot read properties of undefined (reading 'id')",
        "entries": [{ "type": "exception", "data": { "values": [{
            "type": "TypeError",
            "value": "Cannot read properties of undefined (reading 'id') at placeOrder",
        }] } }]
    }))
}

fn ok(body: Value) -> (u16, String) {
    (200, body.to_string())
}

fn refuse(message: &str) -> (u16, String) {
    (
        404,
        json!({ "success": false, "errors": [{ "message": message }] }).to_string(),
    )
}

/// A release's place in the flow: land a change, cut its candidate, record its checks and publish it. Returns the release's
/// id and version.
fn land_and_publish(hub: &Hub, pid: &str, number: usize, note: &str) -> (String, String) {
    let (_, before) = hub.call("GET", &format!("/v1/projects/{pid}/releases"), None);
    let known: Vec<String> = before["releases"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| text(&r["id"]))
        .collect();
    let (status, up) = hub.send_bytes(
        "PUT",
        &format!("/v1/projects/{pid}/uploads"),
        note.as_bytes(),
        "application/octet-stream",
    );
    assert_eq!(status, 201, "{up}");
    let (status, added) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/files"),
        Some(json!({ "folder": "docs", "files": [{ "path": format!("note-{number}.md"), "hash": up["hash"] }] })),
    );
    assert_eq!(status, 200, "{added}\n{}", hub.log());
    let listed = hub.wait_for(&format!("/v1/projects/{pid}/releases"), WAIT, |r| {
        r["releases"]
            .as_array()
            .is_some_and(|r| r.iter().any(|r| !known.contains(&text(&r["id"]))))
    });
    let release = listed["releases"]
        .as_array()
        .and_then(|r| r.iter().find(|r| !known.contains(&text(&r["id"]))).cloned())
        .unwrap_or_else(|| panic!("release {number} is cut: {listed}\n{}", hub.log()));
    let id = text(&release["id"]);
    let version = text(&release["version"]);
    let (status, candidate) = hub.call(
        "POST",
        &format!("/v1/releases/{id}/candidate"),
        Some(json!({})),
    );
    assert_eq!(status, 200, "{candidate}\n{}", hub.log());
    let (status, checked) = hub.call(
        "POST",
        &format!("/v1/releases/{id}/checks"),
        Some(json!({ "passed": true, "reason": "the checks passed" })),
    );
    assert_eq!(status, 200, "{checked}\n{}", hub.log());
    let (status, published) = hub.call(
        "POST",
        &format!("/v1/releases/{id}/publish"),
        Some(json!({})),
    );
    assert_eq!(status, 200, "{published}\n{}", hub.log());
    (id, version)
}

/// The deploy of `version` to `env`, once it has ended, from the environments.
fn ended_deploy(envs: &Value, env: &str, version: &str) -> Option<Value> {
    envs.as_array()?.iter().find(|e| e["name"] == json!(env))?["deploys"]
        .as_array()?
        .iter()
        .find(|d| {
            d["version"] == json!(version)
                && !matches!(
                    d["status"].as_str(),
                    Some("rolling" | "recorded" | "sending" | "waiting")
                )
        })
        .cloned()
}

/// A project with a production environment that serves Workers script `site` and takes alerts for `site-web`.
fn project_with_production(hub: &Hub) -> String {
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Site", "goal": "Serve the site" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, saved) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "release_continuous": true })),
    );
    assert_eq!(status, 200, "{saved}\n{}", hub.log());
    let (status, env) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/environments/production"),
        Some(json!({
            "kind": "production",
            "track": "manual",
            "protected": false,
            "variables": {
                "CLOUDFLARE_WORKER": "site",
                "ALERT_SERVICE": "site-web",
                "DEPLOY_WINDOW_SECS": "1"
            }
        })),
    );
    assert_eq!(status, 200, "{env}\n{}", hub.log());
    pid
}

/// The Sentry webhook's path, after the owner made its token and stored the signing secret.
fn sentry_path(hub: &Hub) -> String {
    let (status, made) = hub.call(
        "POST",
        "/v1/connectors/sentry/webhook",
        Some(json!({ "secret": SENTRY_SECRET })),
    );
    assert!(status < 300, "{made}\n{}", hub.log());
    text(&made["path"])
}

/// The alert's body with its `firstSeen` taken out, so the alert starts when the hub hears it.
fn alert_body(bytes: &[u8], action: Option<&str>) -> Vec<u8> {
    let mut body: Value = serde_json::from_slice(bytes).expect("a fixture");
    if let Some(action) = action {
        body["action"] = json!(action);
    }
    if let Some(issue) = body["data"]["issue"].as_object_mut() {
        issue.remove("firstSeen");
    }
    serde_json::to_vec(&body).expect("a body")
}

fn sign(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SENTRY_SECRET.as_bytes()).expect("a key");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A hub with Cloudflare and Sentry enabled, both fakes running, and the project's production environment.
struct World {
    hub: Hub,
    pid: String,
    cdn: Arc<Cdn>,
    sentry_fake: FakeDaemon,
    loki_fake: FakeDaemon,
    _scratch: Scratch,
}

fn world(name: &str) -> World {
    let scratch = Scratch::new(name);
    let cdn = Arc::new(Cdn::default());
    let cloudflare_fake = {
        let cdn = Arc::clone(&cdn);
        FakeDaemon::serve(move |method, path, body| cloudflare(&cdn, method, path, body))
    };
    let sentry_fake = FakeDaemon::serve(|_, path, _| sentry(path));
    let loki_fake = FakeDaemon::serve(|_, path, _| loki(path));
    let loki_base = format!("http://127.0.0.1:{}", loki_fake.port);
    let artifact_path = scratch.0.join("site.bin");
    std::fs::create_dir_all(&scratch.0).unwrap();
    std::fs::write(
        &artifact_path,
        b"export default { fetch() { return new Response('ok'); } };",
    )
    .unwrap();
    let cloudflare_base = format!("http://127.0.0.1:{}/client/v4", cloudflare_fake.port);
    let sentry_base = format!("http://127.0.0.1:{}", sentry_fake.port);
    let artifact = artifact_path.to_string_lossy().into_owned();
    let hub = Hub::start_with_env(
        &scratch,
        None,
        None,
        None,
        &[
            ("APZ_CLOUDFLARE_API", cloudflare_base.as_str()),
            ("CLOUDFLARE_API_TOKEN", CF_TOKEN),
            ("CLOUDFLARE_ACCOUNT_ID", "acct-test"),
            ("SENTRY_URL", sentry_base.as_str()),
            ("SENTRY_AUTH_TOKEN", SENTRY_TOKEN),
            ("LOKI_URL", loki_base.as_str()),
            ("LOKI_TOKEN", LOKI_TOKEN),
            ("APZ_RELEASE_BUILD_STAND_IN", artifact.as_str()),
        ],
    );
    hub.wait_for("/v1/weft", WAIT, |w| w["running"] == true);
    for connector in ["cloudflare", "sentry"] {
        let (status, view) = hub.call(
            "PUT",
            &format!("/v1/connectors/{connector}"),
            Some(json!({ "enabled": true })),
        );
        assert_eq!(status, 200, "{view}\n{}", hub.log());
    }
    let pid = project_with_production(&hub);
    World {
        hub,
        pid,
        cdn,
        sentry_fake,
        loki_fake,
        _scratch: scratch,
    }
}

#[test]
fn a_sentry_alert_rolls_the_correlating_deploy_back_and_writes_the_postmortem() {
    let w = world("incidents-rollback");
    let hub = &w.hub;
    let pid = w.pid.clone();
    let (_, first) = land_and_publish(hub, &pid, 1, "# First\n");
    // Weft names an artifact by its bytes: the second build is different bytes, as a new release's would be.
    std::fs::write(
        w._scratch.0.join("site.bin"),
        b"export default { fetch() { return new Response('ok 2'); } };",
    )
    .unwrap();
    let (_, second) = land_and_publish(hub, &pid, 2, "# Second\n");
    for version in [&first, &second] {
        let (status, deploy) = hub.call(
            "POST",
            &format!("/v1/projects/{pid}/environments/production/deploy"),
            Some(json!({ "version": version })),
        );
        assert!(status < 300, "{deploy}\n{}", hub.log());
        let envs = hub.wait_for(&format!("/v1/projects/{pid}/environments"), WAIT, |e| {
            ended_deploy(e, "production", version).is_some()
        });
        assert_eq!(
            ended_deploy(&envs, "production", version).map(|d| text(&d["status"])),
            Some("passed".into()),
            "{envs}\n{}",
            hub.log()
        );
    }

    let path = sentry_path(hub);
    let body = alert_body(SENTRY_ALERT, None);
    let signature = sign(&body);
    let (status, answer) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &signature),
        ],
        &body,
    );
    assert_eq!(status, 202, "{answer}\n{}", hub.log());
    assert_eq!(
        answer["alerts"][0]["outcome"],
        json!("rollback"),
        "{answer}\n{}",
        hub.log()
    );

    // The rollback deploys the first release through the deploy path, recorded by the incident.
    // The incident's own deploy of the first release, not the owner's earlier one of the same version.
    let rollback = |e: &Value| {
        e.as_array()?
            .iter()
            .find(|e| e["name"] == json!("production"))?["deploys"]
            .as_array()?
            .iter()
            .find(|d| {
                d["version"] == json!(first)
                    && d["by"] == json!("incident")
                    && !matches!(
                        d["status"].as_str(),
                        Some("rolling" | "recorded" | "sending" | "waiting")
                    )
            })
            .cloned()
    };
    let envs = hub.wait_for(&format!("/v1/projects/{pid}/environments"), WAIT, |e| {
        rollback(e).is_some()
    });
    let back = rollback(&envs).expect("the rollback ends");
    assert_eq!(back["status"], json!("passed"), "{envs}\n{}", hub.log());
    let production = envs
        .as_array()
        .and_then(|e| e.iter().find(|e| e["name"] == json!("production")).cloned())
        .unwrap();
    assert_eq!(production["version"], json!(first), "{production}");

    // Cloudflare was sent the first release again as a new version (v3, after v1 and v2), served at 100%.
    let seen = w.cdn.live.lock().unwrap().clone();
    assert_eq!(seen.get("site").map(String::as_str), Some("v3"), "{seen:?}");

    // The incident: opened, triaged (the deploy of the second release is its suspect), acted on, told and written up.
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    let incidents = listed["incidents"].as_array().cloned().unwrap_or_default();
    assert_eq!(incidents.len(), 1, "{listed}\n{}", hub.log());
    let incident = &incidents[0];
    let id = text(&incident["id"]);
    assert_eq!(incident["status"], json!("open"));
    assert_eq!(incident["outcome"], json!("rollback"), "{incident}");
    assert_eq!(
        incident["suspect"],
        json!(format!("production@{second}")),
        "{incident}"
    );
    assert_eq!(incident["rollback"], json!(first), "{incident}");
    assert_eq!(incident["seen"], json!(1));
    assert!(
        text(&incident["logs"]).contains("Cannot read properties"),
        "{incident}"
    );
    let steps: Vec<String> = incident["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| format!("{}={}", text(&s["stage"]), text(&s["outcome"])))
        .collect();
    assert_eq!(
        steps,
        ["triage=passed", "act=passed", "tell=passed", "after=passed"],
        "{incident}"
    );
    assert_eq!(
        incident["postmortem"],
        json!(format!("docs/incidents/{id}.md")),
        "{incident}"
    );

    // The postmortem's lessons are memory proposals, and the incident shows them.
    let (_, one) = hub.call("GET", &format!("/v1/projects/{pid}/incidents/{id}"), None);
    let lessons = one["lesson_texts"].as_array().cloned().unwrap_or_default();
    assert!(
        lessons
            .iter()
            .any(|l| l["kind"] == json!("pitfall") && text(&l["text"]).contains("production")),
        "a pitfall is proposed: {one}"
    );
    assert!(
        lessons.iter().any(|l| l["kind"] == json!("decision")),
        "{one}"
    );

    // The owner is told in the project's chat and by an incident card.
    let (_, cards) = hub.call("GET", "/v1/cards", None);
    assert!(
        cards
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["kind"] == json!("incident") && c["project"] == json!(pid)),
        "{cards}"
    );

    // The source's secret is never in the hub's log.
    assert!(
        !hub.log().contains(SENTRY_TOKEN),
        "the Sentry token is in the hub's log"
    );
    // The Sentry API was asked for the issue's latest event, the token only in its header.
    let asked = w.sentry_fake.seen();
    assert!(
        asked
            .iter()
            .any(|(_, p, _)| p == "/api/0/issues/4521187/events/latest/"),
        "{asked:?}"
    );
    assert!(asked.iter().all(|(_, p, _)| !p.contains(SENTRY_TOKEN)));
}

#[test]
fn an_alert_with_no_correlating_deploy_opens_a_fix_task_with_its_spec() {
    let w = world("incidents-fix-task");
    let hub = &w.hub;
    let pid = w.pid.clone();
    let path = sentry_path(hub);
    let body = alert_body(SENTRY_ALERT, None);
    let (status, answer) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &sign(&body)),
        ],
        &body,
    );
    assert_eq!(status, 202, "{answer}\n{}", hub.log());
    assert_eq!(
        answer["alerts"][0]["outcome"],
        json!("fix_task"),
        "{answer}\n{}",
        hub.log()
    );

    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    let incident = &listed["incidents"][0];
    let task = text(&incident["task"]);
    assert!(!task.is_empty(), "{incident}");
    assert_eq!(incident["outcome"], json!("fix_task"));
    let (status, spec) = hub.call("GET", &format!("/v1/tasks/{task}"), None);
    assert_eq!(status, 200, "{spec}\n{}", hub.log());
    assert_eq!(spec["role"], json!("builder"), "{spec}");
    let goal = text(&spec["goal"]);
    assert!(
        goal.contains("TypeError: Cannot read properties of undefined"),
        "{spec}"
    );
    assert!(
        goal.contains("Cannot read properties of undefined (reading 'id') at placeOrder"),
        "the logs are in the spec: {goal}"
    );
}

#[test]
fn a_bad_signature_is_refused_a_duplicate_folds_and_a_resolved_alert_closes_its_incident() {
    let w = world("incidents-lifecycle");
    let hub = &w.hub;
    let pid = w.pid.clone();
    let path = sentry_path(hub);
    let body = alert_body(SENTRY_ALERT, None);

    let (status, refused) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", "00"),
        ],
        &body,
    );
    assert_eq!(status, 401, "{refused}");
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    assert_eq!(
        listed["incidents"].as_array().map(Vec::len),
        Some(0),
        "a refused alert opens nothing"
    );

    let (status, opened) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &sign(&body)),
        ],
        &body,
    );
    assert_eq!(status, 202, "{opened}\n{}", hub.log());
    let (status, folded) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &sign(&body)),
        ],
        &body,
    );
    assert_eq!(status, 202, "{folded}\n{}", hub.log());
    assert_eq!(folded["alerts"][0]["outcome"], json!("folded"), "{folded}");
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    let incidents = listed["incidents"].as_array().cloned().unwrap_or_default();
    assert_eq!(incidents.len(), 1, "{listed}");
    assert_eq!(incidents[0]["seen"], json!(2), "{listed}");
    let id = text(&incidents[0]["id"]);

    let resolved = alert_body(SENTRY_RESOLVED, None);
    let (status, closed) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &sign(&resolved)),
        ],
        &resolved,
    );
    assert_eq!(status, 202, "{closed}\n{}", hub.log());
    assert_eq!(
        closed["alerts"][0]["outcome"],
        json!("resolved"),
        "{closed}"
    );
    let (_, one) = hub.call("GET", &format!("/v1/projects/{pid}/incidents/{id}"), None);
    assert_eq!(one["status"], json!("resolved"), "{one}");
    assert!(one["resolved_ms"].as_u64().unwrap_or(0) > 0, "{one}");
    let messages = project_messages(hub, &pid);
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with(&format!("Incident {id}:")) && m.contains("resolved")),
        "{messages:?}"
    );

    // A resolved incident is closed: a second resolution of it finds nothing open to close.
    let (_, again) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/incidents/{id}/resolve"),
        Some(json!({})),
    );
    assert!(again.get("error").is_some(), "{again}");
}

#[test]
fn an_alertmanager_alert_opens_and_closes_through_its_bearer_secret() {
    let w = world("incidents-alertmanager");
    let hub = &w.hub;
    let pid = w.pid.clone();
    let (status, made) = hub.call(
        "POST",
        "/v1/connectors/alertmanager/webhook",
        Some(json!({ "secret": "am-secret" })),
    );
    assert!(status < 300, "{made}\n{}", hub.log());
    let path = text(&made["path"]);
    let (status, _) = hub.call(
        "PUT",
        "/v1/connectors/alertmanager",
        Some(json!({ "enabled": true })),
    );
    assert_eq!(status, 200);
    let firing = json!({
        "status": "firing",
        "receiver": "plaza",
        "alerts": [{
            "status": "firing",
            "labels": { "alertname": "HighErrorRate", "service": "site-web", "severity": "critical" },
            "annotations": { "summary": "Error ratio above 5%" },
            "startsAt": "2026-10-24T10:00:00Z",
            "endsAt": "0001-01-01T00:00:00Z",
            "fingerprint": "a1b2"
        }]
    });
    let bytes = firing.to_string().into_bytes();
    let (status, no_secret) =
        hub.send_with_headers("POST", &path, &[("authorization", "Bearer wrong")], &bytes);
    assert_eq!(status, 401, "{no_secret}");
    let (status, opened) = hub.send_with_headers(
        "POST",
        &path,
        &[("authorization", "Bearer am-secret")],
        &bytes,
    );
    assert_eq!(status, 202, "{opened}\n{}", hub.log());
    assert_eq!(
        opened["alerts"][0]["outcome"],
        json!("fix_task"),
        "{opened}\n{}",
        hub.log()
    );
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    let id = text(&listed["incidents"][0]["id"]);
    let resolved = json!({
        "status": "resolved",
        "alerts": [{
            "status": "resolved",
            "labels": { "alertname": "HighErrorRate", "service": "site-web", "severity": "critical" },
            "annotations": { "summary": "Error ratio above 5%" },
            "startsAt": "2026-10-24T10:00:00Z",
            "endsAt": "2026-10-24T10:12:00Z",
            "fingerprint": "a1b2"
        }]
    })
    .to_string()
    .into_bytes();
    let (status, closed) = hub.send_with_headers(
        "POST",
        &path,
        &[("authorization", "Bearer am-secret")],
        &resolved,
    );
    assert_eq!(status, 202, "{closed}");
    assert_eq!(
        closed["alerts"][0]["outcome"],
        json!("resolved"),
        "{closed}"
    );
    let (_, one) = hub.call("GET", &format!("/v1/projects/{pid}/incidents/{id}"), None);
    assert_eq!(one["status"], json!("resolved"), "{one}");
}

#[test]
fn an_alert_for_a_service_no_project_serves_is_acknowledged_and_opens_nothing() {
    let w = world("incidents-unmapped");
    let hub = &w.hub;
    let path = sentry_path(hub);
    let mut body: Value = serde_json::from_slice(SENTRY_ALERT).unwrap();
    body["data"]["issue"]["project"]["slug"] = json!("someone-else");
    let bytes = serde_json::to_vec(&body).unwrap();
    let (status, answer) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &sign(&bytes)),
        ],
        &bytes,
    );
    assert_eq!(status, 202, "{answer}\n{}", hub.log());
    assert_eq!(
        answer["alerts"][0]["outcome"],
        json!("unmapped"),
        "{answer}"
    );
}

/// Every message of the project's chats, in order.
fn project_messages(hub: &Hub, pid: &str) -> Vec<String> {
    let (_, chats) = hub.call("GET", &format!("/v1/chats?project={pid}"), None);
    let mut texts = Vec::new();
    for chat in chats.as_array().cloned().unwrap_or_default() {
        let (_, messages) = hub.call(
            "GET",
            &format!("/v1/chats/{}/messages", text(&chat["id"])),
            None,
        );
        let list = messages
            .as_array()
            .cloned()
            .or_else(|| messages["messages"].as_array().cloned())
            .unwrap_or_default();
        texts.extend(list.iter().map(|m| text(&m["text"])));
    }
    texts
}

/// The production environment, naming a logs connector (`LOGS_CONNECTOR`) for its incidents.
fn name_logs(hub: &Hub, pid: &str, logs: &str) {
    let (status, env) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/environments/production"),
        Some(json!({
            "kind": "production",
            "track": "manual",
            "protected": false,
            "variables": {
                "CLOUDFLARE_WORKER": "site",
                "ALERT_SERVICE": "site-web",
                "DEPLOY_WINDOW_SECS": "1",
                "LOGS_CONNECTOR": logs
            }
        })),
    );
    assert_eq!(status, 200, "{env}\n{}", hub.log());
}

#[test]
fn a_landing_the_alert_names_is_the_suspect_when_no_deploy_correlates() {
    let w = world("incidents-landing");
    let hub = &w.hub;
    let pid = w.pid.clone();
    // The owner's file drop lands src/checkout.ts on main: the landing is recorded with its file and author.
    let (status, up) = hub.send_bytes(
        "PUT",
        &format!("/v1/projects/{pid}/uploads"),
        b"export const placeOrder = () => {};\n",
        "application/octet-stream",
    );
    assert_eq!(status, 201, "{up}");
    let (status, added) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/files"),
        Some(json!({ "folder": "", "files": [{ "path": "src/checkout.ts", "hash": up["hash"] }] })),
    );
    assert_eq!(status, 200, "{added}\n{}", hub.log());
    let landings = hub.wait_for(&format!("/v1/projects/{pid}/landings"), WAIT, |l| {
        l["landings"].as_array().is_some_and(|l| !l.is_empty())
    });
    let landing = &landings["landings"][0];
    let change = text(&landing["change"]);
    assert!(
        landing["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == &json!("src/checkout.ts")),
        "{landing}"
    );
    assert_eq!(landing["author"], json!("owner"), "{landing}");

    // The Sentry alert names that file (its culprit) and no deploy happened: the landing is the suspect.
    let path = sentry_path(hub);
    let body = alert_body(SENTRY_ALERT, None);
    let (status, answer) = hub.send_with_headers(
        "POST",
        &path,
        &[
            ("sentry-hook-resource", "issue"),
            ("sentry-hook-signature", &sign(&body)),
        ],
        &body,
    );
    assert_eq!(status, 202, "{answer}\n{}", hub.log());
    assert_eq!(
        answer["alerts"][0]["outcome"],
        json!("fix_task"),
        "{answer}\n{}",
        hub.log()
    );
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    let incident = &listed["incidents"][0];
    assert_eq!(
        incident["suspect"],
        json!(format!("landing {change}")),
        "{incident}"
    );
    let task = text(&incident["task"]);
    let (_, spec) = hub.call("GET", &format!("/v1/tasks/{task}"), None);
    let goal = text(&spec["goal"]);
    assert!(
        goal.contains(&format!("Suspect change: {change}")),
        "{goal}"
    );
    assert!(
        goal.contains("src/checkout.ts"),
        "the file is named: {goal}"
    );
    assert!(goal.contains("by owner"), "the author is named: {goal}");
}

#[test]
fn an_alertmanager_alert_reads_its_service_lines_from_loki_when_the_environment_names_it() {
    let w = world("incidents-loki");
    let hub = &w.hub;
    let pid = w.pid.clone();
    let (status, _) = hub.call(
        "PUT",
        "/v1/connectors/loki",
        Some(json!({ "enabled": true })),
    );
    assert_eq!(status, 200);
    name_logs(hub, &pid, "loki");
    let (status, made) = hub.call(
        "POST",
        "/v1/connectors/alertmanager/webhook",
        Some(json!({ "secret": "am-secret" })),
    );
    assert!(status < 300, "{made}\n{}", hub.log());
    let path = text(&made["path"]);
    let (status, _) = hub.call(
        "PUT",
        "/v1/connectors/alertmanager",
        Some(json!({ "enabled": true })),
    );
    assert_eq!(status, 200);
    let firing = json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": { "alertname": "HighErrorRate", "service": "site-web", "severity": "critical" },
            "annotations": { "summary": "Error ratio above 5%" },
            "startsAt": "2026-10-24T10:00:00Z",
            "endsAt": "0001-01-01T00:00:00Z",
            "fingerprint": "loki1"
        }]
    })
    .to_string()
    .into_bytes();
    let (status, opened) = hub.send_with_headers(
        "POST",
        &path,
        &[("authorization", "Bearer am-secret")],
        &firing,
    );
    assert_eq!(status, 202, "{opened}\n{}", hub.log());
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    let incident = &listed["incidents"][0];
    assert!(
        text(&incident["logs"]).contains("placeOrder: stock row missing for sku 42"),
        "{incident}"
    );
    let task = text(&incident["task"]);
    let (_, spec) = hub.call("GET", &format!("/v1/tasks/{task}"), None);
    assert!(
        text(&spec["goal"]).contains("stock row missing"),
        "the logs are in the spec: {spec}"
    );
    // Loki was asked for the service's lines over the window, its token only in the header.
    let asked = w.loki_fake.seen();
    assert!(
        asked.iter().any(|(_, p, _)| {
            p.starts_with("/loki/api/v1/query_range")
                && p.contains("%7Bservice%3D%22site-web%22%7D")
        }),
        "{asked:?}"
    );
    assert!(
        asked.iter().all(|(_, p, _)| !p.contains(LOKI_TOKEN)),
        "the token is never in a query"
    );
    assert!(
        !hub.log().contains(LOKI_TOKEN),
        "the Loki token is in the hub's log"
    );
}

#[test]
fn a_cloudwatch_delivery_whose_certificate_is_on_a_foreign_host_is_refused_before_anything_is_fetched()
 {
    let w = world("incidents-sns-host");
    let hub = &w.hub;
    let pid = w.pid.clone();
    let (status, _) = hub.call(
        "PUT",
        "/v1/connectors/cloudwatch",
        Some(json!({ "enabled": true })),
    );
    assert_eq!(status, 200);
    let (status, made) = hub.call("POST", "/v1/connectors/cloudwatch/webhook", Some(json!({})));
    assert!(status < 300, "{made}\n{}", hub.log());
    let path = text(&made["path"]);
    let mut envelope: Value = serde_json::from_str(SNS_V2).unwrap();
    envelope["SigningCertURL"] = json!("https://evil.example.com/SimpleNotificationService.pem");
    let body = serde_json::to_vec(&envelope).unwrap();
    let (status, answer) = hub.send_with_headers(
        "POST",
        &path,
        &[("x-amz-sns-message-type", "Notification")],
        &body,
    );
    assert_eq!(status, 401, "{answer}\n{}", hub.log());
    assert!(
        text(&answer["error"]).contains("not on sns.eu-west-1.amazonaws.com"),
        "{answer}"
    );
    let (_, listed) = hub.call("GET", &format!("/v1/projects/{pid}/incidents"), None);
    assert_eq!(
        listed["incidents"].as_array().map(Vec::len),
        Some(0),
        "a refused alarm opens nothing"
    );
}
