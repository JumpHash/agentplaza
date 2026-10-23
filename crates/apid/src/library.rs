//! The plaza's library of agent-made tools. An agent packages a program with the
//! `tool_package` tool and the plaza keeps it as the next version of a named tool.
//! A new version is private to its author until it is proposed; the project's planner approves it, and it serves that
//! project as `x_<name>` tools (PLAN 14.1). A `promote` card makes it available to every project. A tool that needs a
//! connector's secret runs on the hub's glue tier (glue.rs) and waits for the owner's connector card. A version that
//! replaces a built-in tool goes through shadow, trial and all (PLAN 14.2), each stage on evidence. The lifecycle's
//! asks, notices and glue calls are in `tool_life.rs`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use apz_http::{Request, Response, RouteDoc, Router};
use apz_proto::chat::ToolSpec;
use apz_proto::wire::{Program, ProgramFile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::App;

/// The prefix that marks a library tool's name.
pub const PREFIX: &str = "x_";
/// The largest file a package may carry, in bytes.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// The most text a file view returns, in bytes.
const VIEW_BYTES: u64 = 256 * 1024;
/// The wall-clock limit of a library program, in seconds.
const TIMEOUT_S: u64 = 120;
/// The file a glue-tier tool's WebAssembly component is stored as.
pub const COMPONENT_FILE: &str = "component.wasm";
/// A version with at least this many calls and more failures than half of them is proposed for removal.
pub const HARMFUL_CALLS: u64 = 10;
/// A version unused for this long (30 days) is proposed for removal.
pub const UNUSED_MS: u64 = 30 * 24 * 3600 * 1000;
/// The comparisons a replacement needs before its next stage (shadow to trial, trial to all).
pub const STAGE_EVIDENCE: usize = 5;
/// The newest comparisons a regression is judged on, and the failures of the candidate among them that make one.
const WINDOW: usize = 10;
const REGRESSION_FAILURES: usize = 3;
/// The comparisons a version keeps.
const COMPARISONS_KEPT: usize = 1000;

fn default_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// One file of a stored version (a row of the entry's `library_entry_files` child table).
#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    #[serde(default)]
    pub executable: bool,
}

/// One stored version of a library tool: a row of `library_entry`, keyed by its name and version.
#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
#[table(name = "library_entry")]
pub struct Entry {
    #[table(key)]
    pub name: String,
    #[table(key)]
    pub version: u32,
    pub author_cid: u64,
    pub author: String,
    pub description: String,
    #[table(list)]
    pub command: Vec<String>,
    #[serde(default)]
    #[table(list)]
    pub requires: Vec<String>,
    /// The tool's JSON Schema, kept as written.
    #[table(content)]
    pub input_schema: Value,
    /// "private" (the author's draft), "pending" (proposed), "awaiting_owner" (the planner approved it; a tool with a
    /// connector waits for the owner's card), "published", "rejected" or "retired".
    pub state: String,
    #[table(child)]
    pub files: Vec<FileInfo>,
    pub hash: String,
    pub created_ms: u64,
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub review_note: String,
    /// The project whose agent made it ("" when none): a published version serves that project, or every project once
    /// promoted.
    #[serde(default)]
    pub project: String,
    /// The connector whose secret a glue-tier tool uses ("" for a tool that runs in a fork).
    #[serde(default)]
    pub connector: String,
    /// The built-in host tool this version replaces ("" for none).
    #[serde(default)]
    pub replaces: String,
    /// "" (not in use), "shadow", "trial" or "all": the stage of a replacement.
    #[serde(default)]
    pub stage: String,
    /// The one project a trial serves.
    #[serde(default)]
    pub trial_project: String,
    /// Whether the owner made it available to every project.
    #[serde(default)]
    pub promoted: bool,
    /// The test run the proposal reported: "passed", "failed" or "" (none yet).
    #[serde(default)]
    pub tests: String,
    /// Failed calls, and the total of the calls' time in milliseconds (with `calls`, what measurement reads).
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub latency_ms: u64,
    /// Shadow and trial calls: the built-in's result beside the replacement's.
    #[serde(default)]
    #[table(child)]
    pub comparisons: Vec<Comparison>,
    /// The index in `comparisons` where the current stage began: a stage's evidence counts only from there.
    #[serde(default)]
    pub stage_from: u64,
}

/// One call a replacement served or was compared on (a child row of `library_entry`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Comparison {
    pub at_ms: u64,
    /// Whether the built-in ran in this call (a shadow runs it; a trial runs it only as the fallback).
    pub builtin_ran: bool,
    pub builtin_ok: bool,
    pub builtin_ms: u64,
    pub candidate_ok: bool,
    pub candidate_ms: u64,
    /// Both ran and gave the same output.
    pub same: bool,
}

/// A package as the guest reports it from `tool_package`.
#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default = "default_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub publish: bool,
    /// A connector whose secret the tool uses: the tool then runs as a component on the hub (`COMPONENT_FILE`).
    #[serde(default)]
    pub connector: String,
    /// A built-in host tool this one replaces (a connector tool, with a `connector`).
    #[serde(default)]
    pub replaces: String,
    pub files: Vec<PackageFile>,
}

