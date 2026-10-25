//! CVEs, with a report (PLAN 24.3, 12.3). The advisory feeds (OSV, GitHub's advisory database, RustSec's advisory-db, and
//! the ecosystems' audits) are polled on a schedule through their connectors ([`feeds`]). Each advisory is normalised
//! ([`advisory`]: its ids and aliases, ecosystem, package, affected ranges with the ecosystem's version ordering, fixes,
//! severity and CVSS, the vulnerable functions) and matched against `main`'s lock files and every release's SBOM
//! ([`matching`]), so each finding names the affected releases and the environments that run them. Reachability says
//! whether the code reaches the vulnerable code where a tool can tell: govulncheck's traces for Go, and Weft's call
//! references to the named function for Rust. An affected `main` package gets its fix as an upgrade task through
//! `upgrades::for_advisory`, with the advisory's priority (the severity, one band lower when the code does not reach it).
//!
//! The poll runs the `cve` recipe (`recipes/builtin/cve.toml`: match, reach, fix, report). The report is per project: the
//! routes `GET /v1/projects/{id}/cves` (findings and feed records), `GET …/cves/{cid}`, `GET …/cves/export?format=json|csv`
//! and `POST …/cves/poll`. The records are kept in plaza.db (`project_cve`).

pub mod advisory;
pub mod export;
pub mod feeds;
pub mod matching;
pub mod routes;
#[cfg(test)]
mod tests;
pub mod version;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::glue::Outbound;
use crate::state::App;
use crate::upgrades::Tree;
use crate::upgrades::ecosystem::Ecosystem;
use advisory::{Advisory, rank};
use apz_topology::security::{Hit, exec};

/// The recipe a poll runs as.
pub const RECIPE: &str = "cve";
/// The milliseconds between a project's polls (six hours).
pub const POLL_MS: u64 = 6 * 60 * 60 * 1000;
/// The most new findings a poll posts to the project's chat.
const MAX_NOTICES: usize = 5;
/// The audits a poll runs on `main` when their tools are on the hub's PATH.
const AUDITS: [&str; 4] = ["cargo-audit", "npm-audit", "pip-audit", "osv-scanner"];

// ---- the records -------------------------------------------------------------------------------------------------

/// One feed's last poll: when it ran, what it gave, and why it failed (empty when it did not).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct FeedRun {
    #[table(key)]
    pub feed: String,
    pub at_ms: u64,
    pub count: u64,
    #[serde(default)]
    pub error: String,
}

/// A release a finding reaches: its version, and the environments that run that version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct AffectedRelease {
    #[table(key)]
    pub release: String,
    pub version: String,
    #[serde(default)]
    #[table(list)]
    pub environments: Vec<String>,
}

/// One finding: an advisory that reaches a package the project has (in `main`, or in a release).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Finding {
    /// `<ecosystem>:<package>:<advisory id>`.
    #[table(key)]
    pub id: String,
    /// The advisory's ids: its own first, then its aliases.
    #[table(list)]
    pub ids: Vec<String>,
    pub ecosystem: String,
    pub package: String,
    pub title: String,
    /// `critical`, `high`, `medium`, `low` or `unknown`.
    pub severity: String,
    /// The priority: the severity, one band lower when the code does not reach the vulnerable code.
    pub priority: String,
    #[serde(default)]
    pub cvss: String,
    #[serde(default)]
    pub published: String,
    pub source: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    #[table(list)]
    pub functions: Vec<String>,
    /// `main`'s version when `main` is affected, else empty; the environments on the `main` track, when it is.
    #[serde(default)]
    pub main_version: String,
    #[serde(default)]
    #[table(list)]
    pub main_environments: Vec<String>,
    /// The release that fixes `main`'s version (empty when the source names none).
    #[serde(default)]
    pub fixed: String,
    /// `reachable`, `unreachable` or `unknown`, and why.
    pub reachability: String,
    #[serde(default)]
    pub reach_note: String,
    /// `open` while an affected package is in the project, `fixed` once `main` no longer reaches it after a fix.
    pub state: String,
    /// The fix's state: empty (none asked), `started`, `task` (the upgrade's task is made), the upgrade batch's state
    /// (`current`, `tests_failed`, …), `refused`, `not_built`, `no_fix`, `plan_failed`, or `fixed`.
    #[serde(default)]
    pub fix_state: String,
    #[serde(default)]
    pub fix_reason: String,
    /// The plaza task the fix became, when it did.
    #[serde(default)]
    pub fix_task: String,
    #[serde(default)]
    #[table(child = "cve_release")]
    pub releases: Vec<AffectedRelease>,
    pub first_ms: u64,
    pub updated_ms: u64,
}

