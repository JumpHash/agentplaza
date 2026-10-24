//! Dependency upgrades (PLAN 24.2): the `upgrade` recipe. On a schedule, each ecosystem the project has (its manifests at
//! the root of `main`, and its tool on the hub's PATH) is planned against its registry: the patch and minor updates of
//! one ecosystem are one batch, and each major update is a change of its own. A batch is applied to a copy of `main`,
//! its changelogs are read (the registry's metadata, then the repository's CHANGELOG where it is reachable), its tests run
//! (the selected ones for a patch or minor batch, the whole suite for a major), and its changed files are reviewed: a
//! patch or minor batch may change only the lockfile. A batch that passes is opened as a Weft change, its changelog is its
//! intent, and it goes to the landing queue; it lands by itself when the project allows (`upgrade_auto_land`), and waits
//! for the owner's approval otherwise. A major always waits.
//!
//! A CVE that needs an upgrade enters through [`for_advisory`] (PLAN 12.3 feeds it): the same flow, for the one crate and
//! the fixed release. Only Cargo has the advisory path here; the other ecosystems say so as their skip reason.
//!
//! The recipe's steps are plan, rollout (the apply), verify (the tests), review and land (`recipes/builtin/upgrade.toml`).
//! Its run is per project and ecosystem (the subject). The batches are kept in plaza.db (`project_upgrade`).
//!
//! Ports: the hub's Weft ([`Lanes`]), the registries and changelog hosts ([`Fetch`]) and the tools ([`Exec`]). Production
//! uses Weft through the hub and the real network and processes; the tests use an in-memory repository and a local
//! registry, with the real `cargo`.

pub mod cargo;
pub mod ecosystem;
pub mod others;
pub mod routes;
#[cfg(test)]
mod tests;
pub mod version;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use apz_blobs::Hash;
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::App;
use ecosystem::Ecosystem;
use version::Bump;

/// The recipe every ecosystem's run is of, and its trigger's name.
pub const RECIPE: &str = "upgrade";
const TRIGGER: &str = "schedule";
/// The steps of the recipe, in order.
pub const STEPS: [&str; 5] = ["plan", "rollout", "verify", "review", "land"];
/// The minutes between an ecosystem's upgrades when the project sets none: a week.
pub const DEFAULT_MINUTES: u32 = 7 * 24 * 60;
/// The most characters of a changelog a batch keeps.
const EXCERPT_CHARS: usize = 2000;
/// The most batches a project keeps in its record.
const KEEP_BATCHES: usize = 200;

/// The files of a project's tree: path (`/`-separated) to bytes.
pub type Tree = BTreeMap<String, Vec<u8>>;

/// One dependency's update: its name, the release it is at and the release it moves to, what that is, and (for a major)
/// the requirement the manifest moves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub name: String,
    pub from: String,
    pub to: String,
    pub bump: Bump,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub req: Option<String>,
}

/// A tool's answer: whether it succeeded, and its output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a tool in a directory. Production runs the process; a test may run the real one with its environment.
pub trait Exec: Send + Sync {
    fn run(
        &self,
        dir: &Path,
        program: &str,
        args: &[&str],
        env: &[(String, String)],
    ) -> Result<Output, String>;
}

/// The processes of the hub.
pub struct Local;

impl Exec for Local {
    fn run(
        &self,
        dir: &Path,
        program: &str,
        args: &[&str],
        env: &[(String, String)],
    ) -> Result<Output, String> {
        let out = std::process::Command::new(program)
            .args(args)
            .current_dir(dir)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Reads a URL's bytes: `None` for a 404. The registries' indexes and metadata, and the repositories' changelogs.
pub trait Fetch: Send + Sync {
    fn bytes(&self, url: &str) -> Result<Option<Vec<u8>>, String>;
}

/// The network of the hub (the platforms' HTTP client, with its User-Agent).
pub struct Net;

impl Fetch for Net {
    fn bytes(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
        match ureq::get(url).header("User-Agent", "agentplaza").call() {
            Ok(mut resp) => resp
                .body_mut()
                .read_to_vec()
                .map(Some)
                .map_err(|e| format!("{url}: {e}")),
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(e) => Err(format!("{url}: {e}")),
        }
    }
}

/// The project's repository, as the upgrade needs it: its main, and the changes an upgrade opens on it.
pub trait Lanes {
    /// The files of `main`.
    fn main(&self, project: &str) -> Result<Tree, String>;
    /// Opens a change on `main` with these files written (a `None` deletes one), its intent `title` and `summary`, and
    /// returns its id: drafted, sealed and named by the hub.
    fn open(
        &self,
        project: &str,
        title: &str,
        files: &[(String, Option<Vec<u8>>)],
        summary: &str,
    ) -> Result<String, String>;
    /// Keeps a batch's record (before its change is queued, so the landing queue reads its hold).
    fn record(&self, project: &str, batch: &UpgradeBatch) -> Result<(), String>;
    /// Adds a change to the project's landing queue.
    fn queue(&self, project: &str, change: &str) -> Result<(), String>;
}

/// A vulnerability that needs an upgrade (PLAN 24.3 feeds these): the crate, the release that fixes it, and the advisory's
/// priority (kept on the batch's record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    pub id: String,
    pub ecosystem: Ecosystem,
    pub package: String,
    pub fixed: String,
    pub priority: String,
}

/// What a run upgrades: the scheduled plan of the ecosystem, or one advisory's fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Schedule,
    Fix(Advisory),
}

