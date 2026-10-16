//! Unified diffs into hunks. The hub diffs; this turns the text into a [`ChangeSet`]
//! that both apps draw the same way. Reads `git diff` output (with or without
//! `--binary` and `-M`) and `diff -u`. Paths come back as the diff names them, with
//! the `a/` and `b/` prefixes removed; run them through [`crate::path::check_path`]
//! before use.
//!
//! The reverse too: [`write_unified`] writes two texts as git would, for a hub that
//! has the files at both refs but no diff from the git host. Its line diff is Myers'
//! greedy O(ND) algorithm, with a bound past which a file is written as one block.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The files one diff changes, in the order the diff lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub files: Vec<FileChange>,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// The path after the change (the new name for a rename; the old one for a deletion).
    pub path: String,
    /// The path before the change, for a rename or a copy.
    pub old_path: Option<String>,
    pub status: Status,
    /// A binary file has no hunks; the diff only says it differs.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// What happened to the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// One `@@` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The text after the closing `@@` (usually the enclosing function), trimmed.
    pub header: String,
    pub lines: Vec<Line>,
}

/// One line of a hunk, without its prefix character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub kind: LineKind,
    /// The line number on the old side, if the line is on it.
    pub old_no: Option<u32>,
    /// The line number on the new side, if the line is on it.
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// Unchanged (` `).
    Context,
    /// Added (`+`).
    Add,
    /// Removed (`-`).
    Remove,
    /// The `\ No newline at end of file` marker. It belongs to the line before it.
    NoNewline,
}

/// Totals over a change set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
}

impl ChangeSet {
    /// Files, added lines and removed lines.
    pub fn stats(&self) -> Stats {
        let mut s = Stats {
            files: self.files.len(),
            additions: 0,
            deletions: 0,
        };
        for line in self
            .files
            .iter()
            .flat_map(|f| &f.hunks)
            .flat_map(|h| &h.lines)
        {
            match line.kind {
                LineKind::Add => s.additions += 1,
                LineKind::Remove => s.deletions += 1,
                LineKind::Context | LineKind::NoNewline => {}
            }
        }
        s
    }
}

/// A diff that does not parse. `line` is 1-based, in the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "diff line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for DiffError {}

fn err(line: usize, message: &str) -> DiffError {
    DiffError {
        line,
        message: message.to_string(),
    }
}

/// Parses `git diff` or `diff -u` output. Text before the first file (a commit
/// header, a mail preamble) is skipped. A hunk's line counts come from its header,
/// so a removed line that starts with `--` is not taken for a file header.
pub fn parse_unified(text: &str) -> Result<ChangeSet, DiffError> {
    let mut p = Parser::default();
    for (i, raw) in text.split('\n').enumerate() {
        p.line(i + 1, raw)?;
    }
    let last = text.split('\n').count();
    p.finish_file(last)?;
    Ok(ChangeSet { files: p.files })
}

/// A file whose header is being read.
#[derive(Default)]
struct Pending {
    /// Both names from `diff --git`, prefix removed.
    git: Option<(String, String)>,
    /// `---`: `None` if not seen; `Some(None)` for `/dev/null`; else the name.
    old: Option<Option<String>>,
    /// `+++`, as for `old`.
    new: Option<Option<String>>,
    /// `rename from` / `copy from`.
    from: Option<String>,
    /// `rename to` / `copy to`.
    to: Option<String>,
    renamed: bool,
    copied: bool,
    new_file: bool,
    deleted_file: bool,
    binary: bool,
    /// After `GIT binary patch` the payload follows; skip it up to the next file.
    skip_binary: bool,
    hunks: Vec<Hunk>,
}

/// A hunk whose body is being read. The header's counts say how many lines remain.
struct Open {
    hunk: Hunk,
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
}

#[derive(Default)]
struct Parser {
    files: Vec<FileChange>,
    cur: Option<Pending>,
    open: Option<Open>,
}

impl Parser {
    fn line(&mut self, no: usize, raw: &str) -> Result<(), DiffError> {
        let in_hunk = match self.open.as_mut() {
            None => false,
            Some(open) => {
                if raw.starts_with('\\') {
                    open.hunk.lines.push(Line {
                        kind: LineKind::NoNewline,
                        old_no: None,
                        new_no: None,
                        text: raw.trim_start_matches('\\').trim_start().to_string(),
                    });
                    return Ok(());
                }
                if open.old_left > 0 || open.new_left > 0 {
                    return open.body(no, raw);
                }
                true
            }
        };
        if in_hunk {
            self.close_hunk(no)?;
        }
        self.header(no, raw)
    }

    /// Ends the open hunk, if any. Its counts must be used up.
    fn close_hunk(&mut self, no: usize) -> Result<(), DiffError> {
        if let Some(open) = self.open.take() {
            if open.old_left > 0 || open.new_left > 0 {
                return Err(err(no, "hunk ends before its header's line counts"));
            }
            if let Some(p) = self.cur.as_mut() {
                p.hunks.push(open.hunk);
            }
        }
        Ok(())
    }

    /// Ends the current file, if any.
    fn finish_file(&mut self, no: usize) -> Result<(), DiffError> {
        self.close_hunk(no)?;
        if let Some(p) = self.cur.take() {
            self.files.push(p.into_change());
        }
        Ok(())
    }