/// One file of a [`Package`], its bytes base64-encoded.
#[derive(Clone, Debug, Deserialize)]
pub struct PackageFile {
    pub path: String,
    pub content_b64: String,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Deserialize)]
struct Report {
    package: Package,
}

/// The library: its entries in memory, kept in the hub's store (`library_entry`). The tool files stay under
/// `<state>/library/<name>/<version>/files`.
pub struct Store {
    root: PathBuf,
    entries: Mutex<Vec<Entry>>,
}

impl Store {
    /// An empty library whose tool files live under `<state_dir>/library`; [`Store::load`] fills it at start.
    pub fn open(state_dir: impl AsRef<Path>) -> Store {
        Store {
            root: state_dir.as_ref().join("library"),
            entries: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn files_dir(&self, name: &str, version: u32) -> PathBuf {
        self.root.join(name).join(version.to_string()).join("files")
    }

    /// Stores `package` as the next version of its name for `project`: private, or pending review when proposed.
    pub fn insert(
        &self,
        author_cid: u64,
        author: &str,
        project: &str,
        package: Package,
    ) -> Result<Entry, String> {
        validate_name(&package.name)?;
        if package.command.is_empty() && package.connector.is_empty() {
            return Err("a package needs a command".to_string());
        }
        if package.files.is_empty() {
            return Err("a package needs at least one file".to_string());
        }
        if !package.connector.is_empty() && !package.files.iter().any(|f| f.path == COMPONENT_FILE)
        {
            return Err(format!(
                "a connector tool ships its component as {COMPONENT_FILE}"
            ));
        }
        let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
        let mut files = Vec::new();
        for file in &package.files {
            check_rel_path(&file.path)?;
            let bytes = base64_decode(&file.content_b64)
                .ok_or_else(|| format!("file {} is not base64", file.path))?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err(format!("file {} is too large", file.path));
            }
            files.push(FileInfo {
                path: file.path.clone(),
                size: bytes.len() as u64,
                executable: file.executable,
            });
            blobs.push((file.path.clone(), bytes));
        }
        let hash = digest(&blobs);

        let mut entries = self.lock();
        let version = entries
            .iter()
            .filter(|e| e.name == package.name)
            .map(|e| e.version)
            .max()
            .unwrap_or(0)
            + 1;
        let dir = self.files_dir(&package.name, version);
        for (file, (_, bytes)) in package.files.iter().zip(&blobs) {
            let target = dir.join(&file.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mode = if file.executable { 0o755 } else { 0o644 };
            crate::providers::write_atomic(&target, bytes, mode).map_err(|e| e.to_string())?;
        }
        let entry = Entry {
            name: package.name.clone(),
            version,
            author_cid,
            author: author.to_string(),
            description: package.description.clone(),
            command: package.command.clone(),
            requires: package.requires.clone(),
            input_schema: package.input_schema.clone(),
            state: if package.publish {
                "pending"
            } else {
                "private"
            }
            .to_string(),
            files,
            hash,
            created_ms: now_ms(),
            calls: 0,
            review_note: String::new(),
            project: project.to_string(),
            connector: package.connector.clone(),
            replaces: package.replaces.clone(),
            stage: String::new(),
            trial_project: String::new(),
            promoted: false,
            tests: String::new(),
            failures: 0,
            latency_ms: 0,
            comparisons: Vec::new(),
            stage_from: 0,
        };
        entries.push(entry.clone());
        Ok(entry)
    }

    /// The version `name` `version`, as it is stored.
    pub fn find(&self, name: &str, version: u32) -> Option<Entry> {
        self.lock()
            .iter()
            .find(|e| e.name == name && e.version == version)
            .cloned()
    }

    /// Records the test run a proposal reported (in its test fork): passed or failed.
    pub fn record_tests(&self, name: &str, version: u32, passed: bool) -> Result<String, String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        if entry.state != "pending" {
            return Err(format!("v{version} is {}, not pending", entry.state));
        }
        entry.tests = if passed { "passed" } else { "failed" }.to_string();
        Ok(entry.tests.clone())
    }

    /// The planner's verdict on a pending version. An approval needs passed tests. A tool with a connector waits for the
    /// owner (`awaiting_owner`); any other is published. Returns the new state.
    pub fn review(
        &self,
        name: &str,
        version: u32,
        approve: bool,
        note: &str,
    ) -> Result<String, String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        if entry.state != "pending" {
            return Err(format!("v{version} is {}, not pending", entry.state));
        }
        entry.review_note = note.to_string();
        if !approve {
            entry.state = "rejected".to_string();
            return Ok(entry.state.clone());
        }
        if entry.tests != "passed" {
            return Err(format!("v{version}'s tests have not passed"));
        }
        entry.state = if entry.connector.is_empty() {
            "published"
        } else {
            "awaiting_owner"
        }
        .to_string();
        Ok(entry.state.clone())
    }

