//! Store submissions (PLAN 22.3, 9.6): a published release is promoted to a store track, and the promotion submits its build
//! there. TestFlight (App Store Connect: the build, its processing, the beta groups, and for the external group a beta
//! review), the App Store (the build on a version, a phased release, the review submission) and Google Play (the Android
//! Publisher API: an edit, the bundle, the track with its staged rollout, the commit). `promote` on a release names the
//! track (`testflight/internal`, `play/production`, `app-store`, ...) and runs these; any other environment is a deploy
//! environment and goes to `flow::promote` as before.
//!
//! The credentials are the vault's, under each store connector's binding (`connector:app-store-connect`,
//! `connector:google-play`, as the package registries', see `registries`), and every request goes through the glue tier's
//! mediator with the store's manifest (`connectors/manifests/stores`). Each step's outcome is recorded on the release
//! (`ReleaseStoreStep`) as it finishes: a step that is done is not sent again, so a retry resumes from the failed step.
//! The promotion is written to Weft (`release.promote`) only when every step of its track is done.
//!
//! The real accounts are not run here: the tests put fake API servers behind the mediator (`tests.rs`).

mod appstore;
mod jwt;
mod play;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use weft::hash::Hash;

use super::entity::{self, Release, ReleaseArtifact, ReleaseState, ReleaseStoreStep};
use super::flow::{self, Deps};
use super::registries::{BODY_CAP, Credentials, binding_of};
use crate::connectors::manifest::{self, Manifest};
use crate::glue::{Mediator, Outbound, Reply, Secret};

/// The services a track belongs to: App Store Connect (TestFlight and the App Store) or Google Play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    AppStoreConnect,
    GooglePlay,
}

/// A store track a release is promoted to. Its name is the promotion's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    TestFlightInternal,
    TestFlightExternal,
    AppStore,
    PlayInternal,
    PlayClosed,
    PlayOpen,
    PlayProduction,
}

/// Every store track, in the order a release climbs them.
pub const TRACKS: [Track; 7] = [
    Track::TestFlightInternal,
    Track::TestFlightExternal,
    Track::AppStore,
    Track::PlayInternal,
    Track::PlayClosed,
    Track::PlayOpen,
    Track::PlayProduction,
];

impl Track {
    /// The track a promotion's environment names, or `None` for a deploy environment.
    pub fn of(name: &str) -> Option<Track> {
        TRACKS.into_iter().find(|t| t.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Track::TestFlightInternal => "testflight/internal",
            Track::TestFlightExternal => "testflight/external",
            Track::AppStore => "app-store",
            Track::PlayInternal => "play/internal",
            Track::PlayClosed => "play/closed",
            Track::PlayOpen => "play/open",
            Track::PlayProduction => "play/production",
        }
    }

    pub fn service(self) -> Service {
        match self {
            Track::TestFlightInternal | Track::TestFlightExternal | Track::AppStore => {
                Service::AppStoreConnect
            }
            _ => Service::GooglePlay,
        }
    }

    /// The store connector whose manifest and vault binding the track uses.
    fn connector(self) -> &'static str {
        match self.service() {
            Service::AppStoreConnect => "app-store-connect",
            Service::GooglePlay => "google-play",
        }
    }

    /// The track's name in the Android Publisher API: closed testing is `alpha` and open testing is `beta` there.
    fn play_name(self) -> &'static str {
        match self {
            Track::PlayInternal => "internal",
            Track::PlayClosed => "alpha",
            Track::PlayOpen => "beta",
            _ => "production",
        }
    }
}

/// What a promotion asks of its track: whether an App Store version is a phased release (on by default), and the fraction
/// of users a Play production rollout reaches (above 0 and below 1; absent is the whole track).
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub phased: bool,
    pub rollout: Option<f64>,
}

