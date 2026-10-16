//! The SKILL.md frontmatter parser (a YAML subset, see `crate::parse`).
//!
//! Accepted: `key: value` with a plain, "double" or 'single' quoted value (a `#` after a space starts a comment);
//! `key: >` or `key: |` block scalars (folded or literal, the indented lines below); `key:` followed by `  - item`
//! lines (a list) or `  sub: value` lines (a one-level map); blank lines and `#` comment lines. Anything else is an
//! error, with the 1-based SKILL.md line.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Frontmatter, Parsed};

type Res<T> = Result<T, (u32, String)>;

/// (1-based SKILL.md line, the line without its line break).
type Line<'a> = (u32, &'a str);

/// What follows a `key:`.
enum Value {
    Scalar(String),
    List(Vec<String>),
    Map(BTreeMap<String, String>),
}

pub fn parse(skill_md: &str) -> Res<Parsed> {
    let raws: Vec<&str> = skill_md.split_inclusive('\n').collect();
    if raws.first().map(|r| text(r)) != Some("---") {
        return Err((1, "SKILL.md must open with a --- line".into()));
    }
    let close = (1..raws.len())
        .find(|&i| text(raws[i]) == "---")
        .ok_or((1u32, "the frontmatter has no closing --- line".to_string()))?;
    let lines: Vec<Line<'_>> = (1..close)
        .map(|i| ((i + 1) as u32, text(raws[i])))
        .collect();
    for &(no, t) in &lines {
        if t.chars()
            .take_while(|c| c.is_whitespace())
            .any(|c| c == '\t')
        {
            return Err((no, "tabs are not allowed for indentation".into()));
        }
    }

    let mut fm = Frontmatter::default();
    let mut seen = BTreeSet::new();
    let mut i = 0;
    while i < lines.len() {
        let (no, t) = lines[i];
        if t.trim().is_empty() || t.trim_start().starts_with('#') {
            i += 1;
            continue;
        }
        if t.starts_with(' ') {
            return Err((no, "indented line with no key above it".into()));
        }
        let (key, rest) = split_key(no, t)?;
        if !seen.insert(key) {
            return Err((no, format!("duplicate key `{key}`")));
        }
        let rest = rest.trim();
        let (value, next) = if rest.is_empty() {
            child_block(&lines, i + 1)?
        } else if let Some(folded) = block_style(rest) {
            let (s, next) = block_scalar(&lines, i + 1, folded)?;
            (Value::Scalar(s), next)
        } else {
            (Value::Scalar(scalar(no, rest)?), i + 1)
        };
        assign(&mut fm, no, key, value)?;
        i = next;
    }

    Ok(Parsed {
        frontmatter: fm,
        body: raws[close + 1..].concat(),
        body_line: close as u32 + 2,
    })
}

/// A line without its `\n` or `\r\n`.
fn text(raw: &str) -> &str {
    let t = raw.strip_suffix('\n').unwrap_or(raw);
    t.strip_suffix('\r').unwrap_or(t)
}

fn indent(t: &str) -> usize {
    t.len() - t.trim_start_matches(' ').len()
}

/// `key: rest` into the key and the text after the colon. The key is a letter, digit, `-`, `_` or `.` run.
fn split_key(no: u32, t: &str) -> Res<(&str, &str)> {
    let (key, rest) = t
        .split_once(':')
        .ok_or((no, "expected a `key: value` line".to_string()))?;
    let valid = !key.is_empty()
        && !key.starts_with('-')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid {
        return Err((no, format!("`{key}` is not a key")));
    }
    if !rest.is_empty() && !rest.starts_with(' ') {
        return Err((no, "a space must follow the colon".into()));
    }
    Ok((key, rest))
}

