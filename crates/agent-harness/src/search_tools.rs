//! Code search: `glob` (files by pattern, newest first) and `grep` (contents by regex,
//! ripgrep-like), both confined to the workspace.
//!
//! Both walk the workspace with plain `std::fs` calls, as the file tools do. A
//! symlink is never followed, so a link cannot walk a search out of the volume;
//! the harness's `.plaza` state directory, `.git`, `node_modules` and `target` are
//! never entered, at any depth. Every path in a result is relative to the
//! workspace root, and every result is capped at [`MAX_OUTPUT_BYTES`].

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use globset::{GlobBuilder, GlobMatcher};
use regex::RegexBuilder;
use serde_json::Value;

use crate::files::{PLAZA_DIR, WORKSPACE_ROOT, Workspace};

/// The most files `glob` lists.
const GLOB_MAX_FILES: usize = 500;

/// The most entries `grep` returns in `content` mode when `head_limit` is absent.
const CONTENT_DEFAULT_HEAD: usize = 250;

/// The most entries `grep` returns in the other modes when `head_limit` is absent.
const DEFAULT_HEAD: usize = 500;

/// The most bytes one result may occupy: 64 KiB.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Room kept free at the end of a capped result for its footer notes.
const FOOTER_RESERVE: usize = 256;

/// The largest file `grep` searches. A bigger one is passed over.
const GREP_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// How much of a file is checked for a NUL byte before it is taken as text.
const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// The most context lines `grep` shows either side of a match.
const MAX_CONTEXT_LINES: usize = 50;

/// How much of one matching line `grep` prints, in bytes.
const MAX_LINE_BYTES: usize = 1000;

/// Names never entered by either tool, at any depth.
const SKIPPED_NAMES: [&str; 4] = [PLAZA_DIR, ".git", "node_modules", "target"];

/// One file a search may look at.
struct Candidate {
    /// The file on disk, inside the workspace.
    path: PathBuf,
    /// The path relative to the workspace root, `/`-separated: how a result names it.
    rel: String,
    /// The path relative to the directory the search started from: what `glob`
    /// matches, and what a `grep` `glob` filter is tried against.
    rel_start: String,
    /// When the file was last modified.
    modified: SystemTime,
    /// The file's size in bytes.
    size: u64,
}

/// Files under the workspace matching a glob pattern, newest first.
///
/// Arguments: `pattern` (required; `*` stays within one directory, `**` crosses
/// directories, as globset reads it), `path` (optional, relative to the workspace;
/// the search starts there and the pattern is matched relative to it).
///
/// Output: one workspace-relative path per line, newest-modified first (ties by
/// path), at most 500; past that, a `[… N more]` line. No match is `no files match`.
pub fn glob(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let pattern = required_str(input, "pattern")?;
    let matcher = path_matcher(&pattern)?;
    let (root, start) = search_root(workspace, input)?;
    let mut found: Vec<Candidate> = candidates(&root, &start)?
        .into_iter()
        .filter(|file| matcher.is_match(&file.rel_start))
        .collect();
    sort_newest_first(&mut found);
    let total = found.len();
    if total == 0 {
        return Ok("no files match".to_string());
    }
    let shown: Vec<String> = found
        .iter()
        .take(GLOB_MAX_FILES)
        .map(|file| file.rel.clone())
        .collect();
    let footer = (total > GLOB_MAX_FILES).then(|| format!("[… {} more]", total - GLOB_MAX_FILES));
    Ok(join_capped(&shown, footer))
}

