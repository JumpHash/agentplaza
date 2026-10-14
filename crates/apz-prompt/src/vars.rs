//! `{{variable}}` substitution and slugs.

use std::collections::BTreeMap;

/// The variables a prompt may use. apid fills them per agent when it builds the system prompt.
pub const VARIABLES: [&str; 5] = ["name", "role", "cid", "tier", "plaza"];

/// Every `{{ name }}` in `body` (spaces inside the braces allowed): (1-based line, name).
fn placeholders(body: &str) -> Vec<(u32, &str, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for (i, line) in body.split_inclusive('\n').enumerate() {
        let mut from = 0;
        while let Some(start) = line[from..].find("{{") {
            let open = from + start;
            let Some(len) = line[open + 2..].find("}}") else {
                break;
            };
            let name = line[open + 2..open + 2 + len].trim();
            let close = open + 2 + len + 2;
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                out.push((i as u32 + 1, name, offset + open..offset + close));
            }
            from = close;
        }
        offset += line.len();
    }
    out
}

/// The placeholders whose names are not in `VARIABLES`: (1-based line, name).
pub fn unknown_vars(body: &str) -> Vec<(u32, String)> {
    placeholders(body)
        .into_iter()
        .filter(|(_, n, _)| !VARIABLES.contains(n))
        .map(|(l, n, _)| (l, n.to_string()))
        .collect()
}

/// `body` with each `{{name}}` found in `vars` replaced by its value. Unknown names are left as written.
pub fn render(body: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut last = 0;
    for (_, name, range) in placeholders(body) {
        if let Some(value) = vars.get(name) {
            out.push_str(&body[last..range.start]);
            out.push_str(value);
            last = range.end;
        }
    }
    out.push_str(&body[last..]);
    out
}

/// A prompt id from a title: lower-case ASCII letters and digits joined by single dashes, at most 63 characters,
/// starting with a letter or digit. "prompt" when nothing is left.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_end_matches('-').chars().take(63).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "prompt".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_known_and_keeps_unknown() {
        let vars = BTreeMap::from([("name", "Ada".to_string()), ("cid", "7".to_string())]);
        assert_eq!(
            render("Hi {{name}} ({{ cid }}), {{other}}.", &vars),
            "Hi Ada (7), {{other}}."
        );
    }

    #[test]
    fn unknown_vars_reports_lines() {
        assert_eq!(
            unknown_vars("ok {{role}}\n{{nope}} and {{ x_y }}"),
            vec![(2, "nope".into()), (2, "x_y".into())]
        );
        assert!(unknown_vars("{{ }} {{a b}} {not}").is_empty());
    }

    #[test]
    fn slugs() {
        assert_eq!(
            slug("  The Builder's Prompt! v2 "),
            "the-builder-s-prompt-v2"
        );
        assert_eq!(slug("¿¿"), "prompt");
        assert_eq!(slug(&"a".repeat(80)).len(), 63);
    }
}
