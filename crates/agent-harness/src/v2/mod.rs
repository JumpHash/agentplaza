//! The v2 tools: the agent's hands on its VM's workspace, one module per family.
//!
//! * [`read`] reads several files or line ranges in one call, and an outline first for a large file.
//! * [`edit`] applies exact-text edits and whole-file creates across files, each file all or nothing.
//! * [`find`] lists files by glob and searches their contents, from a warm file index.
//! * [`search`] returns the matches of a regex with their context lines.
//! * [`run`] runs commands in one persistent shell per work item, and background jobs that report their end.
//! * [`run_tests`] picks the workspace's test runner, runs it through [`run`], and parses its failures.
//!
//! Each tool's output is one JSON object, as a string. A cut result says where to continue (`continue`), and
//! a result carries `notes` when something the agent should know arrived since its last result: a file it read
//! changed underneath it, or a background job ended. The per-tool budgets are the constants of each module.
//!
//! Read tracking lives per workspace and per work item ([`Lane`]): a work item that starts over starts with no
//! tracked reads, and its persistent shell is ended. The tools call each other only through this module.
//!
//! The path rules are [`crate::files`]': every path a tool takes is resolved through [`Workspace::resolve`].

pub mod edit;
pub mod find;
pub mod read;
pub mod run;
pub mod run_tests;
pub mod search;
mod track;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use crate::files::Workspace;
use crate::tools::ToolError;

/// The most notes one result carries; the rest wait for the next result.
const MAX_NOTES: usize = 32;

/// The workspaces the lanes are kept for, by the root they were configured with.
static LANES: LazyLock<Mutex<BTreeMap<PathBuf, Arc<Lane>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// What the v2 tools keep for one workspace: its read tracking and persistent shell (both per work item), its
/// background jobs and file index, and the notes waiting for the next result.
pub(crate) struct Lane {
    /// The workspace's reads, by path, for the work item they were made in.
    pub(crate) tracking: Mutex<track::Tracking>,
    /// The persistent shell `run` writes commands to; none until the first command.
    pub(crate) shell: Mutex<Option<run::Shell>>,
    /// The background jobs that run, and the id of the next one.
    pub(crate) jobs: Mutex<run::Jobs>,
    /// The file index `find` and `search` answer from.
    pub(crate) index: Mutex<Option<Arc<find::Index>>>,
    /// Notes for the next result: changed files and ended jobs, in the order they happened.
    pub(crate) notes: Mutex<Vec<String>>,
    /// The project memory each work item was shown, by work item (anchors.rs).
    pub(crate) anchored: Mutex<BTreeMap<u64, crate::anchors::Memo>>,
}

impl Lane {
    fn new() -> Self {
        Self {
            tracking: Mutex::new(track::Tracking::default()),
            shell: Mutex::new(None),
            jobs: Mutex::new(run::Jobs::default()),
            index: Mutex::new(None),
            notes: Mutex::new(Vec::new()),
            anchored: Mutex::new(BTreeMap::new()),
        }
    }

    /// Queues a note for the next result. A note is kept until a result carries it.
    pub(crate) fn note(&self, text: String) {
        let mut notes = locked(&self.notes);
        if notes.len() < MAX_NOTES {
            notes.push(text);
        }
    }
}

/// The lane of `workspace`, made on first use.
pub(crate) fn lane_of(workspace: &Workspace) -> Arc<Lane> {
    let mut lanes = locked(&LANES);
    Arc::clone(
        lanes
            .entry(workspace.dir().to_path_buf())
            .or_insert_with(|| Arc::new(Lane::new())),
    )
}