/// One project's CVE record: its feeds' last polls, its findings, and when it last polled.
#[derive(Debug, Clone, Default, Serialize, Deserialize, apz_store::Table)]
#[table(name = "project_cve")]
struct ProjectCves {
    #[table(key)]
    project: String,
    #[serde(default)]
    polled_ms: u64,
    #[serde(default)]
    #[table(child = "cve_feed")]
    feeds: Vec<FeedRun>,
    #[serde(default)]
    #[table(child = "cve_finding")]
    findings: Vec<Finding>,
}

/// Every project's CVE record.
#[derive(Default)]
pub struct Store {
    items: Mutex<Vec<ProjectCves>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The table of the CVE records: `project_cve`, one row per project.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(ProjectCves)
}

impl Store {
    fn mutate<T>(&self, project: &str, change: impl FnOnce(&mut ProjectCves) -> T) -> T {
        let mut items = lock(&self.items);
        let at = match items.iter().position(|p| p.project == project) {
            Some(at) => at,
            None => {
                items.push(ProjectCves {
                    project: project.to_string(),
                    ..ProjectCves::default()
                });
                items.len() - 1
            }
        };
        change(&mut items[at])
    }

    /// The project's findings, most severe first.
    pub fn findings(&self, project: &str) -> Vec<Finding> {
        let mut out = lock(&self.items)
            .iter()
            .find(|p| p.project == project)
            .map(|p| p.findings.clone())
            .unwrap_or_default();
        out.sort_by(|a, b| {
            rank(&b.priority)
                .cmp(&rank(&a.priority))
                .then(a.id.cmp(&b.id))
        });
        out
    }

    pub fn finding(&self, project: &str, id: &str) -> Option<Finding> {
        self.findings(project).into_iter().find(|f| f.id == id)
    }

    /// Each feed's last poll.
    pub fn feeds(&self, project: &str) -> Vec<FeedRun> {
        lock(&self.items)
            .iter()
            .find(|p| p.project == project)
            .map(|p| p.feeds.clone())
            .unwrap_or_default()
    }

    /// When the project last polled (0: never).
    pub fn polled_ms(&self, project: &str) -> u64 {
        lock(&self.items)
            .iter()
            .find(|p| p.project == project)
            .map_or(0, |p| p.polled_ms)
    }

    /// Keeps that a poll started now (so a schedule does not start it again until the next interval).
    pub fn mark_polled(&self, project: &str, now: u64) {
        self.mutate(project, |p| p.polled_ms = now);
    }

    /// Replaces the project's feeds and findings with a poll's.
    pub fn replace(&self, project: &str, feeds: Vec<FeedRun>, findings: Vec<Finding>) {
        self.mutate(project, |p| {
            p.feeds = feeds;
            p.findings = findings;
        });
    }

    /// Changes one finding (the fix's record, from the upgrade flow).
    pub fn update(&self, project: &str, id: &str, change: impl FnOnce(&mut Finding)) -> bool {
        self.mutate(project, |p| {
            match p.findings.iter_mut().find(|f| f.id == id) {
                Some(f) => {
                    change(f);
                    true
                }
                None => false,
            }
        })
    }

    /// The rows of every project's record, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(def(), lock(&self.items).iter())
    }

    /// Replaces the records with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<ProjectCves> = apz_store::table::load_list(conn, def())?;
        *lock(&self.items) = stored;
        Ok(())
    }
}

// ---- the poll ----------------------------------------------------------------------------------------------------

/// The package of a release: the release's id and version, and the SBOM of each of its artifacts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseIn {
    pub id: String,
    pub version: String,
    pub sboms: Vec<String>,
}

