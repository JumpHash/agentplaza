//! The IDE's workspace tools for the owner: the operator's view of an agent's `/workspace`.
//!
//! The hub reaches these through `tell` like every operator tool, and [`crate::tools::serve`] runs them.
//! The owner is the principal, so a write needs no approval, but each write first takes a checkpoint
//! labelled `owner edit: <path>`, and a write that names a stale hash is refused as `conflict:`. Every
//! reply is JSON text.
//!
//! The file, run and terminal operations are [`apz_ops`], which the workstation daemon shares. This module
//! reads the tools' arguments and serves the replies, and keeps what is the harness's own: the checkpoints
//! every owner write takes ([`checkpoints`]), `ide_changes`, `ide_checkpoints` and `ide_restore`.
//!
//! # Replies
//!
//! Each reply is under the operator's output cap. Text is paged in chunks of at most [`CHUNK`] bytes on
//! character boundaries. Nothing is cut silently: a reply says `done`, `next` or `truncated`.
//!
//! # Runs
//!
//! `ide_run_start` starts `sh -c` in the workspace and returns at once, so a run never holds a tell past its
//! five seconds. See [`apz_ops::Runs`] for the limits.

use std::path::Path;

use apz_ide::path::is_secret;
use apz_ops::page::{OUTPUT_BUDGET, chunk_end};
use apz_ops::{OpError, Root, Runs, WriteHooks};
use serde_json::{Value, json};

use crate::bash;
use crate::checkpoints::{self, Summary};
use crate::files::{self, Workspace};
use crate::tools::{ToolError, reply_json};

/// The largest diff `ide_changes` renders.
const MAX_DIFF: usize = 8 * 1024 * 1024;

/// The runs the harness holds. One process, one workspace: a static is enough.
static RUNS: Runs = Runs::new();

/// `ide_list {path, cursor?}`: one page of a directory, directories first, then by name.
///
/// Output: `{"entries": [{path, kind, size, hash, binary}], "next"}`. `next` is the cursor for the following
/// page, or null. Secret paths are skipped, and `.plaza` at the root is hidden.
pub fn list(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let path = files::string_arg(args, "path")?;
    let cursor = match args.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) => Some(cursor.as_str()),
        Some(_) => return Err(files::invalid("\"cursor\" must be a string")),
    };
    reply_json(&apz_ops::files::list(&root(workspace), &path, cursor)?)
}

/// `ide_read {path, offset?}`: a chunk of a text file from byte `offset`.
///
/// Output: `{"path", "size", "hash", "offset", "text", "done"}`. `hash` is the sha256 of the whole file, which
/// a later `ide_write` sends back as `expected_hash`. A file over 1 MiB, a binary file and a file that is not
/// UTF-8 are refused.
pub fn read(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let path = files::string_arg(args, "path")?;
    let offset = match args.get("offset") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
        ),
    };
    reply_json(&apz_ops::files::read(&root(workspace), &path, offset)?)
}

/// `ide_write {path, text, expected_hash}`: replaces a text file, or creates it.
///
/// `expected_hash` is the file's hash as last read, or `null` for a file that must not exist yet. A mismatch
/// is refused as `conflict: <current hash>` (or `conflict: missing`) and nothing is written or checkpointed.
/// Otherwise the file is checkpointed as `owner edit: <path>` and written atomically. Output: `{"path", "hash",
/// "checkpoint"}`.
pub fn write(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let path = files::string_arg(args, "path")?;
    let text = files::string_arg(args, "text")?;
    let expected = match args.get("expected_hash") {
        Some(Value::Null) => None,
        Some(Value::String(hash)) => Some(hash.clone()),
        Some(_) => {
            return Err(files::invalid(
                "\"expected_hash\" must be a string, or null for a file that must not exist yet",
            ));
        }
        None => {
            return Err(files::invalid(
                "\"expected_hash\" is required: the hash last read, or null for a file that must not exist yet",
            ));
        }
    };
    let mut owner_edit = OwnerEdit { workspace };
    let reply = apz_ops::files::write(
        &root(workspace),
        &path,
        &text,
        expected.as_deref(),
        &mut owner_edit,
    )?;
    reply_json(&reply)
}

/// The owner's writes: each takes an `owner edit` checkpoint before it changes a file, and gives the folders
/// and temp file it creates to the agent user, as the file tools always have.
struct OwnerEdit<'a> {
    workspace: &'a Workspace,
}

