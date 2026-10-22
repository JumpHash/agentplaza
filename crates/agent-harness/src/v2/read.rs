//! `read`: the lines of one or more files, numbered as `cat -n` numbers them, in one call.
//!
//! Arguments: `files`, an array of 1 to [`MAX_READ_FILES`] objects, each with `path` (required, relative) and
//! `start` and `end` (optional, 1-based and inclusive). A file with no range over [`OUTLINE_LINES`] lines
//! returns its outline ([`outline`]) and its first [`OUTLINE_HEAD`] lines; its `continue` says where to read on.
//!
//! Output: `{"files": [...]}`, one entry per request in order. An entry is `{"path", "lines", "content"}` — `lines`
//! the file's line count, `content` the numbered lines shown — with `outline` for a large file, `continue` when
//! the shown lines stop short of what was asked (the call's [`READ_BUDGET`] ran out), and `error` in place of
//! the rest when the file cannot be read. A file read again while it is unchanged, and the lines shown are still
//! the ones asked for, is `{"path", "lines", "unchanged_since"}` with no content: the call that showed it.
//! A binary file is `{"path", "bytes", "binary": true}`.

use std::fs;

use regex::Regex;
use serde_json::{Value, json};

use crate::files::{self, BINARY_PROBE_BYTES, Workspace};
use crate::tools::ToolError;

use super::track::{self, Seen, Stamp, digest_of, stamp_of};
use super::{Lane, fit, locked, refusal_text};

/// The most files one `read` call asks for.
pub const MAX_READ_FILES: usize = 32;

/// The most bytes of content one `read` call returns: 48 KiB, across all its files.
pub const READ_BUDGET: usize = 48 * 1024;

/// The largest file `read` opens: 8 MiB. A bigger one is refused rather than read in part.
pub const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// A file over this many lines, read without a range, returns its outline and its first [`OUTLINE_HEAD`] lines.
pub const OUTLINE_LINES: usize = 400;

/// How many lines a large file shows when it is read without a range.
pub const OUTLINE_HEAD: usize = 200;

/// The most outline entries one file returns.
const MAX_OUTLINE: usize = 300;

/// The longest line `read` returns whole, in characters. A longer one is cut to this many and ends in `…`.
const MAX_LINE_CHARS: usize = 2000;

/// One request of a `read` call.
struct Request {
    path: String,
    start: Option<usize>,
    end: Option<usize>,
}

/// Runs a `read` call: each file in turn, sharing the call's budget.
pub(crate) fn read(
    workspace: &Workspace,
    lane: &Lane,
    call: u64,
    input: &Value,
) -> Result<Value, ToolError> {
    let items = match input.get("files") {
        Some(Value::Array(items)) => items,
        _ => {
            return Err(files::invalid(
                "\"files\" must be an array of {path, start?, end?}",
            ));
        }
    };
    if items.is_empty() {
        return Err(files::invalid("\"files\" must name at least one file"));
    }
    if items.len() > MAX_READ_FILES {
        return Err(files::limit(format!(
            "{} files, over the {MAX_READ_FILES}-file cap; read the rest in another call",
            items.len()
        )));
    }
    let requests = items.iter().map(request).collect::<Result<Vec<_>, _>>()?;
    let mut budget = READ_BUDGET;
    let files: Vec<Value> = requests
        .iter()
        .map(
            |request| match shown(workspace, lane, call, request, &mut budget) {
                Ok(entry) => entry,
                Err(err) => json!({ "path": request.path, "error": refusal_text(&err) }),
            },
        )
        .collect();
    Ok(json!({ "files": files }))
}

