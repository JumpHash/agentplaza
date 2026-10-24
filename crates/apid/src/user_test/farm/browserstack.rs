//! BrowserStack (App Automate, PLAN 19.3, 8.5): the app and its instrumentation package are uploaded, a build runs the package
//! on the devices, the build is polled until it ends, and its sessions are read as the run's results.
//!
//! The credential is the vault's pair (`BROWSERSTACK_USERNAME`, `BROWSERSTACK_ACCESS_KEY`), and the request carries
//! `Authorization: Basic base64(user:key)`, which the manifest's `Basic ${SECRET}` puts in the header.
//!
//! The request shapes are ASSUMED from BrowserStack's public App Automate documentation, not checked against the live API
//! (no account is used here). Assumed: the uploads are `multipart/form-data` with the file in a field `file`, answering
//! `app_url` (`/app-automate/upload`) and `test_suite_url` (`/app-automate/espresso/v2/test-suite`); the build is a JSON
//! `POST /app-automate/espresso/v2/build` with `app`, `testSuite` and `devices`, answering `build_id`; the build is read
//! with `GET /app-automate/espresso/v2/builds/{build_id}`, whose `status` is `running` or `queued` until it ends, and whose
//! `sessions` each name their `id`, `device`, `status` (`passed` for a pass), `public_url`, `video_url`, `logs_url` and `test_cases` (`name`, `status`,
//! `reason`). Any of these that differs from the live API is a change to this file only.

use base64::Engine as _;
use serde_json::{Value, json};

use super::{
    Api, Artifact, Deps, Report, Session, TestResult, answered, collect, credential, manifest_of,
    secrets,
};
use crate::recipes::model::FarmTest;

const BASE: &str = "https://api-cloud.browserstack.com";
const CONNECTOR: &str = "browserstack";
/// The multipart boundary of the uploads: a fixed string (the uploads are APKs, binary files in which it does not occur in
/// practice; a file that held it would break its form, and that is not checked here).
const BOUNDARY: &str = "apz-farm-7f3a9c";

/// Runs the suite on BrowserStack: upload, build, poll, read.
pub fn run(deps: &Deps, config: &FarmTest, app: &[u8], tests: &[u8]) -> Result<Report, String> {
    let values = secrets(
        deps,
        CONNECTOR,
        &["BROWSERSTACK_USERNAME", "BROWSERSTACK_ACCESS_KEY"],
    )?;
    let basic =
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", values[0], values[1]));
    let approved = manifest_of(CONNECTOR)?;
    let api = Api {
        outbound: deps.outbound,
        egress: approved.egress.clone(),
        secret: Some(credential(&approved, &basic)),
    };
    let app_url = upload(&api, "/app-automate/upload", "app.apk", app, "app_url")?;
    let suite_url = upload(
        &api,
        "/app-automate/espresso/v2/test-suite",
        "tests.apk",
        tests,
        "test_suite_url",
    )?;
    let build = api.json(
        "POST",
        &format!("{BASE}/app-automate/espresso/v2/build"),
        Some(&build_request(&app_url, &suite_url, &config.devices)),
    )?;
    let build_id = build["build_id"]
        .as_str()
        .ok_or("the build names no build_id")?
        .to_string();
    let url = format!("{BASE}/app-automate/espresso/v2/builds/{build_id}");
    for poll in 0..deps.pace.polls {
        let doc = api.json("GET", &url, None)?;
        if !running(&doc) {
            return Ok(collect(&api, report(&build_id, &doc)));
        }
        if poll + 1 < deps.pace.polls {
            std::thread::sleep(deps.pace.interval);
        }
    }
    Err(format!(
        "build {build_id} is still running after {} checks",
        deps.pace.polls
    ))
}

/// The build's request: the uploaded app and suite, and the devices by the farm's names.
pub fn build_request(app_url: &str, suite_url: &str, devices: &[String]) -> Value {
    json!({"app": app_url, "testSuite": suite_url, "devices": devices})
}

/// Uploads one file as a multipart form, and answers the address the farm gives it (`field`).
fn upload(api: &Api, path: &str, name: &str, bytes: &[u8], field: &str) -> Result<String, String> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let url = format!("{BASE}{path}");
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    let reply = api.send("POST", &url, &[("Content-Type", &content_type)], &body)?;
    answered("POST", &url, &reply)?;
    let doc: Value = serde_json::from_str(&reply.body)
        .map_err(|e| format!("the upload answered text that is not JSON: {e}"))?;
    doc[field]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("the upload names no {field}"))
}

/// Whether a build is still going (its status is `running` or `queued`).
fn running(doc: &Value) -> bool {
    matches!(doc["status"].as_str(), Some("running") | Some("queued"))
}

/// The report of a finished build: its sessions, with each one's tests and artifacts.
pub fn report(build_id: &str, doc: &Value) -> Report {
    let sessions = doc["sessions"]
        .as_array()
        .map(|list| list.iter().map(session).collect())
        .unwrap_or_default();
    Report::of(build_id.to_string(), sessions)
}

fn session(s: &Value) -> Session {
    let tests = s["test_cases"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|t| TestResult {
                    name: t["name"].as_str().unwrap_or_default().to_string(),
                    passed: t["status"].as_str() == Some("passed"),
                    reason: t["reason"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    // The video and the log of a session are downloaded into the run (the names of these fields are assumed, see above).
    let artifacts = [("video.mp4", "video_url"), ("logs.txt", "logs_url")]
        .into_iter()
        .filter_map(|(name, field)| {
            s[field].as_str().map(|url| Artifact {
                name: name.to_string(),
                url: url.to_string(),
            })
        })
        .collect();
    Session {
        id: s["id"].as_str().unwrap_or_default().to_string(),
        device: s["device"].as_str().unwrap_or_default().to_string(),
        passed: s["status"].as_str() == Some("passed"),
        links: s["public_url"]
            .as_str()
            .map(|u| vec![u.to_string()])
            .unwrap_or_default(),
        tests,
        artifacts,
    }
}