impl WriteHooks for OwnerEdit<'_> {
    fn before_write(&mut self, path: &Path) -> Result<Option<String>, OpError> {
        let label = format!("owner edit: {}", path.to_string_lossy());
        checkpoints::snapshot(self.workspace, &label)
            .map(Some)
            .map_err(OpError::Internal)
    }

    fn created(&mut self, path: &Path) {
        files::give_to_agent(path);
    }
}

/// `ide_changes {base?, offset?}`: the unified diff from a base checkpoint to the workspace now.
///
/// `base` is `work_start` (the default: the latest checkpoint a work item took at its start, or the oldest
/// checkpoint when there is none) or `checkpoint:<n>`. Output: `{"base", "checkpoint", "offset", "total", "diff",
/// "done"}`, where `checkpoint` is the base's id and `diff` a chunk of the git-style diff, which
/// `apz_ide::parse_unified` reads. Secret files are left out.
pub fn changes(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let base = match args.get("base") {
        None | Some(Value::Null) => "work_start".to_string(),
        Some(Value::String(base)) => base.clone(),
        Some(_) => {
            return Err(files::invalid(
                "\"base\" must be \"work_start\" or \"checkpoint:<n>\"",
            ));
        }
    };
    let summaries = checkpoints::summaries(workspace);
    let chosen = resolve_base(&summaries, &base)?;
    let diff = checkpoints::unified_since(workspace, chosen.n, |path| !is_secret(path))
        .map_err(internal)?;
    if diff.len() > MAX_DIFF {
        return Err(files::limit(format!(
            "the diff is {} bytes, over the {MAX_DIFF}-byte cap; use a later base",
            diff.len()
        )));
    }
    let offset = match args.get("offset") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .and_then(|at| usize::try_from(at).ok())
            .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
    };
    if offset > diff.len() {
        return Err(files::invalid(format!(
            "offset {offset} is past the end of the diff ({} bytes)",
            diff.len()
        )));
    }
    if !diff.is_char_boundary(offset) {
        return Err(files::invalid(format!(
            "offset {offset} is inside a character of the diff"
        )));
    }
    let reply = |end: usize| {
        json!({
            "base": base,
            "checkpoint": chosen.id,
            "offset": offset,
            "total": diff.len(),
            "diff": &diff[offset..end],
            "done": end == diff.len(),
        })
        .to_string()
    };
    let end = chunk_end(&diff, offset, |end| reply(end).len() <= OUTPUT_BUDGET);
    Ok(reply(end))
}

/// `ide_checkpoints`: every checkpoint, newest first: `{"checkpoints": [{n, label, created_ms, files}]}`.
pub fn list_checkpoints(workspace: &Workspace, _args: &Value) -> Result<String, ToolError> {
    let rows: Vec<Value> = checkpoints::summaries(workspace)
        .iter()
        .rev()
        .map(|s| json!({"n": s.n, "label": s.label, "created_ms": s.at_ms, "files": s.files}))
        .collect();
    Ok(json!({ "checkpoints": rows }).to_string())
}

/// `ide_restore {n}`: takes an `owner restore` checkpoint of the current state, then restores checkpoint `n`.
/// Output: `{"restored": n, "checkpoint": <the owner restore checkpoint>}`.
pub fn restore(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let n = match args.get("n") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| files::invalid("\"n\" must be a whole number"))?,
        None => return Err(files::invalid("\"n\" is required: the checkpoint number")),
    };
    if !checkpoints::summaries(workspace).iter().any(|s| s.n == n) {
        return Err(files::not_found(format!("no checkpoint {n}")));
    }
    let checkpoint = checkpoints::snapshot(workspace, "owner restore").map_err(internal)?;
    // The new checkpoint may have pruned the oldest one; then there is nothing to roll back to.
    if !checkpoints::summaries(workspace).iter().any(|s| s.n == n) {
        return Err(files::not_found(format!(
            "checkpoint {n} was pruned by the owner restore checkpoint {checkpoint}; nothing was restored"
        )));
    }
    checkpoints::restore(workspace, &json!({ "id": n.to_string() })).map_err(internal)?;
    Ok(json!({ "restored": n, "checkpoint": checkpoint }).to_string())
}

