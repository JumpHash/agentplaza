//! A release through the deploy recipe on a scratch hub (PLAN 23, 11.1 and 11.4): a release is cut, checked and published
//! through the hub's own routes (the build is the one stand-in, `APZ_RELEASE_BUILD_STAND_IN`), and it goes to a staging
//! environment and then to a protected production one against a fake Cloudflare. A fake monitoring source (a Prometheus-style
//! query API) is steady for the first release. For the second, the source's error rate rises: the production deploy is rolled
//! back on its own to the first release, a rollback deploy is recorded, an incident card is opened, and the project's chat
//! hears each step.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(60);
const TOKEN: &str = "cf-test-token-health";
const PROM_TOKEN: &str = "prom-test-token-health";

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
            // A partial split (a stage below 100%) leaves the live version as it is; a full one makes its version live.
            if let Some(full) = full {
                cdn.live.lock().unwrap().insert(script.to_string(), full);
            }
            ok(json!({ "success": true, "result": {} }))
        }
        _ => refuse(&format!("{method} {path} is not expected")),
    }
}

/// The monitoring source: a Prometheus query answer for the error ratio and for the p95 latency (in seconds).
fn monitoring(gauge: &Mutex<(f64, f64)>, path: &str) -> (u16, String) {
    let (error_rate, p95_seconds) = *gauge.lock().unwrap();
    let value = if path.contains("histogram_quantile") {
        p95_seconds
    } else {
        error_rate
    };
    let body = json!({
        "status": "success",
        "data": { "resultType": "vector", "result": [{ "metric": {}, "value": [1, value.to_string()] }] }
    });
    ok(body)
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

/// A release's place in the flow: land a change (a file drop on a continuous project), take the draft it cuts, cut its
/// candidate, record its checks (ci passed) and publish it. Returns the release's id and version.
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
    // The newest listing comes first; the draft this landing cut is the one that was not there before.
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
    assert_eq!(published["state"], "published", "{published}");
    (id, version)
}

/// The deploy of `version` to `env`, once it has ended (passed, rolled back or failed), from the environments.
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

