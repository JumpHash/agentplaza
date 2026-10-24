//! Device farms against fake farm APIs (PLAN 8.5). The BrowserStack host is one fake server on 127.0.0.1 that answers the
//! run's calls in order; the transport sends the host to it (`registries::tests::Redirect`), and every request goes through
//! the glue mediator with the farm's manifest, so the egress check and the credential's header are the hub's own. The real
//! accounts are not used.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use super::{Deps, Pace, evidence, run};
use crate::connectors::manifest;
use crate::recipes::model::FarmTest;
use crate::releases::registries::tests::{Log, Redirect, Seen, add, fake, test_vault};

const HOST: &str = "api-cloud.browserstack.com";

fn config(farm: &str) -> FarmTest {
    FarmTest {
        farm: farm.into(),
        app: "blob-app".into(),
        tests: "blob-tests".into(),
        devices: vec![
            "Google Pixel 7-13.0".into(),
            "Samsung Galaxy S23-13.0".into(),
        ],
    }
}

/// The fake BrowserStack. The uploads answer their addresses and the build its id. The build reads `running` on the first
/// read and ends on the second (or never ends when `endless`): one session passes, and the other fails one test with a reason.
fn farm_api(endless: bool) -> (u16, Log) {
    let reads = AtomicUsize::new(0);
    fake(
        move |seen: &Seen| match (seen.method.as_str(), seen.path.as_str()) {
            ("POST", "/app-automate/upload") => (200, json!({"app_url": "bs://app1"}).to_string()),
            ("POST", "/app-automate/espresso/v2/test-suite") => {
                (200, json!({"test_suite_url": "bs://suite1"}).to_string())
            }
            ("POST", "/app-automate/espresso/v2/build") => (
                200,
                json!({"build_id": "b1", "message": "Success"}).to_string(),
            ),
            ("GET", "/app-automate/espresso/v2/builds/b1") => {
                let read = reads.fetch_add(1, Ordering::SeqCst);
                if endless || read == 0 {
                    (200, json!({"status": "running"}).to_string())
                } else {
                    (
                    200,
                    json!({"status": "done", "sessions": [
                        {
                            "id": "s1",
                            "device": "Google Pixel 7-13.0",
                            "status": "passed",
                            "public_url": "https://app-automate.browserstack.com/s1",
                            "test_cases": [{"name": "checkout_shows_receipt", "status": "passed"}]
                        },
                        {
                            "id": "s2",
                            "device": "Samsung Galaxy S23-13.0",
                            "status": "failed",
                            "public_url": "https://app-automate.browserstack.com/s2",
                            "test_cases": [{
                                "name": "checkout_shows_receipt",
                                "status": "failed",
                                "reason": "no Receipt on screen"
                            }]
                        }
                    ]})
                    .to_string(),
                )
                }
            }
            _ => (404, "{}".into()),
        },
    )
}

fn credentials(name: &str, with_key: bool) -> crate::vault::Store {
    let vault = test_vault(name, &[]);
    add(
        &vault,
        "BROWSERSTACK_USERNAME",
        "connector:browserstack",
        "alice",
    );
    if with_key {
        add(
            &vault,
            "BROWSERSTACK_ACCESS_KEY",
            "connector:browserstack",
            "k3y",
        );
    }
    vault
}

#[test]
fn a_browserstack_suite_runs_on_its_devices_and_its_results_are_the_run_evidence() {
    let (port, log) = farm_api(false);
    let redirect = Redirect::new(vec![(HOST, port)]);
    let vault = credentials("farm-browserstack-ok", true);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: Pace {
            polls: 3,
            interval: Duration::ZERO,
        },
    };
    let report = run(&deps, &config("browserstack"), b"APP-BYTES", b"TEST-BYTES").unwrap();
    assert_eq!(report.run, "b1");
    assert_eq!(report.sessions.len(), 2);
    assert!(!report.passed(), "one session failed");

    // The results come back as evidence: a row per session with its links, and a row per failed test with its reason.
    let rows = evidence(&report, "browserstack");
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0].note,
        "passed; https://app-automate.browserstack.com/s1"
    );
    assert_eq!(rows[1].scenario, "browserstack Samsung Galaxy S23-13.0");
    assert!(
        rows.iter()
            .any(|r| r.action == "test checkout_shows_receipt"
                && r.note == "failed: no Receipt on screen"),
        "{rows:?}"
    );
    assert!(rows.iter().enumerate().all(|(i, r)| r.seq == i as u32));

    // Every request carries the vault's credential in the manifest's header, and the uploads carry the bytes.
    let seen = log.lock().unwrap();
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("alice:k3y")
    );
    assert!(
        seen.iter()
            .all(|s| s.headers.get("authorization") == Some(&basic))
    );
    let app = seen
        .iter()
        .find(|s| s.path == "/app-automate/upload")
        .unwrap();
    assert!(String::from_utf8_lossy(&app.body).contains("APP-BYTES"));
    let build = seen
        .iter()
        .find(|s| s.path == "/app-automate/espresso/v2/build")
        .unwrap();
    let body: Value = serde_json::from_slice(&build.body).unwrap();
    assert_eq!(body["app"], "bs://app1");
    assert_eq!(body["testSuite"], "bs://suite1");
    assert_eq!(body["devices"][1], "Samsung Galaxy S23-13.0");
    let reads = seen
        .iter()
        .filter(|s| s.path == "/app-automate/espresso/v2/builds/b1")
        .count();
    assert_eq!(reads, 2, "the build is polled until it ends");
}

#[test]
fn a_missing_key_and_a_build_that_never_ends_say_why() {
    let vault = credentials("farm-browserstack-nokey", false);
    let redirect = Redirect::new(vec![]);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: Pace {
            polls: 1,
            interval: Duration::ZERO,
        },
    };
    let why = run(&deps, &config("browserstack"), b"a", b"t").unwrap_err();
    assert!(
        why.contains("BROWSERSTACK_ACCESS_KEY is not in the vault"),
        "{why}"
    );

    let (port, _log) = farm_api(true);
    let redirect = Redirect::new(vec![(HOST, port)]);
    let vault = credentials("farm-browserstack-endless", true);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: Pace {
            polls: 2,
            interval: Duration::ZERO,
        },
    };
    let why = run(&deps, &config("browserstack"), b"a", b"t").unwrap_err();
    assert!(why.contains("still running after 2 checks"), "{why}");
}

#[test]
fn a_farm_whose_run_is_not_wired_is_refused_by_name() {
    let vault = credentials("farm-other", true);
    let redirect = Redirect::new(vec![]);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: Pace {
            polls: 1,
            interval: Duration::ZERO,
        },
    };
    for farm in ["firebase-test-lab", "aws-device-farm"] {
        let why = run(&deps, &config(farm), b"a", b"t").unwrap_err();
        assert!(why.contains(farm) && why.contains("not wired"), "{why}");
    }
}

#[test]
fn the_farm_manifests_pass_the_checks_and_name_their_hosts() {
    let manifests = manifest::farms();
    assert_eq!(manifests.len(), 3);
    for m in &manifests {
        assert_eq!(manifest::check(m), Vec::<String>::new(), "{}", m.id);
        assert!(!m.egress.is_empty(), "{}", m.id);
    }
    let browserstack = manifests.iter().find(|m| m.id == "browserstack").unwrap();
    assert_eq!(browserstack.egress, [HOST]);
    assert_eq!(browserstack.auth_format, "Basic ${SECRET}");
}