    /// A line outside any hunk body.
    fn header(&mut self, no: usize, raw: &str) -> Result<(), DiffError> {
        if self.cur.as_ref().is_some_and(|p| p.skip_binary) && !raw.starts_with("diff --git ") {
            return Ok(());
        }
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            self.finish_file(no)?;
            let git =
                parse_git_header(rest).ok_or_else(|| err(no, "unreadable diff --git line"))?;
            self.cur = Some(Pending {
                git: Some(git),
                ..Pending::default()
            });
            return Ok(());
        }
        if let Some(v) = raw.strip_prefix("--- ") {
            let value = name_value(v).map(|n| strip(&n, "a/"));
            if let Some(p) = self.cur.as_mut()
                && p.old.is_none()
                && p.hunks.is_empty()
            {
                p.old = Some(value);
                return Ok(());
            }
            self.finish_file(no)?;
            self.cur = Some(Pending {
                old: Some(value),
                ..Pending::default()
            });
            return Ok(());
        }
        if let Some(v) = raw.strip_prefix("+++ ") {
            let value = name_value(v).map(|n| strip(&n, "b/"));
            return match self.cur.as_mut() {
                Some(p) if p.old.is_some() && p.new.is_none() && p.hunks.is_empty() => {
                    p.new = Some(value);
                    Ok(())
                }
                _ => Err(err(no, "+++ line without a matching ---")),
            };
        }
        if raw.starts_with("@@ ") {
            if self.cur.is_none() {
                return Err(err(no, "hunk before any file header"));
            }
            let (old_start, old_lines, new_start, new_lines, header) =
                parse_hunk_header(raw).ok_or_else(|| err(no, "unreadable @@ hunk header"))?;
            self.open = Some(Open {
                hunk: Hunk {
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    header,
                    lines: Vec::new(),
                },
                old_left: old_lines,
                new_left: new_lines,
                old_no: old_start,
                new_no: new_start,
            });
            return Ok(());
        }
        let Some(p) = self.cur.as_mut() else {
            return Ok(()); // preamble before the first file
        };
        if raw.starts_with("new file mode ") {
            p.new_file = true;
        } else if raw.starts_with("deleted file mode ") {
            p.deleted_file = true;
        } else if let Some(v) = raw.strip_prefix("rename from ") {
            p.renamed = true;
            p.from = Some(name_value(v).unwrap_or_default());
        } else if let Some(v) = raw.strip_prefix("rename to ") {
            p.to = Some(name_value(v).unwrap_or_default());
        } else if let Some(v) = raw.strip_prefix("copy from ") {
            p.copied = true;
            p.from = Some(name_value(v).unwrap_or_default());
        } else if let Some(v) = raw.strip_prefix("copy to ") {
            p.to = Some(name_value(v).unwrap_or_default());
        } else if raw.starts_with("Binary files ") && raw.ends_with(" differ") {
            p.binary = true;
        } else if raw == "GIT binary patch" {
            p.binary = true;
            p.skip_binary = true;
        } else if raw.starts_with(['+', ' ']) || (raw.starts_with('-') && !raw.starts_with("--")) {
            // A hunk body line with no hunk open: the header's counts were already used up.
            return Err(err(no, "line outside a hunk"));
        }
        // Anything else (index, mode, similarity, a signature line) is not needed.
        Ok(())
    }
}

impl Open {
    /// One line of the body. A blank line is an empty context line (some tools strip the space).
    fn body(&mut self, no: usize, raw: &str) -> Result<(), DiffError> {
        let (kind, text) = match raw.as_bytes().first() {
            None => (LineKind::Context, ""),
            Some(b' ') => (LineKind::Context, &raw[1..]),
            Some(b'-') => (LineKind::Remove, &raw[1..]),
            Some(b'+') => (LineKind::Add, &raw[1..]),
            Some(_) => return Err(err(no, "hunk line must start with a space, - or +")),
        };
        let on_old = kind != LineKind::Add;
        let on_new = kind != LineKind::Remove;
        if (on_old && self.old_left == 0) || (on_new && self.new_left == 0) {
            return Err(err(no, "more lines than the hunk header counts"));
        }
        let line = Line {
            kind,
            old_no: on_old.then_some(self.old_no),
            new_no: on_new.then_some(self.new_no),
            text: text.to_string(),
        };
        if on_old {
            self.old_left -= 1;
            self.old_no += 1;
        }
        if on_new {
            self.new_left -= 1;
            self.new_no += 1;
        }
        self.hunk.lines.push(line);
        Ok(())
    }
}

impl Pending {
    fn into_change(self) -> FileChange {
        let old_side = self.old.clone().flatten();
        let new_side = self.new.clone().flatten();
        let status = if self.renamed {
            Status::Renamed
        } else if self.copied || self.new_file || self.old == Some(None) {
            Status::Added
        } else if self.deleted_file || self.new == Some(None) {
            Status::Deleted
        } else {
            Status::Modified
        };
        let (git_a, git_b) = self.git.clone().unzip();
        let path = self
            .to
            .clone()
            .or(new_side)
            .or(old_side.clone())
            .or(git_b)
            .or(git_a)
            .unwrap_or_default();
        let old_path = if self.renamed || self.copied {
            self.from.clone().or(old_side)
        } else {
            None
        };
        FileChange {
            path,
            old_path,
            status,
            binary: self.binary,
            hunks: self.hunks,
        }
    }
}

/// `a/x b/x` (unquoted, or quoted per name) to the two names, prefix removed.
/// An unquoted line with a space in the name is split where both sides agree, else at ` b/`.
fn parse_git_header(rest: &str) -> Option<(String, String)> {
    let (a, b) = if rest.starts_with('"') {
        let (tok, after) = take_quoted(rest)?;
        (unquote(tok)?, side_value(after.strip_prefix(' ')?)?)
    } else {
        let split = rest
            .match_indices(' ')
            .map(|(i, _)| i)
            .find(|&i| strip(&rest[..i], "a/") == strip(&rest[i + 1..], "b/"))
            .or_else(|| rest.find(" b/"))
            .or_else(|| rest.find(" \""))?;
        (rest[..split].to_string(), side_value(&rest[split + 1..])?)
    };
    Some((strip(&a, "a/"), strip(&b, "b/")))
}

/// The name in a `---` or `+++` value: a tab ends it (git puts one after a name with
/// a space; `diff -u` puts a timestamp there). `None` for `/dev/null`.
fn name_value(v: &str) -> Option<String> {
    let v = v.split('\t').next().unwrap_or(v);
    if v == "/dev/null" {
        return None;
    }
    if v.starts_with('"') {
        return Some(unquote(v).unwrap_or_else(|| v.to_string()));
    }
    Some(v.to_string())
}

/// A name that is either quoted or bare, as a whole.
fn side_value(s: &str) -> Option<String> {
    if s.starts_with('"') {
        unquote(take_quoted(s)?.0)
    } else {
        Some(s.to_string())
    }
}

