//! Device farms as connectors (PLAN 19.3, 8.5): a change's instrumentation suite runs on a farm's devices, and the farm's
//! results come back as the run's evidence: the outcome of each session and each failed test, the links, and the artifacts
//! (a video, a log), which are downloaded into the hub's object store. A farm is a connector like the stores (9.6): its
//! manifest is under `connectors/manifests/farms`, its credentials are the vault's under `connector:<id>`, and every request
//! goes through the glue tier's mediator with that manifest (the egress check, the header the credential goes in, the
//! redaction of a text reply).
//!
//! The three farms of the plan are run end to end: BrowserStack (App Automate, `browserstack`), Firebase Test Lab
//! (`firebase`, a service account's JWT exchanged for a token, the packages in Cloud Storage, a test matrix) and AWS Device
//! Farm (`aws`, each call signed with SigV4 by apz-gateway's signer). The real accounts are not used: the tests put a fake
//! farm API behind the mediator (`tests.rs`).

pub mod aws;
pub mod browserstack;
pub mod firebase;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::connectors::manifest::{self, Manifest};
use crate::glue::{ARTIFACT_CAP, Mediator, Outbound, Reply, Secret};
use crate::recipes::model::FarmTest;
use crate::recipes::runs::EvidenceRow;
use crate::releases::registries::{BODY_CAP, Credentials, binding_of};

/// What a farm run reads and writes: the transport (the hub's, in production), the vault and the wait between polls.
pub struct Deps<'a> {
    pub outbound: &'a dyn Outbound,
    pub vault: &'a dyn Credentials,
    pub pace: Pace,
}

/// How long a run waits on the farm: `polls` checks, `interval` apart.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    pub polls: u32,
    pub interval: Duration,
}

impl Pace {
    /// A device run takes minutes: the farm is asked every 30 seconds for an hour.
    pub const PRODUCTION: Pace = Pace {
        polls: 120,
        interval: Duration::from_secs(30),
    };

    /// Sleeps between two checks, and not after the last.
    pub(super) fn wait(&self, poll: u32) {
        if poll + 1 < self.polls {
            std::thread::sleep(self.interval);
        }
    }
}

/// The most artifacts one run keeps: a video and a log for each of a few sessions.
pub const MAX_FILES: usize = 8;

/// One test a device ran, and its outcome (`reason` is the farm's words for a failure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub reason: String,
}

/// An artifact a farm gives a session: its name, and the address it is downloaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub url: String,
}

/// One device's session: the farm's id for it, the device, its outcome, the links the farm gives (its dashboard), the tests
/// it ran and the artifacts it made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub device: String,
    pub passed: bool,
    pub links: Vec<String>,
    pub tests: Vec<TestResult>,
    pub artifacts: Vec<Artifact>,
}

/// An artifact downloaded from the farm: the session it belongs to, its name, its media type and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub session: String,
    pub device: String,
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A finished farm run: the farm's id for it, its sessions, the artifacts it kept and the artifacts it did not (with why).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub run: String,
    pub sessions: Vec<Session>,
    pub files: Vec<Fetched>,
    pub skipped: Vec<String>,
}

