//! `find`: files by glob, and the files whose contents match a regex, from a file index that stays warm.
//!
//! Arguments: `glob` (optional; a pattern with no `/` matches a file's name at any depth, one with a `/` matches
//! its path under `path`), `content` (optional; a regex the lines of a file must match), `path` (optional; the
//! directory to look under, default the workspace), `max` (optional, default [`DEFAULT_MAX`]), and `after`
//! (optional; resumes a cut result, see [`parse_after`]). At least one of `glob` and `content` is needed.
//!
//! The index lists every file the workspace holds, honouring `.gitignore`, and skips `.plaza`, `.git`,
//! `node_modules` and `target`. It is built on first use, and is kept while the directories it walked keep their
//! modification times; a directory that changed rebuilds it. A glob answers from the index at once.
//!
//! Output: `{"files": [...]}` for a glob (the paths, sorted), and with `content` also `{"matches": [{"path",
//! "line", "text"}]}`, with `files` the files those matches are in. A cut result (the budget [`FIND_BUDGET`],
//! or `max`) says `continue` with the arguments that resume it, `after` set to the last path, or `path:line`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use globset::{GlobBuilder, GlobMatcher};
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use ignore::WalkBuilder;
use serde_json::{Map, Value, json};

use crate::files::{self, Workspace};
use crate::tools::ToolError;

use super::{Lane, continue_hint, fit, locked};

/// The entries a `find` call returns when it is not told how many: 200.
pub const DEFAULT_MAX: usize = 200;

/// The most entries one `find` call returns.
pub const MAX_MAX: usize = 2000;

/// The most bytes of result one `find` call returns: 16 KiB.
pub const FIND_BUDGET: usize = 16 * 1024;

/// The most bytes of one matching line `find` returns.
pub const MAX_LINE_BYTES: usize = 300;

/// The most bytes of file content one `find` call reads in total: 64 MiB.
const SCAN_BYTES: u64 = 64 * 1024 * 1024;

/// Directories and files the index never enters, at any depth.
const SKIPPED: [&str; 4] = [".plaza", ".git", "node_modules", "target"];

/// One file in the index.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The file's path relative to the workspace root, `/`-separated.
    pub(crate) rel: String,
    /// The file on disk.
    pub(crate) path: PathBuf,
}

/// The workspace's files, sorted by path, and the directories they were listed from with their times.
#[derive(Debug)]
pub(crate) struct Index {
    /// The canonical root the index was built under.
    root: PathBuf,
    /// Every file, sorted by path.
    pub(crate) files: Vec<Entry>,
    /// Every directory the walk listed, with its modification time when the walk saw it.
    dirs: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Index {
    /// Walks `root`, honouring `.gitignore`, and lists its files and directories.
    fn build(root: &Path) -> Index {
        let mut builder = WalkBuilder::new(root);
        builder
            .hidden(false)
            .require_git(false)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !SKIPPED.contains(&name))
            });
        let mut files = Vec::new();
        let mut dirs = vec![(root.to_path_buf(), mtime(root))];
        for result in builder.build() {
            let Ok(entry) = result else { continue };
            let path = entry.path();
            if path == root {
                continue;
            }
            match entry.file_type() {
                Some(kind) if kind.is_dir() => dirs.push((path.to_path_buf(), mtime(path))),
                Some(kind) if kind.is_file() => files.push(Entry {
                    rel: super::label_under(root, path),
                    path: path.to_path_buf(),
                }),
                _ => {}
            }
        }
        files.sort_by(|left, right| left.rel.cmp(&right.rel));
        Index {
            root: root.to_path_buf(),
            files,
            dirs,
        }
    }

    /// Whether every directory the walk listed still has the modification time it had: then nothing was added,
    /// removed or renamed inside them, and the index is current.
    fn is_current(&self) -> bool {
        self.dirs.iter().all(|(path, seen)| mtime(path) == *seen)
    }
}

/// A path's modification time, or None when it cannot be read.
fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// The index of `workspace`, built or rebuilt as its directories change, and shared with the caller.
pub(crate) fn index(workspace: &Workspace, lane: &Lane) -> Result<Arc<Index>, ToolError> {
    let root = workspace.canonical_root()?;
    let mut held = locked(&lane.index);
    if let Some(current) = held.as_ref()
        && current.root == root
        && current.is_current()
    {
        return Ok(Arc::clone(current));
    }
    let built = Arc::new(Index::build(&root));
    *held = Some(Arc::clone(&built));
    Ok(built)
}

/// Resolves the `path` argument to a directory of the workspace: its label, relative to the root (`.` for the
/// root itself).
pub(crate) fn scope(workspace: &Workspace, input: &Value) -> Result<String, ToolError> {
    let raw = optional_str(input, "path")?.unwrap_or_default();
    let (path, label) = workspace.resolve(&raw)?;
    let metadata = fs::metadata(&path)
        .map_err(|err| files::io_tool_error(err, format!("cannot search {label}")))?;
    if !metadata.is_dir() {
        return Err(files::invalid(format!("{label} is not a directory")));
    }
    Ok(label)
}