/// The kind of a batch: the patch and minor batch, one major, or an advisory's fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Minor,
    Major,
    Fix,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Minor => "minor",
            Kind::Major => "major",
            Kind::Fix => "fix",
        }
    }
}

/// The settings a run takes from its project and the hub.
pub struct Ports<'a> {
    pub lanes: &'a dyn Lanes,
    pub exec: &'a dyn Exec,
    pub fetch: &'a dyn Fetch,
    /// The PATH the ecosystems' tools are looked up on.
    pub path_var: String,
    /// Extra environment for the tools (a test runs cargo offline against its registry with it).
    pub env: Vec<(String, String)>,
    /// Whether a patch or minor batch lands by itself (the project's `upgrade_auto_land`).
    pub auto_land: bool,
    pub now: u64,
}

/// What a run did: why an ecosystem was skipped, whether its plan worked, and the batches it made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub ecosystem: Option<Ecosystem>,
    pub skipped: Option<String>,
    pub plan: Result<(), String>,
    pub batches: Vec<UpgradeBatch>,
}

impl Default for Report {
    fn default() -> Self {
        Report {
            ecosystem: None,
            skipped: None,
            plan: Ok(()),
            batches: Vec::new(),
        }
    }
}

/// One batch of a project's upgrades, as its record keeps it (plaza.db, `upgrade_batch`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct UpgradeBatch {
    #[table(key)]
    pub id: String,
    pub ecosystem: String,
    pub kind: String,
    /// `current` (nothing to update), `apply_failed`, `tests_failed`, `review_failed`, `open_failed`, `waiting` (for the
    /// owner's approval), `queued` (it lands by itself), or `advisory` for a fix's record of its advisory.
    pub state: String,
    /// The Weft change the batch is (empty when it made none).
    pub change: String,
    /// Whether the batch waits for the owner's approval before it lands.
    pub hold: bool,
    /// Its updates, one per line: `name from -> to (bump)`.
    pub summary: String,
    /// The tests it ran: `full`, or the selected packages.
    pub tests: String,
    /// The changelogs it read (one excerpt per update that had one).
    pub changelog: String,
    /// Why it stopped, or the advisory it fixes.
    pub reason: String,
    pub at_ms: u64,
}

/// When an ecosystem's upgrades next run (a child of the project's record).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Schedule {
    #[table(key)]
    pub ecosystem: String,
    /// When it next runs (Unix ms); 0 for an ecosystem that is skipped.
    pub next_ms: u64,
    /// Why it is skipped (its tool is not on the hub's PATH), or empty.
    pub skipped: String,
}

/// One project's upgrade record: its schedules and its batches.
#[derive(Debug, Clone, Default, Serialize, Deserialize, apz_store::Table)]
#[table(name = "project_upgrade")]
struct ProjectUpgrades {
    #[table(key)]
    project: String,
    /// Whether the project's ecosystems were detected (the schedules are made once, then kept).
    #[serde(default)]
    detected: bool,
    #[serde(default)]
    #[table(child = "upgrade_schedule")]
    schedules: Vec<Schedule>,
    #[serde(default)]
    #[table(child = "upgrade_batch")]
    batches: Vec<UpgradeBatch>,
}

/// Every project's upgrade record.
#[derive(Default)]
pub struct Store {
    items: Mutex<Vec<ProjectUpgrades>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The table of the upgrade records: `project_upgrade`, one row per project.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(ProjectUpgrades)
}

impl Store {
    fn mutate<T>(&self, project: &str, change: impl FnOnce(&mut ProjectUpgrades) -> T) -> T {
        let mut items = lock(&self.items);
        let at = match items.iter().position(|p| p.project == project) {
            Some(at) => at,
            None => {
                items.push(ProjectUpgrades {
                    project: project.to_string(),
                    ..ProjectUpgrades::default()
                });
                items.len() - 1
            }
        };
        change(&mut items[at])
    }

    /// Keeps a batch: replaces the one with its id, or adds it (the newest first, the record bounded).
    pub fn record(&self, project: &str, batch: UpgradeBatch) {
        self.mutate(project, |p| {
            p.batches.retain(|b| b.id != batch.id);
            p.batches.insert(0, batch);
            p.batches.truncate(KEEP_BATCHES);
        });
    }