impl Report {
    /// A report of the farm's sessions, before its artifacts are downloaded.
    pub fn of(run: String, sessions: Vec<Session>) -> Report {
        Report {
            run,
            sessions,
            files: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// Passed when the farm ran at least one session and every session passed.
    pub fn passed(&self) -> bool {
        !self.sessions.is_empty() && self.sessions.iter().all(|s| s.passed)
    }
}

/// Runs `config`'s suite on its farm: `app` and `tests` are the bytes of the app's build and of the instrumentation package.
pub fn run(deps: &Deps, config: &FarmTest, app: &[u8], tests: &[u8]) -> Result<Report, String> {
    match config.farm.as_str() {
        "browserstack" => browserstack::run(deps, config, app, tests),
        "firebase-test-lab" => firebase::run(deps, config, app, tests),
        "aws-device-farm" => aws::run(deps, config, app, tests),
        other => Err(format!("{other} is not a device farm this hub runs")),
    }
}

/// The run's evidence rows: one per session (its device, its outcome and its links), one per failed test (with the farm's
/// reason), one per artifact kept (stored by `store`, which answers the blob it went to) and one per artifact not kept. A
/// row's screenshot is the blob of an image, and empty otherwise (a farm's screens are its own).
pub fn evidence(
    report: &Report,
    farm: &str,
    mut store: impl FnMut(&Fetched) -> Result<String, String>,
) -> Vec<EvidenceRow> {
    let mut rows: Vec<EvidenceRow> = Vec::new();
    let mut push = |scenario: String, action: String, screenshot: String, note: String| {
        rows.push(EvidenceRow {
            seq: rows.len() as u32,
            scenario,
            step: 0,
            action,
            tree: String::new(),
            screenshot,
            note,
        });
    };
    for session in &report.sessions {
        let outcome = if session.passed { "passed" } else { "failed" };
        push(
            format!("{farm} {}", session.device),
            format!("session {}", session.id),
            String::new(),
            format!("{outcome}; {}", session.links.join(" ")),
        );
        for test in session.tests.iter().filter(|t| !t.passed) {
            push(
                format!("{farm} {}", session.device),
                format!("test {}", test.name),
                String::new(),
                format!("failed: {}", test.reason),
            );
        }
    }
    for file in &report.files {
        let (note, screenshot) = match store(file) {
            Ok(blob) => {
                let picture = if file.mime.starts_with("image/") {
                    blob.clone()
                } else {
                    String::new()
                };
                (
                    format!("{}; {} bytes; blob {blob}", file.mime, file.bytes.len()),
                    picture,
                )
            }
            Err(why) => (format!("not stored: {why}"), String::new()),
        };
        push(
            format!("{farm} {}", file.device),
            format!("artifact {}", file.name),
            screenshot,
            note,
        );
    }
    for skipped in &report.skipped {
        push(
            farm.to_string(),
            "artifact".into(),
            String::new(),
            format!("not kept: {skipped}"),
        );
    }
    rows
}

/// Downloads a report's artifacts through the farm's API, so that the mediator checks each address (its host is in the
/// manifest's egress). The first `MAX_FILES` are kept, each at most `ARTIFACT_CAP` bytes; the rest are named in `skipped`.
pub(super) fn collect(api: &Api, mut report: Report) -> Report {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for session in &report.sessions {
        for artifact in &session.artifacts {
            if files.len() >= MAX_FILES {
                skipped.push(format!(
                    "{} of {}: the run keeps {MAX_FILES} files",
                    artifact.name, session.device
                ));
                continue;
            }
            match download(api, &artifact.url) {
                Ok(bytes) => files.push(Fetched {
                    session: session.id.clone(),
                    device: session.device.clone(),
                    name: artifact.name.clone(),
                    mime: mime_of(&artifact.name).into(),
                    bytes,
                }),
                Err(why) => skipped.push(format!("{} of {}: {why}", artifact.name, session.device)),
            }
        }
    }
    report.files = files;
    report.skipped = skipped;
    report
}

/// One artifact's bytes: a GET through the mediator. A reply larger than [`ARTIFACT_CAP`] is refused, not cut.
fn download(api: &Api, url: &str) -> Result<Vec<u8>, String> {
    let reply = api.send("GET", url, &[], &[])?;
    answered("GET", url, &reply)?;
    if reply.bytes.len() > ARTIFACT_CAP {
        return Err(format!("larger than {} MiB", ARTIFACT_CAP >> 20));
    }
    Ok(reply.bytes)
}

/// The media type of an artifact, by its name's extension.
pub fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or_default() {
        "mp4" => "video/mp4",
        "png" => "image/png",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Seconds since the epoch, for a token's times.
pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Nanoseconds since the epoch: a name that no earlier upload of the hub has.
pub(super) fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// One farm's API as the mediator sends it: the egress its manifest names, and the credential its manifest puts in a header
/// (none for a farm that signs each request).
pub(super) struct Api<'a> {
    pub outbound: &'a dyn Outbound,
    pub egress: Vec<String>,
    pub secret: Option<Secret>,
}

impl Api<'_> {
    /// One request with its headers (the credential is added by the mediator). The reply is the transport's.
    pub(super) fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<Reply, String> {
        let headers: BTreeMap<String, String> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Mediator {
            egress: &self.egress,
            secret: self.secret.as_ref(),
            outbound: self.outbound,
            loopback_http: false,
            body_cap: BODY_CAP,
        }
        .send_bytes(method, url, &headers, body)
    }

    /// A JSON call: its answer as JSON, or why it was refused.
    pub(super) fn json(
        &self,
        method: &str,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, String> {
        let text = body.map(Value::to_string).unwrap_or_default();
        let headers: &[(&str, &str)] = if body.is_some() {
            &[("Content-Type", "application/json")]
        } else {
            &[]
        };
        let reply = self.send(method, url, headers, text.as_bytes())?;
        answered(method, url, &reply)?;
        serde_json::from_str(&reply.body).map_err(|e| {
            format!(
                "{method} {} answered text that is not JSON: {e}",
                path_of(url)
            )
        })
    }
}

/// Ok when the reply is a success; otherwise the method, the address (without its query), the status and the start of the
/// answer.
pub(super) fn answered(method: &str, url: &str, reply: &Reply) -> Result<(), String> {
    if (200..300).contains(&reply.status) {
        return Ok(());
    }
    Err(format!(
        "{method} {} answered {}: {}",
        path_of(url),
        reply.status,
        reply.body.chars().take(200).collect::<String>()
    ))
}

fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// The approved manifest of a farm connector (`connectors/manifests/farms`).
pub(super) fn manifest_of(connector: &str) -> Result<Manifest, String> {
    manifest::farms()
        .into_iter()
        .find(|m| m.id == connector)
        .ok_or_else(|| format!("no manifest for the {connector} connector"))
}

/// The credential a farm's requests carry, in the header and format its manifest names.
pub(super) fn credential(manifest: &Manifest, value: &str) -> Secret {
    Secret {
        header: manifest.auth_header.clone(),
        value: manifest.auth_format.replace("${SECRET}", value),
    }
}

/// The values of a farm's secrets, read through the vault under the connector's binding. A secret the vault does not hold
/// fails the run that needs it.
pub(super) fn secrets(deps: &Deps, connector: &str, names: &[&str]) -> Result<Vec<String>, String> {
    let binding = binding_of(connector);
    names
        .iter()
        .map(|name| {
            deps.vault
                .secret(name, &binding)?
                .ok_or_else(|| format!("{name} is not in the vault"))
        })
        .collect()
}