/// Whether `rel` is inside the scope's directory.
pub(crate) fn in_scope(scope: &str, rel: &str) -> bool {
    scope == "." || rel == scope || rel.starts_with(&format!("{scope}/"))
}

/// The glob matcher for `pattern`, and whether the pattern names a path (so it matches under the scope, not
/// only the file's name).
pub(crate) fn glob_matcher(pattern: &str) -> Result<(GlobMatcher, bool), ToolError> {
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|err| files::invalid(format!("{pattern:?} is not a glob: {err}")))?
        .compile_matcher();
    Ok((matcher, pattern.contains('/')))
}

/// Whether `rel` matches the glob, given the scope it is searched under.
pub(crate) fn glob_hit(matcher: &(GlobMatcher, bool), scope: &str, rel: &str) -> bool {
    let subject = if matcher.1 {
        if scope == "." {
            rel
        } else {
            match rel.strip_prefix(&format!("{scope}/")) {
                Some(under) => under,
                None => return false,
            }
        }
    } else {
        rel.rsplit('/').next().unwrap_or(rel)
    };
    matcher.0.is_match(subject)
}

/// A resume point: the last path a cut result returned, and the line of the last match in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct After {
    /// The path resumed after.
    pub(crate) path: String,
    /// The last line returned in that file, when the result was of matches.
    pub(crate) line: Option<u64>,
}

/// Parses an `after` argument: `path`, or `path:line` when the text after the last colon is a line number.
pub(crate) fn parse_after(text: &str) -> After {
    match text.rsplit_once(':') {
        Some((path, line)) if !path.is_empty() && line.parse::<u64>().is_ok() => After {
            path: path.to_string(),
            line: line.parse().ok(),
        },
        _ => After {
            path: text.to_string(),
            line: None,
        },
    }
}

/// An optional string argument.
pub(crate) fn optional_str(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(files::invalid(format!("\"{key}\" must be a string"))),
    }
}

/// An optional count argument of at least 1, `default` when absent and at most `cap`.
pub(crate) fn count_arg(
    input: &Value,
    key: &str,
    default: usize,
    cap: usize,
) -> Result<usize, ToolError> {
    match span_arg(input, key, default, cap)? {
        0 => Err(files::invalid(format!("\"{key}\" must be at least 1"))),
        count => Ok(count),
    }
}

/// An optional count argument, `default` when absent and at most `cap`; zero is a count too.
pub(crate) fn span_arg(
    input: &Value,
    key: &str,
    default: usize,
    cap: usize,
) -> Result<usize, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => match value.as_u64() {
            Some(count) if count as usize <= cap => Ok(count as usize),
            Some(count) => Err(files::limit(format!(
                "\"{key}\" is {count}, over the {cap} cap"
            ))),
            None => Err(files::invalid(format!("\"{key}\" must be a whole number"))),
        },
    }
}

/// The arguments a cut result resumes with: the call's own, plus `after`.
pub(crate) fn resume_args(tool: &str, input: &Value, after: &str) -> String {
    let mut args = Map::new();
    for key in ["glob", "content", "pattern", "path", "context", "max"] {
        if let Some(value) = input.get(key)
            && !value.is_null()
        {
            args.insert(key.to_string(), value.clone());
        }
    }
    args.insert("after".to_string(), json!(after));
    continue_hint(tool, &Value::Object(args))
}

/// The text of a matching line, cut to [`MAX_LINE_BYTES`] on a character boundary.
pub(crate) fn cut_text(line: &str) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return line.to_string();
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