/// One `files` entry as a request.
fn request(value: &Value) -> Result<Request, ToolError> {
    let Some(object) = value.as_object() else {
        return Err(files::invalid("each file is an object with a \"path\""));
    };
    let path = match object.get("path") {
        Some(Value::String(path)) => path.clone(),
        _ => return Err(files::invalid("each file needs a string \"path\"")),
    };
    let start = line_number(object.get("start"), "start")?;
    let end = line_number(object.get("end"), "end")?;
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        return Err(files::invalid(format!(
            "{path:?}: \"end\" ({end}) is before \"start\" ({start})"
        )));
    }
    Ok(Request { path, start, end })
}

/// An optional 1-based line number.
fn line_number(value: Option<&Value>, key: &str) -> Result<Option<usize>, ToolError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(line) if line >= 1 => Ok(Some(line as usize)),
            _ => Err(files::invalid(format!(
                "\"{key}\" must be a line number from 1"
            ))),
        },
    }
}

/// What one request shows, as its `files` entry. A refusal is an error for the caller to put in the entry.
fn shown(
    workspace: &Workspace,
    lane: &Lane,
    call: u64,
    request: &Request,
    budget: &mut usize,
) -> Result<Value, ToolError> {
    let (path, label) = workspace.resolve(&request.path)?;
    let metadata = fs::metadata(&path)
        .map_err(|err| files::io_tool_error(err, format!("cannot read {label}")))?;
    if metadata.is_dir() {
        return Err(files::invalid(format!(
            "{label} is a directory; find lists a directory's files"
        )));
    }
    if metadata.len() > MAX_READ_BYTES {
        return Err(files::limit(format!(
            "{label} is {} bytes, over the {MAX_READ_BYTES}-byte read cap",
            metadata.len()
        )));
    }
    let bytes =
        fs::read(&path).map_err(|err| files::io_tool_error(err, format!("cannot read {label}")))?;
    if bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0) {
        return Ok(json!({ "path": label, "bytes": bytes.len(), "binary": true }));
    }
    let text = String::from_utf8(bytes.clone()).map_err(|err| {
        files::invalid(format!(
            "{label} is not UTF-8 text (an invalid sequence at byte {})",
            err.utf8_error().valid_up_to()
        ))
    })?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total = lines.len();
    let digest = digest_of(&bytes);
    let stamp = stamp_of(&path).unwrap_or(Stamp::UNKNOWN);

    let large = request.start.is_none() && request.end.is_none() && total > OUTLINE_LINES;
    let (first, last) = if large {
        (1, OUTLINE_HEAD)
    } else {
        let first = request.start.unwrap_or(1);
        (first, request.end.unwrap_or(total).min(total))
    };

    // Shown already, unchanged, and the lines asked for are among them: the content is not sent again.
    let since = locked(&lane.tracking).seen(&label).and_then(|seen| {
        (seen.digest == Some(digest) && seen.window.0 <= first && last <= seen.window.1)
            .then_some(seen.call)
    });
    if let Some(call_shown) = since {
        return Ok(json!({ "path": label, "lines": total, "unchanged_since": call_shown }));
    }

    let mut entry = json!({ "path": label, "lines": total });
    if large {
        let outline = outline(&label, &lines);
        let (outline, cut) = fit(outline, (*budget).min(READ_BUDGET / 4));
        *budget = budget.saturating_sub(outline.iter().map(|entry| entry.to_string().len()).sum());
        entry["outline"] = json!(outline);
        if cut {
            entry["outline_cut"] = json!(true);
        }
    }

    let mut content = String::new();
    let mut shown_last = first.saturating_sub(1);
    let mut cut_at = None;
    if first <= last {
        for number in first..=last {
            let numbered = numbered_line(number, lines[number - 1]);
            if numbered.len() > *budget {
                cut_at = Some(number);
                break;
            }
            *budget -= numbered.len();
            content.push_str(&numbered);
            shown_last = number;
        }
    }
    entry["content"] = json!(content);

    let resume = match cut_at {
        Some(number) => Some(number),
        None if large => Some(OUTLINE_HEAD + 1),
        None => None,
    };
    if let Some(start) = resume {
        entry["continue"] = json!(super::continue_hint(
            "read",
            &json!({ "path": label, "start": start })
        ));
    }
    if first > total && total > 0 {
        entry["past_end"] = json!(true);
    }
    if shown_last >= first {
        lane_record(lane, &label, digest, stamp, (first, shown_last), call);
    }
    Ok(entry)
}