/// An environment and the version it runs (empty: nothing deployed), on its track.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvIn {
    pub name: String,
    pub track: String,
    pub version: String,
}

/// Reads the call graph: how many call references to a name `main`'s code holds. None when the graph is not there.
pub trait Calls {
    fn calls(&self, name: &str) -> Option<usize>;
}

/// Starts a fix: the upgrade flow's `for_advisory` in production.
pub trait Fixer {
    fn start(&self, advisory: &crate::upgrades::Advisory) -> Result<(), String>;
}

/// What a poll reads: `main`, the releases, the environments, the feeds' network, the call graph, the audits' hits on
/// `main` (by tool), govulncheck's output on `main` (None: it did not run), the findings before, and the time.
pub struct Inputs<'a> {
    pub main: &'a Tree,
    pub releases: &'a [ReleaseIn],
    pub environments: &'a [EnvIn],
    pub outbound: &'a dyn Outbound,
    pub calls: &'a dyn Calls,
    pub fixer: &'a dyn Fixer,
    pub audits: &'a [(String, Vec<Hit>)],
    pub govulncheck: Option<&'a str>,
    pub previous: &'a [Finding],
    pub now: u64,
}

/// What a poll found: the feeds' records, the findings, the ids that are new, and whether a fix was refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub feeds: Vec<FeedRun>,
    pub findings: Vec<Finding>,
    pub new: Vec<String>,
    pub refused: bool,
    /// The notes of what did not read (a lock file, an SBOM, a feed's error) and the feeds' failures.
    pub notes: Vec<String>,
}

/// The finding's id: its ecosystem, package and advisory.
pub fn finding_id(eco: Ecosystem, package: &str, advisory: &str) -> String {
    format!("{}:{package}:{advisory}", eco.name())
}

