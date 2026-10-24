//! `search`: the lines of files that match a regex, each with the lines around it.
//!
//! Arguments: `pattern` (required; a regex), `path` (optional; the directory to search, default the workspace),
//! `glob` (optional; limits the files searched, as in [`super::find`]), `context` (optional; lines either side of
//! each match, default [`DEFAULT_CONTEXT`], at most [`MAX_CONTEXT`]), `max` (optional; matches, default
//! [`DEFAULT_MAX`]) and `after` (optional; resumes a cut result from the line after `path:line`).
//!
//! Output: `{"matches": [{"path", "line", "text", "before": [{"line", "text"}], "after": [...]}], "skipped": n}`,
//! in path order and then line order. `skipped` counts the files not searched: binary, not UTF-8 text, or over
//! [`MAX_FILE_BYTES`]. A cut result (the budget [`SEARCH_BUDGET`], or `max`) says `continue` with the arguments
//! that resume it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use serde_json::{Value, json};

use crate::files::{self, Workspace};
use crate::tools::ToolError;

use super::find::{
    self, DEFAULT_MAX, MAX_MAX, count_arg, cut_text, glob_hit, glob_matcher, in_scope,
    optional_str, parse_after, resume_args, span_arg,
};
use super::{Lane, fit};

/// The lines either side of a match a call shows when it is not told how many: 2.
pub const DEFAULT_CONTEXT: usize = 2;

/// The most lines either side of a match a call shows: 20.
pub const MAX_CONTEXT: usize = 20;

/// The most bytes of result one `search` call returns: 24 KiB.
pub const SEARCH_BUDGET: usize = 24 * 1024;

/// The largest file `search` reads. A bigger one is skipped and counted.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// The lines a match's text and its context were read from, by line number.
#[derive(Default)]
struct Lines {
    /// The matching lines, in order.
    matched: Vec<u64>,
    /// Every line the searcher reported, matching or context, with its text.
    text: BTreeMap<u64, String>,
}

impl Sink for Lines {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, found: &SinkMatch<'_>) -> Result<bool, io::Error> {
        if let Some(number) = found.line_number() {
            self.matched.push(number);
            self.text.insert(number, line_text(found.bytes()));
        }
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, io::Error> {
        if let Some(number) = context.line_number() {
            self.text
                .entry(number)
                .or_insert_with(|| line_text(context.bytes()));
        }
        Ok(true)
    }
}

/// A line's text without its line ending.
fn line_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

/// Runs a `search` call.
pub(crate) fn search(
    workspace: &Workspace,
    lane: &Lane,
    input: &Value,
) -> Result<Value, ToolError> {
    let pattern = match optional_str(input, "pattern")? {
        Some(pattern) if !pattern.is_empty() => pattern,
        Some(_) => return Err(files::invalid("\"pattern\" must not be empty")),
        None => return Err(files::invalid("\"pattern\" is required")),
    };
    let context = span_arg(input, "context", DEFAULT_CONTEXT, MAX_CONTEXT)?;
    let max = count_arg(input, "max", DEFAULT_MAX, MAX_MAX)?;
    let after = optional_str(input, "after")?.map(|text| parse_after(&text));
    let regex = RegexMatcher::new(&pattern)
        .map_err(|err| files::invalid(format!("\"pattern\" is not a regex: {err}")))?;
    let scope = find::scope(workspace, input)?;
    let index = find::index(workspace, lane)?;
    let glob = optional_str(input, "glob")?;
    let matcher = glob.as_deref().map(glob_matcher).transpose()?;

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .before_context(context)
        .after_context(context)
        .binary_detection(grep_searcher::BinaryDetection::quit(b'\x00'))
        .build();
    let mut matches: Vec<Value> = Vec::new();
    let mut skipped = 0usize;
    let mut cut = false;
    'files: for entry in index.files.iter() {
        if !in_scope(&scope, &entry.rel)
            || matcher
                .as_ref()
                .is_some_and(|m| !glob_hit(m, &scope, &entry.rel))
        {
            continue;
        }
        if after
            .as_ref()
            .is_some_and(|after| entry.rel.as_str() < after.path.as_str())
        {
            continue;
        }
        if !searchable(&entry.path) {
            skipped += 1;
            continue;
        }
        let Ok(found) = search_file(&entry.path, &regex, &mut searcher, context) else {
            skipped += 1;
            continue;
        };
        for (line, before, after_lines, text) in found {
            if let Some(after) = &after
                && after.path == entry.rel
                && after.line.is_some_and(|done| line <= done)
            {
                continue;
            }
            if matches.len() >= max {
                cut = true;
                break 'files;
            }
            matches.push(json!({
                "path": entry.rel,
                "line": line,
                "text": cut_text(&text),
                "before": before,
                "after": after_lines,
            }));
        }
    }
    let (matches, budget_cut) = fit(matches, SEARCH_BUDGET);
    let mut reply = json!({ "matches": matches, "skipped": skipped });
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
            reply["continue"] = json!(resume_args("search", input, &last));
        }
        reply["truncated"] = json!(true);
    }
    Ok(reply)
}