fn strip(s: &str, prefix: &str) -> String {
    s.strip_prefix(prefix).unwrap_or(s).to_string()
}

/// Splits off a leading quoted token. Returns the token (with quotes) and the rest.
fn take_quoted(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some((&s[..=i], &s[i + 1..])),
            _ => i += 1,
        }
    }
    None
}

/// Decodes a git C-style quoted name: `\"`, `\\`, `\t` and friends, and `\NNN` octal bytes (UTF-8 in pieces).
fn unquote(tok: &str) -> Option<String> {
    let inner = tok.strip_prefix('"')?.strip_suffix('"')?;
    let b = inner.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let c = *b.get(i + 1)?;
        i += 2;
        match c {
            b'"' | b'\\' => out.push(c),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            b'0'..=b'7' => {
                let mut v = u32::from(c - b'0');
                for _ in 0..2 {
                    match b.get(i) {
                        Some(d @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(u8::try_from(v).ok()?);
            }
            _ => return None,
        }
    }
    String::from_utf8(out).ok()
}

/// `@@ -a[,b] +c[,d] @@ text`. A missing count is 1.
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, tail) = rest.split_once(" @@")?;
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some((
        old_start,
        old_lines,
        new_start,
        new_lines,
        tail.trim_start().to_string(),
    ))
}

fn range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

// The writer. A file is written as git writes one: the header lines, then `@@` blocks with `context`
// unchanged lines around each change. Past `MAX_WRITE_LINES` lines on either side, or `MAX_WRITE_EDITS`
// edits, no line diff is searched for: the whole old file is removed and the new one added, which is
// still a valid diff.

/// Past this many lines on either side, no line diff is searched for.
const MAX_WRITE_LINES: usize = 20_000;
/// The most edits the line diff searches for before it gives up.
const MAX_WRITE_EDITS: usize = 2_000;
/// The mode written for a new or deleted file, as git writes it for a regular file.
const FILE_MODE: &str = "100644";

/// A line's text, and whether a newline ends it. The last line of a file may have none.
type Row<'a> = (&'a str, bool);

/// One step of an edit script: a line kept (its old and new index), a line removed (old index), or a
/// line added (new index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Same(usize, usize),
    Gone(usize),
    New(usize),
}

/// Writes the change from `old` to `new` as git writes a text file's diff: the `diff --git` line, the
/// `new file mode` or `deleted file mode` line for an add or a delete, the rename lines when the paths
/// differ, then `---`, `+++` and the hunks with `context` lines around each change. `None` is the absent
/// side. Empty when nothing changed (the same path, the same text). Paths that git would quote are quoted.
pub fn write_unified(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old: &str,
    new: &str,
    context: usize,
) -> String {
    if old_path.is_none() && new_path.is_none() {
        return String::new();
    }
    let (a, b) = (rows(old), rows(new));
    let body = hunks(&edit_script(&a, &b), &a, &b, context);
    if old_path.is_some() && old_path == new_path && body.is_empty() {
        return String::new();
    }
    let mut out = file_head(old_path, new_path);
    if !body.is_empty() {
        out.push_str(&format!(
            "--- {}\n+++ {}\n",
            side(old_path, "a/"),
            side(new_path, "b/")
        ));
        out.push_str(&body);
    }
    out
}

/// Writes a change that has no lines to show: a binary file, or one too large to diff. The same header as
/// [`write_unified`], then git's `Binary files a/x and b/x differ` line (`/dev/null` for the absent side).
/// Empty when both paths are absent.
pub fn write_binary(old_path: Option<&str>, new_path: Option<&str>) -> String {
    if old_path.is_none() && new_path.is_none() {
        return String::new();
    }
    let mut out = file_head(old_path, new_path);
    out.push_str(&format!(
        "Binary files {} and {} differ\n",
        side(old_path, "a/"),
        side(new_path, "b/")
    ));
    out
}

/// The `diff --git` line, then the add, delete and rename lines. A new or deleted file names one path on
/// both sides, as git does.
fn file_head(old_path: Option<&str>, new_path: Option<&str>) -> String {
    let a_name = old_path.or(new_path).unwrap_or_default();
    let b_name = new_path.or(old_path).unwrap_or_default();
    let mut out = format!(
        "diff --git {} {}\n",
        token(&format!("a/{a_name}")),
        token(&format!("b/{b_name}"))
    );
    if old_path.is_none() {
        out.push_str(&format!("new file mode {FILE_MODE}\n"));
    }
    if new_path.is_none() {
        out.push_str(&format!("deleted file mode {FILE_MODE}\n"));
    }
    if let (Some(from), Some(to)) = (old_path, new_path)
        && from != to
    {
        out.push_str(&format!(
            "rename from {}\nrename to {}\n",
            token(from),
            token(to)
        ));
    }
    out
}

/// One side of a `---`, `+++` or `Binary files` line: `prefix` and the path, or `/dev/null` when absent.
fn side(path: Option<&str>, prefix: &str) -> String {
    match path {
        Some(p) => token(&format!("{prefix}{p}")),
        None => "/dev/null".to_string(),
    }
}

/// `text` as a git header token: bare, or C-quoted when it holds a byte git would escape.
fn token(text: &str) -> String {
    let plain = text
        .bytes()
        .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\');
    if plain { text.to_string() } else { quote(text) }
}

/// `text` as a C-style quoted string, the way git quotes a path: escapes for the usual control bytes, octal
/// for the rest (non-ASCII included, so each UTF-8 byte is one escape).
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for b in text.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            0x20..=0x7e => out.push(char::from(b)),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

/// The lines of `text`, each with whether a newline ends it. `""` has none.
fn rows(text: &str) -> Vec<Row<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(i) => {
                out.push((&rest[..i], true));
                rest = &rest[i + 1..];
            }
            None => {
                out.push((rest, false));
                break;
            }
        }
    }
    out
}

/// The edit script from `old` to `new`, or the whole-file replacement past the size bounds.
fn edit_script(old: &[Row], new: &[Row]) -> Vec<Op> {
    if old.len() > MAX_WRITE_LINES || new.len() > MAX_WRITE_LINES {
        return replaced(old.len(), new.len());
    }
    myers(old, new).unwrap_or_else(|| replaced(old.len(), new.len()))
}