impl Options {
    /// Reads a promotion's body for `track`. `rollout` applies to Google Play's production track only.
    pub fn of(body: &Value, track: Track) -> Result<Options, String> {
        let phased = match &body["phased"] {
            Value::Null => true,
            Value::Bool(b) => *b,
            _ => return Err("phased is true or false".into()),
        };
        let rollout = match &body["rollout"] {
            Value::Null => None,
            v => {
                let fraction = v
                    .as_f64()
                    .filter(|f| *f > 0.0 && *f <= 1.0)
                    .ok_or("rollout is a fraction above 0 and at most 1")?;
                if track != Track::PlayProduction {
                    return Err(format!(
                        "{} has no staged rollout: only play/production does",
                        track.name()
                    ));
                }
                (fraction < 1.0).then_some(fraction)
            }
        };
        Ok(Options { phased, rollout })
    }
}

/// How long a step waits on the store's processing: `polls` checks, `interval` apart. A wait that runs out fails its step,
/// and a retry resumes the wait.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    pub polls: u32,
    pub interval: Duration,
}

impl Pace {
    pub const PRODUCTION: Pace = Pace {
        polls: 10,
        interval: Duration::from_secs(30),
    };
}

/// The publish step's dependencies: the transport (the hub's, in production), the vault, the clock and the processing wait.
pub struct Step<'a> {
    pub outbound: &'a dyn Outbound,
    pub vault: &'a dyn Credentials,
    pub at_ms: u64,
    pub pace: Pace,
}

/// Promotes a published release to a store track (9.6). Its steps run in order, and each is recorded on the release as it
/// finishes; the first that fails stops the run and stays on the release as failed, until a retry resumes from it. The
/// promotion is written to Weft (`release.promote`, the track as its environment) only when every step is done.
pub fn promote(
    d: &Deps,
    id: &str,
    track: Track,
    options: &Options,
    step: &Step,
) -> Result<Release, String> {
    let rel = flow::get(d, id)?;
    if rel.promotions.iter().any(|p| p.environment == track.name()) {
        return Ok(rel);
    }
    if !matches!(rel.state, ReleaseState::Published | ReleaseState::Promoted) {
        return Err("only a published release is promoted".into());
    }
    let art = artifact_of(&rel, track)?;
    let hash = art
        .digest
        .parse::<Hash>()
        .map_err(|e| format!("{}: its digest is not a hash: {e}", art.target))?;
    let bytes = d.weft.blob(&hash)?;
    let run = Run {
        releases: d.releases,
        id,
        track,
        at_ms: step.at_ms,
    };
    match track.service() {
        Service::AppStoreConnect => appstore::submit(&run, &rel, track, options, step, &bytes)?,
        Service::GooglePlay => play::submit(&run, track, options, step, &bytes)?,
    }
    flow::promote(d, id, track.name())
}

/// The artifact a track submits: the iOS build for App Store Connect, the Android bundle for Google Play.
fn artifact_of(rel: &Release, track: Track) -> Result<&ReleaseArtifact, String> {
    let wanted = |platform: &str| match track.service() {
        Service::AppStoreConnect => platform.starts_with("ios-"),
        Service::GooglePlay => platform == "android-aarch64" || platform == "android-arm64",
    };
    let what = match track.service() {
        Service::AppStoreConnect => "an iOS build",
        Service::GooglePlay => "an Android bundle",
    };
    rel.artifacts
        .iter()
        .find(|a| a.target.rsplit_once('@').is_some_and(|(_, p)| wanted(p)))
        .ok_or_else(|| format!("the release has no {what} for {}", track.name()))
}

/// One track's run: the release's store steps, read and recorded as each finishes.
pub(super) struct Run<'a> {
    releases: &'a entity::Store,
    id: &'a str,
    track: Track,
    at_ms: u64,
}