    /// The owner's answer to a tool's connector card: published or rejected.
    pub fn settle(&self, name: &str, version: u32, approve: bool) -> Result<String, String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        if entry.state != "awaiting_owner" {
            return Err(format!(
                "v{version} is {}, not awaiting the owner",
                entry.state
            ));
        }
        entry.state = if approve { "published" } else { "rejected" }.to_string();
        Ok(entry.state.clone())
    }

    /// Makes a published version available to every project.
    pub fn promote(&self, name: &str, version: u32) -> Result<(), String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        if entry.state != "published" {
            return Err(format!("v{version} is {}, not published", entry.state));
        }
        entry.promoted = true;
        Ok(())
    }

    /// Retires a published version: it is no longer offered, and any replacement it made ends.
    pub fn retire(&self, name: &str, version: u32) -> Result<(), String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        if entry.state != "published" {
            return Err(format!("v{version} is {}, not published", entry.state));
        }
        entry.state = "retired".to_string();
        entry.stage = String::new();
        Ok(())
    }

    /// Moves a published replacement to its next stage: shadow, then trial (for `trial_project`), then all. Each move
    /// needs the comparisons of the stage before (`STAGE_EVIDENCE`) and no regression. A side-effecting built-in is
    /// never shadowed (its shadow would need a scratch fork, which the hub does not run yet).
    pub fn set_stage(
        &self,
        name: &str,
        version: u32,
        stage: &str,
        trial_project: &str,
        side_effects: bool,
    ) -> Result<(), String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        if entry.state != "published" {
            return Err(format!("v{version} is {}, not published", entry.state));
        }
        if entry.replaces.is_empty() {
            return Err(format!("v{version} replaces no built-in tool"));
        }
        let from = (entry.stage_from as usize).min(entry.comparisons.len());
        let staged = &entry.comparisons[from..];
        let evidence = staged.len() >= STAGE_EVIDENCE && !regressed(staged);
        let next = match (entry.stage.as_str(), stage) {
            ("", "shadow") if side_effects => {
                return Err(format!(
                    "`{}` has side effects: it is shadowed only in a scratch fork, which this hub does not run yet",
                    entry.replaces
                ));
            }
            ("", "shadow") => "shadow",
            ("shadow", "trial") if evidence => {
                if trial_project.is_empty() {
                    return Err("a trial names its project".to_string());
                }
                entry.trial_project = trial_project.to_string();
                "trial"
            }
            ("trial", "all") if evidence => "all",
            (from, to) => {
                let from = if from.is_empty() { "none" } else { from };
                return Err(format!(
                    "a replacement moves from {from} to {to} only in order, with {STAGE_EVIDENCE} comparisons and no regression"
                ));
            }
        };
        entry.stage = next.to_string();
        entry.stage_from = entry.comparisons.len() as u64;
        Ok(())
    }

    /// Ends a replacement's stage (its regression switched it back): the built-in serves again.
    pub fn switch_back(&self, name: &str, version: u32) {
        if let Ok(entry) = find_mut(&mut self.lock(), name, version) {
            entry.stage = String::new();
        }
    }

    /// The published version that serves a call of built-in `builtin` for `project` (at shadow, at its trial's project,
    /// or at all), if any.
    pub fn replacement_for(&self, builtin: &str, project: Option<&str>) -> Option<Entry> {
        self.lock()
            .iter()
            .filter(|e| e.state == "published" && e.replaces == builtin)
            .filter(|e| match e.stage.as_str() {
                "shadow" | "all" => true,
                "trial" => project.is_some_and(|p| p == e.trial_project),
                _ => false,
            })
            .max_by_key(|e| e.version)
            .cloned()
    }

    /// Adds a comparison to a version and returns whether the version now regresses.
    pub fn compare(&self, name: &str, version: u32, cmp: Comparison) -> Result<bool, String> {
        let mut entries = self.lock();
        let entry = find_mut(&mut entries, name, version)?;
        entry.comparisons.push(cmp);
        let excess = entry.comparisons.len().saturating_sub(COMPARISONS_KEPT);
        entry.comparisons.drain(..excess);
        entry.stage_from = entry.stage_from.saturating_sub(excess as u64);
        Ok(regressed(&entry.comparisons))
    }

    /// Counts a call of the newest version of `tool` (`x_<name>`, or the name alone) that is not rejected: its outcome
    /// and time. Returns that version.
    pub fn measure(&self, tool: &str, ok: bool, ms: u64) -> Option<Entry> {
        let name = tool.strip_prefix(PREFIX)?;
        let mut entries = self.lock();
        let entry = entries
            .iter_mut()
            .filter(|e| e.name == name && !matches!(e.state.as_str(), "rejected"))
            .max_by_key(|e| e.version)?;
        entry.calls += 1;
        entry.latency_ms += ms;
        if !ok {
            entry.failures += 1;
        }
        Some(entry.clone())
    }

    /// Counts a call of a known version (a hub-run call names the version it ran).
    pub fn measure_version(&self, name: &str, version: u32, ok: bool, ms: u64) {
        if let Ok(entry) = find_mut(&mut self.lock(), name, version) {
            entry.calls += 1;
            entry.latency_ms += ms;
            if !ok {
                entry.failures += 1;
            }
        }
    }

    /// The versions that should be proposed for removal at `now`: harmful ones and ones unused for `UNUSED_MS`.
    pub fn removal_candidates(&self, now: u64) -> Vec<Entry> {
        self.lock()
            .iter()
            .filter(|e| e.state == "published" && (harmful(e) || unused(e, now)))
            .cloned()
            .collect()
    }

    /// The component a glue-tier version ships.
    pub fn component(&self, name: &str, version: u32) -> Option<Vec<u8>> {
        fs::read(self.files_dir(name, version).join(COMPONENT_FILE)).ok()
    }

    /// Removes one version and its files.
    pub fn delete(&self, name: &str, version: u32) -> Result<(), String> {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|e| !(e.name == name && e.version == version));
        if entries.len() == before {
            return Err("no such version".to_string());
        }
        let _ = fs::remove_dir_all(self.root.join(name).join(version.to_string()));
        Ok(())
    }

    /// Every stored version, newest first.
    pub fn all(&self) -> Vec<Entry> {
        let mut entries = self.lock().to_vec();
        entries.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
        entries
    }

    /// Stored versions whose name, description or a file path contains `query` (case-insensitive), newest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Entry> {
        let needle = query.to_lowercase();
        self.all()
            .into_iter()
            .filter(|entry| {
                entry.name.to_lowercase().contains(&needle)
                    || entry.description.to_lowercase().contains(&needle)
                    || entry
                        .files
                        .iter()
                        .any(|file| file.path.to_lowercase().contains(&needle))
            })
            .take(limit)
            .collect()
    }

    /// The newest version of each name that agent `cid` may see in `project`.
    pub fn offered(&self, cid: u64, project: Option<&str>) -> Vec<Entry> {
        latest(&self.lock(), cid, project)
    }

    /// The file list of one version, with text content up to `VIEW_BYTES` per file.
    pub fn view(&self, name: &str, version: u32) -> Result<Vec<Value>, String> {
        let entry = self
            .lock()
            .iter()
            .find(|e| e.name == name && e.version == version)
            .cloned()
            .ok_or_else(|| "no such version".to_string())?;
        let dir = self.files_dir(name, version);
        Ok(entry
            .files
            .iter()
            .map(|f| {
                let content = if f.size <= VIEW_BYTES { fs::read_to_string(dir.join(&f.path)).ok() } else { None };
                json!({ "path": f.path, "size": f.size, "executable": f.executable, "content": content })
            })
            .collect())
    }

    /// The newest version of the visible tool `tool` (`x_<name>`) for agent `cid` in `project`.
    pub fn visible(&self, cid: u64, project: Option<&str>, tool: &str) -> Option<Entry> {
        let name = tool.strip_prefix(PREFIX)?;
        self.lock()
            .iter()
            .filter(|e| e.name == name && offerable(e, cid, project))
            .max_by_key(|e| e.version)
            .cloned()
    }

    /// The program behind the visible tool `tool` (`x_<name>`) with its files, for a tool that runs in a fork. A tool
    /// that runs on the glue tier has none.
    pub fn program(&self, cid: u64, project: Option<&str>, tool: &str) -> Option<Program> {
        let entry = self.visible(cid, project, tool)?;
        if !entry.connector.is_empty() {
            return None;
        }
        let dir = self.files_dir(&entry.name, entry.version);
        let files = entry
            .files
            .iter()
            .map(|f| {
                let bytes = fs::read(dir.join(&f.path)).ok()?;
                Some(ProgramFile {
                    path: f.path.clone(),
                    content_b64: base64_encode(&bytes),
                    executable: f.executable,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Program {
            name: entry.name,
            version: entry.version,
            hash: entry.hash,
            files,
            command: entry.command,
            requires: entry.requires,
            timeout_s: TIMEOUT_S,
        })
    }
}

/// A version agent `cid` may see in `project`: published ones of the project (or of every project once promoted), and
/// its own drafts for the author.
fn offerable(entry: &Entry, cid: u64, project: Option<&str>) -> bool {
    let shared = entry.state == "published"
        && (entry.promoted || project.is_some_and(|p| p == entry.project));
    let own = entry.author_cid == cid
        && matches!(
            entry.state.as_str(),
            "private" | "pending" | "awaiting_owner"
        );
    shared || own
}

/// Whether a version has failed too often to keep serving: at least `HARMFUL_CALLS` calls, more than half failed.
pub fn harmful(entry: &Entry) -> bool {
    entry.calls >= HARMFUL_CALLS && entry.failures * 2 > entry.calls
}

/// Whether a published version has gone unused for `UNUSED_MS` at `now`.
fn unused(entry: &Entry, now: u64) -> bool {
    entry.calls == 0 && now.saturating_sub(entry.created_ms) >= UNUSED_MS
}

/// Whether the newest comparisons show a regression: the candidate failed at least `REGRESSION_FAILURES` times in the
/// last `WINDOW`, and more often than the built-in did (when it ran).
pub fn regressed(comparisons: &[Comparison]) -> bool {
    let window = &comparisons[comparisons.len().saturating_sub(WINDOW)..];
    let candidate = window.iter().filter(|c| !c.candidate_ok).count();
    let builtin = window
        .iter()
        .filter(|c| c.builtin_ran && !c.builtin_ok)
        .count();
    candidate >= REGRESSION_FAILURES && candidate > builtin
}

fn find_mut<'a>(
    entries: &'a mut [Entry],
    name: &str,
    version: u32,
) -> Result<&'a mut Entry, String> {
    entries
        .iter_mut()
        .find(|e| e.name == name && e.version == version)
        .ok_or_else(|| "no such version".to_string())
}