/// Polls the feeds and matches what they give against the project: one pass of the `cve` recipe's steps (match, reach,
/// fix, report). Pure over its inputs: the network, the call graph and the fixer are the ports.
pub fn poll(inputs: &Inputs) -> Report {
    let mut notes = Vec::new();
    let (main_packages, lock_notes) = matching::main_packages(inputs.main);
    notes.extend(lock_notes);
    let mut release_packages = Vec::new();
    for release in inputs.releases {
        let mut packages = Vec::new();
        for text in &release.sboms {
            match matching::sbom_packages(text) {
                Ok(found) => packages.extend(found),
                Err(e) => notes.push(format!("{}: {e}", release.id)),
            }
        }
        release_packages.push(packages);
    }
    let mut every: BTreeSet<(Ecosystem, String)> = main_packages
        .iter()
        .map(|p| (p.ecosystem, p.name.clone()))
        .collect();
    for packages in &release_packages {
        every.extend(packages.iter().map(|p| (p.ecosystem, p.name.clone())));
    }
    let (mut found, polled) = feeds::poll(inputs.outbound, &every);
    for (tool, hits) in inputs.audits {
        found.extend(feeds::audit(tool, hits));
    }
    let feed_runs: Vec<FeedRun> = polled
        .iter()
        .map(|p| FeedRun {
            feed: p.feed.clone(),
            at_ms: inputs.now,
            count: p.count as u64,
            error: p.error.clone(),
        })
        .collect();
    notes.extend(
        polled
            .iter()
            .filter(|p| !p.error.is_empty())
            .map(|p| format!("{}: {}", p.feed, p.error)),
    );
    let feeds_ok = polled.iter().all(|p| p.error.is_empty());
    let advisories = advisory::merge(found);
    let govulncheck = inputs.govulncheck.map(feeds::govulncheck_reached);
    let mut findings = Vec::new();
    let mut refused = false;
    for adv in &advisories {
        let main_hits = matching::reached(adv, &main_packages);
        let mut releases = Vec::new();
        for (release, packages) in inputs.releases.iter().zip(&release_packages) {
            // An audit's observation is about main's lock alone: it reaches no release.
            let hit = match adv.affects {
                advisory::Affects::Observed => Vec::new(),
                _ => matching::reached(adv, packages),
            };
            if let Some(first) = hit.first() {
                releases.push(AffectedRelease {
                    release: release.id.clone(),
                    version: first.version.clone(),
                    environments: inputs
                        .environments
                        .iter()
                        .filter(|e| !e.version.is_empty() && e.version == release.version)
                        .map(|e| e.name.clone())
                        .collect(),
                });
            }
        }
        if main_hits.is_empty() && releases.is_empty() {
            continue;
        }
        let id = finding_id(adv.ecosystem, &adv.package, adv.id());
        let prev = inputs.previous.iter().find(|f| f.id == id);
        let (reachability, reach_note) = reach(adv, govulncheck.as_ref(), inputs.calls);
        let priority = if reachability == "unreachable" {
            lower(&adv.severity)
        } else {
            adv.severity.clone()
        };
        let main_version = main_hits
            .first()
            .map(|p| p.version.clone())
            .unwrap_or_default();
        let fixed = if main_version.is_empty() {
            String::new()
        } else {
            adv.fix_for(&main_version).unwrap_or_default()
        };
        let mut finding = Finding {
            id: id.clone(),
            ids: adv.ids.clone(),
            ecosystem: adv.ecosystem.name().into(),
            package: adv.package.clone(),
            title: adv.title.clone(),
            severity: adv.severity.clone(),
            priority,
            cvss: adv.cvss.clone(),
            published: adv.published.clone(),
            source: adv.source.clone(),
            url: adv.url.clone(),
            functions: adv.functions.clone(),
            main_version: main_version.clone(),
            main_environments: if main_version.is_empty() {
                Vec::new()
            } else {
                inputs
                    .environments
                    .iter()
                    .filter(|e| e.track == "main")
                    .map(|e| e.name.clone())
                    .collect()
            },
            fixed: fixed.clone(),
            reachability,
            reach_note,
            state: "open".into(),
            fix_state: prev.map(|p| p.fix_state.clone()).unwrap_or_default(),
            fix_reason: prev.map(|p| p.fix_reason.clone()).unwrap_or_default(),
            fix_task: prev.map(|p| p.fix_task.clone()).unwrap_or_default(),
            releases,
            first_ms: prev.map_or(inputs.now, |p| p.first_ms),
            updated_ms: inputs.now,
        };
        if main_version.is_empty() {
            // `main` no longer reaches it: a fix that was asked for has worked.
            if matches!(
                prev.map(|p| p.fix_state.as_str()),
                Some("started" | "task" | "current")
            ) {
                finding.fix_state = "fixed".into();
                finding.state = "fixed".into();
            }
        } else if fixed.is_empty() {
            finding.fix_state = "no_fix".into();
            finding.fix_reason =
                format!("the source names no release after {main_version} that fixes it");
        } else if finding.fix_state.is_empty() || finding.fix_state == "refused" {
            // A fix is asked for once; one already started (or its task made) waits for the upgrade flow to settle it.
            if adv.ecosystem != Ecosystem::Cargo {
                finding.fix_state = "not_built".into();
                finding.fix_reason = format!(
                    "fixes of {} are not built: only Cargo has them",
                    adv.ecosystem.name()
                );
            } else {
                let fix = crate::upgrades::Advisory {
                    id: adv.id().to_string(),
                    ecosystem: Ecosystem::Cargo,
                    package: adv.package.clone(),
                    fixed: fixed.clone(),
                    priority: finding.priority.clone(),
                };
                match inputs.fixer.start(&fix) {
                    Ok(()) => {
                        finding.fix_state = "started".into();
                        finding.fix_reason = String::new();
                    }
                    Err(e) => {
                        refused = true;
                        finding.fix_state = "refused".into();
                        finding.fix_reason = e;
                    }
                }
            }
        }
        findings.push(finding);
    }
    // A finding that vanished is kept only as a fix's record, when the feeds all answered: a feed that failed says nothing
    // about what it did not return.
    if !feeds_ok {
        for prev in inputs.previous {
            if !findings.iter().any(|f| f.id == prev.id) {
                findings.push(prev.clone());
            }
        }
    } else {
        for prev in inputs.previous {
            if !findings.iter().any(|f| f.id == prev.id)
                && matches!(
                    prev.fix_state.as_str(),
                    "started" | "task" | "current" | "fixed"
                )
            {
                let mut kept = prev.clone();
                kept.state = "fixed".into();
                kept.fix_state = "fixed".into();
                kept.main_version = String::new();
                kept.main_environments = Vec::new();
                kept.updated_ms = inputs.now;
                findings.push(kept);
            }
        }
    }
    let new = findings
        .iter()
        .filter(|f| !inputs.previous.iter().any(|p| p.id == f.id))
        .map(|f| f.id.clone())
        .collect();
    Report {
        feeds: feed_runs,
        findings,
        new,
        refused,
        notes,
    }
}

