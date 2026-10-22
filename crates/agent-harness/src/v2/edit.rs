//! `edit`: exact-text replacements and whole-file creates, across files, in one call.
//!
//! Arguments (either array may be absent, and at least one is not empty): `create`, an array of
//! `{path, content}` that writes each whole file; and `edits`, an array of `{path, old, new, replace_all?}`
//! that replaces the exact text `old` with `new`. `old` must occur once unless `replace_all` is true, so an
//! edit says enough surrounding text to be unique. `new` may be empty, to delete.
//!
//! The operations on one file apply in order (its creates first, then its edits, each against the text the
//! earlier ones left), all or nothing: a refused one leaves that file as it was and says which one refused. A
//! file's result is independent of the others'.
//!
//! Output: `{"files": [...]}`, one entry per file in first-mention order: `{"path", "ok": true, "lines_changed",
//! "created"?, "replacements"?}` or `{"path", "ok": false, "error"}`. `lines_changed` counts the lines the edits
//! touched: for a replacement, the larger of its old and new line counts, per occurrence replaced. A written file
//! is recorded as read, so the agent needs no re-read to edit on from it.

use std::collections::BTreeMap;
use std::fs;

use serde_json::{Value, json};

use crate::files::{self, MAX_FILE_BYTES, MAX_WRITE_BYTES, Workspace};
use crate::tools::ToolError;

use super::track::{self, Seen, Stamp};
use super::{Lane, locked, refusal_text};

/// The most operations one `edit` call makes, creates and edits together.
pub const MAX_EDITS: usize = 256;

/// The largest `old` text: 4 KiB, the same as the file tools' needle cap.
pub const MAX_NEEDLE_BYTES: usize = 4 * 1024;

/// One operation on a file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// Write the whole file.
    Create(String),
    /// Replace exact text.
    Replace(Replacement),
}

/// One exact-text replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Replacement {
    old: String,
    new: String,
    replace_all: bool,
}

/// Runs an `edit` call.
pub(crate) fn edit(
    workspace: &Workspace,
    lane: &Lane,
    call: u64,
    input: &Value,
) -> Result<Value, ToolError> {
    let grouped = group(parse(input)?);
    let files: Vec<Value> = grouped
        .iter()
        .map(
            |(path, ops)| match apply_file(workspace, lane, call, path, ops) {
                Ok(entry) => entry,
                Err(err) => json!({ "path": path, "ok": false, "error": refusal_text(&err) }),
            },
        )
        .collect();
    Ok(json!({ "files": files }))
}

/// The call's operations as `(path, ops)` pairs, the paths in first-mention order. The creates of a file come
/// before its edits, whatever order the arrays had them in.
fn group(ops: Vec<(String, Op, bool)>) -> Vec<(String, Vec<Op>)> {
    let mut order: Vec<String> = Vec::new();
    let mut by_path: BTreeMap<String, (Vec<Op>, Vec<Op>)> = BTreeMap::new();
    for (path, op, is_create) in ops {
        if !by_path.contains_key(&path) {
            order.push(path.clone());
        }
        let (creates, edits) = by_path.entry(path).or_default();
        if is_create {
            creates.push(op);
        } else {
            edits.push(op);
        }
    }
    order
        .into_iter()
        .map(|path| {
            let (mut creates, edits) = by_path.remove(&path).unwrap_or_default();
            creates.extend(edits);
            (path, creates)
        })
        .collect()
}

/// The operations the input asks for, each with its path and whether it is a create.
fn parse(input: &Value) -> Result<Vec<(String, Op, bool)>, ToolError> {
    let creates = array_of(input, "create")?;
    let edits = array_of(input, "edits")?;
    let total = creates.len() + edits.len();
    if total == 0 {
        return Err(files::invalid(
            "give \"edits\" (exact replacements) or \"create\" (whole files)",
        ));
    }
    if total > MAX_EDITS {
        return Err(files::limit(format!(
            "{total} operations, over the {MAX_EDITS}-operation cap; split the call"
        )));
    }
    let mut ops = Vec::with_capacity(total);
    for item in creates {
        let path = path_of(item)?;
        let content = text_of(item, "content")?;
        if content.len() as u64 > MAX_WRITE_BYTES {
            return Err(files::limit(format!(
                "{path:?}: the content is {} bytes, over the {MAX_WRITE_BYTES}-byte write cap",
                content.len()
            )));
        }
        ops.push((path, Op::Create(content), true));
    }
    for item in edits {
        let path = path_of(item)?;
        let old = text_of(item, "old")?;
        let new = text_of(item, "new")?;
        let replace_all = match item.get("replace_all") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err(files::invalid("\"replace_all\" must be true or false")),
        };
        if old.is_empty() {
            return Err(files::invalid(format!(
                "{path:?}: \"old\" must not be empty"
            )));
        }
        if old.len() > MAX_NEEDLE_BYTES {
            return Err(files::limit(format!(
                "{path:?}: \"old\" is {} bytes, over the {MAX_NEEDLE_BYTES}-byte cap",
                old.len()
            )));
        }
        if new.len() as u64 > MAX_WRITE_BYTES {
            return Err(files::limit(format!(
                "{path:?}: \"new\" is {} bytes, over the {MAX_WRITE_BYTES}-byte cap",
                new.len()
            )));
        }
        if old == new {
            return Err(files::invalid(format!(
                "{path:?}: \"old\" and \"new\" are the same; the edit would change nothing"
            )));
        }
        ops.push((
            path,
            Op::Replace(Replacement {
                old,
                new,
                replace_all,
            }),
            false,
        ));
    }
    Ok(ops)
}