/// The lines under a `key:` with nothing after it: blank, comment or indented lines. Empty means an empty string.
fn child_block(lines: &[Line<'_>], from: usize) -> Res<(Value, usize)> {
    let mut end = from;
    while end < lines.len() && (lines[end].1.trim().is_empty() || lines[end].1.starts_with(' ')) {
        end += 1;
    }
    let children: Vec<Line<'_>> = lines[from..end]
        .iter()
        .copied()
        .filter(|(_, t)| !t.trim().is_empty() && !t.trim_start().starts_with('#'))
        .collect();
    let Some(&(_, first)) = children.first() else {
        return Ok((Value::Scalar(String::new()), end));
    };
    let base = indent(first);
    if let Some(&(no, _)) = children.iter().find(|(_, t)| indent(t) != base) {
        return Err((no, "inconsistent indentation".into()));
    }
    if first.trim_start().starts_with('-') {
        let mut items = Vec::new();
        for &(no, t) in &children {
            let body = t.trim_start();
            let item = if body == "-" {
                ""
            } else if let Some(v) = body.strip_prefix("- ") {
                v
            } else {
                return Err((no, "expected a `- item` line".into()));
            };
            items.push(scalar(no, item.trim())?);
        }
        Ok((Value::List(items), end))
    } else {
        let mut map = BTreeMap::new();
        for &(no, t) in &children {
            let (key, rest) = split_key(no, t.trim_start())?;
            let rest = rest.trim();
            if block_style(rest).is_some() {
                return Err((no, "block scalars are only allowed at the top level".into()));
            }
            if map.insert(key.to_string(), scalar(no, rest)?).is_some() {
                return Err((no, format!("duplicate key `{key}`")));
            }
        }
        Ok((Value::Map(map), end))
    }
}

/// `>` (folded, true) or `|` (literal, false), with an optional chomping mark that is ignored.
fn block_style(rest: &str) -> Option<bool> {
    match rest {
        ">" | ">-" | ">+" => Some(true),
        "|" | "|-" | "|+" => Some(false),
        _ => None,
    }
}

/// The indented lines under a block indicator. Trailing blank lines are dropped; folded text joins lines with a
/// space and a blank line with a line break.
fn block_scalar(lines: &[Line<'_>], from: usize, folded: bool) -> Res<(String, usize)> {
    let mut end = from;
    while end < lines.len() && (lines[end].1.trim().is_empty() || lines[end].1.starts_with(' ')) {
        end += 1;
    }
    let block: Vec<Line<'_>> = lines[from..end].to_vec();
    let base = block
        .iter()
        .find(|(_, t)| !t.trim().is_empty())
        .map(|(_, t)| indent(t))
        .unwrap_or(0);
    let mut body: Vec<&str> = Vec::new();
    for &(no, t) in &block {
        if t.trim().is_empty() {
            body.push("");
            continue;
        }
        if indent(t) < base {
            return Err((no, "this line is indented less than the block".into()));
        }
        body.push(&t[base..]);
    }
    while body.last() == Some(&"") {
        body.pop();
    }
    let text = if folded {
        let mut out = String::new();
        let mut pending = 0usize;
        for l in body {
            if l.is_empty() {
                pending += 1;
                continue;
            }
            if !out.is_empty() {
                if pending == 0 {
                    out.push(' ');
                } else {
                    out.push_str(&"\n".repeat(pending));
                }
            }
            out.push_str(l);
            pending = 0;
        }
        out
    } else {
        body.join("\n")
    };
    Ok((text, end))
}

/// A one-line value: a quoted string, or a plain one without its trailing comment.
fn scalar(no: u32, s: &str) -> Res<String> {
    match s.chars().next() {
        Some(q @ ('"' | '\'')) => quoted(no, s, q),
        _ => Ok(strip_comment(s).trim().to_string()),
    }
}

/// `"..."` with `\"`, `\\`, `\n`, `\t` escapes, or `'...'` with `''` for a quote. Only a blank or a comment may
/// follow the closing quote.
fn quoted(no: u32, s: &str, quote: char) -> Res<String> {
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((idx, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, other)) => out.push(other),
                None => break,
            }
        } else if c == quote {
            if quote == '\'' && matches!(chars.clone().next(), Some((_, '\''))) {
                chars.next();
                out.push('\'');
            } else {
                let tail = s[1 + idx + c.len_utf8()..].trim();
                return if tail.is_empty() || tail.starts_with('#') {
                    Ok(out)
                } else {
                    Err((no, "text after the closing quote".into()))
                };
            }
        } else {
            out.push(c);
        }
    }
    Err((no, "the quoted value is not closed on this line".into()))
}

/// The value without a `#` comment: a `#` that starts the value or follows a space or tab.
fn strip_comment(s: &str) -> &str {
    let mut after_space = true;
    for (i, c) in s.char_indices() {
        if c == '#' && after_space {
            return s[..i].trim_end();
        }
        after_space = c == ' ' || c == '\t';
    }
    s
}