/// The reachability of an advisory's vulnerable code in the project, and why. govulncheck's traces decide Go; Rust's named
/// functions are checked against the call graph's references; anything else is unknown.
fn reach(
    adv: &Advisory,
    govulncheck: Option<&Result<BTreeSet<String>, String>>,
    calls: &dyn Calls,
) -> (String, String) {
    if adv.ecosystem == Ecosystem::Go {
        return match govulncheck {
            None => (
                "unknown".into(),
                "govulncheck did not run on main (it is not on the hub's PATH)".into(),
            ),
            Some(Err(e)) => (
                "unknown".into(),
                format!("govulncheck's output did not read: {e}"),
            ),
            Some(Ok(reached)) => {
                if adv.ids.iter().any(|id| reached.contains(id)) {
                    (
                        "reachable".into(),
                        "govulncheck traces a call from main to the vulnerable code".into(),
                    )
                } else {
                    (
                        "unreachable".into(),
                        "govulncheck finds no call from main to the vulnerable code".into(),
                    )
                }
            }
        };
    }
    if adv.ecosystem == Ecosystem::Cargo && !adv.functions.is_empty() {
        let mut reached = Vec::new();
        let mut unknown = Vec::new();
        let mut silent = Vec::new();
        for function in &adv.functions {
            let name = function.rsplit("::").next().unwrap_or(function);
            match calls.calls(name) {
                Some(0) => silent.push(function.clone()),
                Some(_) => reached.push(function.clone()),
                None => unknown.push(function.clone()),
            }
        }
        return if !reached.is_empty() {
            (
                "reachable".into(),
                format!("main's code calls {}", reached.join(", ")),
            )
        } else if unknown.is_empty() {
            (
                "unreachable".into(),
                format!("the call graph has no call to {}", silent.join(", ")),
            )
        } else {
            (
                "unknown".into(),
                "the call graph is not available for main".into(),
            )
        };
    }
    (
        "unknown".into(),
        "the advisory names no vulnerable function a tool can check".into(),
    )
}

/// One band lower (a finding the code does not reach): `low` stays `low`.
fn lower(severity: &str) -> String {
    match severity {
        "critical" => "high",
        "high" => "medium",
        "medium" | "low" => "low",
        _ => "unknown",
    }
    .into()
}

// ---- the run (production) ----------------------------------------------------------------------------------------

/// A call graph read from Weft: the references to a name in `main`'s revision that are calls.
pub struct WeftCalls<'a> {
    pub app: &'a App,
    pub project: &'a str,
}

impl Calls for WeftCalls<'_> {
    fn calls(&self, name: &str) -> Option<usize> {
        let repo = self.app.projects.get(self.project)?.repo?;
        let detail = self.app.weft.get(&format!("/repos/{repo}")).ok()?;
        let rev = detail["refs"]["main"].as_str()?.to_string();
        let found = self
            .app
            .weft
            .get(&format!(
                "/revisions/{rev}/names?name={}",
                weft::server::percent_encode(name)
            ))
            .ok()?;
        Some(
            found["references"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["kind"].as_str() == Some("call"))
                .count(),
        )
    }
}

/// Starts an advisory's fix through the upgrade flow (PLAN 24.2's `for_advisory`).
pub struct UpgradeFixer {
    pub hub: Arc<App>,
    pub project: String,
}

impl Fixer for UpgradeFixer {
    fn start(&self, advisory: &crate::upgrades::Advisory) -> Result<(), String> {
        crate::upgrades::for_advisory(&self.hub, &self.project, advisory.clone())
    }
}