/// The array under `key`, or none when the key is absent.
fn array_of<'a>(input: &'a Value, key: &str) -> Result<&'a [Value], ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(files::invalid(format!("\"{key}\" must be an array"))),
    }
}

/// The `path` of one operation: a string, not empty.
fn path_of(item: &Value) -> Result<String, ToolError> {
    match item.get("path") {
        Some(Value::String(path)) if !path.is_empty() => Ok(path.clone()),
        Some(Value::String(_)) => Err(files::invalid("a \"path\" must not be empty")),
        _ => Err(files::invalid("each operation needs a string \"path\"")),
    }
}

/// A string field of one operation: present and a string, though it may be empty (the caller decides whether
/// an empty one is allowed).
fn text_of(item: &Value, key: &str) -> Result<String, ToolError> {
    match item.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(files::invalid(format!("\"{key}\" must be a string"))),
        None => Err(files::invalid(format!("\"{key}\" is required"))),
    }
}

/// Applies one file's operations and writes the file once, when they all apply.
fn apply_file(
    workspace: &Workspace,
    lane: &Lane,
    call: u64,
    path: &str,
    ops: &[Op],
) -> Result<Value, ToolError> {
    let (target, label) = workspace.resolve(path)?;
    files::refuse_symlink_target(&target, &label)?;
    let existed = match fs::metadata(&target) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(files::invalid(format!(
                "{label} is a directory; an edit writes one file"
            )));
        }
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(files::io_tool_error(err, format!("cannot open {label}"))),
    };

    let mut text: Option<String> = None;
    let mut replacements = 0;
    let mut lines_changed = 0;
    let mut created = false;
    for (index, op) in ops.iter().enumerate() {
        match op {
            Op::Create(content) => {
                lines_changed += count_lines(content);
                text = Some(content.clone());
                created = !existed;
            }
            Op::Replace(replacement) => {
                let current = match text.take() {
                    Some(current) => current,
                    None => files::read_text(&target, &label)?,
                };
                let (updated, count) = replace_exact(&current, &label, replacement)
                    .map_err(|err| refused_at(index, err))?;
                lines_changed += count
                    * replacement
                        .old
                        .lines()
                        .count()
                        .max(replacement.new.lines().count());
                replacements += count;
                text = Some(updated);
            }
        }
    }
    let content = text.unwrap_or_default();

    if let Some(parent) = target.parent() {
        files::create_dirs(parent).map_err(|err| {
            files::io_tool_error(err, format!("cannot create the folders of {label}"))
        })?;
    }
    files::write_atomic(&target, content.as_bytes())
        .map_err(|err| files::io_tool_error(err, format!("cannot write {label}")))?;
    locked(&lane.tracking).record(
        &label,
        Seen {
            digest: Some(track::digest_of(content.as_bytes())),
            stamp: track::stamp_of(&target).unwrap_or(Stamp::UNKNOWN),
            window: (1, content.lines().count()),
            call,
        },
    );

    let mut entry = json!({ "path": label, "ok": true, "lines_changed": lines_changed });
    if ops.iter().any(|op| matches!(op, Op::Create(_))) {
        entry["created"] = json!(created);
    }
    if replacements > 0 {
        entry["replacements"] = json!(replacements);
    }
    if let Some(check) = after_write(workspace, &label) {
        entry["check"] = check;
    }
    Ok(entry)
}

/// The hook after a file is written: formatting (1.7) and diagnostics (1.8) report their results here, under
/// `check` in the file's entry. Until they land a write has none.
fn after_write(_workspace: &Workspace, _label: &str) -> Option<Value> {
    None
}

