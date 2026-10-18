//! Line diffs: hunks, unified text, and which lines changed.

use serde::Serialize;
use similar::{ChangeTag, TextDiff};

#[derive(Clone, Debug, Serialize)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    /// `(' ' | '+' | '-', text)`, text without its newline.
    pub lines: Vec<(char, String)>,
}

/// Whether bytes look like text (UTF-8 and no NUL in the first 8 KB).
pub fn is_text(bytes: &[u8]) -> bool {
    !bytes[..bytes.len().min(8192)].contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// The hunks turning `old` into `new`, with `context` lines around each change.
pub fn hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    for group in diff.grouped_ops(context) {
        let first = &group[0];
        let last = &group[group.len() - 1];
        let old_start = first.old_range().start;
        let new_start = first.new_range().start;
        let old_lines = last.old_range().end - old_start;
        let new_lines = last.new_range().end - new_start;
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let tag = match change.tag() {
                    ChangeTag::Equal => ' ',
                    ChangeTag::Insert => '+',
                    ChangeTag::Delete => '-',
                };
                lines.push((tag, change.value().trim_end_matches('\n').to_string()));
            }
        }
        out.push(Hunk {
            old_start: old_start + 1,
            old_lines,
            new_start: new_start + 1,
            new_lines,
            lines,
        });
    }
    out
}

/// Lines added and removed.
pub fn counts(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut add = 0;
    let mut del = 0;
    for c in diff.iter_all_changes() {
        match c.tag() {
            ChangeTag::Insert => add += 1,
            ChangeTag::Delete => del += 1,
            ChangeTag::Equal => {}
        }
    }
    (add, del)
}

/// The 1-based lines of `old` that were removed and of `new` that were added.
pub fn changed_lines(old: &str, new: &str) -> (Vec<usize>, Vec<usize>) {
    let diff = TextDiff::from_lines(old, new);
    let mut old_changed = Vec::new();
    let mut new_changed = Vec::new();
    for c in diff.iter_all_changes() {
        match c.tag() {
            ChangeTag::Delete => old_changed.push(c.old_index().unwrap() + 1),
            ChangeTag::Insert => new_changed.push(c.new_index().unwrap() + 1),
            ChangeTag::Equal => {}
        }
    }
    (old_changed, new_changed)
}

/// For each line of `new`, the line of `old` it was kept from (`None` when it is new).
pub fn kept_from(old: &str, new: &str) -> Vec<Option<usize>> {
    let diff = TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    for c in diff.iter_all_changes() {
        match c.tag() {
            ChangeTag::Equal => out.push(c.old_index()),
            ChangeTag::Insert => out.push(None),
            ChangeTag::Delete => {}
        }
    }
    out
}

/// A unified diff for one file.
pub fn unified(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    for h in hunks(old, new, 3) {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        ));
        for (tag, text) in h.lines {
            out.push(tag);
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_lines_and_counts_agree() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\nd\n";
        assert_eq!(counts(old, new), (2, 1));
        assert_eq!(changed_lines(old, new), (vec![2], vec![2, 4]));
        assert_eq!(kept_from(old, new), vec![Some(0), None, Some(2), None]);
        let h = hunks(old, new, 1);
        assert_eq!(h.len(), 1);
        assert!(unified("x", old, new).contains("+B"));
    }
}