/// Records what an upgrade flow did with a fix this advisory asked for: its task, its batch's state and reason.
pub fn fix_settled(
    app: &App,
    project: &str,
    advisory: &crate::upgrades::Advisory,
    report: &crate::upgrades::Report,
) {
    let id = finding_id(advisory.ecosystem, &advisory.package, &advisory.id);
    app.cves.update(project, &id, |f| record_fix(f, report));
}

/// A finding's fix, as the upgrade flow's report settled it: a skip or a failed plan, else the batch's state, reason and
/// task (a batch that made its task says `task`).
pub fn record_fix(finding: &mut Finding, report: &crate::upgrades::Report) {
    if let Some(why) = &report.skipped {
        finding.fix_state = "not_built".into();
        finding.fix_reason = why.clone();
    } else if let Err(why) = &report.plan {
        finding.fix_state = "plan_failed".into();
        finding.fix_reason = why.clone();
    } else if let Some(batch) = report.batches.first() {
        finding.fix_state = if batch.state == "task" {
            "task".into()
        } else {
            batch.state.clone()
        };
        finding.fix_reason = batch.reason.clone();
        finding.fix_task = batch.task.clone();
    }
}

/// Polls the project now, on its own thread: the record, the feeds, the `cve` recipe's run and the project's chat.
pub fn start(app: &Arc<App>, project: &str) {
    static POLLING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    {
        let mut busy = lock(&POLLING);
        if !busy.insert(project.to_string()) {
            return;
        }
    }
    let (app, project) = (Arc::clone(app), project.to_string());
    std::thread::Builder::new()
        .name("cve-poll".into())
        .spawn(move || {
            run(&app, &project);
            lock(&POLLING).remove(&project);
        })
        .expect("spawn the CVE poll thread");
}

/// One poll of a project: its main, releases and environments read from the hub, its audits run on a copy of main, and the
/// poll's report kept. Returns the report (empty when the project has no repository yet).
pub fn run(app: &Arc<App>, project: &str) -> Option<Report> {
    let now = apz_plaza::now_ms();
    app.cves.mark_polled(project, now);
    let lanes = crate::upgrades::WeftLanes { app: app.as_ref() };
    let main = match crate::upgrades::Lanes::main(&lanes, project) {
        Ok(main) => main,
        Err(e) => {
            app.cves.replace(
                project,
                vec![FeedRun {
                    feed: "main".into(),
                    at_ms: now,
                    count: 0,
                    error: e,
                }],
                app.cves.findings(project),
            );
            return None;
        }
    };
    let releases: Vec<ReleaseIn> = app
        .releases
        .list(project)
        .into_iter()
        .map(|r| ReleaseIn {
            id: r.id.clone(),
            version: r.version.clone(),
            sboms: r
                .artifacts
                .iter()
                .filter(|a| !a.sbom.is_empty())
                .map(|a| a.sbom.clone())
                .collect(),
        })
        .collect();
    let environments: Vec<EnvIn> = crate::environments::list(app, project)
        .into_iter()
        .map(|e| EnvIn {
            name: e.name,
            track: e.track,
            version: e.version.unwrap_or_default(),
        })
        .collect();
    let path_var = std::env::var("PATH").unwrap_or_default();
    let (audits, govulncheck) = run_tools(&main, &path_var);
    let hub = app.handle().upgrade()?;
    let fixer = UpgradeFixer {
        hub: Arc::clone(&hub),
        project: project.to_string(),
    };
    let calls = WeftCalls {
        app: hub.as_ref(),
        project,
    };
    let previous = app.cves.findings(project);
    let report = poll(&Inputs {
        main: &main,
        releases: &releases,
        environments: &environments,
        outbound: &crate::glue::HttpsOutbound,
        calls: &calls,
        fixer: &fixer,
        audits: &audits,
        govulncheck: govulncheck.as_deref(),
        previous: &previous,
        now,
    });
    app.cves
        .replace(project, report.feeds.clone(), report.findings.clone());
    settle(app, project, &report, now);
    Some(report)
}