    /// The batches of a project, newest first.
    pub fn batches(&self, project: &str) -> Vec<UpgradeBatch> {
        lock(&self.items)
            .iter()
            .find(|p| p.project == project)
            .map(|p| p.batches.clone())
            .unwrap_or_default()
    }

    /// Whether the batch whose change is `change` waits for the owner (None: it is not a batch of an upgrade).
    pub fn holds(&self, change: &str) -> Option<bool> {
        lock(&self.items)
            .iter()
            .flat_map(|p| p.batches.iter())
            .find(|b| !change.is_empty() && b.change == change)
            .map(|b| b.hold)
    }

    pub fn schedules(&self, project: &str) -> Vec<Schedule> {
        lock(&self.items)
            .iter()
            .find(|p| p.project == project)
            .map(|p| p.schedules.clone())
            .unwrap_or_default()
    }

    /// Whether the project's ecosystems were detected, and its schedules.
    pub fn detected(&self, project: &str) -> bool {
        lock(&self.items)
            .iter()
            .any(|p| p.project == project && p.detected)
    }

    /// Sets the project's schedules (its detection, once).
    pub fn set_schedules(&self, project: &str, schedules: Vec<Schedule>) {
        self.mutate(project, |p| {
            p.detected = true;
            p.schedules = schedules;
        });
    }

    /// Moves an ecosystem's next run to `next_ms`.
    pub fn advance(&self, project: &str, ecosystem: &str, next_ms: u64) {
        self.mutate(project, |p| {
            if let Some(s) = p.schedules.iter_mut().find(|s| s.ecosystem == ecosystem) {
                s.next_ms = next_ms;
            }
        });
    }

    /// The projects with a schedule due at `now`, and the ecosystem of each.
    pub fn due(&self, now: u64) -> Vec<(String, String)> {
        lock(&self.items)
            .iter()
            .flat_map(|p| {
                p.schedules
                    .iter()
                    .filter(move |s| s.skipped.is_empty() && s.next_ms > 0 && s.next_ms <= now)
                    .map(move |s| (p.project.clone(), s.ecosystem.clone()))
            })
            .collect()
    }

    /// The rows of every project's record, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(def(), lock(&self.items).iter())
    }

    /// Replaces the records with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<ProjectUpgrades> = apz_store::table::load_list(conn, def())?;
        *lock(&self.items) = stored;
        Ok(())
    }
}

// ── the flow ──────────────────────────────────────────────────────────────────────────────────────────────────────

/// Runs one ecosystem's upgrade request against the project's main and reports what it did.
pub fn process(ports: &Ports, project: &str, eco: Ecosystem, request: &Request) -> Report {
    let mut report = Report {
        ecosystem: Some(eco),
        ..Report::default()
    };
    if let Err(why) = ecosystem::available(eco, &ports.path_var) {
        report.skipped = Some(why);
        return report;
    }
    let tree = match ports.lanes.main(project) {
        Ok(tree) => tree,
        Err(e) => {
            report.plan = Err(e);
            return report;
        }
    };
    let planned: Result<Vec<(Kind, Vec<Update>)>, String> = match (eco, request) {
        (Ecosystem::Cargo, Request::Schedule) => cargo::plan(&tree, ports.fetch).map(|plan| {
            let mut out = Vec::new();
            if !plan.minor.is_empty() {
                out.push((Kind::Minor, plan.minor));
            }
            out.extend(plan.majors.into_iter().map(|u| (Kind::Major, vec![u])));
            out
        }),
        (Ecosystem::Cargo, Request::Fix(advisory)) => {
            fix_update(&tree, advisory).map(|found| match found {
                Some(u) => {
                    let kind = if u.bump == Bump::Major {
                        Kind::Major
                    } else {
                        Kind::Fix
                    };
                    vec![(kind, vec![u])]
                }
                None => Vec::new(),
            })
        }
        (other, Request::Schedule) => {
            let dir = scratch(ports.now);
            let planned = materialize(&tree, &dir)
                .and_then(|()| others::outdated(other, &dir, ports.exec, &ports.env));
            let _ = std::fs::remove_dir_all(&dir);
            planned.map(|plan| {
                let mut out = Vec::new();
                if !plan.minor.is_empty() {
                    out.push((Kind::Minor, plan.minor));
                }
                out.extend(plan.majors.into_iter().map(|u| (Kind::Major, vec![u])));
                out
            })
        }
        (other, Request::Fix(_)) => {
            report.skipped = Some(format!(
                "{} fixes of an advisory are not built: only Cargo has them",
                other.name()
            ));
            return report;
        }
    };
    match planned {
        Ok(batches) => {
            for (kind, updates) in batches {
                let batch = batch_of(ports, project, eco, kind, &updates, &tree, request);
                report.batches.push(batch);
            }
        }
        Err(e) => report.plan = Err(e),
    }
    report
}