/// Records what a read showed, so a re-read of the same lines of the unchanged file is answered without them.
fn lane_record(
    lane: &Lane,
    label: &str,
    digest: track::Digest32,
    stamp: Stamp,
    window: (usize, usize),
    call: u64,
) {
    locked(&lane.tracking).record(
        label,
        Seen {
            digest: Some(digest),
            stamp,
            window,
            call,
        },
    );
}

/// `cat -n` for one line: the number right-aligned in six columns, a tab, the line and a newline. A line over
/// [`MAX_LINE_CHARS`] characters is cut to that many and ends in `…`.
pub(crate) fn numbered_line(number: usize, line: &str) -> String {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let body = if line.chars().count() > MAX_LINE_CHARS {
        let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    };
    format!("{number:>6}\t{body}\n")
}

/// The language a file's extension names, for the outline patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    Rust,
    Go,
    Python,
    TypeScript,
    Swift,
    Kotlin,
    Java,
    CSharp,
    Shell,
    Generic,
}

fn language(label: &str) -> Language {
    let extension = label.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Language::Rust,
        "go" => Language::Go,
        "py" => Language::Python,
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Language::TypeScript,
        "swift" => Language::Swift,
        "kt" | "kts" => Language::Kotlin,
        "java" => Language::Java,
        "cs" => Language::CSharp,
        "sh" | "bash" | "zsh" => Language::Shell,
        _ => Language::Generic,
    }
}

/// The patterns a language's top-level definitions match, in order. A pattern with a kind names the definition
/// that kind; one without captures the kind and then the name.
fn patterns(language: Language) -> &'static [(Option<&'static str>, &'static str)] {
    match language {
        Language::Rust => &[
            (
                None,
                r"^(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const|extern)\s+)*(fn|struct|enum|trait|type|mod|const|static|union)\s+(\w+)",
            ),
            (
                Some("impl"),
                r"^impl\b(?:<[^>]*>)?\s+(?:[\w:<>&' ,]+?\s+for\s+)?(\w+)",
            ),
            (Some("macro"), r"^macro_rules!\s+(\w+)"),
        ],
        Language::Go => &[
            (Some("func"), r"^func\s+(?:\([^)]*\)\s*)?(\w+)"),
            (None, r"^(type|var|const)\s+(\w+)"),
        ],
        Language::Python => &[(None, r"^(?:async\s+)?(class|def)\s+(\w+)")],
        Language::TypeScript => &[
            (
                None,
                r"^(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?:async\s+)?(class|function|interface|type|enum|namespace)\s*\*?\s*(\w+)",
            ),
            (None, r"^(?:export\s+)?(const|let|var)\s+(\w+)"),
        ],
        Language::Swift => &[(
            None,
            r"^(?:(?:public|private|internal|fileprivate|open|final)\s+)*(class|struct|enum|protocol|extension|func|actor|typealias)\s+([\w.]+)",
        )],
        Language::Kotlin => &[(
            None,
            r"^(?:(?:public|private|internal|protected|data|sealed|open|abstract|enum|inner|value|suspend)\s+)*(class|interface|object|fun|typealias)\s+([\w.<>]+)",
        )],
        Language::Java => &[(
            None,
            r"^(?:(?:public|private|protected|static|final|abstract|sealed)\s+)*(class|interface|enum|record)\s+(\w+)",
        )],
        Language::CSharp => &[(
            None,
            r"^(?:(?:public|private|protected|internal|static|sealed|abstract|partial|readonly|unsafe)\s+)*(class|struct|interface|enum|record|namespace)\s+(\w+)",
        )],
        Language::Shell => &[
            (Some("function"), r"^function\s+(\w[\w.-]*)"),
            (Some("function"), r"^(\w[\w.-]*)\s*\(\)\s*\{?"),
        ],
        Language::Generic => &[(Some("definition"), r"^([A-Za-z_]\w*)\s*[(:={]")],
    }
}