/// Runs a `find` call.
pub(crate) fn find(workspace: &Workspace, lane: &Lane, input: &Value) -> Result<Value, ToolError> {
    let glob = optional_str(input, "glob")?;
    let content = optional_str(input, "content")?;
    if glob.is_none() && content.is_none() {
        return Err(files::invalid(
            "give a \"glob\" to find paths, a \"content\" regex, or both",
        ));
    }
    let max = count_arg(input, "max", DEFAULT_MAX, MAX_MAX)?;
    let after = optional_str(input, "after")?.map(|text| parse_after(&text));
    let scope = scope(workspace, input)?;
    let index = index(workspace, lane)?;
    let matcher = glob.as_deref().map(glob_matcher).transpose()?;
    let candidates: Vec<&Entry> = index
        .files
        .iter()
        .filter(|entry| in_scope(&scope, &entry.rel))
        .filter(|entry| {
            matcher
                .as_ref()
                .is_none_or(|m| glob_hit(m, &scope, &entry.rel))
        })
        .filter(|entry| {
            after
                .as_ref()
                .is_none_or(|after| entry.rel.as_str() > after.path.as_str())
        })
        .collect();

    let Some(content) = content else {
        let names: Vec<Value> = candidates
            .iter()
            .take(max)
            .map(|entry| json!(entry.rel))
            .collect();
        let truncated = candidates.len() > max;
        let (names, cut) = fit(names, FIND_BUDGET);
        let mut reply = json!({ "files": names });
        if truncated || cut {
            let last = reply["files"]
                .as_array()
                .and_then(|names| names.last())
                .and_then(Value::as_str);
            let resume = last.unwrap_or_default().to_string();
            reply["continue"] = json!(resume_args("find", input, &resume));
            reply["truncated"] = json!(true);
        }
        return Ok(reply);
    };

    let regex = RegexMatcher::new(&content)
        .map_err(|err| files::invalid(format!("\"content\" is not a regex: {err}")))?;
    let mut searcher = line_searcher();
    let mut matches: Vec<Value> = Vec::new();
    let mut scanned = 0u64;
    let mut cut = false;
    for entry in candidates {
        if matches.len() >= max || scanned >= SCAN_BYTES {
            cut = true;
            break;
        }
        let Ok(lines) = matching_lines(&entry.path, &regex, &mut searcher) else {
            continue;
        };
        scanned += fs::metadata(&entry.path).map_or(0, |m| m.len());
        for (line, text) in lines {
            if let Some(after) = &after
                && after.path == entry.rel
                && after.line.is_some_and(|done| line <= done)
            {
                continue;
            }
            if matches.len() >= max {
                cut = true;
                break;
            }
            matches.push(json!({ "path": entry.rel, "line": line, "text": cut_text(&text) }));
        }
    }
    let (matches, budget_cut) = fit(matches, FIND_BUDGET);
    let mut files: Vec<String> = Vec::new();
    for found in &matches {
        let path = found["path"].as_str().unwrap_or_default();
        if files.last().map(String::as_str) != Some(path) {
            files.push(path.to_string());
        }
    }
    let mut reply = json!({ "files": files, "matches": matches });
    if cut || budget_cut {
        let last = reply["matches"]
            .as_array()
            .and_then(|matches| matches.last())
            .map(|found| {
                format!(
                    "{}:{}",
                    found["path"].as_str().unwrap_or_default(),
                    found["line"]
                )
            });
        if let Some(last) = last {
            reply["continue"] = json!(resume_args("find", input, &last));
        }
        reply["truncated"] = json!(true);
    }
    Ok(reply)
}

/// A searcher that numbers lines and stops at the first NUL byte, so a binary file is not searched as text.
pub(crate) fn line_searcher() -> Searcher {
    SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build()
}

/// The lines of `path` that match `regex`, as `(line number, text)`. A file that is not UTF-8 text is an error:
/// the caller skips it.
pub(crate) fn matching_lines(
    path: &Path,
    regex: &RegexMatcher,
    searcher: &mut Searcher,
) -> io::Result<Vec<(u64, String)>> {
    let mut found = Vec::new();
    searcher.search_path(
        regex,
        path,
        UTF8(|number, line| {
            found.push((number, line.trim_end_matches(['\r', '\n']).to_string()));
            Ok(true)
        }),
    )?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_after_names_a_path_or_a_path_and_line() {
        assert_eq!(
            parse_after("src/a.rs:42"),
            After {
                path: "src/a.rs".into(),
                line: Some(42)
            }
        );
        assert_eq!(
            parse_after("src/a.rs"),
            After {
                path: "src/a.rs".into(),
                line: None
            }
        );
        assert_eq!(parse_after("odd:name").path, "odd:name");
    }

    #[test]
    fn a_name_glob_matches_at_any_depth_and_a_path_glob_under_the_scope() {
        let name = glob_matcher("*.rs").expect("a glob");
        assert!(glob_hit(&name, ".", "src/deep/a.rs"));
        assert!(!glob_hit(&name, ".", "src/a.toml"));
        let path = glob_matcher("src/*.rs").expect("a glob");
        assert!(glob_hit(&path, ".", "src/a.rs"));
        assert!(!glob_hit(&path, ".", "src/deep/a.rs"));
        let scoped = glob_matcher("**/*.rs").expect("a glob");
        assert!(glob_hit(&scoped, "src", "src/a.rs"));
        assert!(glob_hit(&scoped, "src", "src/x/a.rs"));
        assert!(!glob_hit(&scoped, "src", "lib/a.rs"));
    }

    #[test]
    fn a_scope_holds_its_directory_and_nothing_next_to_it() {
        assert!(in_scope(".", "anything"));
        assert!(in_scope("src", "src/a.rs"));
        assert!(!in_scope("src", "srcs/a.rs"));
    }

    #[test]
    fn a_long_matching_line_is_cut_on_a_character_boundary() {
        let line = "é".repeat(200);
        let cut = cut_text(&line);
        assert!(cut.ends_with('…'));
        assert!(cut.len() <= MAX_LINE_BYTES + 3);
    }
}