/// Stores a key. The known keys take their own shape; any other key keeps its value as a string (a list joined
/// with `, `, a map as `k: v` pairs).
fn assign(fm: &mut Frontmatter, no: u32, key: &str, value: Value) -> Res<()> {
    match key {
        "name" => fm.name = one(no, key, value)?,
        "description" => fm.description = one(no, key, value)?,
        "license" => fm.license = Some(one(no, key, value)?).filter(|s| !s.is_empty()),
        "allowed-tools" => {
            fm.allowed_tools = match value {
                Value::List(items) => items,
                Value::Scalar(s) => s
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect(),
                Value::Map(_) => {
                    return Err((no, "`allowed-tools` takes a list or a string".into()));
                }
            }
        }
        "metadata" => {
            fm.metadata = match value {
                Value::Map(map) => map,
                Value::Scalar(s) if s.is_empty() => BTreeMap::new(),
                _ => return Err((no, "`metadata` takes `key: value` lines under it".into())),
            }
        }
        _ => {
            let raw = match value {
                Value::Scalar(s) => s,
                Value::List(items) => items.join(", "),
                Value::Map(map) => map
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            fm.extra.insert(key.to_string(), raw);
        }
    }
    Ok(())
}

fn one(no: u32, key: &str, value: Value) -> Res<String> {
    match value {
        Value::Scalar(s) => Ok(s),
        _ => Err((no, format!("`{key}` takes a single value"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Parsed {
        parse(s).unwrap_or_else(|e| panic!("should parse: {e:?}"))
    }

    fn err(s: &str) -> (u32, String) {
        parse(s).expect_err("should not parse")
    }

    #[test]
    fn valid_frontmatter_parses() {
        // plain_keys_and_the_body: plain keys parse, and the body starts after the closing line.
        {
            let p = ok(
                "---\nname: pdf-forms\ndescription: Fill forms. Use when: a PDF has fields.\n---\n# Title\nbody\n",
            );
            assert_eq!(p.frontmatter.name, "pdf-forms");
            assert_eq!(
                p.frontmatter.description,
                "Fill forms. Use when: a PDF has fields."
            );
            assert_eq!(p.body, "# Title\nbody\n");
            assert_eq!(p.body_line, 5);
        }
        // quoted_values_unescape: double and single quoted values are unescaped; `#` inside
        // quotes is not a comment.
        {
            let p = ok(
                "---\nname: \"a-b\"\ndescription: \"Say \\\"hi\\\" # not a comment\" # real comment\nlicense: 'it''s MIT'\n---\n",
            );
            assert_eq!(p.frontmatter.name, "a-b");
            assert_eq!(p.frontmatter.description, "Say \"hi\" # not a comment");
            assert_eq!(p.frontmatter.license.as_deref(), Some("it's MIT"));
        }
        // folded_and_literal_block_scalars: `>` folds lines and `|` keeps them.
        {
            let p = ok(
                "---\ndescription: >\n  Fill forms\n  for PDFs.\n\n  Use when needed.\nname: x\n---\n",
            );
            assert_eq!(
                p.frontmatter.description,
                "Fill forms for PDFs.\nUse when needed."
            );
            assert_eq!(p.frontmatter.name, "x");
            let p = ok("---\ndescription: |\n  one\n    indented\n  two\nname: x\n---\n");
            assert_eq!(p.frontmatter.description, "one\n  indented\ntwo");
        }
        // list_and_string_allowed_tools: allowed-tools accepts a list or a comma/space string.
        {
            let p = ok("---\nallowed-tools:\n  - Read\n  - \"Bash(git:*)\"\n  - Grep\n---\n");
            assert_eq!(p.frontmatter.allowed_tools, ["Read", "Bash(git:*)", "Grep"]);
            let p = ok("---\nallowed-tools: Read, Grep  Bash\n---\n");
            assert_eq!(p.frontmatter.allowed_tools, ["Read", "Grep", "Bash"]);
        }
        // metadata_map_and_extra_keys: `metadata` is a map, unknown keys go to `extra`.
        {
            let p = ok(
                "---\nmetadata:\n  author: me\n  version: \"1\" # x\ncompatibility: any tool\n---\n",
            );
            assert_eq!(
                p.frontmatter.metadata.get("author").map(String::as_str),
                Some("me")
            );
            assert_eq!(
                p.frontmatter.metadata.get("version").map(String::as_str),
                Some("1")
            );
            assert_eq!(
                p.frontmatter.extra.get("compatibility").map(String::as_str),
                Some("any tool")
            );
        }
        // comments_and_blank_lines_are_ignored: comment and blank lines do not affect values.
        {
            let p = ok("---\n# top\n\nname: x # trailing\n  # indented comment\n---\n");
            assert_eq!(p.frontmatter.name, "x");
        }
        // the_first_closing_line_ends_the_frontmatter: later `---` lines stay in the body.
        {
            let p = ok("---\nname: x\n---\nbody\n---\nmore\n");
            assert_eq!(p.body, "body\n---\nmore\n");
        }
    }

    #[test]
    fn broken_skill_md_reports_the_line() {
        assert_eq!(err("name: x\n").0, 1);
        assert_eq!(err("---\nname: x\n").0, 1);
        assert_eq!(err("---\nallowed-tools:\n\t- Read\n---\n").0, 3);
        assert_eq!(err("---\nname x\n---\n").0, 2);
        assert_eq!(err("---\n  name: x\n---\n").0, 2);
        assert_eq!(err("---\nname: a\nname: b\n---\n").0, 3);
        assert_eq!(err("---\nname: \"abc\n---\n").0, 2);
        assert_eq!(err("---\nmetadata:\n  a: 1\n    b: 2\n---\n").0, 4);
        assert_eq!(err("---\nmetadata: nope\n---\n").0, 2);
        assert_eq!(err("---\nname:x\n---\n").0, 2);
    }
}
