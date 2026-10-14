//! Unified diffs into hunks. The hub diffs; this turns the text into a [`ChangeSet`]
//! that both apps draw the same way. Reads `git diff` output (with or without
//! `--binary` and `-M`) and `diff -u`. Paths come back as the diff names them, with
//! the `a/` and `b/` prefixes removed; run them through [`crate::path::check_path`]
//! before use.

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
    const SPACES: &str = include_str!("../tests/fixtures/spaces.diff");
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
    fn add_has_no_old_side() {
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

    #[test]
    fn delete_has_no_new_side() {
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
    fn binary_file_has_no_hunks() {
        let f = one(BINARY);
        assert_eq!(f.path, "bin.dat");
        assert_eq!(f.status, Status::Modified);
        assert!(f.binary);
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn binary_patch_payload_is_skipped() {
        let f = one(BINARY_PATCH);
        assert_eq!(f.path, "bin.dat");
        assert!(f.binary);
        assert!(f.hunks.is_empty());
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
    fn zero_context_diff_has_omitted_counts() {
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

    #[test]
    fn a_pure_insert_with_zero_old_lines_has_no_context() {
        let f = one("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,0 +4 @@\n+added\n");
        assert_eq!((f.hunks[0].old_start, f.hunks[0].old_lines), (3, 0));
        assert_eq!((f.hunks[0].new_start, f.hunks[0].new_lines), (4, 1));
        assert_eq!(f.hunks[0].lines[0].new_no, Some(4));
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
    fn name_with_a_space_has_no_trailing_tab() {
        let f = one(SPACES);
        assert_eq!(f.path, "with space.txt");
        assert_eq!(f.status, Status::Modified);
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
    fn multi_file_stats_count_lines_on_each_side() {
        let set = parse_unified(MULTI_FILE).unwrap();
        assert_eq!(
            set.stats(),
            Stats {
                files: 11,
                additions: 11,
                deletions: 12
            }
        );
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
    fn stats_on_a_single_file() {
        let set = parse_unified(MODIFY).unwrap();
        assert_eq!(
            set.stats(),
            Stats {
                files: 1,
                additions: 2,
                deletions: 1
            }
        );
        assert_eq!(
            parse_unified("").unwrap().stats(),
            Stats {
                files: 0,
                additions: 0,
                deletions: 0
            }
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
    fn empty_input_has_no_files() {
        assert!(parse_unified("").unwrap().files.is_empty());
        assert!(parse_unified("\n\n").unwrap().files.is_empty());
    }

    #[test]
    fn a_file_mode_change_has_no_hunks() {
        let f = one("diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
        assert_eq!(f.path, "run.sh");
        assert_eq!(f.status, Status::Modified);
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn an_empty_new_file_is_added_without_hunks() {
        let f = one(
            "diff --git a/empty.txt b/empty.txt\nnew file mode 100644\nindex 0000000..e69de29\n",
        );
        assert_eq!(f.path, "empty.txt");
        assert_eq!(f.status, Status::Added);
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn a_pure_rename_has_no_hunks() {
        let f = one(
            "diff --git a/x.txt b/y.txt\nsimilarity index 100%\nrename from x.txt\nrename to y.txt\n",
        );
        assert_eq!(f.path, "y.txt");
        assert_eq!(f.old_path.as_deref(), Some("x.txt"));
        assert_eq!(f.status, Status::Renamed);
    }

    #[test]
    fn a_rename_to_a_name_with_a_space_uses_the_rename_lines() {
        let f = one(
            "diff --git a/a b.txt b/c d.txt\nsimilarity index 100%\nrename from a b.txt\nrename to c d.txt\n",
        );
        assert_eq!(f.path, "c d.txt");
        assert_eq!(f.old_path.as_deref(), Some("a b.txt"));
    }

    #[test]
    fn a_quoted_rename_is_decoded() {
        let f = one(
            "diff --git \"a/caf\\303\\251.txt\" \"b/na\\303\\257ve.txt\"\nsimilarity index 100%\nrename from \"caf\\303\\251.txt\"\nrename to \"na\\303\\257ve.txt\"\n",
        );
        assert_eq!(f.path, "naïve.txt");
        assert_eq!(f.old_path.as_deref(), Some("café.txt"));
    }

    #[test]
    fn a_copy_is_an_added_file_with_its_source() {
        let f = one(
            "diff --git a/x.txt b/y.txt\nsimilarity index 100%\ncopy from x.txt\ncopy to y.txt\n",
        );
        assert_eq!(f.status, Status::Added);
        assert_eq!(f.path, "y.txt");
        assert_eq!(f.old_path.as_deref(), Some("x.txt"));
    }

    #[test]
    fn a_plain_hunk_with_one_line_each_side_parses() {
        let f = one("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(f.path, "x");
        assert_eq!(shape(&f), [(LineKind::Remove, "a"), (LineKind::Add, "b")]);
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
    fn a_hunk_header_with_a_section_title_keeps_it() {
        let f = one("--- a/x\n+++ b/x\n@@ -10,2 +10,2 @@ fn main() {\n-a\n+b\n c\n");
        assert_eq!(f.hunks[0].header, "fn main() {");
        assert_eq!((f.hunks[0].old_start, f.hunks[0].old_lines), (10, 2));
    }

    #[test]
    fn a_truncated_hunk_is_an_error() {
        let e = parse_unified("--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n-b\n").unwrap_err();
        assert!(e.message.contains("before its header"), "{e}");
    }

    #[test]
    fn a_hunk_with_extra_lines_is_an_error() {
        // The header said one line each side; the '+c' after the hunk closed is outside it.
        let e = parse_unified("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+c\n").unwrap_err();
        assert_eq!(e.line, 6);
        assert!(e.message.contains("outside a hunk"), "{e}");
    }

    #[test]
    fn a_hunk_line_without_a_prefix_is_an_error() {
        let e = parse_unified("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\nfoo\n").unwrap_err();
        assert_eq!(e.line, 5);
        assert!(e.message.contains("must start"), "{e}");
    }

    #[test]
    fn a_context_line_needs_both_sides_counted() {
        let e = parse_unified("--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n a\n").unwrap_err();
        assert!(
            e.message.contains("more lines") || e.message.contains("before"),
            "{e}"
        );
    }

    #[test]
    fn a_malformed_hunk_header_is_an_error() {
        for h in ["@@ x @@", "@@ -1 @@", "@@ -1,z +1 @@", "@@ -a +1 @@"] {
            let e = parse_unified(&format!("--- a/x\n+++ b/x\n{h}\n")).unwrap_err();
            assert!(e.message.contains("@@"), "{h}: {e}");
        }
    }

    #[test]
    fn a_hunk_before_any_file_is_an_error() {
        let e = parse_unified("@@ -1 +1 @@\n-a\n+b\n").unwrap_err();
        assert_eq!(e.line, 1);
    }

    #[test]
    fn a_plus_line_without_minus_is_an_error() {
        let e = parse_unified("+++ b/x\n").unwrap_err();
        assert!(e.message.contains("---"), "{e}");
    }

    #[test]
    fn a_stray_line_after_a_finished_hunk_is_an_error() {
        let e = parse_unified("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+stray\n").unwrap_err();
        assert!(e.message.contains("outside a hunk"), "{e}");
    }

    #[test]
    fn a_diff_error_shows_its_line() {
        let e = DiffError {
            line: 7,
            message: "bad".into(),
        };
        assert_eq!(e.to_string(), "diff line 7: bad");
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
}