/// The update an advisory's fix is: the locked release of its crate, moved to the fixed one. None when the lock is already
/// at or past the fix.
fn fix_update(tree: &Tree, advisory: &Advisory) -> Result<Option<Update>, String> {
    let lock = tree
        .get("Cargo.lock")
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .ok_or("Cargo.lock is not in the project")?;
    let Some((_, from, _)) = cargo::locked(&lock)?
        .into_iter()
        .find(|(name, _, registry)| *registry && *name == advisory.package)
    else {
        return Err(format!("{} is not in Cargo.lock", advisory.package));
    };
    let from_v =
        version::Version::parse(&from).ok_or_else(|| format!("{from} is not a release"))?;
    let to_v = version::Version::parse(&advisory.fixed)
        .ok_or_else(|| format!("the fix {} is not a release", advisory.fixed))?;
    if from_v >= to_v {
        return Ok(None);
    }
    let bump = version::Bump::between(&from_v, &to_v);
    Ok(Some(Update {
        name: advisory.package.clone(),
        from,
        to: advisory.fixed.clone(),
        bump,
        req: (bump == Bump::Major).then(|| cargo::requirement_of(&to_v)),
    }))
}

/// Applies one batch to a copy of the tree and records what came of it. Its change is opened and queued when it passes.
fn batch_of(
    ports: &Ports,
    project: &str,
    eco: Ecosystem,
    kind: Kind,
    updates: &[Update],
    tree: &Tree,
    request: &Request,
) -> UpgradeBatch {
    let now = ports.now;
    let mut batch = UpgradeBatch {
        id: format!("{}-{}-{now}", eco.name(), kind.name()),
        ecosystem: eco.name().to_string(),
        kind: kind.name().to_string(),
        summary: updates
            .iter()
            .map(|u| format!("{} {} -> {} ({:?})", u.name, u.from, u.to, u.bump).to_lowercase())
            .collect::<Vec<_>>()
            .join("\n"),
        at_ms: now,
        ..UpgradeBatch::default()
    };
    if let Request::Fix(advisory) = request {
        batch.reason = format!(
            "{} ({}, {})",
            advisory.id, advisory.priority, advisory.package
        );
    }
    let dir = scratch(now);
    let job = Job {
        project,
        eco,
        kind,
        updates,
        tree,
        dir: &dir,
    };
    let outcome = build_and_check(ports, &job, &mut batch);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err((state, why)) = outcome {
        batch.state = state.to_string();
        batch.reason = if batch.reason.is_empty() {
            why
        } else {
            format!("{}; {why}", batch.reason)
        };
    }
    batch
}

/// One batch's inputs: its project, ecosystem, kind and updates, the tree it starts from, and the scratch copy it runs in.
struct Job<'a> {
    project: &'a str,
    eco: Ecosystem,
    kind: Kind,
    updates: &'a [Update],
    tree: &'a Tree,
    dir: &'a Path,
}

/// The batch's steps in a scratch copy of the tree: apply, read the changelogs, test, review, then open and queue. A
/// stop is `(state, why)`; a batch that got to its change returns Ok.
fn build_and_check(
    ports: &Ports,
    job: &Job,
    batch: &mut UpgradeBatch,
) -> Result<(), (&'static str, String)> {
    let Job {
        project,
        eco,
        kind,
        updates,
        tree,
        dir,
    } = *job;
    materialize(tree, dir).map_err(|e| ("apply_failed", e))?;
    let major = kind == Kind::Major || updates.iter().any(|u| u.bump == Bump::Major);
    apply(ports, eco, kind, updates, dir).map_err(|e| ("apply_failed", e))?;
    let changed = changed_files(dir, tree).map_err(|e| ("apply_failed", e))?;
    if changed.is_empty() {
        batch.state = "current".into();
        return Ok(());
    }

    // The changelogs: what each update says (recorded on the change, not checked by the tests).
    let mut notes = Vec::new();
    for u in updates {
        let text = changelog(ports.fetch, eco, &u.name, &u.from, &u.to);
        if !text.is_empty() {
            notes.push(format!("{} {} -> {}\n{text}", u.name, u.from, u.to));
        }
        if text.is_empty() {
            notes.push(format!(
                "{} {} -> {}: no changelog reachable",
                u.name, u.from, u.to
            ));
        }
    }
    batch.changelog = notes.join("\n\n");

    // The tests: the whole suite for a major, else the packages that reach an updated crate.
    let (program, args, scope) =
        test_plan(ports, eco, dir, updates, major).map_err(|e| ("tests_failed", e))?;
    batch.tests = scope;
    let run = run_tests(ports, dir, &program, &args).map_err(|e| ("tests_failed", e))?;
    if !run.ok {
        return Err((
            "tests_failed",
            cargo::tail(&format!("{}{}", run.stdout, run.stderr)),
        ));
    }

    // The review: a patch or minor batch changes the lockfile only; a major may change the manifest and the lockfile.
    let stray: Vec<&str> = changed
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| !others::allowed(eco, major, p))
        .collect();
    if !stray.is_empty() || changed.iter().any(|(_, bytes)| bytes.is_none()) {
        return Err((
            "review_failed",
            format!("the batch changes files it may not: {}", stray.join(", ")),
        ));
    }

    let title = match (kind, updates) {
        (Kind::Minor, _) => format!(
            "Upgrade {} {} crates (patch and minor)",
            eco.name(),
            updates.len()
        ),
        (Kind::Major, [u]) => format!("Upgrade {} to {} (major)", u.name, u.to),
        (Kind::Fix, [u]) => format!("Fix {} {} (advisory)", u.name, u.to),
        _ => format!("Upgrade {} ({})", eco.name(), kind.name()),
    };
    let summary = format!(
        "{}\n\n{}",
        batch.summary,
        if batch.changelog.is_empty() {
            "No changelog was reachable.".into()
        } else {
            batch.changelog.clone()
        }
    );
    let change = ports
        .lanes
        .open(project, &title, &changed, &summary)
        .map_err(|e| ("open_failed", e))?;
    batch.hold = major || !ports.auto_land;
    batch.state = if batch.hold { "waiting" } else { "queued" }.into();
    batch.change = change.clone();
    ports
        .lanes
        .record(project, batch)
        .map_err(|e| ("open_failed", e))?;
    ports
        .lanes
        .queue(project, &change)
        .map_err(|e| ("open_failed", e))?;
    Ok(())
}