impl Run<'_> {
    /// The reference a step recorded when it was done (a done step is not sent again).
    fn done(&self, step: &str) -> Option<String> {
        let rel = self.releases.get(self.id)?;
        rel.store_steps
            .iter()
            .find(|s| s.track == self.track.name() && s.step == step && s.state == "done")
            .map(|s| s.reference.clone())
    }

    fn record(&self, step: &str, outcome: &Result<String, String>) {
        let (state, reference, reason) = match outcome {
            Ok(reference) => ("done", reference.clone(), String::new()),
            Err(reason) => ("failed", String::new(), reason.clone()),
        };
        let track = self.track.name();
        let at_ms = self.at_ms;
        self.releases.update(self.id, |r| {
            let row = ReleaseStoreStep {
                track: track.into(),
                step: step.into(),
                state: state.into(),
                reference,
                reason,
                at_ms,
            };
            // A step keeps its place in the run's order when it is sent again.
            match r
                .store_steps
                .iter_mut()
                .find(|s| s.track == track && s.step == step)
            {
                Some(existing) => *existing = row,
                None => r.store_steps.push(row),
            }
            r.updated_ms = at_ms;
        });
    }

    /// Runs a step unless it is done. Its reference is recorded; a failure is recorded and stops the run.
    pub(super) fn run(
        &self,
        step: &str,
        f: impl FnOnce() -> Result<String, String>,
    ) -> Result<String, String> {
        if let Some(reference) = self.done(step) {
            return Ok(reference);
        }
        let outcome = f();
        self.record(step, &outcome);
        outcome.map_err(|e| format!("{}: {step} failed: {e}", self.track.name()))
    }

    /// A step sent on every run (the credential exchange, whose answer expires): recorded, never resumed.
    pub(super) fn fresh<T>(
        &self,
        step: &str,
        f: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let outcome = f();
        let recorded = outcome
            .as_ref()
            .map(|_| String::new())
            .map_err(Clone::clone);
        self.record(step, &recorded);
        outcome.map_err(|e| format!("{}: {step} failed: {e}", self.track.name()))
    }
}

/// One store's API: the egress its requests may reach and the credential they carry. Every request goes through the glue
/// tier's mediator, so the egress check, the body cap and the redaction of the credential from a reply are the glue's.
pub(super) struct Api<'a> {
    pub outbound: &'a dyn Outbound,
    pub egress: Vec<String>,
    pub secret: Option<Secret>,
}

impl Api<'_> {
    /// Sends one request with its headers (the credential is added by the mediator).
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
        let mediator = Mediator {
            egress: &self.egress,
            secret: self.secret.as_ref(),
            outbound: self.outbound,
            loopback_http: false,
            body_cap: BODY_CAP,
        };
        mediator.send_bytes(method, url, &headers, body)
    }

    /// A JSON call: its answer as JSON (`null` when it is empty), or why it was refused.
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
        if reply.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&reply.body).map_err(|e| {
            format!(
                "{method} {} answered text that is not JSON: {e}",
                path_of(url)
            )
        })
    }
}

/// Ok when the reply is a success; otherwise why: the method and address (without its query), the status, and the start of
/// the answer.
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

/// The host of an https address, in lower case, with no port (a store names its upload addresses itself).
pub(super) fn https_host(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("{} is not an https address", path_of(url)))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(['@', ':']) {
        return Err(format!("{} is not a plain https address", path_of(url)));
    }
    Ok(authority.to_ascii_lowercase())
}

/// The approved manifest of a store connector (`connectors/manifests/stores`): its egress hosts, and the header its
/// credential goes in. A store's requests may reach only the hosts its manifest names.
pub(super) fn manifest_of(connector: &str) -> Result<Manifest, String> {
    manifest::stores()
        .into_iter()
        .find(|m| m.id == connector)
        .ok_or_else(|| format!("no manifest for the {connector} connector"))
}

/// The credential a store's requests carry, in the header and format its manifest names.
pub(super) fn credential(manifest: &Manifest, token: &str) -> Secret {
    Secret {
        header: manifest.auth_header.clone(),
        value: manifest.auth_format.replace("${SECRET}", token),
    }
}

/// The values of a connector's secrets, read through the vault under its binding: a secret the vault does not hold for that
/// binding fails the step that needs it.
pub(super) fn secrets(step: &Step, connector: &str, names: &[&str]) -> Result<Vec<String>, String> {
    let binding = binding_of(connector);
    names
        .iter()
        .map(|name| {
            step.vault
                .secret(name, &binding)?
                .ok_or_else(|| format!("{name} is not in the vault"))
        })
        .collect()
}

/// The id of a response document (`{"data": {"id": …}}`).
pub(super) fn data_id(doc: &Value) -> Result<String, String> {
    doc["data"]["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the answer names no id".to_string())
}

/// The id of a list entry (`{"id": …}`).
pub(super) fn item_id(item: &Value) -> Result<String, String> {
    item["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the entry names no id".to_string())
}