/// `ide_search {q, glob?, max?}`: a plain substring search of the text files.
///
/// Case-insensitive when `q` has no uppercase letters. `glob` matches the path relative to the workspace.
/// `max` defaults to and is capped at 200. Output: `{"matches": [{path, line, text}], "truncated"}`.
pub fn search(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let q = files::string_arg(args, "q")?;
    let glob = match args.get("glob") {
        None | Some(Value::Null) => None,
        Some(Value::String(pattern)) => Some(pattern.as_str()),
        Some(_) => return Err(files::invalid("\"glob\" must be a string")),
    };
    let max = match args.get("max") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| files::invalid("\"max\" must be a whole number"))?
                as usize,
        ),
    };
    reply_json(&apz_ops::files::search(&root(workspace), &q, glob, max)?)
}

/// `ide_run_start {command, timeout_s?}`: starts `sh -c command` in the workspace and returns at once.
///
/// Output: `{"run": id}`. The run is the agent's, in its own process group, with the agent's environment.
pub fn run_start(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let command = args.get("command").and_then(Value::as_str).unwrap_or("");
    let timeout_s =
        match args.get("timeout_s") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_u64().ok_or_else(|| {
                files::invalid("\"timeout_s\" must be a whole number of seconds")
            })?),
        };
    let env = bash::agent_env(workspace.dir()).map_err(internal)?;
    reply_json(&RUNS.start(&root(workspace), &env, command, timeout_s)?)
}

/// `ide_run_poll {run, offset?}`: the run's output from `offset`, at most one chunk.
///
/// Output: `{"run", "offset", "output", "next", "done", "exit_code", "timed_out"}`.
pub fn run_poll(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let run = files::string_arg(args, "run")?;
    let offset = match args.get("offset") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
        ),
    };
    reply_json(&RUNS.poll(&run, offset)?)
}

/// `ide_run_stop {run}`: kills the run's process group and waits briefly for it to end.
/// Output: `{"run", "stopped", "done"}`.
pub fn run_stop(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let run = files::string_arg(args, "run")?;
    reply_json(&RUNS.stop(&run)?)
}

/// The workspace root the shared operations work in.
fn root(workspace: &Workspace) -> Root {
    Root::new(workspace.dir())
}

fn internal(reason: impl Into<String>) -> ToolError {
    ToolError::Internal {
        reason: reason.into(),
    }
}

