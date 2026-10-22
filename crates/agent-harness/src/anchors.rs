//! The project memory a read or an edit meets (PLAN 9.5): the active entries anchored to the files the call named are added
//! to its result, under `memory`, once per entry per work item. The hub answers each path through `memory_anchored`, a
//! host tool request like any other: a nested one, registered under the call, so the agent loop is not held up by it.
//! The answers are kept per work item, so a file is asked about once, and an entry is shown once however many files
//! name it.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use apz_proto::wire::{Decision, GuestMessage, HostMessage};
use serde_json::{Value, json};

use crate::files::Workspace;
use crate::lane::Shared;
use crate::v2;

/// How long a path's answer may take before the call goes on without it.
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The host tool that answers for a path.
const TOOL: &str = "memory_anchored";

/// What a work item has been told about the project memory: the entries each path answered with, and the entries shown.
#[derive(Default)]
pub(crate) struct Memo {
    answers: HashMap<String, Vec<Value>>,
    shown: HashSet<String>,
}

/// Distinguishes the nested ids of the answers a process asks for.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// The result of a read or an edit with the project memory it meets: the entries anchored to the paths `input` names
/// that this work item has not been shown yet, under `memory`. A call that is not a read or an edit, a failed one, and a
/// result with nothing new are returned as they were.
pub(crate) fn surface<S: Write, W: Write>(
    shared: &Shared<'_, S, W>,
    workspace: &Workspace,
    work: u64,
    call: &str,
    name: &str,
    input: &Value,
    outcome: Result<String, String>,
) -> Result<String, String> {
    let Ok(output) = &outcome else {
        return outcome;
    };
    if !matches!(name, "read" | "edit") {
        return outcome;
    }
    let paths = paths_of(input);
    let lane = v2::lane_of(workspace);
    // The paths not yet answered for this work item are asked for without the lock: the answers take a round trip each.
    let missing: Vec<String> = {
        let memo = locked(&lane.anchored);
        let answered = memo.get(&work);
        paths
            .iter()
            .filter(|path| !answered.is_some_and(|m| m.answers.contains_key(*path)))
            .cloned()
            .collect()
    };
    let asked: Vec<(String, Vec<Value>)> = missing
        .into_iter()
        .filter_map(|path| match ask(shared, work, call, &path) {
            Ok(entries) => Some((path, entries)),
            // A path whose answer failed is asked for again by the next call that names it.
            Err(_) => None,
        })
        .collect();
    let shown = {
        let mut memos = locked(&lane.anchored);
        let memo = memos.entry(work).or_default();
        memo.answers.extend(asked);
        reveal(memo, &paths)
    };
    if shown.is_empty() {
        return outcome;
    }
    // Every read and edit answers with a JSON object; the entries join it.
    match serde_json::from_str::<Value>(output) {
        Ok(Value::Object(mut result)) => {
            result.insert("memory".into(), Value::Array(shown));
            Ok(Value::Object(result).to_string())
        }
        _ => outcome,
    }
}

/// The entries anchored to `paths` that the work item has not been shown yet, each once, in the order the paths name them.
fn reveal(memo: &mut Memo, paths: &[String]) -> Vec<Value> {
    let mut shown: Vec<Value> = Vec::new();
    for path in paths {
        for entry in memo.answers.get(path).into_iter().flatten() {
            let id = entry["id"].as_str().unwrap_or_default().to_string();
            if memo.shown.insert(id) {
                shown.push(entry.clone());
            }
        }
    }
    shown
}

/// The paths a read or an edit names: `files` of a read, and `edits` and `create` of an edit, each item's `path`. Each once.
fn paths_of(input: &Value) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for key in ["files", "edits", "create"] {
        let named = input
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("path").and_then(Value::as_str))
            .filter(|path| !path.is_empty());
        for path in named {
            if !paths.iter().any(|known| known == path) {
                paths.push(path.to_string());
            }
        }
    }
    paths
}

/// Asks the hub which active entries are anchored to `path`: a nested tool request under a fresh id of `call`, and the
/// entries of its answer. An answer that does not come in time, or is a refusal, is an error.
fn ask<S: Write, W: Write>(
    shared: &Shared<'_, S, W>,
    work: u64,
    call: &str,
    path: &str,
) -> Result<Vec<Value>, String> {
    let id = format!("{call}.m{}", NEXT.fetch_add(1, Ordering::SeqCst) + 1);
    // Registered before the request goes out, so the hub's answer always finds its taker.
    let inbox = shared.nested().register(&id);
    let answer = match shared.say(&GuestMessage::ToolRequest {
        work,
        call: id.clone(),
        name: TOOL.to_string(),
        input: json!({ "paths": [path] }),
    }) {
        Ok(()) => wait(&inbox),
        Err(err) => Err(format!("cannot reach the hub: {err}")),
    };
    shared.nested().unregister(&id);
    let value: Value = serde_json::from_str(&answer?).map_err(|err| err.to_string())?;
    Ok(value["entries"].as_array().cloned().unwrap_or_default())
}

/// Waits for the hub's decision on a nested request: its output when it allowed the call.
fn wait(inbox: &Receiver<HostMessage>) -> Result<String, String> {
    let deadline = Instant::now() + ANSWER_WAIT;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("the hub did not answer in time".into());
        }
        match inbox.recv_timeout(deadline - now) {
            Ok(HostMessage::ToolDecision {
                decision: Decision::Allow | Decision::Done,
                output,
                is_error: false,
                ..
            }) => return Ok(output),
            Ok(HostMessage::ToolDecision { output, .. }) => return Err(output),
            Ok(_) => {}
            Err(_) => return Err("the hub did not answer".into()),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_or_an_edit_names_each_path_once() {
        let input = json!({
            "files": [{"path": "crates/apid/src/a.rs"}, {"path": "crates/apid/src/b.rs", "lines": [1, 9]}],
            "edits": [{"path": "crates/apid/src/a.rs", "old": "x", "new": "y"}],
            "create": [{"path": "crates/apid/src/c.rs"}, {"path": ""}]
        });
        assert_eq!(
            paths_of(&input),
            [
                "crates/apid/src/a.rs",
                "crates/apid/src/b.rs",
                "crates/apid/src/c.rs"
            ]
        );
        assert!(paths_of(&json!({"pattern": "x"})).is_empty());
    }

    #[test]
    fn an_entry_is_shown_once_per_work_item_however_many_paths_name_it() {
        let mut memo = Memo::default();
        let entry = json!({"id": "P-0017", "kind": "pitfall", "title": "move waits", "text": "...", "status": "active"});
        memo.answers.insert("a.rs".into(), vec![entry.clone()]);
        memo.answers.insert("b.rs".into(), vec![entry]);
        let paths = ["a.rs".to_string(), "b.rs".to_string()];
        assert_eq!(reveal(&mut memo, &paths).len(), 1);
        assert!(reveal(&mut memo, &paths).is_empty());
    }
}