/// The newest offerable version of each name, as agent `cid` sees the library in `project`.
fn latest(entries: &[Entry], cid: u64, project: Option<&str>) -> Vec<Entry> {
    let mut best: BTreeMap<&str, &Entry> = BTreeMap::new();
    for entry in entries.iter().filter(|e| offerable(e, cid, project)) {
        match best.get(entry.name.as_str()) {
            Some(current) if current.version >= entry.version => {}
            _ => {
                best.insert(entry.name.as_str(), entry);
            }
        }
    }
    best.values().map(|e| (*e).clone()).collect()
}

/// The tool specs agent `cid` is offered from the library, in the project it works on.
pub fn specs_for(app: &App, cid: u64) -> Vec<ToolSpec> {
    let project = crate::projects::project_of(app, cid);
    app.library
        .offered(cid, project.as_deref())
        .into_iter()
        .map(|e| ToolSpec {
            deferred: false,
            name: format!("{PREFIX}{}", e.name),
            description: format!("{} (made by {}, v{})", e.description, e.author, e.version),
            input_schema: e.input_schema,
        })
        .collect()
}

/// The program behind a visible library tool, for the guest to run.
pub fn program(app: &App, cid: u64, name: &str) -> Option<Program> {
    let project = crate::projects::project_of(app, cid);
    app.library.program(cid, project.as_deref(), name)
}

