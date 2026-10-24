//! Device farms against fake farm APIs (PLAN 8.5). Each farm's API is one fake server on 127.0.0.1 that answers the run's
//! calls in order; the transport sends each farm host to it (`registries::tests::Redirect`), and every request goes through
//! the glue mediator with the farm's manifest, so the egress check, the credential's header and the signing are the hub's
//! own. The real accounts are not used. The Firebase key is generated here (`openssl genpkey`), as the Play tests do.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use super::{Deps, Pace, Report, evidence, run};
use crate::connectors::manifest;
use crate::recipes::model::FarmTest;
use crate::releases::registries::tests::{Log, Redirect, Seen, add, fake, test_vault};

const BROWSERSTACK: &str = "api-cloud.browserstack.com";
const BROWSERSTACK_MEDIA: &str = "app-automate.browserstack.com";

fn config(farm: &str, devices: &[&str]) -> FarmTest {
    FarmTest {
        farm: farm.into(),
        app: "blob-app".into(),
        tests: "blob-tests".into(),
        devices: devices.iter().map(|d| d.to_string()).collect(),
    }
}

fn quick() -> Pace {
    Pace {
        polls: 3,
        interval: Duration::ZERO,
    }
}

/// Stores each kept file under a name of its own, and answers it.
fn store_names(file: &super::Fetched) -> Result<String, String> {
    Ok(format!("blob-{}", file.name))
}

/// The fake BrowserStack. The uploads answer their addresses and the build its id. The build reads `running` on the first
/// read and ends on the second (or never ends when `endless`): one session passes, and the other fails one test with a reason.
/// The passing session has a video in its media host.
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
                            "video_url": "https://app-automate.browserstack.com/v1/s1/video.mp4",
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
            ("GET", "/v1/s1/video.mp4") => (200, "MP4-BROWSERSTACK".into()),
            _ => (404, "{}".into()),
        },
    )
}

fn browserstack_vault(name: &str, with_key: bool) -> crate::vault::Store {
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
fn a_browserstack_suite_runs_on_its_devices_and_its_results_and_video_come_back_as_evidence() {
    let (port, log) = farm_api(false);
    let redirect = Redirect::new(vec![(BROWSERSTACK, port), (BROWSERSTACK_MEDIA, port)]);
    let vault = browserstack_vault("farm-browserstack-ok", true);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: quick(),
    };
    let report = run(
        &deps,
        &config(
            "browserstack",
            &["Google Pixel 7-13.0", "Samsung Galaxy S23-13.0"],
        ),
        b"APP-BYTES",
        b"TEST-BYTES",
    )
    .unwrap();
    assert_eq!(report.run, "b1");
    assert_eq!(report.sessions.len(), 2);
    assert!(!report.passed(), "one session failed");
    // The passing session's video is downloaded whole, and nothing was skipped.
    assert_eq!(report.files.len(), 1, "{:?}", report.skipped);
    assert_eq!(report.files[0].name, "video.mp4");
    assert_eq!(report.files[0].mime, "video/mp4");
    assert_eq!(report.files[0].bytes, b"MP4-BROWSERSTACK");

    // The results come back as evidence: a row per session, a row per failed test and a row per artifact, each with its blob.
    let rows = evidence(&report, "browserstack", store_names);
    assert_eq!(rows.len(), 4, "{rows:?}");
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
    assert!(
        rows.iter().any(|r| r.action == "artifact video.mp4"
            && r.note == "video/mp4; 16 bytes; blob blob-video.mp4"),
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
            .filter(|s| s.path.starts_with("/app-automate"))
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
    let vault = browserstack_vault("farm-browserstack-nokey", false);
    let redirect = Redirect::new(vec![]);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: Pace {
            polls: 1,
            interval: Duration::ZERO,
        },
    };
    let why = run(&deps, &config("browserstack", &["d"]), b"a", b"t").unwrap_err();
    assert!(
        why.contains("BROWSERSTACK_ACCESS_KEY is not in the vault"),
        "{why}"
    );

    let (port, _log) = farm_api(true);
    let redirect = Redirect::new(vec![(BROWSERSTACK, port)]);
    let vault = browserstack_vault("farm-browserstack-endless", true);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: Pace {
            polls: 2,
            interval: Duration::ZERO,
        },
    };
    let why = run(&deps, &config("browserstack", &["d"]), b"a", b"t").unwrap_err();
    assert!(why.contains("still running after 2 checks"), "{why}");
}