fn apply(
    ports: &Ports,
    eco: Ecosystem,
    kind: Kind,
    updates: &[Update],
    dir: &Path,
) -> Result<(), String> {
    match (eco, kind) {
        (Ecosystem::Cargo, Kind::Minor) => cargo::apply_minor(dir, ports.exec, &ports.env),
        (Ecosystem::Cargo, Kind::Major) => match updates {
            [u] => cargo::apply_major(dir, ports.exec, &ports.env, u),
            _ => Err("a major batch is one crate".into()),
        },
        (Ecosystem::Cargo, Kind::Fix) => match updates {
            [u] => cargo::apply_fix(dir, ports.exec, &ports.env, &u.name, &u.to),
            _ => Err("a fix is one crate".into()),
        },
        (other, Kind::Minor) => others::apply_minor(other, dir, ports.exec, &ports.env, updates),
        (other, Kind::Major) => match updates {
            [u] => others::apply_major(other, dir, ports.exec, &ports.env, u),
            _ => Err("a major batch is one package".into()),
        },
        (other, Kind::Fix) => Err(format!("a fix of {} is not built", other.name())),
    }
}

/// The test command of a batch and the scope it names in the record: the selected packages of a patch or minor batch
/// (Cargo), else the whole suite.
fn test_plan(
    ports: &Ports,
    eco: Ecosystem,
    dir: &Path,
    updates: &[Update],
    major: bool,
) -> Result<(String, Vec<String>, String), String> {
    if eco != Ecosystem::Cargo {
        let (program, args) = others::suite(eco);
        return Ok((
            program.to_string(),
            args.into_iter().map(str::to_string).collect(),
            "full".into(),
        ));
    }
    if major {
        return Ok(("cargo".into(), cargo::test_args(None), "full".into()));
    }
    let names: BTreeSet<String> = updates.iter().map(|u| u.name.clone()).collect();
    let selected = cargo::selected(dir, ports.exec, &ports.env, &names)?;
    if selected.is_empty() {
        // No workspace package reaches the update in the graph: the whole suite stands in for the selection.
        return Ok((
            "cargo".into(),
            cargo::test_args(None),
            "full (no selection reaches the update)".into(),
        ));
    }
    let scope = format!("selected: {}", selected.join(", "));
    Ok(("cargo".into(), cargo::test_args(Some(&selected)), scope))
}

fn run_tests(ports: &Ports, dir: &Path, program: &str, args: &[String]) -> Result<Output, String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ports.exec.run(dir, program, &refs, &ports.env)
}

/// The files of `tree` written into `dir`.
fn materialize(tree: &Tree, dir: &Path) -> Result<(), String> {
    for (path, bytes) in tree {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

/// The files of `dir` that differ from `tree` (`None` for one `tree` has and `dir` lost). The build output is not a file
/// of the project: `target/` and `.git/` are skipped.
pub fn changed_files(dir: &Path, tree: &Tree) -> Result<Vec<(String, Option<Vec<u8>>)>, String> {
    let mut found: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((at, prefix)) = stack.pop() {
        for entry in std::fs::read_dir(&at).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if (prefix.is_empty() && name == "target") || name == ".git" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, rel));
            } else {
                found.insert(rel, std::fs::read(&path).map_err(|e| e.to_string())?);
            }
        }
    }
    let mut out = Vec::new();
    for (path, bytes) in &found {
        if tree.get(path) != Some(bytes) {
            out.push((path.clone(), Some(bytes.clone())));
        }
    }
    for path in tree.keys() {
        if !found.contains_key(path) {
            out.push((path.clone(), None));
        }
    }
    Ok(out)
}