/// Stores a package reported by a guest's `tool_package` call and announces it when pending. A package that uses a
/// connector runs on the glue tier, so it must name the connector and ship its component; one that replaces a built-in
/// tool must name a built-in host tool and be a glue-tier tool.
pub fn accept(app: &App, cid: u64, output: &str) -> Result<Entry, String> {
    let report: Report = serde_json::from_str(output).map_err(|e| e.to_string())?;
    check_package(&report.package)?;
    let author = app.agents.get(cid).map(|a| a.label()).unwrap_or_default();
    let project = crate::projects::project_of(app, cid).unwrap_or_default();
    let entry = app.library.insert(cid, &author, &project, report.package)?;
    announce(app, &entry.name, entry.version, &entry.state);
    // The author can use its new tool at once: its tool list is pushed again.
    if let Some(agent) = app.agents.get(cid) {
        crate::agents::push_profile(app, &agent);
    }
    Ok(entry)
}

/// The checks a package passes before it is stored (the tool's proposal, not its tests).
fn check_package(package: &Package) -> Result<(), String> {
    if !package.connector.is_empty() {
        if crate::connectors::find(&package.connector).is_none() {
            return Err(format!("there is no connector `{}`", package.connector));
        }
        let component = package
            .files
            .iter()
            .find(|f| f.path == COMPONENT_FILE)
            .and_then(|f| base64_decode(&f.content_b64))
            .ok_or_else(|| format!("a connector tool ships its component as {COMPONENT_FILE}"))?;
        crate::glue::check_component(&component)?;
    }
    if !package.replaces.is_empty() {
        if package.connector.is_empty() {
            return Err(
                "a tool that replaces a built-in one runs on the hub: name its connector".into(),
            );
        }
        if !matches!(
            crate::tools::runs(&package.replaces),
            Some(crate::tools::Runs::Host)
        ) {
            return Err(format!("`{}` is not a built-in tool", package.replaces));
        }
    }
    Ok(())
}

fn announce(app: &App, name: &str, version: u32, state: &str) {
    app.bus.publish(
        "library",
        &json!({ "name": name, "version": version, "state": state }),
    );
}

type Calls = Mutex<HashMap<(u64, String), ()>>;

fn package_calls() -> &'static Calls {
    static CALLS: OnceLock<Calls> = OnceLock::new();
    CALLS.get_or_init(Calls::default)
}

/// Remembers that guest `cid` made the `tool_package` call `call`.
pub fn remember_package_call(cid: u64, call: &str) {
    package_calls()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert((cid, call.to_string()), ());
}

/// Whether `call` was a `tool_package` call; forgets it either way.
pub fn take_package_call(cid: u64, call: &str) -> bool {
    package_calls()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(cid, call.to_string()))
        .is_some()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/library", |app, _req, _p| list(app))
        .doc(RouteDoc::new("List the library's tool versions"))
        .post("/v1/library/sweep", |app, _req, _p| sweep(app))
        .doc(RouteDoc::new(
            "Propose removing the harmful and unused tool versions",
        ))
        .post("/v1/library/{name}/{version}/tests", |app, req, p| {
            record_tests_route(app, req, p.get("name"), p.get("version"))
        })
        .doc(RouteDoc::new("Record a proposal's test run (passed or failed)").errors(&[400, 409]))
        .post("/v1/library/{name}/{version}/promote", |app, _req, p| {
            promote(app, p.get("name"), p.get("version"))
        })
        .doc(
            RouteDoc::new("Ask the owner to make a published tool available to every project")
                .errors(&[404, 409]),
        )
        .post("/v1/library/{name}/{version}/stage", |app, req, p| {
            stage(app, req, p.get("name"), p.get("version"))
        })
        .doc(
            RouteDoc::new("Move a replacement to its next stage (shadow, trial, all)")
                .errors(&[400, 409]),
        )
        .get("/v1/library/{name}/{version}/files", |app, req, p| {
            files(
                app,
                p.get("name"),
                p.get("version"),
                req.query_param("path"),
            )
        })
        .doc(RouteDoc::new("List a tool version's files, or read one").errors(&[404]))
        .post("/v1/library/{name}/{version}/review", |app, req, p| {
            review(app, req, p.get("name"), p.get("version"))
        })
        .doc(RouteDoc::new("Approve or reject a tool version").errors(&[400, 409]))
        .delete("/v1/library/{name}/{version}", |app, _req, p| {
            delete(app, p.get("name"), p.get("version"))
        })
        .doc(RouteDoc::new("Delete a tool version").errors(&[404]))
}