/// Runs the v2 tool `name` for `work`, with `input`. Returns the JSON the tool made, or the refusal the shared
/// taxonomy names (see [`ToolError`]).
///
/// Before the tool runs, the lane starts over if `work` is a work item it has not served, and the reads of
/// the lane's files are checked: a changed one gets a note. The notes queued so far go into the result. A call
/// that refuses keeps its notes for the next one.
pub fn serve(
    workspace: &Workspace,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, ToolError> {
    let lane = lane_of(workspace);
    lane.enter(work);
    track::changes(workspace, &lane);
    let call = locked(&lane.tracking).next_call();
    let mut reply = match name {
        "read" => read::read(workspace, &lane, call, input)?,
        "edit" => edit::edit(workspace, &lane, call, input)?,
        "find" => find::find(workspace, &lane, input)?,
        "search" => search::search(workspace, &lane, input)?,
        "run" => run::run(workspace, &lane, input)?,
        "run_tests" => run_tests::run_tests(workspace, &lane, input)?,
        other => return Err(ToolError::UnknownTool(other.to_string())),
    };
    let notes: Vec<String> = std::mem::take(&mut *locked(&lane.notes));
    if !notes.is_empty() {
        reply["notes"] = json!(notes);
    }
    reply_json(&reply)
}

/// Stops what `workspace`'s agent started and keeps running: its persistent shell and its background jobs. The
/// agent is stopping, so the notes its jobs would report are dropped with them.
pub fn stop_all(workspace: &Workspace) {
    let lane = lane_of(workspace);
    run::stop(&lane);
    locked(&lane.notes).clear();
}

impl Lane {
    /// Starts the lane over for `work` when it is a work item it has not served yet: the tracked reads go, and
    /// the persistent shell ends (its directory and variables belong to the work item that made them).
    fn enter(&self, work: u64) {
        let mut tracking = locked(&self.tracking);
        if tracking.work() == Some(work) {
            return;
        }
        tracking.start(work);
        drop(tracking);
        let shell = locked(&self.shell).take();
        drop(shell);
    }
}

/// The JSON text of a reply.
pub(crate) fn reply_json(reply: &Value) -> Result<String, ToolError> {
    serde_json::to_string(reply).map_err(|err| ToolError::Internal {
        reason: format!("cannot encode the reply: {err}"),
    })
}

/// A count of entries that fit in `budget` bytes of their JSON, in order, and whether any were left out. An
/// entry is kept whole or not at all, so a result never ends in a half-written entry.
pub(crate) fn fit(entries: Vec<Value>, budget: usize) -> (Vec<Value>, bool) {
    let mut used = 0;
    let mut kept = Vec::new();
    for entry in entries {
        let size = entry.to_string().len() + 1;
        if used + size > budget {
            return (kept, true);
        }
        used += size;
        kept.push(entry);
    }
    (kept, false)
}

/// The `continue` hint for a tool: the tool's name and the arguments that resume its cut result.
pub(crate) fn continue_hint(tool: &str, args: &Value) -> String {
    format!("continue: {tool} {args}")
}

/// A lock whose holder panicked stays usable: the data it guards is still consistent here.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The workspace-relative label of `path` under `root`: `/`-separated, `.` for the root itself.
pub(crate) fn label_under(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
        Ok(rest) => rest
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

/// The text of a refusal for a per-file result, cut short enough that a result can carry many of them.
pub(crate) fn refusal_text(err: &ToolError) -> String {
    let text = err.to_string();
    if text.len() <= 512 {
        return text;
    }
    let mut end = 512;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_whole_entries_in_order_and_says_when_it_cut() {
        let entries: Vec<Value> = (0..5)
            .map(|n| json!({"n": n, "pad": "x".repeat(10)}))
            .collect();
        let (kept, cut) = fit(entries.clone(), 1_000);
        assert_eq!(kept.len(), 5);
        assert!(!cut);
        let (kept, cut) = fit(entries, 40);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(cut);
    }

    #[test]
    fn a_label_is_relative_and_the_root_is_a_dot() {
        let root = Path::new("/ws");
        assert_eq!(label_under(root, Path::new("/ws")), ".");
        assert_eq!(label_under(root, Path::new("/ws/src/a.rs")), "src/a.rs");
    }

    #[test]
    fn a_long_refusal_is_cut_on_a_character_boundary() {
        let long = ToolError::Denied {
            reason: "é".repeat(400),
        };
        let text = refusal_text(&long);
        assert!(text.ends_with('…'), "{text}");
        assert!(text.len() <= 515, "{}", text.len());
    }
}