fn environment_version(envs: &Value, env: &str) -> Value {
    envs.as_array()
        .and_then(|e| e.iter().find(|e| e["name"] == json!(env)).cloned())
        .map(|e| e["version"].clone())
        .unwrap_or(Value::Null)
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

#[test]
fn a_published_release_deploys_through_staging_to_production_and_a_breach_rolls_back_on_its_own() {
    let scratch = Scratch::new("deploy-health");
    let cdn = Arc::new(Cdn::default());
    let gauge = Arc::new(Mutex::new((0.001_f64, 0.12_f64)));
    let cloudflare_fake = {
        let cdn = Arc::clone(&cdn);
        FakeDaemon::serve(move |method, path, body| cloudflare(&cdn, method, path, body))
    };
    let monitoring_fake = {
        let gauge = Arc::clone(&gauge);
        FakeDaemon::serve(move |_, path, _| monitoring(&gauge, path))
    };
    let artifact_path = scratch.0.join("site.bin");
    std::fs::create_dir_all(&scratch.0).unwrap();
    std::fs::write(
        &artifact_path,
        b"export default { fetch() { return new Response('ok'); } };",
    )
    .unwrap();
    let cloudflare_base = format!("http://127.0.0.1:{}/client/v4", cloudflare_fake.port);
    let monitoring_base = format!("http://127.0.0.1:{}", monitoring_fake.port);
    let artifact = artifact_path.to_string_lossy().into_owned();
    let hub = Hub::start_with_env(
        &scratch,
        None,
        None,
        None,
        &[
            ("APZ_CLOUDFLARE_API", cloudflare_base.as_str()),
            ("CLOUDFLARE_API_TOKEN", TOKEN),
            ("CLOUDFLARE_ACCOUNT_ID", "acct-test"),
            ("PROMETHEUS_URL", monitoring_base.as_str()),
            ("PROMETHEUS_TOKEN", PROM_TOKEN),
            ("APZ_RELEASE_BUILD_STAND_IN", artifact.as_str()),
        ],
    );
    hub.wait_for("/v1/weft", WAIT, |w| w["running"] == true);

    // The two connectors the deploy uses are enabled by the owner.
    for connector in ["cloudflare", "prometheus"] {
        let (status, view) = hub.call(
            "PUT",
            &format!("/v1/connectors/{connector}"),
            Some(json!({ "enabled": true })),
        );
        assert_eq!(status, 200, "{view}\n{}", hub.log());
    }

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

    // Staging follows each published release; production is protected and waits for a passing staging deploy.
    for (name, kind, protected, worker) in [
        ("staging", "staging", false, "site-staging"),
        ("production", "production", true, "site"),
    ] {
        let (status, env) = hub.call(
            "PUT",
            &format!("/v1/projects/{pid}/environments/{name}"),
            Some(json!({
                "kind": kind,
                "track": "releases",
                "protected": protected,
                "variables": { "CLOUDFLARE_WORKER": worker, "DEPLOY_WINDOW_SECS": "1" }
            })),
        );
        assert_eq!(status, 200, "{env}\n{}", hub.log());
    }

    // The first release: published, it goes to staging on its own, and passes (the source is steady).
    let (_, first) = land_and_publish(&hub, &pid, 1, "# First\n");
    let envs = hub.wait_for(&format!("/v1/projects/{pid}/environments"), WAIT, |e| {
        ended_deploy(e, "staging", &first).is_some()
    });
    let staged = ended_deploy(&envs, "staging", &first)
        .unwrap_or_else(|| panic!("staging deploys {first}: {envs}\n{}", hub.log()));
    assert_eq!(staged["status"], "passed", "{staged}");

    // Production takes it from staging's pass, and its health holds.
    let (status, deploy) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/environments/production/deploy"),
        Some(json!({ "version": first })),
    );
    assert!(status < 300, "{deploy}\n{}", hub.log());
    let envs = hub.wait_for(&format!("/v1/projects/{pid}/environments"), WAIT, |e| {
        ended_deploy(e, "production", &first).is_some()
    });
    assert_eq!(
        ended_deploy(&envs, "production", &first).map(|d| text(&d["status"])),
        Some("passed".into()),
        "{envs}\n{}",
        hub.log()
    );
    assert_eq!(environment_version(&envs, "production"), json!(first));

    // The second release goes to staging and passes; then the source's error rate rises for production's deploy of it.
    // Weft names an artifact by its bytes: the second build is different bytes, as a new release's would be.
    std::fs::write(
        &artifact_path,
        b"export default { fetch() { return new Response('ok 2'); } };",
    )
    .unwrap();
    let (_, second) = land_and_publish(&hub, &pid, 2, "# Second\n");
    let envs = hub.wait_for(&format!("/v1/projects/{pid}/environments"), WAIT, |e| {
        ended_deploy(e, "staging", &second).is_some()
    });
    assert_eq!(
        ended_deploy(&envs, "staging", &second).map(|d| text(&d["status"])),
        Some("passed".into()),
        "{envs}"
    );
    *gauge.lock().unwrap() = (0.5, 0.12);

    let (status, deploy) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/environments/production/deploy"),
        Some(json!({ "version": second })),
    );
    assert!(status < 300, "{deploy}\n{}", hub.log());
    let envs = hub.wait_for(&format!("/v1/projects/{pid}/environments"), WAIT, |e| {
        ended_deploy(e, "production", &second).is_some()
    });
    let breached = ended_deploy(&envs, "production", &second)
        .unwrap_or_else(|| panic!("production ends {second}: {envs}\n{}", hub.log()));
    assert_eq!(breached["status"], "rolled_back", "{breached}");
    assert!(
        text(&breached["detail"]).contains("error rate 50.0% (at most 5.0%)"),
        "{breached}"
    );

    // The production environment is back on the first release, and the rollback is a deploy of its own.
    assert_eq!(
        environment_version(&envs, "production"),
        json!(first),
        "{envs}"
    );
    let production = envs
        .as_array()
        .and_then(|e| e.iter().find(|e| e["name"] == json!("production")).cloned())
        .unwrap();
    let rollback = production["deploys"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["by"] == json!("rollback"))
        .cloned()
        .unwrap_or_else(|| panic!("a rollback deploy is recorded: {production}"));
    assert_eq!(rollback["version"], json!(first), "{rollback}");

    // Cloudflare was sent the first release's live version back at 100%: the same version production served before.
    let seen = cloudflare_fake.seen();
    let splits: Vec<Value> = seen
        .iter()
        .filter(|(m, p, _)| m == "POST" && p.ends_with("/workers/scripts/site/deployments"))
        .map(|(_, _, body)| serde_json::from_str(body).unwrap_or(Value::Null))
        .collect();
    let full = |split: &Value| {
        split["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["percentage"] == json!(100))
            .map(|v| text(&v["version_id"]))
            .unwrap_or_default()
    };
    let first_live = full(splits.first().expect("production's first deploy split"));
    let last_full = full(splits.last().expect("the rollback split"));
    assert_eq!(
        last_full, first_live,
        "the rollback serves the first release again: {splits:?}"
    );
    assert!(
        splits.last().unwrap()["versions"].as_array().unwrap().len() == 1,
        "the rollback serves one version: {splits:?}"
    );

    // The incident is opened: a card in the project's chat with the breach's numbers.
    let (_, cards) = hub.call("GET", "/v1/cards", None);
    let incident = cards
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["kind"] == json!("incident") && c["project"] == json!(pid))
        .cloned()
        .unwrap_or_else(|| panic!("an incident is opened: {cards}\n{}", hub.log()));
    assert!(text(&incident["title"]).contains(&second), "{incident}");
    assert!(
        text(&incident["body"]).contains("error rate 50.0%"),
        "{incident}"
    );

    // The project's chat is told each step: the rollouts, the rollback and how the production deploy ended.
    let messages = project_messages(&hub, &pid);
    for expected in [
        format!("Deploying {first} to staging"),
        format!("Deployed {first} to staging"),
        format!("Deploying {first} to production"),
        format!("Deployed {first} to production"),
        format!("Deploying {second} to production"),
        format!("Rolled production back after a failed check on {second}"),
    ] {
        assert!(
            messages.iter().any(|m| m.starts_with(&expected)),
            "the chat says {expected:?}: {messages:?}"
        );
    }
    // The fake monitoring source was asked the two queries of the health watch, with its token only in the header.
    let monitored = monitoring_fake.seen();
    assert!(
        monitored
            .iter()
            .any(|(_, p, _)| p.contains("histogram_quantile")),
        "the latency is read"
    );
    assert!(monitored.iter().all(|(_, p, _)| !p.contains(PROM_TOKEN)));
    assert!(
        !hub.log().contains(TOKEN),
        "the Cloudflare token is in the hub's log"
    );
    assert!(
        !hub.log().contains(PROM_TOKEN),
        "the monitoring token is in the hub's log"
    );
}