/// Lines of workspace text files matching a regex, ripgrep-like.
///
/// Arguments: `pattern` (required; a regex), `path` (optional, relative to the
/// workspace), `glob` (optional; a pattern without `/` matches the file's name at
/// any depth, one with `/` matches its path relative to `path`), `output_mode`
/// (`files_with_matches` (default: paths, newest first), `content`
/// (`path:line:text` for matches, `path-line-text` for context, `--` between
/// groups that do not touch; path order), or `count` (`path:N`, path order)),
/// `context` (lines either side, content mode, at most 50), `case_insensitive`
/// (default false), and `head_limit` (entries returned; default 250 in content
/// mode, 500 otherwise; 0 for no entry cap; the 64 KiB output cap always applies).
///
/// A file over 4 MiB, a file with a NUL byte in its first 8 KiB, and a file that
/// is not UTF-8 are passed over. An invalid regex is refused with the regex error.
/// A result cut short says so in its last line. No match is `no matches`.
pub fn grep(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let pattern = required_str(input, "pattern")?;
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(bool_arg(input, "case_insensitive")?)
        .build()
        .map_err(|err| format!("invalid regex {pattern:?}: {err}"))?;
    let mode =
        optional_str(input, "output_mode")?.unwrap_or_else(|| "files_with_matches".to_string());
    match mode.as_str() {
        "files_with_matches" | "content" | "count" => {}
        other => {
            return Err(format!(
                "unknown output_mode {other:?}; use files_with_matches, content or count"
            ));
        }
    }
    let context = count_arg(input, "context")?.unwrap_or(0);
    if context > MAX_CONTEXT_LINES {
        return Err(format!(
            "\"context\" is {context} lines, over the {MAX_CONTEXT_LINES}-line cap"
        ));
    }
    let head = count_arg(input, "head_limit")?.unwrap_or(if mode == "content" {
        CONTENT_DEFAULT_HEAD
    } else {
        DEFAULT_HEAD
    });
    let limit = if head == 0 { usize::MAX } else { head };
    let wanted = match optional_str(input, "glob")? {
        Some(pattern) => Some(file_filter(&pattern)?),
        None => None,
    };
    let (root, start) = search_root(workspace, input)?;
    let mut files: Vec<Candidate> = candidates(&root, &start)?
        .into_iter()
        .filter(|file| wanted.as_ref().is_none_or(|keep| keep(file)))
        .collect();
    if mode == "files_with_matches" {
        sort_newest_first(&mut files);
    } else {
        files.sort_by(|left, right| left.rel.cmp(&right.rel));
    }

    let mut entries: Vec<String> = Vec::new();
    for file in &files {
        // One entry past the limit is enough to know the result was cut short.
        if entries.len() > limit {
            break;
        }
        if file.size > GREP_MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&file.path) else {
            continue;
        };
        if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let hits: Vec<usize> = (0..lines.len())
            .filter(|&index| regex.is_match(lines[index]))
            .collect();
        if hits.is_empty() {
            continue;
        }
        match mode.as_str() {
            "count" => entries.push(format!("{}:{}", file.rel, hits.len())),
            "content" => emit_content(&mut entries, &file.rel, &lines, &hits, context),
            _ => entries.push(file.rel.clone()),
        }
    }

    if entries.is_empty() {
        return Ok("no matches".to_string());
    }
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    let footer = truncated.then(|| {
        format!("[… truncated at {limit} entries; raise head_limit or narrow the search]")
    });
    Ok(join_capped(&entries, footer))
}