#[derive(Deserialize)]
struct ReviewBody {
    approve: bool,
    #[serde(default)]
    note: String,
}

fn list(app: &App) -> Response {
    Response::json(200, &app.library.all())
}

/// A version's files; with `?path=`, just that file as `{path, content}` (what the console reads).
fn files(app: &App, name: &str, version: &str, path: Option<&str>) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    match (app.library.view(name, version), path) {
        (Ok(files), None) => Response::json(
            200,
            &json!({ "name": name, "version": version, "files": files }),
        ),
        (Ok(files), Some(path)) => match files
            .into_iter()
            .find(|f| f["path"].as_str() == Some(path))
        {
            Some(file) => Response::json(200, &json!({ "path": path, "content": file["content"] })),
            None => Response::error(404, &format!("no file {path} in {name} v{version}")),
        },
        (Err(e), _) => Response::error(404, &e),
    }
}

fn review(app: &App, req: &Request, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    let body: ReviewBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match crate::tool_life::review(app, name, version, body.approve, &body.note) {
        Ok(state) => Response::json(200, &json!({ "state": state })),
        Err(e) => Response::error(409, &e),
    }
}

#[derive(Deserialize)]
struct TestsBody {
    passed: bool,
}

fn record_tests_route(app: &App, req: &Request, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    let body: TestsBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match app.library.record_tests(name, version, body.passed) {
        Ok(tests) => Response::json(200, &json!({ "tests": tests })),
        Err(e) => Response::error(409, &e),
    }
}

fn promote(app: &App, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    match crate::tool_life::request_promotion(app, name, version) {
        Ok(()) => Response::json(200, &json!({ "asked": true })),
        Err(e) => Response::error(409, &e),
    }
}

#[derive(Deserialize)]
struct StageBody {
    stage: String,
    #[serde(default)]
    project: String,
}

fn stage(app: &App, req: &Request, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    let body: StageBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match crate::tool_life::set_stage(app, name, version, &body.stage, &body.project) {
        Ok(()) => Response::json(200, &json!({ "stage": body.stage })),
        Err(e) => Response::error(409, &e),
    }
}

fn sweep(app: &App) -> Response {
    let asked = crate::tool_life::sweep(app, now_ms());
    Response::json(200, &json!({ "asked": asked }))
}

fn delete(app: &App, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    match app.library.delete(name, version) {
        Ok(()) => Response::json(200, &json!({})),
        Err(e) => Response::error(404, &e),
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "tool name {name:?} may use letters, digits, '_' and '-' only"
        ))
    }
}

fn check_rel_path(path: &str) -> Result<(), String> {
    let ok = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "file path {path:?} must be relative and stay inside the package"
        ))
    }
}