/// Replaces `replacement.old` in `content`: it must occur, and once unless `replace_all`. Returns the updated
/// text and how many occurrences were replaced.
fn replace_exact(
    content: &str,
    label: &str,
    replacement: &Replacement,
) -> Result<(String, usize), ToolError> {
    let found = content.matches(&replacement.old).count();
    if found == 0 {
        return Err(files::not_found(missing_message(
            content,
            label,
            &replacement.old,
        )));
    }
    if found > 1 && !replacement.replace_all {
        return Err(files::invalid(format!(
            "the text to replace appears {found} times in {label}; add surrounding context to make it unique, or set \"replace_all\" to replace all {found}"
        )));
    }
    let updated = if replacement.replace_all {
        content.replace(&replacement.old, &replacement.new)
    } else {
        content.replacen(&replacement.old, &replacement.new, 1)
    };
    if updated.len() as u64 > MAX_FILE_BYTES {
        return Err(files::limit(format!(
            "the edit would make {label} {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            updated.len()
        )));
    }
    let count = if replacement.replace_all { found } else { 1 };
    Ok((updated, count))
}

/// The refusal for an `old` that does not occur, naming the closest line when one contains the first line of `old`.
fn missing_message(content: &str, label: &str, old: &str) -> String {
    let head = old.lines().next().unwrap_or("").trim();
    let closest = if head.is_empty() {
        None
    } else {
        content
            .lines()
            .enumerate()
            .find(|(_, line)| line.trim().contains(head))
    };
    match closest {
        Some((index, line)) => format!(
            "the text to replace does not appear in {label}; closest line is {}: {:?}",
            index + 1,
            line.trim()
        ),
        None => format!("the text to replace does not appear in {label}"),
    }
}

/// A refusal of one operation, named by its place in the file's list: the file is then left as it was.
fn refused_at(index: usize, err: ToolError) -> ToolError {
    let reason = format!("edit {index} refused, nothing written: {}", reason_of(&err));
    match err {
        ToolError::NotFound { .. } => ToolError::NotFound { reason },
        ToolError::LimitExceeded { .. } => ToolError::LimitExceeded { reason },
        _ => ToolError::InvalidInput { reason },
    }
}

/// The reason inside a refusal, without its kind.
fn reason_of(err: &ToolError) -> String {
    match err {
        ToolError::InvalidInput { reason }
        | ToolError::Unavailable { reason }
        | ToolError::Denied { reason }
        | ToolError::NotFound { reason }
        | ToolError::LimitExceeded { reason }
        | ToolError::Conflict { reason }
        | ToolError::Internal { reason } => reason.clone(),
        ToolError::UnknownTool(name) => name.clone(),
    }
}

/// How many lines `text` has, a last line without its newline included.
fn count_lines(text: &str) -> usize {
    text.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_run_before_edits_and_a_file_keeps_its_first_mention() {
        let ops = vec![
            (
                "a.rs".to_string(),
                Op::Replace(Replacement {
                    old: "x".into(),
                    new: "y".into(),
                    replace_all: false,
                }),
                false,
            ),
            ("b.rs".to_string(), Op::Create("new\n".into()), true),
            ("a.rs".to_string(), Op::Create("whole\n".into()), true),
        ];
        let grouped = group(ops);
        assert_eq!(grouped[0].0, "a.rs");
        assert_eq!(grouped[0].1[0], Op::Create("whole\n".into()));
        assert_eq!(grouped[1].0, "b.rs");
    }

    #[test]
    fn an_ambiguous_or_missing_old_text_is_refused_with_what_would_fix_it() {
        let replacement = |old: &str| Replacement {
            old: old.into(),
            new: "z".into(),
            replace_all: false,
        };
        let twice = replace_exact("a\na\n", "f.txt", &replacement("a")).expect_err("two matches");
        assert!(twice.to_string().contains("appears 2 times"), "{twice}");
        let missing = replace_exact(
            "fn main() {\n}\n",
            "f.txt",
            &replacement("fn main() {\n  body"),
        )
        .expect_err("no match");
        assert!(
            missing.to_string().contains("closest line is 1"),
            "{missing}"
        );
        let (updated, count) = replace_exact(
            "a\na\n",
            "f.txt",
            &Replacement {
                old: "a".into(),
                new: "b".into(),
                replace_all: true,
            },
        )
        .expect("every occurrence");
        assert_eq!((updated.as_str(), count), ("b\nb\n", 2));
    }

    #[test]
    fn a_refused_operation_names_its_place_and_keeps_its_kind() {
        let err = refused_at(
            1,
            ToolError::NotFound {
                reason: "gone".into(),
            },
        );
        assert_eq!(
            err.to_string(),
            "not found: edit 1 refused, nothing written: gone"
        );
    }

    #[test]
    fn an_empty_old_or_an_empty_call_is_refused_before_any_file_is_touched() {
        let empty_old = parse(&json!({"edits": [{"path": "a", "old": "", "new": "x"}]}))
            .expect_err("an empty old text");
        assert!(
            empty_old.to_string().contains("must not be empty"),
            "{empty_old}"
        );
        let nothing = parse(&json!({})).expect_err("no operations");
        assert!(
            nothing.to_string().starts_with("invalid input: give"),
            "{nothing}"
        );
    }
}