/// The top-level definitions of a file, by line: `{"line", "kind", "name"}`, at most [`MAX_OUTLINE`] of them.
/// A definition indented under another (a method, a nested type) is not top-level, so it is not listed.
pub(crate) fn outline(label: &str, lines: &[&str]) -> Vec<Value> {
    let language = language(label);
    let compiled: Vec<(Option<&str>, Regex)> = patterns(language)
        .iter()
        .filter_map(|(kind, pattern)| Regex::new(pattern).ok().map(|regex| (*kind, regex)))
        .collect();
    let mut entries = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if entries.len() >= MAX_OUTLINE {
            break;
        }
        for (kind, regex) in &compiled {
            let Some(found) = regex.captures(line) else {
                continue;
            };
            let (kind, name) = match kind {
                Some(kind) => (*kind, found.get(1)),
                None => (found.get(1).map_or("", |m| m.as_str()), found.get(2)),
            };
            let kind = if kind.is_empty() { "definition" } else { kind };
            if let Some(name) = name {
                entries.push(json!({ "line": index + 1, "kind": kind, "name": name.as_str() }));
            }
            break;
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_numbered_like_cat_and_a_long_one_is_cut() {
        assert_eq!(numbered_line(7, "let x = 1;\n"), "     7\tlet x = 1;\n");
        let cut = numbered_line(1, &"a".repeat(MAX_LINE_CHARS + 5));
        assert!(cut.ends_with("…\n"), "{cut:?}");
        assert_eq!(cut.chars().count(), 6 + 1 + MAX_LINE_CHARS + 1 + 1);
    }

    #[test]
    fn the_outline_of_rust_lists_top_level_items_and_not_methods() {
        let source = "use std::fs;\n\npub struct Store {\n    count: u32,\n}\n\nimpl Store {\n    pub fn open() {}\n}\n\nasync fn serve() {}\n";
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let outline = outline("store.rs", &lines);
        let names: Vec<(&str, &str)> = outline
            .iter()
            .map(|entry| {
                (
                    entry["kind"].as_str().unwrap(),
                    entry["name"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [("struct", "Store"), ("impl", "Store"), ("fn", "serve")]
        );
        assert_eq!(outline[1]["line"], 7);
    }

    #[test]
    fn the_outline_follows_each_language_and_falls_back_to_column_zero_names() {
        let cases: [(&str, &str, (&str, &str)); 6] = [
            (
                "app.py",
                "class Parser:\n    def run(self):\n",
                ("class", "Parser"),
            ),
            (
                "main.go",
                "package main\n\nfunc (s *Srv) Serve() {}\n",
                ("func", "Serve"),
            ),
            (
                "util.ts",
                "export default async function load() {}\n",
                ("function", "load"),
            ),
            (
                "Model.swift",
                "public final class Model {}\n",
                ("class", "Model"),
            ),
            (
                "Main.java",
                "public class Main {\n    void x() {}\n}\n",
                ("class", "Main"),
            ),
            (
                "build.sh",
                "deploy() {\n  echo\n}\n",
                ("function", "deploy"),
            ),
        ];
        for (label, source, (kind, name)) in cases {
            let lines: Vec<&str> = source.split_inclusive('\n').collect();
            let first = &outline(label, &lines)[0];
            assert_eq!(first["kind"], kind, "{label}");
            assert_eq!(first["name"], name, "{label}");
        }
        let lines = ["# notes\n", "PORT = 80\n"];
        let generic = outline("notes.txt", &lines);
        assert_eq!(generic[0]["name"], "PORT", "{generic:?}");
    }
}