/// Every old line removed, then every new line added.
fn replaced(old: usize, new: usize) -> Vec<Op> {
    (0..old)
        .map(Op::Gone)
        .chain((0..new).map(Op::New))
        .collect()
}

/// Myers' greedy O(ND) diff. `v[k]` is the furthest x on diagonal k after each round `d`; the rounds'
/// snapshots (diagonals `-d..=d`) are kept for the walk back. None when no path is found within
/// `MAX_WRITE_EDITS` edits.
fn myers(a: &[Row], b: &[Row]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as i64, b.len() as i64);
    let max = n + m;
    let limit = max.min(MAX_WRITE_EDITS as i64);
    let off = max + 1;
    let idx = |k: i64| (k + off) as usize;
    let mut v = vec![0i64; (2 * max + 3) as usize];
    let mut trace: Vec<Vec<i32>> = Vec::new();
    let mut reached = false;
    for d in 0..=limit {
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
                v[idx(k + 1)]
            } else {
                v[idx(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx(k)] = x;
            if x >= n && y >= m {
                reached = true;
                break;
            }
        }
        trace.push((-d..=d).map(|k| v[idx(k)] as i32).collect());
        if reached {
            break;
        }
    }
    if !reached {
        return None;
    }
    // Walk back from the end. Round d's snapshot is at index k + d.
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..trace.len()).rev() {
        let di = d as i64;
        let prev = &trace[d - 1];
        let get = |k: i64| i64::from(prev[(k + di - 1) as usize]);
        let k = x - y;
        let down = k == -di || (k != di && get(k - 1) < get(k + 1));
        let pk = if down { k + 1 } else { k - 1 };
        let px = get(pk);
        let py = px - pk;
        let (xs, ys) = if down { (px, py + 1) } else { (px + 1, py) };
        while x > xs {
            x -= 1;
            y -= 1;
            ops.push(Op::Same(x as usize, y as usize));
        }
        debug_assert_eq!(y, ys);
        if down {
            ops.push(Op::New(py as usize));
        } else {
            ops.push(Op::Gone(px as usize));
        }
        x = px;
        y = py;
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        ops.push(Op::Same(x as usize, y as usize));
    }
    ops.reverse();
    Some(ops)
}