fn scratch(now: u64) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("apid-upgrade-{}-{now}-{n}", std::process::id()))
}

// ── changelogs ────────────────────────────────────────────────────────────────────────────────────────────────────

/// The changelog text of an update, from the registry's metadata (its repository) and the repository's CHANGELOG, when
/// both are reachable. Empty when they are not: the record says so.
pub fn changelog(fetch: &dyn Fetch, eco: Ecosystem, name: &str, from: &str, to: &str) -> String {
    let repo = match eco {
        Ecosystem::Cargo => fetch
            .bytes(&format!("https://crates.io/api/v1/crates/{name}"))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["crate"]["repository"].as_str().map(str::to_string)),
        Ecosystem::Npm => fetch
            .bytes(&format!("https://registry.npmjs.org/{name}"))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| match &v["repository"] {
                Value::String(s) => Some(s.clone()),
                other => other["url"].as_str().map(str::to_string),
            }),
        _ => None,
    };
    let Some(repo) = repo else {
        return String::new();
    };
    let Some(raw) = github_raw(&repo) else {
        return String::new();
    };
    match fetch.bytes(&format!("{raw}/CHANGELOG.md")) {
        Ok(Some(bytes)) => excerpt(&String::from_utf8_lossy(&bytes), from, to),
        _ => String::new(),
    }
}

/// The raw-content URL of a GitHub repository named by its web or git URL.
fn github_raw(repo: &str) -> Option<String> {
    let trimmed = repo
        .trim()
        .trim_start_matches("git+")
        .trim_start_matches("git://")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut parts = trimmed.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let name = parts.next().filter(|s| !s.is_empty())?;
    Some(format!(
        "https://raw.githubusercontent.com/{owner}/{name}/HEAD"
    ))
}

/// The part of a changelog between release `to` (its heading) and release `from`, capped.
pub fn excerpt(text: &str, from: &str, to: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| l.contains(to)) else {
        return String::new();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.contains(from))
        .map(|i| start + 1 + i)
        .unwrap_or_else(|| lines.len().min(start + 40));
    lines[start..end]
        .join("\n")
        .chars()
        .take(EXCERPT_CHARS)
        .collect()
}

// ── the hub's glue ────────────────────────────────────────────────────────────────────────────────────────────────

/// Weft as the upgrade sees it: the project's main, the change an upgrade opens (authored by the hub, as the engine
/// watch's and the memory's are), its record, and the landing queue.
pub struct WeftLanes<'a> {
    pub app: &'a App,
}

impl WeftLanes<'_> {
    fn repo(&self, project: &str) -> Result<String, String> {
        self.app
            .projects
            .get(project)
            .and_then(|p| p.repo)
            .ok_or_else(|| format!("{project} has no Weft repository yet"))
    }
}