/// An RSA key in PEM (PKCS#8), made by openssl, for the Firebase service account.
fn rsa_pem() -> String {
    let out = std::process::Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
        ])
        .output()
        .expect("openssl is on this machine");
    assert!(out.status.success(), "openssl genpkey failed");
    String::from_utf8(out.stdout).unwrap()
}

/// The fake Firebase: the token endpoint, the Cloud Storage upload and read, and the testing API. Its matrix reads `RUNNING`
/// first, then finishes with two executions (one passes with a video, one fails with a log).
fn firebase_api() -> (u16, Log) {
    let reads = AtomicUsize::new(0);
    fake(move |seen: &Seen| {
        let path = seen.path.as_str();
        match (seen.method.as_str(), path) {
            ("POST", "/token") => {
                let form = String::from_utf8_lossy(&seen.body);
                assert!(
                    form.contains("grant_type=urn%3Aietf"),
                    "a JWT bearer grant: {form}"
                );
                (200, json!({"access_token": "tok-1"}).to_string())
            }
            ("POST", p)
                if p.starts_with(
                    "/upload/storage/v1/b/apz-bucket/o?uploadType=media&name=apz-farm-",
                ) =>
            {
                (200, json!({"bucket": "apz-bucket"}).to_string())
            }
            ("POST", "/v1/projects/acme/testMatrices") => (
                200,
                json!({"testMatrixId": "m1", "state": "PENDING"}).to_string(),
            ),
            ("GET", "/v1/projects/acme/testMatrices/m1") => {
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    (200, json!({"state": "RUNNING"}).to_string())
                } else {
                    (200, json!({"state": "FINISHED", "testExecutions": [
                        {
                            "id": "e1",
                            "environment": {"androidDevice": {"androidModelId": "Pixel2", "androidVersionId": "28"}},
                            "outcome": {"outcomeSummary": "SUCCESS"},
                            "testDetails": {"videoUri": "https://storage.googleapis.com/apz-bucket/e1/video.mp4"}
                        },
                        {
                            "id": "e2",
                            "environment": {"androidDevice": {"androidModelId": "Nexus6", "androidVersionId": "25"}},
                            "outcome": {"outcomeSummary": "FAILURE"},
                            "testDetails": {"logUri": "https://storage.googleapis.com/apz-bucket/e2/log.txt"}
                        }
                    ]}).to_string())
                }
            }
            ("GET", "/apz-bucket/e1/video.mp4") => (200, "MP4-FIREBASE".into()),
            ("GET", "/apz-bucket/e2/log.txt") => (200, "instrumentation failed".into()),
            _ => (404, "{}".into()),
        }
    })
}