/// The canonical workspace root, and the canonical directory or file a search
/// starts from. Refuses an absolute path, a `..`, the `.plaza` state directory,
/// and anything that resolves outside the workspace.
fn search_root(workspace: &Workspace, input: &Value) -> Result<(PathBuf, PathBuf), String> {
    let root = fs::canonicalize(workspace.dir()).map_err(|err| {
        format!(
            "the workspace {} is not mounted: {err}",
            workspace.dir().display()
        )
    })?;
    let given = optional_str(input, "path")?.unwrap_or_default();
    // Models name the workspace by its mount point, as the file tools accept it.
    let relative = match given.strip_prefix(WORKSPACE_ROOT) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.trim_start_matches('/'),
        _ => given.as_str(),
    };
    let mut start = root.clone();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) if part == PLAZA_DIR => {
                return Err(format!(
                    "path {given:?} names the harness's own state, which is not searched"
                ));
            }
            Component::Normal(part) => start.push(part),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "path {given:?} is absolute; every path is inside the workspace"
                ));
            }
            Component::ParentDir => {
                return Err(format!(
                    "path {given:?} contains \"..\"; every path is inside the workspace"
                ));
            }
        }
    }
    let start =
        fs::canonicalize(&start).map_err(|err| format!("cannot search {given:?}: {err}"))?;
    if !start.starts_with(&root) {
        return Err(format!("path {given:?} resolves outside the workspace"));
    }
    if start
        .strip_prefix(&root)
        .is_ok_and(|rel| rel.components().any(|part| part.as_os_str() == PLAZA_DIR))
    {
        return Err(format!(
            "path {given:?} resolves into the harness's own state, which is not searched"
        ));
    }
    Ok((root, start))
}

/// Every regular file under `start`, found without following a symlink and
/// without entering a skipped directory. A file `start` names is the one entry.
fn candidates(root: &Path, start: &Path) -> Result<Vec<Candidate>, String> {
    if start.is_file() {
        let meta = fs::metadata(start).map_err(|err| format!("cannot read the file: {err}"))?;
        return Ok(candidate(start.to_path_buf(), root, start, &meta)
            .into_iter()
            .collect());
    }
    let mut found = Vec::new();
    let mut pending = vec![start.to_path_buf()];
    while let Some(dir) = pending.pop() {
        // A directory that cannot be read is passed over; the search goes on.
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if is_skipped(&name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // `file_type` does not follow links, so a symlink is named here and
            // never entered or read.
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && let Ok(meta) = entry.metadata()
                && let Some(one) = candidate(path, root, start, &meta)
            {
                found.push(one);
            }
        }
    }
    Ok(found)
}

/// The search record for one file, or `None` if its path is not under the root.
fn candidate(path: PathBuf, root: &Path, start: &Path, meta: &fs::Metadata) -> Option<Candidate> {
    let rel = joined(path.strip_prefix(root).ok()?);
    let rel_start = match path.strip_prefix(start) {
        Ok(rest) if !rest.as_os_str().is_empty() => joined(rest),
        _ => path.file_name()?.to_string_lossy().into_owned(),
    };
    Some(Candidate {
        path,
        rel,
        rel_start,
        modified: meta.modified().unwrap_or(UNIX_EPOCH),
        size: meta.len(),
    })
}

/// A path's components joined with `/`, whatever the platform's separator.
fn joined(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether a directory entry is one the tools never enter.
fn is_skipped(name: &OsStr) -> bool {
    SKIPPED_NAMES.iter().any(|skip| name == OsStr::new(skip))
}

/// Newest first, ties broken by path, so the order is a property of the tree.
fn sort_newest_first(found: &mut [Candidate]) {
    found.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.rel.cmp(&right.rel))
    });
}

/// A glob matcher where `*` does not cross a `/`.
fn path_matcher(pattern: &str) -> Result<GlobMatcher, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|err| format!("invalid glob pattern {pattern:?}: {err}"))
}

/// The `grep` `glob` filter. A pattern with no `/` names a file at any depth, as
/// ripgrep's `--glob` does; one with a `/` is matched against the path relative
/// to the search's start.
fn file_filter(pattern: &str) -> Result<impl Fn(&Candidate) -> bool + use<>, String> {
    let matcher = path_matcher(pattern)?;
    let by_name = !pattern.contains('/');
    Ok(move |file: &Candidate| {
        if by_name {
            Path::new(&file.rel_start)
                .file_name()
                .is_some_and(|name| matcher.is_match(name))
        } else {
            matcher.is_match(&file.rel_start)
        }
    })
}

