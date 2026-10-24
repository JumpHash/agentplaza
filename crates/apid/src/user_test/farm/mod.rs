//! Device farms as connectors (PLAN 19.3, 8.5): a change's instrumentation suite runs on a farm's devices, and the farm's
//! results come back as the run's evidence. The farm is a connector like the stores (9.6): its manifest is under
//! `connectors/manifests/farms`, its credentials are the vault's under `connector:<id>`, and every request goes through the
//! glue tier's mediator with that manifest (the egress check, the header the credential goes in, the redaction of a reply).
//!
//! Three farms are named by the plan. BrowserStack (App Automate) is run end to end ([`browserstack`]). Firebase Test Lab
//! and AWS Device Farm have their request shapes built and tested (`firebase`, `aws`, test builds only): their runs are not
//! wired here, and a step that names one says so. The real accounts are not used: the tests put a fake farm API behind the
//! mediator (`tests.rs`).

#[cfg(test)]
pub mod aws;
pub mod browserstack;
#[cfg(test)]
pub mod firebase;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::connectors::manifest::{self, Manifest};
use crate::glue::{Mediator, Outbound, Reply, Secret};
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
}

/// One test a device ran, and its outcome (`reason` is the farm's words for a failure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub reason: String,
}

/// One device's session: the farm's id for it, the device, its outcome, the links the farm gives (its dashboard, video,
/// logs) and the tests it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub device: String,
    pub passed: bool,
    pub links: Vec<String>,
    pub tests: Vec<TestResult>,
}

/// A finished farm run: the farm's id for it and its sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub run: String,
    pub sessions: Vec<Session>,
}

impl Report {
    /// Passed when the farm ran at least one session and every session passed.
    pub fn passed(&self) -> bool {
        !self.sessions.is_empty() && self.sessions.iter().all(|s| s.passed)
    }
}

/// The run's evidence: one row per session (its device, its outcome and its links), and one per test that failed (with the
/// farm's reason). A row's screenshot and tree are empty: a farm's screens are its own, and its links point to them.
pub fn evidence(report: &Report, farm: &str) -> Vec<EvidenceRow> {
    let mut rows = Vec::new();
    for session in &report.sessions {
        let outcome = if session.passed { "passed" } else { "failed" };
        rows.push(EvidenceRow {
            seq: rows.len() as u32,
            scenario: format!("{farm} {}", session.device),
            step: 0,
            action: format!("session {}", session.id),
            tree: String::new(),
            screenshot: String::new(),
            note: format!("{outcome}; {}", session.links.join(" ")),
        });
        for test in session.tests.iter().filter(|t| !t.passed) {
            rows.push(EvidenceRow {
                seq: rows.len() as u32,
                scenario: format!("{farm} {}", session.device),
                step: 0,
                action: format!("test {}", test.name),
                tree: String::new(),
                screenshot: String::new(),
                note: format!("failed: {}", test.reason),
            });
        }
    }
    rows
}

/// Runs `config`'s suite on its devices: `app` and `tests` are the bytes of the app's build and of the instrumentation
/// package. The farm's flow is its module's; a farm not wired here is refused with the reason.
pub fn run(deps: &Deps, config: &FarmTest, app: &[u8], tests: &[u8]) -> Result<Report, String> {
    match config.farm.as_str() {
        "browserstack" => browserstack::run(deps, config, app, tests),
        other => Err(format!(
            "the {other} run is not wired on this hub: its request shapes are built and tested, and its run is not done (8.5)"
        )),
    }
}

/// One farm's API as the mediator sends it: the egress its manifest names, and the credential its manifest puts in a header.
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