impl Lanes for WeftLanes<'_> {
    fn main(&self, project: &str) -> Result<Tree, String> {
        let repo = self.repo(project)?;
        let detail = self.app.weft.get(&format!("/repos/{repo}"))?;
        let rev = detail["refs"]["main"]
            .as_str()
            .ok_or_else(|| format!("{repo} has no main"))?
            .to_string();
        let listing = self.app.weft.get(&format!("/revisions/{rev}/files"))?;
        let client = self.app.weft.client("hub")?;
        let mut tree = Tree::new();
        for file in listing["files"].as_array().into_iter().flatten() {
            let Some(path) = file["path"].as_str() else {
                continue;
            };
            let bytes = client.get_bytes(&format!(
                "/revisions/{rev}/file?path={}",
                weft::server::percent_encode(path)
            ))?;
            tree.insert(path.to_string(), bytes);
        }
        Ok(tree)
    }

    fn open(
        &self,
        project: &str,
        title: &str,
        files: &[(String, Option<Vec<u8>>)],
        summary: &str,
    ) -> Result<String, String> {
        let repo = self.repo(project)?;
        let written: Vec<(&String, &Vec<u8>)> = files
            .iter()
            .filter_map(|(p, b)| b.as_ref().map(|b| (p, b)))
            .collect();
        let hashes: Vec<Hash> = written.iter().map(|(_, b)| Hash::of(b)).collect();
        let bytes_of = |h: &Hash| {
            written
                .iter()
                .zip(&hashes)
                .find(|(_, x)| *x == h)
                .map(|((_, b), _)| (*b).clone())
                .ok_or_else(|| format!("no bytes for {}", h.hex()))
        };
        self.app.weft.client("hub")?.upload(&hashes, &bytes_of)?;
        let opened = self.app.weft.post(
            "/lanes",
            &json!({ "repo": repo, "title": title, "author": { "kind": "hub" } }),
        )?;
        let change = opened["change"]
            .as_str()
            .ok_or("Weft opened no lane for the upgrade")?
            .to_string();
        let mut deltas: Vec<Value> = Vec::new();
        let mut covers: Vec<String> = Vec::new();
        let mut hashed = hashes.iter();
        for (path, bytes) in files {
            covers.push(path.clone());
            let after = bytes.as_ref().map(|_| hashed.next().cloned());
            deltas.push(json!({ "path": path, "after": after.flatten(), "mode": 0o100644 }));
        }
        self.app.weft.hub(
            "revision.draft",
            json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["upgrade"] }),
        )?;
        self.app.weft.hub(
            "revision.seal",
            json!({ "change": change, "n": 1, "deltas": deltas }),
        )?;
        self.app.weft.hub(
            "intent.set",
            json!({ "change": change, "n": 1, "intents": [{ "title": title, "summary": summary, "kind": "chore", "covers": covers }] }),
        )?;
        Ok(change)
    }

    fn record(&self, project: &str, batch: &UpgradeBatch) -> Result<(), String> {
        self.app.upgrades.record(project, batch.clone());
        Ok(())
    }

    fn queue(&self, project: &str, change: &str) -> Result<(), String> {
        let repo = self.repo(project)?;
        self.app.weft.hub(
            "queue.add",
            json!({ "repo": repo, "ref": "main", "change": change, "n": 1 }),
        )?;
        Ok(())
    }
}

/// Whether a batch's change waits for the owner (the landing queue asks for it, `weft_hub::queue_policy`). None when the
/// change is not an upgrade's.
pub fn holds(app: &App, change: &str) -> Option<bool> {
    app.upgrades.holds(change)
}

/// Starts one ecosystem's run on its own thread: a schedule's, or an advisory's fix.
pub fn start(app: &Arc<App>, project: &str, eco: Ecosystem, request: Request) {
    let (app, project) = (Arc::clone(app), project.to_string());
    std::thread::Builder::new()
        .name(format!("upgrade-{}", eco.name()))
        .spawn(move || {
            run(&app, &project, eco, &request);
        })
        .expect("spawn the upgrade thread");
}

/// Runs one ecosystem's request now: the flow, the recipe run's steps, and a note in the project's chat.
pub fn run(app: &App, project: &str, eco: Ecosystem, request: &Request) -> Report {
    let now = apz_plaza::now_ms();
    let lanes = WeftLanes { app };
    let ports = Ports {
        lanes: &lanes,
        exec: &Local,
        fetch: &Net,
        path_var: std::env::var("PATH").unwrap_or_default(),
        env: Vec::new(),
        auto_land: app.project_settings.get(project).upgrade_auto_land,
        now,
    };
    let report = process(&ports, project, eco, request);
    settle(app, project, eco, &report, now);
    report
}

/// Settles the run's steps from the report (a skipped ecosystem has no run: its skip is kept on its schedule).
fn settle(app: &App, project: &str, eco: Ecosystem, report: &Report, now: u64) {
    if let Some(why) = &report.skipped {
        app.upgrades.set_skipped(project, eco.name(), why);
        notify(
            app,
            project,
            &format!("Upgrades of {} are skipped: {why}.", eco.name()),
            now,
        );
        return;
    }
    let Some(run) =
        app.recipe_runs
            .start_engine(RECIPE, TRIGGER, None, Some(project), Some(eco.name()), now)
    else {
        return;
    };
    let apply_failed = report.batches.iter().any(|b| b.state == "apply_failed");
    let tests_failed = report.batches.iter().any(|b| b.state == "tests_failed");
    let review_failed = report
        .batches
        .iter()
        .any(|b| b.state == "review_failed" || b.state == "open_failed");
    let landed = report
        .batches
        .iter()
        .any(|b| b.state == "queued" || b.state == "waiting");
    if report.batches.is_empty() || report.batches.iter().all(|b| b.state == "current") {
        app.recipe_runs
            .close_external(&run, Vec::new(), true, "current", now);
    } else {
        let results = [
            report.plan.is_ok(),
            !apply_failed,
            !tests_failed,
            !review_failed,
            landed,
        ];
        for (step, ok) in STEPS.iter().zip(results) {
            app.recipe_runs.started(&run, step, now);
            app.recipe_runs
                .settle(&run, step, &crate::recipes::Report::verdict(ok), now);
            if !ok {
                break;
            }
        }
    }
    for batch in &report.batches {
        let text = match batch.state.as_str() {
            "queued" => format!(
                "Upgrade {} ({}) is queued: it lands by itself. {}",
                eco.name(),
                batch.kind,
                batch.summary
            ),
            "waiting" => format!(
                "Upgrade {} ({}) waits for your approval before it lands. {}",
                eco.name(),
                batch.kind,
                batch.summary
            ),
            "current" => continue,
            other => format!(
                "Upgrade {} ({}) stopped at {other}: {}",
                eco.name(),
                batch.kind,
                batch.reason
            ),
        };
        notify(app, project, &text, now);
    }
    if let Err(e) = &report.plan {
        notify(
            app,
            project,
            &format!("Upgrades of {} could not be planned: {e}", eco.name()),
            now,
        );
    }
}