/// Whether a file is worth searching: a regular file no bigger than [`MAX_FILE_BYTES`].
fn searchable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_FILE_BYTES)
}

/// The matches of `regex` in `path`, each with the lines `context` either side: `(line, before, after, text)`.
fn search_file(
    path: &Path,
    regex: &RegexMatcher,
    searcher: &mut Searcher,
    context: usize,
) -> io::Result<Vec<(u64, Vec<Value>, Vec<Value>, String)>> {
    let mut lines = Lines::default();
    searcher.search_path(regex, path, &mut lines)?;
    let mut found = Vec::new();
    for &number in &lines.matched {
        let text = lines.text.get(&number).cloned().unwrap_or_default();
        let before = (number.saturating_sub(context as u64).max(1)..number)
            .filter_map(|line| {
                lines
                    .text
                    .get(&line)
                    .map(|text| json!({ "line": line, "text": text }))
            })
            .collect();
        let after = (number + 1..=number + context as u64)
            .filter_map(|line| {
                lines
                    .text
                    .get(&line)
                    .map(|text| json!({ "line": line, "text": text }))
            })
            .collect();
        found.push((number, before, after, text));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::lane_of;

    /// A scratch workspace with the given files.
    fn workspace(name: &str, files: &[(&str, &str)]) -> Workspace {
        let root =
            std::env::temp_dir().join(format!("apz-v2-search-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, text) in files {
            let full = root.join(path);
            fs::create_dir_all(full.parent().expect("a parent")).expect("a folder");
            fs::write(full, text).expect("a file");
        }
        Workspace::at(root)
    }

    #[test]
    fn a_match_carries_the_lines_either_side_of_it() {
        let ws = workspace("context", &[("src/a.rs", "one\ntwo\nthree\nfour\nfive\n")]);
        let lane = lane_of(&ws);
        let reply =
            search(&ws, &lane, &json!({ "pattern": "three", "context": 1 })).expect("a search");
        let found = &reply["matches"][0];
        assert_eq!(found["path"], "src/a.rs");
        assert_eq!(found["line"], 3);
        assert_eq!(found["before"], json!([{"line": 2, "text": "two"}]));
        assert_eq!(found["after"], json!([{"line": 4, "text": "four"}]));
    }

    #[test]
    fn the_default_context_is_two_lines_and_a_binary_file_is_skipped_and_counted() {
        let ws = workspace(
            "defaults",
            &[("a.txt", "1\n2\n3\nhit\n5\n6\n7\n"), ("b.bin", "hit\0\0\n")],
        );
        let lane = lane_of(&ws);
        let reply = search(&ws, &lane, &json!({ "pattern": "hit" })).expect("a search");
        let found = &reply["matches"][0];
        assert_eq!(found["before"].as_array().map(Vec::len), Some(2));
        assert_eq!(found["after"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            reply["matches"].as_array().map(Vec::len),
            Some(1),
            "{reply}"
        );
    }

    #[test]
    fn a_cut_result_names_the_match_it_resumes_after() {
        let text: String = (1..=10).map(|n| format!("hit {n}\n")).collect();
        let ws = workspace("cut", &[("a.txt", &text)]);
        let lane = lane_of(&ws);
        let reply = search(
            &ws,
            &lane,
            &json!({ "pattern": "hit", "max": 3, "context": 0 }),
        )
        .expect("a search");
        assert_eq!(reply["truncated"], true);
        assert!(
            reply["continue"]
                .as_str()
                .unwrap()
                .contains("\"after\":\"a.txt:3\""),
            "{reply}"
        );
        let next = search(
            &ws,
            &lane,
            &json!({ "pattern": "hit", "max": 3, "context": 0, "after": "a.txt:3" }),
        )
        .expect("the next page");
        assert_eq!(next["matches"][0]["line"], 4);
    }

    #[test]
    fn a_bad_regex_is_refused_with_its_reason() {
        let ws = workspace("regex", &[("a.txt", "x\n")]);
        let lane = lane_of(&ws);
        let err = search(&ws, &lane, &json!({ "pattern": "(" })).expect_err("not a regex");
        assert!(err.to_string().contains("not a regex"), "{err}");
    }
}