/// The base of a `base` argument: the checkpoint it names.
fn resolve_base<'a>(summaries: &'a [Summary], base: &str) -> Result<&'a Summary, ToolError> {
    if summaries.is_empty() {
        return Err(files::not_found("no checkpoints yet"));
    }
    if base == "work_start" {
        // Checkpoints are oldest first: the latest work start, else the oldest checkpoint.
        return Ok(summaries
            .iter()
            .rev()
            .find(|s| s.label.starts_with("work-"))
            .unwrap_or(&summaries[0]));
    }
    let n = base
        .strip_prefix("checkpoint:")
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| {
            files::invalid(format!(
                "\"base\" must be \"work_start\" or \"checkpoint:<n>\", not {base:?}"
            ))
        })?;
    summaries
        .iter()
        .find(|s| s.n == n)
        .ok_or_else(|| files::not_found(format!("no checkpoint {n}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoints::sha_hex;
    use crate::tools;
    use apz_ide::diff::parse_unified;
    use apz_ops::page::CHUNK;
    use apz_proto::wire::MAX_TOOL_OUTPUT;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> (PathBuf, Workspace) {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-ide-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);
        (dir, workspace)
    }

    fn ok(result: Result<String, ToolError>) -> Value {
        let text = result.expect("the tool succeeds");
        assert!(
            text.len() <= MAX_TOOL_OUTPUT,
            "a reply of {} bytes is over the cap",
            text.len()
        );
        serde_json::from_str(&text).expect("the reply is JSON")
    }

    #[test]
    fn an_owner_write_takes_an_owner_edit_checkpoint_and_a_refused_one_takes_none() {
        let (dir, ws) = scratch("write");
        let first = ok(write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "one\n", "expected_hash": null }),
        ));
        assert_eq!(first["hash"], json!(sha_hex(b"one\n")));
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "one\n");
        let labels: Vec<String> = checkpoints::summaries(&ws)
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert!(
            labels.contains(&"owner edit: docs/a.txt".to_string()),
            "{labels:?}"
        );
        assert_eq!(
            first["checkpoint"].as_str().unwrap(),
            checkpoints::summaries(&ws).last().unwrap().id
        );

        let before = checkpoints::summaries(&ws).len();
        let stale = sha_hex(b"zero\n");
        let text = write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "two\n", "expected_hash": stale }),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(text, format!("conflict: {}", sha_hex(b"one\n")));
        assert_eq!(
            checkpoints::summaries(&ws).len(),
            before,
            "a refused write takes no checkpoint"
        );
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "one\n");

        let second = ok(write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "two\n", "expected_hash": sha_hex(b"one\n") }),
        ));
        assert_eq!(second["hash"], json!(sha_hex(b"two\n")));
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "two\n");
        assert_eq!(
            checkpoints::summaries(&ws).len(),
            before + 1,
            "an accepted write takes one checkpoint"
        );
    }

    #[test]
    fn a_write_must_say_what_hash_it_replaces() {
        let (_dir, ws) = scratch("write-args");
        let missing = write(&ws, &json!({ "path": "y.txt", "text": "y" })).unwrap_err();
        assert!(
            matches!(missing, ToolError::InvalidInput { .. }),
            "{missing:?}"
        );
        let wrong = write(
            &ws,
            &json!({ "path": "y.txt", "text": "y", "expected_hash": 3 }),
        )
        .unwrap_err();
        assert!(matches!(wrong, ToolError::InvalidInput { .. }), "{wrong:?}");
    }

    #[test]
    fn changes_parse_with_the_shared_parser_and_leave_secrets_out() {
        let (dir, ws) = scratch("changes");
        fs::write(
            dir.join("keep.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        fs::write(dir.join("bin.dat"), b"a\0b").unwrap();
        fs::write(dir.join(".env"), "TOKEN=old\n").unwrap();
        checkpoints::snapshot(&ws, "work-7").unwrap();

        fs::write(
            dir.join("keep.txt"),
            "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        fs::remove_file(dir.join("gone.txt")).unwrap();
        fs::create_dir_all(dir.join("sub dir")).unwrap();
        fs::write(dir.join("sub dir/new file.txt"), "fresh\n").unwrap();
        fs::write(dir.join("bin.dat"), b"c\0d").unwrap();
        fs::write(dir.join(".env"), "TOKEN=new\n").unwrap();

        let reply = ok(changes(&ws, &json!({})));
        assert_eq!(reply["base"], json!("work_start"));
        assert_eq!(reply["done"], json!(true));
        let diff = reply["diff"].as_str().unwrap();
        assert!(!diff.contains("TOKEN"), "secrets are left out");
        let parsed = parse_unified(diff).expect("the shared parser reads the diff");
        let by_path: std::collections::BTreeMap<&str, &apz_ide::FileChange> =
            parsed.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert_eq!(by_path.len(), 4, "{by_path:?}");
        let keep = by_path["keep.txt"];
        assert_eq!(keep.status, apz_ide::Status::Modified);
        assert_eq!(keep.hunks.len(), 1);
        assert_eq!(by_path["gone.txt"].status, apz_ide::Status::Deleted);
        assert_eq!(
            by_path["sub dir/new file.txt"].status,
            apz_ide::Status::Added
        );
        assert!(by_path["bin.dat"].binary);
        assert_eq!(by_path["bin.dat"].status, apz_ide::Status::Modified);

        // The same diff by a checkpoint number, and by its absence.
        let n = checkpoints::summaries(&ws)
            .iter()
            .find(|s| s.label == "work-7")
            .unwrap()
            .n;
        let by_number = ok(changes(&ws, &json!({ "base": format!("checkpoint:{n}") })));
        assert_eq!(by_number["diff"], reply["diff"]);
        assert!(changes(&ws, &json!({ "base": "nope" })).is_err());
        assert!(changes(&ws, &json!({ "base": "checkpoint:999999" })).is_err());
    }

    #[test]
    fn a_long_diff_pages_in_chunks_that_join_back_into_the_whole() {
        let (dir, ws) = scratch("changes-pages");
        let old: String = (0..6000)
            .map(|i| format!("line {i} with some padding text\n"))
            .collect();
        fs::write(dir.join("big.txt"), &old).unwrap();
        checkpoints::snapshot(&ws, "work-1").unwrap();
        let new: String = (0..6000)
            .map(|i| format!("LINE {i} with some padding text\n"))
            .collect();
        fs::write(dir.join("big.txt"), &new).unwrap();

        let mut offset = 0;
        let mut joined = String::new();
        let mut total: usize;
        loop {
            let reply = ok(changes(&ws, &json!({ "offset": offset })));
            total = reply["total"].as_u64().unwrap() as usize;
            let piece = reply["diff"].as_str().unwrap();
            joined.push_str(piece);
            offset += piece.len();
            if reply["done"] == json!(true) {
                break;
            }
        }
        assert_eq!(joined.len(), total);
        assert!(total > CHUNK, "the diff spans several chunks");
        let parsed = parse_unified(&joined).expect("the joined diff parses");
        assert_eq!(parsed.files[0].path, "big.txt");
        assert!(parsed.files[0].hunks.iter().any(|h| !h.lines.is_empty()));
    }

    #[test]
    fn restore_round_trips_and_keeps_an_owner_restore_checkpoint() {
        let (dir, ws) = scratch("restore");
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        let n = {
            checkpoints::snapshot(&ws, "work-3").unwrap();
            checkpoints::summaries(&ws).last().unwrap().n
        };
        ok(write(
            &ws,
            &json!({ "path": "a.txt", "text": "two\n", "expected_hash": sha_hex(b"one\n") }),
        ));
        fs::write(dir.join("b.txt"), "extra\n").unwrap();

        let reply = ok(restore(&ws, &json!({ "n": n })));
        assert_eq!(reply["restored"], json!(n));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
        assert!(!dir.join("b.txt").exists());
        let labels: Vec<String> = checkpoints::summaries(&ws)
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert!(labels.contains(&"owner restore".to_string()), "{labels:?}");
        assert_eq!(
            reply["checkpoint"].as_str().unwrap(),
            checkpoints::summaries(&ws)
                .iter()
                .find(|s| s.label == "owner restore")
                .unwrap()
                .id
        );
        assert!(matches!(
            restore(&ws, &json!({ "n": 999_999 })),
            Err(ToolError::NotFound { .. })
        ));
    }

    #[test]
    fn the_checkpoint_list_is_newest_first() {
        let (dir, ws) = scratch("checkpoint-list");
        fs::write(dir.join("a.txt"), "1").unwrap();
        checkpoints::snapshot(&ws, "work-1").unwrap();
        fs::write(dir.join("a.txt"), "2").unwrap();
        checkpoints::snapshot(&ws, "work-2").unwrap();
        let rows = ok(list_checkpoints(&ws, &json!({})));
        let rows = rows["checkpoints"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["label"], json!("work-2"));
        assert_eq!(rows[1]["label"], json!("work-1"));
        assert_eq!(rows[0]["files"], json!(1));
        assert!(rows[0]["created_ms"].as_u64().unwrap() > 0);
    }

    #[test]
    fn the_file_tools_reply_through_their_names() {
        let (dir, ws) = scratch("served-files");
        fs::write(dir.join("a.txt"), "needle here\n").unwrap();
        let listed = ok(tools::serve(&ws, "ide_list", &json!({ "path": "" })));
        assert_eq!(listed["entries"][0]["path"], json!("a.txt"));
        let read_back = ok(tools::serve(&ws, "ide_read", &json!({ "path": "a.txt" })));
        assert_eq!(read_back["text"], json!("needle here\n"));
        let found = ok(tools::serve(&ws, "ide_search", &json!({ "q": "needle" })));
        assert_eq!(found["matches"][0]["line"], json!(1));
        assert!(
            tools::serve(&ws, "ide_read", &json!({ "path": "../x" })).is_err(),
            "a path out of the workspace is refused through the name too"
        );
    }

    #[test]
    fn a_run_replies_through_its_names() {
        let (_dir, ws) = scratch("served-run");
        let started = ok(tools::serve(
            &ws,
            "ide_run_start",
            &json!({ "command": "echo ran-$((1+1))" }),
        ));
        let run = started["run"].as_str().unwrap().to_string();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let finished = loop {
            let poll = ok(tools::serve(&ws, "ide_run_poll", &json!({ "run": run })));
            if poll["done"] == json!(true) {
                break poll;
            }
            assert!(std::time::Instant::now() < deadline, "the run did not end");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(finished["exit_code"], json!(0));
        assert!(finished["output"].as_str().unwrap().contains("ran-2"));
        let stopped = ok(tools::serve(&ws, "ide_run_stop", &json!({ "run": run })));
        assert_eq!(stopped["stopped"], json!(false));
    }
}