/// The audits and govulncheck run on a copy of `main` (the tools that are on the PATH). Each audit's hits are read by
/// 10.8's parsers; govulncheck's output is kept for the reachability check.
fn run_tools(main: &Tree, path_var: &str) -> (Vec<(String, Vec<Hit>)>, Option<String>) {
    let dir = std::env::temp_dir().join(format!("apid-cves-{}", apz_plaza::now_ms()));
    if std::fs::create_dir_all(&dir).is_err() {
        return (Vec::new(), None);
    }
    let mut audits = Vec::new();
    for (path, bytes) in main {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&target, bytes);
    }
    for tool in AUDITS {
        let outcome = apz_topology::security::audit::run_audit(tool, &dir, path_var);
        if outcome.ran {
            audits.push((tool.to_string(), outcome.hits));
        }
    }
    let govulncheck = if dir.join("go.mod").is_file() {
        exec::tool(path_var, "govulncheck").and_then(|tool| {
            exec::run(
                &tool,
                &["-json", "./..."],
                &dir,
                apz_topology::security::TOOL_LIMIT,
            )
            .ok()
            .map(|ran| ran.stdout)
        })
    } else {
        None
    };
    if let Some(out) = &govulncheck
        && let Ok(hits) = apz_topology::security::audit::parse_govulncheck(out)
    {
        audits.push(("govulncheck".to_string(), hits));
    }
    let _ = std::fs::remove_dir_all(&dir);
    (audits, govulncheck)
}

/// Runs the `cve` recipe's steps for a poll and posts what is new to the project's chat.
fn settle(app: &App, project: &str, report: &Report, now: u64) {
    if let Some(run) =
        app.recipe_runs
            .start_engine(RECIPE, "schedule", None, Some(project), None, now)
    {
        let results = [
            report.feeds.iter().all(|f| f.error.is_empty()),
            true,
            !report.refused,
            true,
        ];
        for (step, ok) in ["match", "reach", "fix", "report"].into_iter().zip(results) {
            app.recipe_runs.started(&run, step, now);
            app.recipe_runs
                .settle(&run, step, &crate::recipes::Report::verdict(ok), now);
            if !ok {
                break;
            }
        }
    }
    let chat = crate::work_loop::project_chat(app, project, now);
    // A first poll of a project with many advisories says so in a few notices, not one per finding.
    for finding in report
        .findings
        .iter()
        .filter(|f| report.new.contains(&f.id))
        .take(MAX_NOTICES)
    {
        let releases = finding.releases.len();
        let text = if finding.main_version.is_empty() {
            format!(
                "{} ({}) reaches {} in {} release(s): {}.",
                finding.ids.first().map_or("", String::as_str),
                finding.priority,
                finding.package,
                releases,
                finding.reach_note
            )
        } else {
            format!(
                "{} ({}) reaches {} {} on main ({}); fix {}: {}.",
                finding.ids.first().map_or("", String::as_str),
                finding.priority,
                finding.package,
                finding.main_version,
                finding.reachability,
                if finding.fixed.is_empty() {
                    "not named"
                } else {
                    finding.fixed.as_str()
                },
                finding.fix_state
            )
        };
        crate::work_loop::notice(app, &chat, &text, now);
    }
}

/// Schedules: a project with a repository polls when its last poll is an interval old (its first poll runs at once).
pub fn tick(app: &Arc<App>, now: u64) {
    if !app.weft.running() {
        return;
    }
    for project in app.projects.list() {
        if project.repo.is_none() || project.archived {
            continue;
        }
        let last = app.cves.polled_ms(&project.id);
        if last == 0 || now >= last + POLL_MS {
            start(app, &project.id);
        }
    }
}

/// The JSON of a finding list's counts: how many are open, fixed, and by priority.
pub fn counts(findings: &[Finding]) -> Json {
    let mut by_priority: BTreeMap<&str, usize> = BTreeMap::new();
    for f in findings {
        *by_priority.entry(f.priority.as_str()).or_default() += 1;
    }
    json!({
        "total": findings.len(),
        "open": findings.iter().filter(|f| f.state == "open").count(),
        "fixed": findings.iter().filter(|f| f.state == "fixed").count(),
        "by_priority": by_priority,
    })
}