/// Appends the content-mode lines of one file: each match as `path:line:text`,
/// each context line as `path-line-text`, and a `--` between groups that do not
/// touch. Context windows that overlap or abut are merged into one group.
fn emit_content(
    entries: &mut Vec<String>,
    rel: &str,
    lines: &[&str],
    hits: &[usize],
    context: usize,
) {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &hit in hits {
        let first = hit.saturating_sub(context);
        let last = hit.saturating_add(context).min(lines.len() - 1);
        match ranges.last_mut() {
            Some(range) if first <= range.1 + 1 => range.1 = range.1.max(last),
            _ => ranges.push((first, last)),
        }
    }
    for (index, &(first, last)) in ranges.iter().enumerate() {
        if index > 0 {
            entries.push("--".to_string());
        }
        for (line, text) in lines.iter().enumerate().take(last + 1).skip(first) {
            let text = cut_text(text);
            let number = line + 1;
            if hits.binary_search(&line).is_ok() {
                entries.push(format!("{rel}:{number}:{text}"));
            } else {
                entries.push(format!("{rel}-{number}-{text}"));
            }
        }
    }
}

/// `line`, cut to [`MAX_LINE_BYTES`] on a character boundary, with a marker when
/// it was cut.
fn cut_text(line: &str) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return line.to_string();
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [… line cut]", &line[..end])
}

/// The lines joined by newlines, then `footer`, kept within [`MAX_OUTPUT_BYTES`].
/// The first line that would overflow ends the lines, and a note says how many
/// were left out.
fn join_capped(lines: &[String], footer: Option<String>) -> String {
    let budget = MAX_OUTPUT_BYTES - FOOTER_RESERVE;
    let mut out = String::new();
    let mut written = 0;
    for line in lines {
        if out.len() + line.len() + 1 > budget {
            break;
        }
        out.push_str(line);
        out.push('\n');
        written += 1;
    }
    if written < lines.len() {
        out.push_str(&format!(
            "[… {} more; output capped at 64 KiB]\n",
            lines.len() - written
        ));
    }
    if let Some(note) = footer {
        out.push_str(&note);
        out.push('\n');
    }
    out.trim_end_matches('\n').to_string()
}

/// A required, non-empty string argument.
fn required_str(input: &Value, key: &str) -> Result<String, String> {
    match input.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(Value::String(_)) => Err(format!("{key:?} must not be empty")),
        Some(_) => Err(format!("{key:?} must be a string")),
        None => Err(format!("{key:?} is required")),
    }
}

/// An optional string argument; absent or null is `None`.
fn optional_str(input: &Value, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{key:?} must be a string")),
    }
}

/// An optional whole-number argument; absent or null is `None`.
fn count_arg(input: &Value, key: &str) -> Result<Option<usize>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|n| Some(usize::try_from(n).unwrap_or(usize::MAX)))
            .ok_or_else(|| format!("{key:?} must be a whole number")),
    }
}