/// The `@@` blocks of an edit script. Each keeps `context` unchanged lines either side of its changes, and
/// changes separated by no more than `2 * context` unchanged lines share one block, as git joins them.
fn hunks(ops: &[Op], old: &[Row], new: &[Row], context: usize) -> String {
    let mut old_before = vec![0usize; ops.len() + 1];
    let mut new_before = vec![0usize; ops.len() + 1];
    for (i, op) in ops.iter().enumerate() {
        old_before[i + 1] = old_before[i] + usize::from(!matches!(op, Op::New(_)));
        new_before[i + 1] = new_before[i] + usize::from(!matches!(op, Op::Gone(_)));
    }
    // Each block is (first change, last change) as indexes into `ops`.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for c in (0..ops.len()).filter(|&i| !matches!(ops[i], Op::Same(..))) {
        match blocks.last_mut() {
            Some(last) if c - last.1 - 1 <= 2 * context => last.1 = c,
            _ => blocks.push((c, c)),
        }
    }
    let mut out = String::new();
    for (first, last) in blocks {
        let s = first.saturating_sub(context);
        let e = (last + context).min(ops.len() - 1);
        let body = &ops[s..=e];
        let old_n = body.iter().filter(|op| !matches!(op, Op::New(_))).count();
        let new_n = body.iter().filter(|op| !matches!(op, Op::Gone(_))).count();
        // A run with no lines on a side starts at the line before it, as git numbers it.
        let old_start = if old_n == 0 {
            old_before[s]
        } else {
            old_before[s] + 1
        };
        let new_start = if new_n == 0 {
            new_before[s]
        } else {
            new_before[s] + 1
        };
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            span(old_start, old_n),
            span(new_start, new_n)
        ));
        for op in body {
            let (sign, row) = match *op {
                Op::Same(x, _) => (' ', old[x]),
                Op::Gone(x) => ('-', old[x]),
                Op::New(y) => ('+', new[y]),
            };
            out.push(sign);
            out.push_str(row.0);
            out.push('\n');
            if !row.1 {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    out
}

/// A hunk range as `start,count`, with the count left out when it is one.
fn span(start: usize, count: usize) -> String {
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFY: &str = include_str!("../tests/fixtures/modify.diff");
    const ADD: &str = include_str!("../tests/fixtures/add.diff");
    const DELETE: &str = include_str!("../tests/fixtures/delete.diff");
    const RENAME: &str = include_str!("../tests/fixtures/rename.diff");
    const BINARY: &str = include_str!("../tests/fixtures/binary.diff");
    const BINARY_PATCH: &str = include_str!("../tests/fixtures/binary_patch.diff");
    const NO_NEWLINE: &str = include_str!("../tests/fixtures/no_newline.diff");
    const MULTI_HUNK: &str = include_str!("../tests/fixtures/multi_hunk.diff");
    const UNIFIED0: &str = include_str!("../tests/fixtures/unified0.diff");
    const QUOTED: &str = include_str!("../tests/fixtures/quoted.diff");
    const DASHES: &str = include_str!("../tests/fixtures/dashes.diff");
    const MULTI_FILE: &str = include_str!("../tests/fixtures/multi_file.diff");
    const PLAIN_U: &str = include_str!("../tests/fixtures/plain_u.diff");

    fn one(text: &str) -> FileChange {
        let mut set = parse_unified(text).unwrap();
        assert_eq!(set.files.len(), 1, "expected one file");
        set.files.remove(0)
    }

    /// (kind, text) for every line of every hunk, in order.
    fn shape(f: &FileChange) -> Vec<(LineKind, &str)> {
        f.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .map(|l| (l.kind, l.text.as_str()))
            .collect()
    }

    #[test]
    fn modify_is_one_hunk_with_numbered_lines() {
        let f = one(MODIFY);
        assert_eq!(f.path, "a.txt");
        assert_eq!(f.old_path, None);
        assert_eq!(f.status, Status::Modified);
        assert!(!f.binary);
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 3, 1, 4)
        );
        assert_eq!(h.header, "");
        assert_eq!(
            shape(&f),
            [
                (LineKind::Context, "alpha"),
                (LineKind::Remove, "beta"),
                (LineKind::Add, "BETA"),
                (LineKind::Context, "gamma"),
                (LineKind::Add, "delta"),
            ]
        );
        let lines = &h.lines;
        assert_eq!((lines[0].old_no, lines[0].new_no), (Some(1), Some(1)));
        assert_eq!((lines[1].old_no, lines[1].new_no), (Some(2), None));
        assert_eq!((lines[2].old_no, lines[2].new_no), (None, Some(2)));
        assert_eq!((lines[3].old_no, lines[3].new_no), (Some(3), Some(3)));
        assert_eq!((lines[4].old_no, lines[4].new_no), (None, Some(4)));
    }

    #[test]
    fn add_and_delete_have_no_opposite_side() {
        // An added file has no old side: every line is an add with no old number.
        {
            let f = one(ADD);
            assert_eq!(f.path, "added.txt");
            assert_eq!(f.status, Status::Added);
            assert_eq!(f.old_path, None);
            assert_eq!(f.hunks.len(), 1);
            assert_eq!((f.hunks[0].old_start, f.hunks[0].old_lines), (0, 0));
            assert_eq!((f.hunks[0].new_start, f.hunks[0].new_lines), (1, 2));
            assert_eq!(
                shape(&f),
                [(LineKind::Add, "new one"), (LineKind::Add, "new two")]
            );
            assert!(f.hunks[0].lines.iter().all(|l| l.old_no.is_none()));
        }
        // A deleted file has no new side: every line is a remove with no new number.
        {
            let f = one(DELETE);
            assert_eq!(f.path, "deleted.txt");
            assert_eq!(f.status, Status::Deleted);
            assert_eq!(f.old_path, None);
            assert_eq!((f.hunks[0].new_start, f.hunks[0].new_lines), (0, 0));
            assert_eq!(
                shape(&f),
                [
                    (LineKind::Remove, "gone one"),
                    (LineKind::Remove, "gone two")
                ]
            );
            assert!(f.hunks[0].lines.iter().all(|l| l.new_no.is_none()));
        }
    }

    #[test]
    fn rename_keeps_both_names_and_its_hunk() {
        let f = one(RENAME);
        assert_eq!(f.status, Status::Renamed);
        assert_eq!(f.path, "renamed.txt");
        assert_eq!(f.old_path.as_deref(), Some("old.txt"));
        assert!(!f.binary);
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(
            shape(&f),
            [
                (LineKind::Context, "line 1"),
                (LineKind::Context, "line 2"),
                (LineKind::Context, "line 3"),
                (LineKind::Remove, "line 4"),
                (LineKind::Add, "line 4 changed"),
                (LineKind::Context, "line 5"),
                (LineKind::Context, "line 6"),
                (LineKind::Context, "line 7"),
            ]
        );
    }

    #[test]
    fn binary_diffs_have_no_hunks() {
        // A binary file change is flagged binary and has no hunks.
        {
            let f = one(BINARY);
            assert_eq!(f.path, "bin.dat");
            assert_eq!(f.status, Status::Modified);
            assert!(f.binary);
            assert!(f.hunks.is_empty());
        }
        // A binary patch payload is skipped: still binary, still no hunks.
        {
            let f = one(BINARY_PATCH);
            assert_eq!(f.path, "bin.dat");
            assert!(f.binary);
            assert!(f.hunks.is_empty());
        }
    }

    #[test]
    fn no_newline_marker_belongs_to_the_line_before_it() {
        let f = one(NO_NEWLINE);
        assert_eq!(f.hunks.len(), 1);
        let kinds: Vec<LineKind> = f.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Context,
                LineKind::Remove,
                LineKind::NoNewline,
                LineKind::Add,
                LineKind::NoNewline,
            ]
        );
        let lines = &f.hunks[0].lines;
        assert_eq!(lines[1].text, "last without newline");
        assert_eq!((lines[2].old_no, lines[2].new_no), (None, None));
        assert_eq!(lines[3].text, "last changed without newline");
        assert_eq!(lines[4].text, "No newline at end of file");
    }

    #[test]
    fn multi_hunk_numbers_each_hunk_from_its_own_header() {
        let f = one(MULTI_HUNK);
        assert_eq!(f.hunks.len(), 2);
        let (h1, h2) = (&f.hunks[0], &f.hunks[1]);
        assert_eq!(
            (h1.old_start, h1.old_lines, h1.new_start, h1.new_lines),
            (1, 6, 1, 6)
        );
        assert_eq!(h1.header, "");
        assert_eq!(
            (h2.old_start, h2.old_lines, h2.new_start, h2.new_lines),
            (24, 7, 24, 7)
        );
        assert_eq!(h2.header, "row 23");
        let removed: Vec<&str> = f
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == LineKind::Remove)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(removed, ["row 3", "row 27"]);
        let last = h2.lines.last().unwrap();
        assert_eq!((last.old_no, last.new_no), (Some(30), Some(30)));
    }

    #[test]
    fn zero_context_hunks_keep_their_counts() {
        // A zero-context diff omits the context counts; its hunks keep their own numbers.
        {
            let set = parse_unified(UNIFIED0).unwrap();
            assert_eq!(set.files.len(), 2);
            let long = &set.files[0];
            assert_eq!(long.hunks.len(), 2);
            assert_eq!((long.hunks[0].old_start, long.hunks[0].old_lines), (3, 1));
            assert_eq!((long.hunks[0].new_start, long.hunks[0].new_lines), (3, 1));
            assert_eq!(long.hunks[0].header, "row 2");
            assert_eq!(long.hunks[0].lines.len(), 2);
            let one_line = &set.files[1];
            assert_eq!(one_line.path, "one.txt");
            assert_eq!(one_line.hunks[0].lines.len(), 2);
            assert_eq!(
                (one_line.hunks[0].old_lines, one_line.hunks[0].new_lines),
                (1, 1)
            );
        }
        // A pure insert with zero old lines has no context and starts at its own new line.
        {
            let f = one("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,0 +4 @@\n+added\n");
            assert_eq!((f.hunks[0].old_start, f.hunks[0].old_lines), (3, 0));
            assert_eq!((f.hunks[0].new_start, f.hunks[0].new_lines), (4, 1));
            assert_eq!(f.hunks[0].lines[0].new_no, Some(4));
        }
    }

    #[test]
    fn quoted_utf8_path_is_decoded() {
        let f = one(QUOTED);
        assert_eq!(f.path, "café.txt");
        assert_eq!(f.status, Status::Modified);
        assert_eq!(
            shape(&f),
            [(LineKind::Remove, "café"), (LineKind::Add, "café changed")]
        );
    }

    #[test]
    fn removed_lines_that_look_like_headers_stay_in_the_hunk() {
        let f = one(DASHES);
        assert_eq!(f.path, "dashes.txt");
        assert_eq!(
            shape(&f),
            [
                (LineKind::Remove, "-- comment one"),
                (LineKind::Context, "keep"),
                (LineKind::Remove, "-- comment two"),
                (LineKind::Context, "end"),
            ]
        );
    }

    #[test]
    fn multi_file_keeps_order_and_each_status() {
        let set = parse_unified(MULTI_FILE).unwrap();
        let got: Vec<(&str, Status, bool)> = set
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status, f.binary))
            .collect();
        assert_eq!(
            got,
            [
                ("a.txt", Status::Modified, false),
                ("added.txt", Status::Added, false),
                ("bin.dat", Status::Modified, true),
                ("café.txt", Status::Modified, false),
                ("dashes.txt", Status::Modified, false),
                ("deleted.txt", Status::Deleted, false),
                ("long.txt", Status::Modified, false),
                ("nonl.txt", Status::Modified, false),
                ("one.txt", Status::Modified, false),
                ("renamed.txt", Status::Renamed, false),
                ("with space.txt", Status::Modified, false),
            ]
        );
        assert_eq!(set.files[9].old_path.as_deref(), Some("old.txt"));
        assert_eq!(set.files.iter().filter(|f| f.old_path.is_some()).count(), 1);
    }

    #[test]
    fn stats_count_lines_on_each_side() {
        for (case, input, want) in [
            (
                "a multi-file diff",
                MULTI_FILE,
                Stats {
                    files: 11,
                    additions: 11,
                    deletions: 12,
                },
            ),
            (
                "a single modified file",
                MODIFY,
                Stats {
                    files: 1,
                    additions: 2,
                    deletions: 1,
                },
            ),
            (
                "an empty diff",
                "",
                Stats {
                    files: 0,
                    additions: 0,
                    deletions: 0,
                },
            ),
        ] {
            assert_eq!(parse_unified(input).unwrap().stats(), want, "{case}");
        }
    }

    #[test]
    fn plain_diff_u_without_a_git_header() {
        let set = parse_unified(PLAIN_U).unwrap();
        assert_eq!(set.files.len(), 1);
        let f = &set.files[0];
        assert_eq!(f.path, "new.txt");
        assert_eq!(f.old_path, None);
        assert_eq!(f.status, Status::Modified);
        assert_eq!(
            shape(f),
            [
                (LineKind::Context, "x"),
                (LineKind::Remove, "y"),
                (LineKind::Add, "Y"),
                (LineKind::Context, "z"),
                (LineKind::Add, "w")
            ]
        );
    }

    #[test]
    fn text_before_the_first_file_is_skipped() {
        let text = format!(
            "From 1a2b Mon Sep 17 00:00:00 2001\nSubject: [PATCH] x\n---\n a.txt | 2 +-\n 1 file changed\n\n{MODIFY}\n-- \n2.43.0\n"
        );
        let set = parse_unified(&text).unwrap();
        assert_eq!(set.files.len(), 1);
        assert_eq!(set.files[0].path, "a.txt");
        assert_eq!(set.files[0].hunks[0].lines.len(), 5);
    }

    #[test]
    fn header_only_diffs_have_no_hunks() {
        for (case, input, path, status) in [
            (
                "a file mode change",
                "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n",
                "run.sh",
                Status::Modified,
            ),
            (
                "an empty new file",
                "diff --git a/empty.txt b/empty.txt\nnew file mode 100644\nindex 0000000..e69de29\n",
                "empty.txt",
                Status::Added,
            ),
        ] {
            let f = one(input);
            assert_eq!(f.path, path, "{case}");
            assert_eq!(f.status, status, "{case}");
            assert!(f.hunks.is_empty(), "{case}");
        }
    }

    #[test]
    fn renames_and_copies_name_both_sides_and_have_no_hunks() {
        // (case, header, path, old path, status): the names come from the rename/copy lines.
        let cases = [
            (
                "pure rename",
                "diff --git a/x.txt b/y.txt\nsimilarity index 100%\nrename from x.txt\nrename to y.txt\n",
                "y.txt",
                Some("x.txt"),
                Status::Renamed,
            ),
            (
                "rename to a name with a space",
                "diff --git a/a b.txt b/c d.txt\nsimilarity index 100%\nrename from a b.txt\nrename to c d.txt\n",
                "c d.txt",
                Some("a b.txt"),
                Status::Renamed,
            ),
            (
                "quoted rename",
                "diff --git \"a/caf\\303\\251.txt\" \"b/na\\303\\257ve.txt\"\nsimilarity index 100%\nrename from \"caf\\303\\251.txt\"\nrename to \"na\\303\\257ve.txt\"\n",
                "naïve.txt",
                Some("café.txt"),
                Status::Renamed,
            ),
            (
                "copy",
                "diff --git a/x.txt b/y.txt\nsimilarity index 100%\ncopy from x.txt\ncopy to y.txt\n",
                "y.txt",
                Some("x.txt"),
                Status::Added,
            ),
        ];
        for (case, text, path, old_path, status) in cases {
            let f = one(text);
            assert_eq!(f.path, path, "{case}");
            assert_eq!(f.old_path.as_deref(), old_path, "{case}");
            assert_eq!(f.status, status, "{case}");
            assert!(f.hunks.is_empty(), "{case}");
        }
    }

    #[test]
    fn a_blank_line_in_a_hunk_is_an_empty_context_line() {
        let f = one("--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n\n-c\n+d\n");
        assert_eq!(
            shape(&f),
            [
                (LineKind::Context, "a"),
                (LineKind::Context, ""),
                (LineKind::Remove, "c"),
                (LineKind::Add, "d")
            ]
        );
    }

    #[test]
    fn malformed_unified_diffs_are_errors() {
        // (case, input, expected error line if pinned, expected message fragment if pinned)
        for (case, input, line, msg) in [
            (
                "a truncated hunk",
                "--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n-b\n",
                None,
                Some("before its header"),
            ),
            (
                // The header said one line each side; the '+c' after the hunk closed is outside it.
                "a hunk with extra lines",
                "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+c\n",
                Some(6),
                Some("outside a hunk"),
            ),
            (
                "a hunk line without a prefix",
                "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\nfoo\n",
                Some(5),
                Some("must start"),
            ),
            (
                "a context line that needs both sides counted",
                "--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n a\n",
                None,
                Some("more lines"),
            ),
            (
                "a hunk before any file",
                "@@ -1 +1 @@\n-a\n+b\n",
                Some(1),
                None,
            ),
            (
                "a plus line without a minus line",
                "+++ b/x\n",
                None,
                Some("---"),
            ),
        ] {
            let e = parse_unified(input).unwrap_err();
            if let Some(line) = line {
                assert_eq!(e.line, line, "{case}");
            }
            if let Some(msg) = msg {
                assert!(e.message.contains(msg), "{case}: {e}");
            }
        }
        // Each malformed hunk header is reported as an error that mentions "@@".
        for h in ["@@ x @@", "@@ -1 @@", "@@ -1,z +1 @@", "@@ -a +1 @@"] {
            let e = parse_unified(&format!("--- a/x\n+++ b/x\n{h}\n")).unwrap_err();
            assert!(e.message.contains("@@"), "{h}: {e}");
        }
    }

    #[test]
    fn change_set_round_trips_through_json() {
        let set = parse_unified(NO_NEWLINE).unwrap();
        let v = serde_json::to_value(&set).unwrap();
        assert_eq!(v["files"][0]["status"], "modified");
        assert_eq!(v["files"][0]["hunks"][0]["lines"][2]["kind"], "no_newline");
        let back: ChangeSet = serde_json::from_value(v).unwrap();
        assert_eq!(back, set);
    }

    /// Applies a one-file diff to `old` and returns what it makes. Each side's newline status is checked too.
    fn apply(old: &str, f: &FileChange) -> String {
        let old_rows = rows(old);
        let mut out: Vec<(String, bool)> = Vec::new();
        let mut cur = 0;
        for h in &f.hunks {
            let begin = if h.old_lines == 0 {
                h.old_start as usize
            } else {
                h.old_start as usize - 1
            };
            assert!(begin >= cur, "hunks out of order");
            out.extend(
                old_rows[cur..begin]
                    .iter()
                    .map(|&(t, nl)| (t.to_string(), nl)),
            );
            cur = begin;
            for (i, line) in h.lines.iter().enumerate() {
                if line.kind == LineKind::NoNewline {
                    continue;
                }
                let nl =
                    !matches!(h.lines.get(i + 1), Some(next) if next.kind == LineKind::NoNewline);
                match line.kind {
                    LineKind::Context | LineKind::Remove => {
                        assert_eq!(
                            old_rows[cur],
                            (line.text.as_str(), nl),
                            "old side, row {cur}"
                        );
                        cur += 1;
                        if line.kind == LineKind::Context {
                            out.push((line.text.clone(), nl));
                        }
                    }
                    LineKind::Add => out.push((line.text.clone(), nl)),
                    LineKind::NoNewline => {}
                }
            }
        }
        out.extend(old_rows[cur..].iter().map(|&(t, nl)| (t.to_string(), nl)));
        out.iter()
            .map(|(t, nl)| if *nl { format!("{t}\n") } else { t.clone() })
            .collect()
    }

    /// A small deterministic generator, so the random cases repeat.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % n
        }

        fn word(&mut self) -> &'static str {
            WORDS[self.below(WORDS.len() as u64) as usize]
        }
    }

    /// Lines that repeat, so the diffs have matches; some look like diff syntax, which a hunk must carry as text.
    const WORDS: &[&str] = &[
        "a", "b", "c", "", "x y", "-- dash", "--- z", "+p", "\\ slash", "café", "tab\tin",
    ];

    fn join(lines: &[&str], final_newline: bool) -> String {
        let mut s = lines.join("\n");
        if final_newline && !lines.is_empty() {
            s.push('\n');
        }
        s
    }

    fn random_file(rng: &mut Rng, max: u64) -> String {
        let n = rng.below(max + 1);
        let lines: Vec<&str> = (0..n).map(|_| rng.word()).collect();
        join(&lines, rng.below(4) != 0)
    }

    /// `old` with a few lines inserted, removed or replaced, and a final newline that may have come or gone.
    fn mutate(rng: &mut Rng, old: &str) -> String {
        let mut lines: Vec<String> = rows(old).iter().map(|(t, _)| t.to_string()).collect();
        for _ in 0..rng.below(5) {
            let at = rng.below(lines.len() as u64 + 1) as usize;
            match rng.below(3) {
                0 => lines.insert(at, rng.word().to_string()),
                1 if at < lines.len() => {
                    lines.remove(at);
                }
                _ if at < lines.len() => lines[at] = rng.word().to_string(),
                _ => {}
            }
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        join(&refs, rng.below(4) != 0)
    }

    #[test]
    fn a_small_change_is_written_as_git_writes_it() {
        let text = write_unified(Some("f"), Some("f"), "a\nb\nc\n", "a\nB\nc\n", 1);
        assert_eq!(
            text,
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
        );
    }

    #[test]
    fn a_missing_final_newline_is_marked_on_the_line_before_it() {
        let text = write_unified(Some("f"), Some("f"), "a", "a\n", 0);
        assert_eq!(
            text,
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+a\n"
        );
        let f = one(&text);
        assert_eq!(apply("a", &f), "a\n");
    }

    #[test]
    fn an_added_file_has_no_old_side_and_a_deleted_one_no_new() {
        let add = write_unified(None, Some("n.txt"), "", "x\n", 3);
        assert_eq!(
            add,
            "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+x\n"
        );
        let del = write_unified(Some("d.txt"), None, "x\n", "", 3);
        assert_eq!(
            del,
            "diff --git a/d.txt b/d.txt\ndeleted file mode 100644\n--- a/d.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n"
        );
        assert_eq!(one(&add).status, Status::Added);
        assert_eq!(one(&del).status, Status::Deleted);
    }

    #[test]
    fn an_empty_new_file_is_a_header_with_no_hunks() {
        let text = write_unified(None, Some("e.txt"), "", "", 3);
        assert_eq!(text, "diff --git a/e.txt b/e.txt\nnew file mode 100644\n");
        assert_eq!(one(&text).status, Status::Added);
    }

    #[test]
    fn a_rename_writes_both_names_and_its_changes() {
        let text = write_unified(Some("old.txt"), Some("new.txt"), "a\nb\n", "a\nc\n", 3);
        let f = one(&text);
        assert_eq!(
            (f.status, f.path.as_str(), f.old_path.as_deref()),
            (Status::Renamed, "new.txt", Some("old.txt"))
        );
        assert_eq!(apply("a\nb\n", &f), "a\nc\n");
        // A rename with no change to the text has no hunks.
        let pure = write_unified(Some("old.txt"), Some("new.txt"), "a\n", "a\n", 3);
        let g = one(&pure);
        assert_eq!(
            (g.status, g.old_path.as_deref(), g.hunks.len()),
            (Status::Renamed, Some("old.txt"), 0)
        );
    }

    #[test]
    fn a_binary_change_has_the_header_and_no_lines() {
        let f = one(&write_binary(Some("bin.dat"), Some("bin.dat")));
        assert!(f.binary);
        assert_eq!(
            (f.path.as_str(), f.status, f.hunks.len()),
            ("bin.dat", Status::Modified, 0)
        );
        let added = one(&write_binary(None, Some("new.bin")));
        assert!(added.binary);
        assert_eq!(added.status, Status::Added);
        assert_eq!(
            write_binary(None, Some("new.bin")).lines().last(),
            Some("Binary files /dev/null and b/new.bin differ")
        );
    }

    #[test]
    fn names_git_would_quote_are_quoted_and_read_back() {
        let name = "dir/caf\u{e9} \"x\"\ttab.txt";
        let text = write_unified(Some(name), Some(name), "a\n", "b\n", 0);
        assert!(
            text.starts_with("diff --git \"a/dir/caf\\303\\251"),
            "{text}"
        );
        let f = one(&text);
        assert_eq!(f.path, name);
        assert_eq!(apply("a\n", &f), "b\n");
    }

    #[test]
    fn written_diffs_apply_back_to_their_new_side() {
        let mut rng = Rng(7);
        for case in 0..500 {
            let old = random_file(&mut rng, 40);
            let new = if case % 5 == 0 {
                random_file(&mut rng, 40)
            } else {
                mutate(&mut rng, &old)
            };
            for context in [0, 1, 3] {
                let text = write_unified(Some("f.txt"), Some("f.txt"), &old, &new, context);
                if old == new {
                    assert!(
                        text.is_empty(),
                        "case {case}: an unchanged file writes nothing"
                    );
                    continue;
                }
                let f = one(&text);
                assert_eq!(
                    (f.path.as_str(), f.status),
                    ("f.txt", Status::Modified),
                    "case {case}"
                );
                assert_eq!(
                    apply(&old, &f),
                    new,
                    "case {case} context {context}\n{text}"
                );
            }
        }
    }

    #[test]
    fn added_deleted_and_renamed_files_round_trip() {
        let mut rng = Rng(99);
        for _ in 0..200 {
            let text = random_file(&mut rng, 25);
            let add = one(&write_unified(None, Some("n.txt"), "", &text, 2));
            assert_eq!((add.path.as_str(), add.status), ("n.txt", Status::Added));
            assert_eq!(apply("", &add), text);
            let del = one(&write_unified(Some("d.txt"), None, &text, "", 2));
            assert_eq!((del.path.as_str(), del.status), ("d.txt", Status::Deleted));
            assert_eq!(apply(&text, &del), "");
            let other = mutate(&mut rng, &text);
            let ren = one(&write_unified(
                Some("old.txt"),
                Some("new.txt"),
                &text,
                &other,
                3,
            ));
            assert_eq!(
                (ren.path.as_str(), ren.old_path.as_deref(), ren.status),
                ("new.txt", Some("old.txt"), Status::Renamed)
            );
            assert_eq!(apply(&text, &ren), other);
        }
    }

    #[test]
    fn oversized_writes_are_one_replaced_block_that_still_applies() {
        // A file past the line bound is written as one replaced block.
        {
            let old: String = (0..=MAX_WRITE_LINES)
                .map(|i| format!("line {i}\n"))
                .collect();
            let new = old.replacen("line 5\n", "changed\n", 1);
            let f = one(&write_unified(
                Some("big.txt"),
                Some("big.txt"),
                &old,
                &new,
                3,
            ));
            assert_eq!(f.hunks.len(), 1);
            assert_eq!(apply(&old, &f), new);
        }
        // Too many edits for the search is also one replaced block that still applies.
        {
            let old: String = (0..1_500).map(|i| format!("a{i}\n")).collect();
            let new: String = (0..1_500).map(|i| format!("b{i}\n")).collect();
            let f = one(&write_unified(Some("x.txt"), Some("x.txt"), &old, &new, 3));
            assert_eq!(f.hunks.len(), 1);
            assert_eq!(apply(&old, &f), new);
        }
    }
}