/// The content digest the guest checks before running a program: SHA-256 over the files
/// sorted by path, each as path, a NUL, its length (8 bytes, big-endian) and its bytes —
/// exactly as the harness computes it (`agent-harness/src/library.rs`).
fn digest(blobs: &[(String, Vec<u8>)]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<&(String, Vec<u8>)> = blobs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (path, bytes) in sorted {
        hasher.update(path.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let value = B64.iter().position(|x| *x == c)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Store {
    /// The entries as they are now (the hub's saver writes what changed).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let entries = self.lock();
        apz_store::RowSet::list(def(), entries.iter())
    }

    /// Replaces the entries with what the hub's store holds (at start), in their stored order.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Entry> = apz_store::table::load_list(conn, def())?;
        *self.lock() = stored;
        Ok(())
    }
}

/// The library's tables: `library_entry`, with its files and its command and requires lists as child tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue;

    #[test]
    fn the_entries_come_back_whole_from_their_table() {
        let a = store("roundtrip-a");
        let mut tool = package("tool", true);
        tool.requires = vec!["requests".to_string()];
        tool.input_schema =
            json!({ "type": "object", "properties": { "n": { "type": "integer" } } });
        tool.files.push(PackageFile {
            path: "lib/util.py".to_string(),
            content_b64: base64_encode(b"def twice(n): return 2 * n"),
            executable: true,
        });
        a.insert(1, "alpha", "p1", tool).unwrap();
        a.record_tests("tool", 1, true).unwrap();
        a.review("tool", 1, true, "fine").unwrap();
        a.insert(2, "beta", "p1", package("tool", false)).unwrap();
        assert!(a.program(1, Some("p1"), "x_tool").is_some());
        let mut replacement = connector_package("fetcher", "firecrawl");
        replacement.replaces = "fetch_url".to_string();
        a.insert(3, "gamma", "p2", replacement).unwrap();
        a.record_tests("fetcher", 1, true).unwrap();
        a.review("fetcher", 1, true, "").unwrap();
        a.settle("fetcher", 1, true).unwrap();
        a.compare("fetcher", 1, comparison(true, true, true))
            .unwrap();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &a.rows().unwrap()).unwrap();
        let b = store("roundtrip-b");
        b.load(&conn).unwrap();
        assert_eq!(b.all().len(), 3);
        assert_eq!(
            serde_json::to_value(&*b.lock()).unwrap(),
            serde_json::to_value(&*a.lock()).unwrap()
        );
    }

    fn store(tag: &str) -> Store {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Store::open(std::env::temp_dir().join(format!("apid-library-{tag}-{nanos}")))
    }

    fn package(name: &str, publish: bool) -> Package {
        Package {
            name: name.to_string(),
            description: "adds numbers".to_string(),
            command: vec!["python3".to_string(), "main.py".to_string()],
            requires: Vec::new(),
            input_schema: json!({ "type": "object" }),
            publish,
            connector: String::new(),
            replaces: String::new(),
            files: vec![PackageFile {
                path: "main.py".to_string(),
                content_b64: base64_encode(b"print(1)"),
                executable: false,
            }],
        }
    }

    /// A proposal that uses `connector`'s secret: its component is the test component.
    fn connector_package(name: &str, connector: &str) -> Package {
        Package {
            command: Vec::new(),
            connector: connector.to_string(),
            files: vec![PackageFile {
                path: COMPONENT_FILE.to_string(),
                content_b64: base64_encode(&glue::tests::component()),
                executable: false,
            }],
            ..package(name, true)
        }
    }

    fn comparison(builtin_ok: bool, candidate_ok: bool, same: bool) -> Comparison {
        Comparison {
            at_ms: 1,
            builtin_ran: true,
            builtin_ok,
            builtin_ms: 4,
            candidate_ok,
            candidate_ms: 6,
            same,
        }
    }

    #[test]
    fn accept_stores_a_private_version_with_its_files() {
        let store = store("accept");
        let entry = store
            .insert(7, "scout", "p1", package("adder", false))
            .unwrap();
        assert_eq!((entry.version, entry.state.as_str()), (1, "private"));
        assert_eq!(entry.project, "p1");
        let on_disk = fs::read(store.files_dir("adder", 1).join("main.py")).unwrap();
        assert_eq!(on_disk, b"print(1)");
    }

    #[test]
    fn publish_waits_for_passed_tests_and_the_planners_approval() {
        let store = store("review");
        let entry = store
            .insert(7, "scout", "p1", package("adder", true))
            .unwrap();
        assert_eq!(entry.state, "pending");
        assert!(store.review("adder", 1, true, "fine").is_err());
        assert_eq!(store.record_tests("adder", 1, true).unwrap(), "passed");
        assert_eq!(store.review("adder", 1, true, "fine").unwrap(), "published");
        assert!(store.review("adder", 1, true, "again").is_err());
    }

    #[test]
    fn a_tool_serves_its_project_until_it_is_promoted_and_stops_when_retired() {
        let store = store("visibility");
        store
            .insert(7, "scout", "p1", package("adder", true))
            .unwrap();
        store.record_tests("adder", 1, true).unwrap();
        assert!(store.offered(8, Some("p1")).is_empty());
        store.review("adder", 1, true, "").unwrap();
        assert_eq!(store.offered(8, Some("p1"))[0].version, 1);
        assert!(store.offered(8, Some("p2")).is_empty());
        assert!(store.offered(8, None).is_empty());
        store.promote("adder", 1).unwrap();
        assert_eq!(store.offered(8, Some("p2"))[0].version, 1);
        store
            .insert(7, "scout", "p1", package("adder", false))
            .unwrap();
        assert_eq!(store.offered(7, Some("p1"))[0].version, 2);
        assert_eq!(store.offered(8, Some("p2"))[0].version, 1);
        store.retire("adder", 1).unwrap();
        assert!(store.offered(8, Some("p2")).is_empty());
    }

    #[test]
    fn program_assembles_the_visible_files_and_measurement_counts_the_calls() {
        let store = store("program");
        store
            .insert(7, "scout", "p1", package("adder", true))
            .unwrap();
        store.record_tests("adder", 1, true).unwrap();
        store.review("adder", 1, true, "").unwrap();
        assert!(store.program(8, Some("p1"), "x_adder").is_some());
        assert!(store.program(8, Some("p1"), "x_missing").is_none());
        assert!(store.program(8, Some("p1"), "adder").is_none());
        assert!(store.program(8, Some("p2"), "x_adder").is_none());
        let program = store.program(7, Some("p1"), "x_adder").unwrap();
        assert_eq!(
            (program.name.as_str(), program.version, program.timeout_s),
            ("adder", 1, 120)
        );
        assert_eq!(
            base64_decode(&program.files[0].content_b64).unwrap(),
            b"print(1)"
        );
        store.measure("x_adder", true, 5).unwrap();
        store.measure("x_adder", false, 7).unwrap();
        assert!(store.measure("adder", false, 7).is_none());
        let entry = store.find("adder", 1).unwrap();
        assert_eq!((entry.calls, entry.failures, entry.latency_ms), (2, 1, 12));
    }

    #[test]
    fn a_secret_tool_waits_for_the_connector_card_and_runs_on_the_glue_tier_only() {
        let store = store("secret");
        store
            .insert(7, "scout", "p1", connector_package("fetcher", "firecrawl"))
            .unwrap();
        store.record_tests("fetcher", 1, true).unwrap();
        assert_eq!(
            store.review("fetcher", 1, true, "").unwrap(),
            "awaiting_owner"
        );
        assert!(store.program(7, Some("p1"), "x_fetcher").is_none());
        assert_eq!(store.settle("fetcher", 1, true).unwrap(), "published");
        assert!(store.program(7, Some("p1"), "x_fetcher").is_none());
        assert!(store.visible(7, Some("p1"), "x_fetcher").is_some());
        assert_eq!(
            store.component("fetcher", 1).unwrap(),
            glue::tests::component()
        );
        assert!(
            store
                .insert(
                    7,
                    "scout",
                    "p1",
                    Package {
                        files: vec![],
                        ..connector_package("bad", "firecrawl")
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn a_replacement_moves_through_its_stages_on_evidence_and_switches_back_on_regression() {
        let store = store("stages");
        let mut replacement = connector_package("fetcher", "firecrawl");
        replacement.replaces = "fetch_url".to_string();
        store.insert(7, "scout", "p1", replacement).unwrap();
        store.record_tests("fetcher", 1, true).unwrap();
        store.review("fetcher", 1, true, "").unwrap();
        store.settle("fetcher", 1, true).unwrap();

        assert!(store.set_stage("fetcher", 1, "shadow", "", true).is_err());
        assert!(store.set_stage("fetcher", 1, "trial", "p1", false).is_err());
        store.set_stage("fetcher", 1, "shadow", "", false).unwrap();
        assert!(store.replacement_for("fetch_url", Some("p1")).is_some());
        assert!(store.set_stage("fetcher", 1, "trial", "p1", false).is_err());
        for _ in 0..STAGE_EVIDENCE {
            assert!(
                !store
                    .compare("fetcher", 1, comparison(true, true, true))
                    .unwrap()
            );
        }
        store.set_stage("fetcher", 1, "trial", "p1", false).unwrap();
        assert!(store.replacement_for("fetch_url", Some("p2")).is_none());
        assert!(store.replacement_for("fetch_url", Some("p1")).is_some());
        assert!(store.set_stage("fetcher", 1, "all", "", false).is_err());
        for _ in 0..STAGE_EVIDENCE {
            store
                .compare("fetcher", 1, comparison(true, true, true))
                .unwrap();
        }
        store.set_stage("fetcher", 1, "all", "", false).unwrap();
        assert!(store.replacement_for("fetch_url", Some("p9")).is_some());

        // Three candidate failures in a row, where the built-in held, is a regression: the built-in serves again.
        let mut regressed_now = false;
        for _ in 0..3 {
            regressed_now = store
                .compare("fetcher", 1, comparison(true, false, false))
                .unwrap();
        }
        assert!(regressed_now);
        store.switch_back("fetcher", 1);
        assert!(store.replacement_for("fetch_url", Some("p1")).is_none());
        assert_eq!(store.find("fetcher", 1).unwrap().stage, "");
    }

    #[test]
    fn a_side_effecting_built_in_is_never_shadowed_and_a_harmful_tool_is_proposed_for_removal() {
        let store = store("removal");
        let mut replacement = connector_package("poster", "firecrawl");
        replacement.replaces = "post_comment".to_string();
        store.insert(7, "scout", "p1", replacement).unwrap();
        store.record_tests("poster", 1, true).unwrap();
        store.review("poster", 1, true, "").unwrap();
        store.settle("poster", 1, true).unwrap();
        assert!(store.set_stage("poster", 1, "shadow", "", true).is_err());

        store
            .insert(7, "scout", "p1", package("flaky", true))
            .unwrap();
        store.record_tests("flaky", 1, true).unwrap();
        store.review("flaky", 1, true, "").unwrap();
        for i in 0..HARMFUL_CALLS {
            store.measure("x_flaky", i >= 6, 3).unwrap();
        }
        let flaky = store.find("flaky", 1).unwrap();
        assert!(harmful(&flaky), "{flaky:?}");
        let names: Vec<String> = store
            .removal_candidates(now_ms())
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"flaky".to_string()));
        assert!(!names.contains(&"poster".to_string()));
        // Unused for thirty days, as seen from a later time: proposed too.
        store
            .insert(7, "scout", "p1", package("idle", true))
            .unwrap();
        store.record_tests("idle", 1, true).unwrap();
        store.review("idle", 1, true, "").unwrap();
        let later = store.find("idle", 1).unwrap().created_ms + UNUSED_MS;
        let names: Vec<String> = store
            .removal_candidates(later)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"idle".to_string()));
    }

    #[test]
    fn a_connector_tool_is_refused_when_it_has_no_component() {
        let store = store("no-component");
        let mut bad = connector_package("fetcher", "firecrawl");
        bad.files.clear();
        assert!(store.insert(7, "scout", "p1", bad).is_err());
    }
}