/// An optional boolean argument; absent or null is `false`.
fn bool_arg(input: &Value, key: &str) -> Result<bool, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(format!("{key:?} must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// A fresh scratch workspace, unique to one test.
    fn scratch(name: &str) -> Workspace {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("apz-search-{}-{name}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    /// Writes `bytes` to `rel` under the workspace, with its modification time
    /// `mtime` seconds after the epoch, so ordering does not depend on the clock.
    fn file(ws: &Workspace, rel: &str, bytes: &[u8], mtime: u64) {
        let path = ws.dir().join(rel);
        fs::create_dir_all(path.parent().expect("a parent")).expect("the parent directory");
        fs::write(&path, bytes).expect("the scratch file");
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("the scratch file, reopened")
            .set_modified(UNIX_EPOCH + Duration::from_secs(mtime))
            .expect("the modification time");
    }

    #[test]
    fn glob_lists_matches_newest_first_and_says_when_none_match() {
        // The original glob_lists_recursive_matches_newest_first: recursive matches
        // list newest first, and a single `*` does not cross a directory separator.
        {
            let ws = scratch("glob-order");
            file(&ws, "a.rs", b"", 1_000);
            file(&ws, "src/b.rs", b"", 3_000);
            file(&ws, "src/deep/c.rs", b"", 2_000);
            file(&ws, "readme.md", b"", 9_000);
            assert_eq!(
                glob(&ws, &json!({ "pattern": "**/*.rs" })).unwrap(),
                "src/b.rs\nsrc/deep/c.rs\na.rs"
            );
            // A single `*` does not cross a directory separator.
            assert_eq!(glob(&ws, &json!({ "pattern": "*.rs" })).unwrap(), "a.rs");
        }
        // The original glob_matches_relative_to_its_path: matches are relative to `path`.
        {
            let ws = scratch("glob-path");
            file(&ws, "a.rs", b"", 1_000);
            file(&ws, "src/b.rs", b"", 3_000);
            file(&ws, "src/deep/c.rs", b"", 2_000);
            assert_eq!(
                glob(&ws, &json!({ "pattern": "**/*.rs", "path": "src" })).unwrap(),
                "src/b.rs\nsrc/deep/c.rs"
            );
        }
        // The original glob_with_no_match_says_so: no match reads "no files match".
        {
            let ws = scratch("glob-none");
            file(&ws, "a.txt", b"", 1_000);
            assert_eq!(
                glob(&ws, &json!({ "pattern": "**/*.rs" })).unwrap(),
                "no files match"
            );
        }
    }

    #[test]
    fn glob_never_enters_the_state_or_build_directories() {
        let ws = scratch("glob-skip");
        file(&ws, ".plaza/state.rs", b"", 9_000);
        file(&ws, ".git/hooks.rs", b"", 9_000);
        file(&ws, "node_modules/pkg/index.rs", b"", 9_000);
        file(&ws, "target/debug/build.rs", b"", 9_000);
        file(&ws, "ok.rs", b"", 1_000);
        assert_eq!(
            glob(&ws, &json!({ "pattern": "**/*.rs" })).unwrap(),
            "ok.rs"
        );
        let err = glob(&ws, &json!({ "pattern": "*", "path": ".plaza" })).unwrap_err();
        assert!(err.contains("harness's own state"), "{err}");
    }

    #[test]
    fn glob_caps_the_list_and_says_how_many_are_left() {
        let ws = scratch("glob-cap");
        for index in 0..502u64 {
            file(&ws, &format!("f{index:03}.rs"), b"", 1_000 + index);
        }
        let out = glob(&ws, &json!({ "pattern": "*.rs" })).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 501);
        assert_eq!(lines[500], "[… 2 more]");
    }

    #[test]
    fn glob_and_grep_refuse_paths_outside_the_workspace() {
        let ws = scratch("refuse");
        let err = glob(&ws, &json!({ "pattern": "*", "path": "../x" })).unwrap_err();
        assert!(err.contains(".."), "{err}");
        let err = glob(&ws, &json!({ "pattern": "*", "path": "/etc" })).unwrap_err();
        assert!(err.contains("absolute"), "{err}");
        let err = grep(&ws, &json!({ "pattern": "x", "path": "../" })).unwrap_err();
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn grep_reports_matches_in_each_output_mode_with_its_filters() {
        // The original grep_files_with_matches_lists_newest_first: files list newest first.
        {
            let ws = scratch("grep-files");
            file(&ws, "a.txt", b"needle\n", 1_000);
            file(&ws, "sub/b.txt", b"x needle\n", 5_000);
            file(&ws, "c.txt", b"nothing\n", 9_000);
            assert_eq!(
                grep(&ws, &json!({ "pattern": "needle" })).unwrap(),
                "sub/b.txt\na.txt"
            );
        }
        // The original grep_content_prints_path_line_and_text_and_marks_context_lines:
        // content prints path, line and text, and marks context lines.
        {
            let ws = scratch("grep-content");
            file(&ws, "f.txt", b"alpha\nneedle here\nbeta\n", 1_000);
            assert_eq!(
                grep(
                    &ws,
                    &json!({ "pattern": "needle", "output_mode": "content" })
                )
                .unwrap(),
                "f.txt:2:needle here"
            );
            file(&ws, "f.txt", b"needle\na\nb\nc\nneedle\n", 1_000);
            let out = grep(
                &ws,
                &json!({ "pattern": "needle", "output_mode": "content", "context": 1 }),
            )
            .unwrap();
            assert_eq!(
                out,
                "f.txt:1:needle\nf.txt-2-a\n--\nf.txt-4-c\nf.txt:5:needle"
            );
        }
        // The original grep_count_reports_matching_lines_per_file: count gives lines per file.
        {
            let ws = scratch("grep-count");
            file(&ws, "f.txt", b"needle\nneedle\nx\n", 1_000);
            file(&ws, "g.txt", b"needle\n", 2_000);
            assert_eq!(
                grep(&ws, &json!({ "pattern": "needle", "output_mode": "count" })).unwrap(),
                "f.txt:2\ng.txt:1"
            );
        }
        // The original grep_case_insensitive_matches_any_case: the flag matches any case.
        {
            let ws = scratch("grep-case");
            file(&ws, "f.txt", b"NEEDLE\n", 1_000);
            assert_eq!(
                grep(
                    &ws,
                    &json!({ "pattern": "needle", "case_insensitive": true })
                )
                .unwrap(),
                "f.txt"
            );
            assert_eq!(
                grep(&ws, &json!({ "pattern": "needle" })).unwrap(),
                "no matches"
            );
        }
        // The original grep_glob_filters_the_files_searched: `glob` limits the files searched.
        {
            let ws = scratch("grep-glob");
            file(&ws, "a.rs", b"needle\n", 2_000);
            file(&ws, "a.txt", b"needle\n", 3_000);
            file(&ws, "sub/b.rs", b"needle\n", 1_000);
            assert_eq!(
                grep(&ws, &json!({ "pattern": "needle", "glob": "*.rs" })).unwrap(),
                "a.rs\nsub/b.rs"
            );
        }
        // The original grep_refuses_an_invalid_regex_with_the_regex_error.
        {
            let ws = scratch("grep-regex");
            file(&ws, "f.txt", b"needle\n", 1_000);
            let err = grep(&ws, &json!({ "pattern": "(" })).unwrap_err();
            assert!(err.contains("invalid regex"), "{err}");
        }
        // The original grep_head_limit_cuts_the_list_and_says_so: the cut is announced.
        {
            let ws = scratch("grep-head");
            file(&ws, "a.txt", b"needle\n", 1_000);
            file(&ws, "b.txt", b"needle\n", 2_000);
            file(&ws, "c.txt", b"needle\n", 3_000);
            let out = grep(&ws, &json!({ "pattern": "needle", "head_limit": 2 })).unwrap();
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines.len(), 3, "{out}");
            assert_eq!(&lines[..2], ["c.txt", "b.txt"]);
            assert!(lines[2].contains("truncated"), "{out}");
        }
    }

    #[test]
    fn grep_skips_plaza_and_binary_files() {
        let ws = scratch("grep-skip");
        file(&ws, ".plaza/state.txt", b"needle\n", 9_000);
        file(&ws, "bin.dat", b"needle\0\x01", 9_000);
        file(&ws, "ok.txt", b"needle\n", 1_000);
        assert_eq!(
            grep(&ws, &json!({ "pattern": "needle" })).unwrap(),
            "ok.txt"
        );
        let err = grep(&ws, &json!({ "pattern": "needle", "path": ".plaza" })).unwrap_err();
        assert!(err.contains("harness's own state"), "{err}");
    }
}