#[test]
fn a_firebase_matrix_is_signed_uploaded_run_and_its_executions_and_artifacts_come_back() {
    let (port, log) = firebase_api();
    let redirect = Redirect::new(vec![
        ("oauth2.googleapis.com", port),
        ("storage.googleapis.com", port),
        ("testing.googleapis.com", port),
    ]);
    let vault = test_vault("farm-firebase-ok", &[]);
    let account =
        json!({"client_email": "farm@acme.iam.gserviceaccount.com", "private_key": rsa_pem()});
    add(
        &vault,
        "FIREBASE_SERVICE_ACCOUNT",
        "connector:firebase-test-lab",
        &account.to_string(),
    );
    add(
        &vault,
        "FIREBASE_PROJECT_ID",
        "connector:firebase-test-lab",
        "acme",
    );
    add(
        &vault,
        "FIREBASE_STORAGE_BUCKET",
        "connector:firebase-test-lab",
        "apz-bucket",
    );
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: quick(),
    };
    let report = run(
        &deps,
        &config("firebase-test-lab", &["Pixel2:28", "Nexus6:25"]),
        b"APP-BYTES",
        b"TEST-BYTES",
    )
    .unwrap();
    assert_eq!(report.run, "m1");
    assert_eq!(report.sessions.len(), 2);
    assert_eq!(report.sessions[0].device, "Pixel2 28");
    assert!(report.sessions[0].passed && !report.sessions[1].passed);
    assert_eq!(report.files.len(), 2, "{:?}", report.skipped);
    assert_eq!(report.files[0].bytes, b"MP4-FIREBASE");
    assert_eq!(report.files[1].mime, "text/plain");

    let rows = evidence(&report, "firebase-test-lab", store_names);
    assert!(
        rows.iter()
            .any(|r| r.action == "session e2" && r.note == "failed; "),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.action == "artifact log.txt" && r.note.contains("blob-log.txt")),
        "{rows:?}"
    );

    let seen = log.lock().unwrap();
    let token_call = seen.iter().find(|s| s.path == "/token").unwrap();
    assert!(String::from_utf8_lossy(&token_call.body).contains("assertion="));
    let matrix = seen
        .iter()
        .find(|s| s.method == "POST" && s.path == "/v1/projects/acme/testMatrices")
        .unwrap();
    assert_eq!(
        matrix.headers.get("authorization").map(String::as_str),
        Some("Bearer tok-1")
    );
    let body: Value = serde_json::from_slice(&matrix.body).unwrap();
    assert!(
        body["testSpecification"]["androidInstrumentationTest"]["appApk"]["gcsPath"]
            .as_str()
            .unwrap()
            .starts_with("gs://apz-bucket/apz-farm-")
    );
    assert_eq!(
        body["environmentMatrix"]["androidDeviceList"]["androidDevices"][1]["androidVersionId"],
        "25"
    );
}

/// The fake AWS Device Farm: its one endpoint answers by `X-Amz-Target`, the uploads' presigned addresses take the bytes, and
/// the run is finished on its first read with two jobs and one video.
fn aws_api() -> (u16, Log) {
    let uploads = AtomicUsize::new(0);
    fake(move |seen: &Seen| {
        let path = seen.path.as_str();
        if seen.method == "PUT" && path.starts_with("/put/") {
            return (200, String::new());
        }
        if seen.method == "GET" && path == "/run1/video.mp4" {
            return (200, "MP4-AWS".into());
        }
        if seen.method != "POST" || path != "/" {
            return (404, "{}".into());
        }
        let target = seen
            .headers
            .get("x-amz-target")
            .map(String::as_str)
            .unwrap_or_default();
        let answer = match target {
            "DeviceFarm_20150623.CreateUpload" => {
                let n = uploads.fetch_add(1, Ordering::SeqCst);
                json!({"upload": {
                    "arn": format!("arn:up{n}"),
                    "url": format!("https://prod-us-west-2-uploads.s3-us-west-2.amazonaws.com/put/{n}"),
                    "status": "INITIALIZED"
                }})
            }
            "DeviceFarm_20150623.GetUpload" => json!({"upload": {"status": "SUCCEEDED"}}),
            "DeviceFarm_20150623.ScheduleRun" => {
                json!({"run": {"arn": "arn:run1", "status": "SCHEDULING"}})
            }
            "DeviceFarm_20150623.GetRun" => {
                json!({"run": {"arn": "arn:run1", "status": "COMPLETED", "result": "FAILED"}})
            }
            "DeviceFarm_20150623.ListJobs" => json!({"jobs": [
                {"arn": "arn:job1", "name": "Pixel 7", "result": "PASSED", "device": {"name": "Pixel 7"}},
                {"arn": "arn:job2", "name": "Galaxy", "result": "FAILED", "device": {"name": "Galaxy S23"}}
            ]}),
            "DeviceFarm_20150623.ListArtifacts" => json!({"artifacts": [
                {"name": "video.mp4", "url": "https://prod-us-west-2-results.s3-us-west-2.amazonaws.com/run1/video.mp4"}
            ]}),
            other => return (400, json!({"__type": other}).to_string()),
        };
        (200, answer.to_string())
    })
}