fn notify(app: &App, project: &str, text: &str, now: u64) {
    let chat = crate::work_loop::project_chat(app, project, now);
    crate::work_loop::notice(app, &chat, text, now);
}

/// Schedules: each project with upgrades on (its `upgrade_minutes`, a week by default) has its ecosystems detected once,
/// and each detected ecosystem runs when its schedule is due. Detection reads Weft, so it runs on its own thread.
pub fn tick(app: &Arc<App>, now: u64) {
    for project in app.projects.list() {
        let pid = project.id.clone();
        let minutes = app
            .project_settings
            .get(&pid)
            .upgrade_minutes
            .unwrap_or(DEFAULT_MINUTES);
        if minutes == 0 {
            continue;
        }
        // A project with no Weft repository yet has nothing to upgrade: it is detected once it has one.
        if !app.upgrades.detected(&pid) && project.repo.is_some() && app.weft.running() {
            detect_in_background(app, &pid, now, minutes);
        }
    }
    for (project, eco) in app.upgrades.due(now) {
        let Some(eco) = Ecosystem::parse(&eco) else {
            continue;
        };
        let minutes = app
            .project_settings
            .get(&project)
            .upgrade_minutes
            .unwrap_or(DEFAULT_MINUTES);
        app.upgrades
            .advance(&project, eco.name(), now + u64::from(minutes) * 60_000);
        start(app, &project, eco, Request::Schedule);
    }
}

fn detect_in_background(app: &Arc<App>, project: &str, now: u64, minutes: u32) {
    static DETECTING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    {
        let mut busy = lock(&DETECTING);
        if !busy.insert(project.to_string()) {
            return;
        }
    }
    let (app, project) = (Arc::clone(app), project.to_string());
    std::thread::Builder::new()
        .name("upgrade-detect".into())
        .spawn(move || {
            if app.weft.running() {
                let lanes = WeftLanes { app: &app };
                match lanes.main(&project) {
                    Ok(tree) => {
                        let paths: Vec<String> = tree.keys().cloned().collect();
                        let path_var = std::env::var("PATH").unwrap_or_default();
                        let schedules = detect_schedules(&paths, &path_var, now, minutes);
                        app.upgrades.set_schedules(&project, schedules);
                    }
                    Err(e) => eprintln!("apid: upgrades of {project}: {e}"),
                }
            }
            lock(&DETECTING).remove(&project);
        })
        .expect("spawn the upgrade detection thread");
}

/// The schedules a project's tree gives: one per detected ecosystem, each due at once (its first run is a minute after
/// the detection, not at it), or skipped with the reason its tool is missing.
pub fn detect_schedules(paths: &[String], path_var: &str, now: u64, minutes: u32) -> Vec<Schedule> {
    ecosystem::detect(paths)
        .into_iter()
        .map(|eco| match ecosystem::available(eco, path_var) {
            Ok(()) => Schedule {
                ecosystem: eco.name().to_string(),
                next_ms: now + u64::from(minutes.min(1)) * 60_000,
                skipped: String::new(),
            },
            Err(why) => Schedule {
                ecosystem: eco.name().to_string(),
                next_ms: 0,
                skipped: why,
            },
        })
        .collect()
}

/// Starts an advisory's fix (PLAN 12.3 feeds this): the crate of the advisory moves to its fixed release through the same
/// batch flow. Only Cargo is built; another ecosystem's fix is refused with its reason.
pub fn for_advisory(app: &Arc<App>, project: &str, advisory: Advisory) -> Result<(), String> {
    if !app.weft.running() {
        return Err("Weft is not running: the fix cannot be opened".into());
    }
    let eco = advisory.ecosystem;
    start(app, project, eco, Request::Fix(advisory));
    Ok(())
}

impl Store {
    /// Keeps why an ecosystem is skipped on its schedule (it is never due until its tool is there).
    pub fn set_skipped(&self, project: &str, ecosystem: &str, why: &str) {
        self.mutate(project, |p| {
            if let Some(s) = p.schedules.iter_mut().find(|s| s.ecosystem == ecosystem) {
                s.skipped = why.to_string();
                s.next_ms = 0;
            }
        });
    }
}