#[test]
fn an_aws_run_is_signed_uploaded_scheduled_read_and_its_jobs_and_video_come_back() {
    let (port, log) = aws_api();
    let redirect = Redirect::new(vec![
        ("devicefarm.us-west-2.amazonaws.com", port),
        ("prod-us-west-2-uploads.s3-us-west-2.amazonaws.com", port),
        ("prod-us-west-2-results.s3-us-west-2.amazonaws.com", port),
    ]);
    let vault = test_vault("farm-aws-ok", &[]);
    add(
        &vault,
        "AWS_ACCESS_KEY_ID",
        "connector:aws-device-farm",
        "AKIDTEST",
    );
    add(
        &vault,
        "AWS_SECRET_ACCESS_KEY",
        "connector:aws-device-farm",
        "wJalrTest",
    );
    add(
        &vault,
        "AWS_DEVICEFARM_PROJECT_ARN",
        "connector:aws-device-farm",
        "arn:project",
    );
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: quick(),
    };
    let report = run(
        &deps,
        &config("aws-device-farm", &["arn:pool"]),
        b"APP-BYTES",
        b"TEST-BYTES",
    )
    .unwrap();
    assert_eq!(report.run, "arn:run1");
    assert_eq!(report.sessions.len(), 2);
    assert_eq!(report.sessions[0].device, "Pixel 7");
    assert!(report.sessions[0].passed && !report.sessions[1].passed);
    assert_eq!(report.files.len(), 1, "{:?}", report.skipped);
    assert_eq!(
        report.files[0].session, "arn:job1",
        "the run's video is the first session's"
    );
    assert_eq!(report.files[0].bytes, b"MP4-AWS");

    let seen = log.lock().unwrap();
    let calls: Vec<&str> = seen
        .iter()
        .filter(|s| s.method == "POST")
        .filter_map(|s| s.headers.get("x-amz-target").map(String::as_str))
        .collect();
    assert_eq!(calls.first(), Some(&"DeviceFarm_20150623.CreateUpload"));
    assert!(calls.contains(&"DeviceFarm_20150623.ScheduleRun"));
    assert!(calls.contains(&"DeviceFarm_20150623.ListArtifacts"));
    // Every JSON call is SigV4-signed with the vault's key, over its own body.
    let scheduled = seen
        .iter()
        .find(|s| {
            s.headers.get("x-amz-target").map(String::as_str)
                == Some("DeviceFarm_20150623.ScheduleRun")
        })
        .unwrap();
    let auth = scheduled.headers.get("authorization").unwrap();
    assert!(
        auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDTEST/"),
        "{auth}"
    );
    assert!(
        auth.contains("/us-west-2/devicefarm/aws4_request"),
        "{auth}"
    );
    assert!(scheduled.headers.contains_key("x-amz-date"));
    let body: Value = serde_json::from_slice(&scheduled.body).unwrap();
    assert_eq!(body["devicePoolArn"], "arn:pool");
    assert_eq!(body["projectArn"], "arn:project");
    assert_eq!(body["test"]["type"], "INSTRUMENTATION");
    // The presigned PUTs carry the bytes, and are not signed.
    let put = seen.iter().find(|s| s.method == "PUT").unwrap();
    assert_eq!(put.body, b"APP-BYTES");
    assert!(!put.headers.contains_key("authorization"));
}

#[test]
fn an_unknown_farm_is_refused_by_name() {
    let vault = test_vault("farm-unknown", &[]);
    let redirect = Redirect::new(vec![]);
    let deps = Deps {
        outbound: &redirect,
        vault: &vault,
        pace: quick(),
    };
    let why = run(&deps, &config("nokia", &["d"]), b"a", b"t").unwrap_err();
    assert!(why.contains("nokia"), "{why}");
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
    assert!(browserstack.egress.iter().any(|h| h == BROWSERSTACK));
    assert!(browserstack.egress.iter().any(|h| h == BROWSERSTACK_MEDIA));
    assert_eq!(browserstack.auth_format, "Basic ${SECRET}");
    let aws = manifests
        .iter()
        .find(|m| m.id == "aws-device-farm")
        .unwrap();
    assert!(
        aws.auth_header.is_empty(),
        "AWS signs each request: no static header"
    );
}

#[test]
fn the_report_passes_only_when_every_session_passed() {
    let empty = Report::of("r".into(), vec![]);
    assert!(!empty.passed(), "a run with no session has not passed");
}
