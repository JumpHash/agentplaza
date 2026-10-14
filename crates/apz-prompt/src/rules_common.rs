//! The rules that apply to every kind (docs/design/prompt-library.md, "Rules").

use std::collections::HashSet;

use crate::doc::{Block, Doc};
use crate::{Finding, Kind, Rule, Severity, VARIABLES, unknown_vars};

const SK: &[Kind] = &[Kind::System, Kind::AgentsMd];

pub const RULES: &[Rule] = &[
    Rule {
        id: "empty",
        severity: Severity::Error,
        kinds: &[],
        title: "The prompt is empty",
        why: "An empty prompt gives the model nothing to work from.",
    },
    Rule {
        id: "too-long",
        severity: Severity::Warning,
        kinds: &[],
        title: "Too long",
        why: "Every token of a system prompt is paid on every turn and long prompts dilute attention. Warning above 32,000 characters, error above 200,000.",
    },
    Rule {
        id: "secret",
        severity: Severity::Error,
        kinds: &[],
        title: "Looks like a credential",
        why: "Prompts are shown to models, stored in history and exported. Keys belong in the vault, never in a prompt.",
    },
    Rule {
        id: "unclosed-fence",
        severity: Severity::Error,
        kinds: &[],
        title: "Unclosed code fence",
        why: "An open ``` swallows the rest of the prompt into a code block, so the model reads instructions as code.",
    },
    Rule {
        id: "template-var",
        severity: Severity::Error,
        kinds: &[],
        title: "Unknown template variable",
        why: "Only name, role, cid, tier and plaza are filled in; any other {{variable}} reaches the model as literal braces.",
    },
    Rule {
        id: "heading-first",
        severity: Severity::Warning,
        kinds: SK,
        title: "Start with a heading",
        why: "A title heading says what the document is, for the model and for the people who maintain it.",
    },
    Rule {
        id: "heading-skip",
        severity: Severity::Warning,
        kinds: &[],
        title: "Heading level skipped",
        why: "Going from # to ### breaks the outline that structures the instructions.",
    },
    Rule {
        id: "empty-section",
        severity: Severity::Warning,
        kinds: &[],
        title: "Empty section",
        why: "A heading with nothing under it promises guidance that is not there.",
    },
    Rule {
        id: "duplicate-heading",
        severity: Severity::Warning,
        kinds: &[],
        title: "Duplicate heading",
        why: "Two sections with one name split the guidance and invite contradictions.",
    },
    Rule {
        id: "duplicate-line",
        severity: Severity::Warning,
        kinds: &[],
        title: "Repeated line",
        why: "Repeating an instruction wastes tokens and, once one copy is edited, the two disagree.",
    },
    Rule {
        id: "shouting",
        severity: Severity::Warning,
        kinds: &[],
        title: "Emphasis inflation",
        why: "When MUST, NEVER and capitals are everywhere, nothing stands out; current models follow calm, specific instructions well. Keep emphasis for the one or two rules that matter most.",
    },
    Rule {
        id: "vague",
        severity: Severity::Info,
        kinds: &[],
        title: "Vague wording",
        why: "Hedges such as \"try to\", \"if possible\" or \"etc.\" leave the model guessing. Say exactly what to do and when.",
    },
    Rule {
        id: "negative-only",
        severity: Severity::Info,
        kinds: &[],
        title: "Mostly don'ts",
        why: "A list of prohibitions without the behaviour you want leaves the model to guess the alternative. Say what to do instead.",
    },
    Rule {
        id: "long-line",
        severity: Severity::Info,
        kinds: &[],
        title: "Very long paragraph line",
        why: "A line over 400 characters usually holds several instructions; a list makes each one visible.",
    },
    Rule {
        id: "fence-language",
        severity: Severity::Info,
        kinds: &[],
        title: "Code fence without a language",
        why: "Naming the language (```bash, ```json) tells the model and the preview how to read the block.",
    },
    Rule {
        id: "trailing-space",
        severity: Severity::Info,
        kinds: &[],
        title: "Trailing whitespace",
        why: "Invisible noise in diffs and, in markdown, two trailing spaces force a line break.",
    },
];

/// Capitalised words that are acronyms or project words, not emphasis.
const ACRONYMS: &[&str] = &[
    "API", "APIS", "URL", "URLS", "JSON", "HTTP", "HTTPS", "YAML", "HTML", "TOML", "AGENTS",
    "README", "TODO", "UUID", "UTF", "WASM", "NOTE", "ASCII", "CSV", "PDF", "SQL", "CLI", "GPU",
    "CPU", "LLM", "LLMS", "AWS", "REST", "CRUD",
];
/// The capitalised words that count as emphasis.
const EMPHASIS: &[&str] = &["MUST", "NEVER", "ALWAYS", "IMPORTANT", "CRITICAL"];
/// Hedges, matched case-insensitively; a phrase must start and end on a word boundary.
const VAGUE: &[&str] = &[
    "try to",
    "if possible",
    "maybe",
    "etc.",
    "and so on",
    "as needed",
    "as appropriate",
    "various",
];
/// How a list item that prohibits something starts, lower-cased.
const NEGATIVE: &[&str] = &["don't", "don\u{2019}t", "do not", "never", "avoid", "no "];

/// The findings of the rules above for `kind`.
pub fn check(kind: Kind, doc: &Doc) -> Vec<Finding> {
    let applies = |id: &str| RULES.iter().any(|r| r.id == id && r.applies_to(kind));
    let text = doc
        .lines
        .iter()
        .map(|l| l.raw)
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    empty(doc, &mut out);
    too_long(&text, &mut out);
    secrets(doc, &mut out);
    unclosed_fence(doc, &mut out);
    template_vars(&text, &mut out);
    if applies("heading-first") {
        heading_first(doc, &mut out);
    }
    heading_skip(doc, &mut out);
    empty_sections(doc, &mut out);
    duplicate_headings(doc, &mut out);
    duplicate_lines(doc, &mut out);
    shouting(doc, &mut out);
    vague(doc, &mut out);
    negative_only(doc, &mut out);
    long_lines(doc, &mut out);
    fence_language(doc, &mut out);
    trailing_space(doc, &mut out);
    out
}

fn empty(doc: &Doc, out: &mut Vec<Finding>) {
    if doc.lines.iter().all(|l| matches!(l.block, Block::Blank)) {
        out.push(Finding::new(
            "empty",
            Severity::Error,
            0,
            "The prompt is empty; write the instructions before saving.",
        ));
    }
}

fn too_long(text: &str, out: &mut Vec<Finding>) {
    let chars = text.chars().count();
    if chars > 200_000 {
        out.push(Finding::new(
            "too-long",
            Severity::Error,
            0,
            format!("The prompt is {chars} characters, past the 200,000 limit; cut it down."),
        ));
    } else if chars > 32_000 {
        out.push(Finding::new(
            "too-long",
            Severity::Warning,
            0,
            format!("The prompt is {chars} characters; past 32,000 every turn pays for all of it."),
        ));
    }
}

fn secrets(doc: &Doc, out: &mut Vec<Finding>) {
    for line in &doc.lines {
        if let Some(message) = secret_message(line.raw) {
            out.push(Finding::new(
                "secret",
                Severity::Error,
                line.number,
                message,
            ));
        }
    }
}

/// The message for a line that holds a credential, if it does. The matched value never goes into the message.
fn secret_message(line: &str) -> Option<&'static str> {
    let alnum = |c: char| c.is_ascii_alphanumeric();
    let token = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let upper_digit = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit();
    if has_token(line, "sk-", 20, token) {
        Some("Line looks like an OpenAI-style key (sk-…); keep it in the vault, not the prompt.")
    } else if ["ghp_", "gho_", "ghs_"]
        .iter()
        .any(|&p| has_token(line, p, 30, alnum))
    {
        Some(
            "Line looks like a GitHub token (ghp_, gho_ or ghs_); keep it in the vault, not the prompt.",
        )
    } else if has_token(line, "github_pat_", 20, |c| {
        c.is_ascii_alphanumeric() || c == '_'
    }) {
        Some(
            "Line looks like a GitHub fine-grained token (github_pat_); keep it in the vault, not the prompt.",
        )
    } else if has_token(line, "AKIA", 16, upper_digit) {
        Some("Line looks like an AWS access key ID (AKIA…); keep it in the vault, not the prompt.")
    } else if ["xoxa-", "xoxb-", "xoxp-", "xoxr-"]
        .iter()
        .any(|&p| has_token(line, p, 10, token))
    {
        Some("Line looks like a Slack token (xox…); keep it in the vault, not the prompt.")
    } else if has_token(line, "apz_mcp_", 16, token) {
        Some("Line looks like a plaza MCP token (apz_mcp_…); keep it in the vault, not the prompt.")
    } else if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
        Some("Line holds a private key block; keep it in the vault, not the prompt.")
    } else if assigned_secret(line) {
        Some(
            "Line assigns a long key-like value to a key, token, secret or password; keep it in the vault.",
        )
    } else {
        None
    }
}

/// Whether `line` has `prefix` at the start of a word, followed by at least `min` characters that `accept` allows.
fn has_token(line: &str, prefix: &str, min: usize, accept: impl Fn(char) -> bool) -> bool {
    line.match_indices(prefix).any(|(at, _)| {
        let word_start = line[..at]
            .chars()
            .next_back()
            .map_or(true, |c| !c.is_ascii_alphanumeric());
        word_start
            && line[at + prefix.len()..]
                .chars()
                .take(min)
                .take_while(|&c| accept(c))
                .count()
                == min
    })
}

/// `key`, `token`, `secret` or `password`, then `=` or `:` within a few characters, then a value of 24 or more
/// characters that mixes letters and digits.
fn assigned_secret(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    ["key", "token", "secret", "password"].iter().any(|&word| {
        lower.match_indices(word).any(|(at, _)| {
            let rest = &line[at + word.len()..];
            let Some((sep, _)) = rest
                .char_indices()
                .take(4)
                .find(|&(_, c)| c == '=' || c == ':')
            else {
                return false;
            };
            let value: String = rest[sep + 1..]
                .trim_start_matches(|c: char| c == ' ' || c == '"' || c == '\'')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(*c))
                .collect();
            value.len() >= 24
                && value.chars().any(|c| c.is_ascii_alphabetic())
                && value.chars().any(|c| c.is_ascii_digit())
        })
    })
}

fn unclosed_fence(doc: &Doc, out: &mut Vec<Finding>) {
    if !doc.open_fence {
        return;
    }
    let line = doc
        .lines
        .iter()
        .rev()
        .find(|l| matches!(l.block, Block::Fence { opening: true, .. }))
        .map_or(0, |l| l.number);
    out.push(Finding::new(
        "unclosed-fence",
        Severity::Error,
        line,
        "This code fence is never closed, so the rest of the prompt reads as code; add the closing ```.",
    ));
}

fn template_vars(text: &str, out: &mut Vec<Finding>) {
    for (line, name) in unknown_vars(text) {
        let placeholder = format!("{}{}{}", "{{", name, "}}");
        let message = format!(
            "{placeholder} is not a template variable; only {} are filled in.",
            VARIABLES.join(", ")
        );
        out.push(Finding::new("template-var", Severity::Error, line, message));
    }
}

fn heading_first(doc: &Doc, out: &mut Vec<Finding>) {
    if let Some(first) = doc.lines.iter().find(|l| !matches!(l.block, Block::Blank)) {
        if !matches!(first.block, Block::Heading { .. }) {
            out.push(Finding::new(
                "heading-first",
                Severity::Warning,
                first.number,
                "Start with a title heading (# …) that says what this document is.",
            ));
        }
    }
}

fn heading_skip(doc: &Doc, out: &mut Vec<Finding>) {
    let mut previous: Option<u8> = None;
    for (line, level, _) in doc.headings() {
        if let Some(prev) = previous {
            if level > prev + 1 {
                out.push(Finding::new(
                    "heading-skip",
                    Severity::Warning,
                    line,
                    format!("This heading is level {level} right after a level {prev}; go one level deeper at a time."),
                ));
            }
        }
        previous = Some(level);
    }
}

fn empty_sections(doc: &Doc, out: &mut Vec<Finding>) {
    for section in doc.sections().into_iter().filter(|s| s.level > 0) {
        let blank = section.span.iter().all(|&n| {
            doc.line(n)
                .map_or(true, |l| matches!(l.block, Block::Blank))
        });
        if blank {
            out.push(Finding::new(
                "empty-section",
                Severity::Warning,
                section.line,
                format!("The section \"{}\" has nothing under its heading; write its guidance or remove it.", section.title),
            ));
        }
    }
}

fn duplicate_headings(doc: &Doc, out: &mut Vec<Finding>) {
    let mut seen: HashSet<(u8, String)> = HashSet::new();
    for (line, level, text) in doc.headings() {
        if !seen.insert((level, text.to_lowercase())) {
            out.push(Finding::new(
                "duplicate-heading",
                Severity::Warning,
                line,
                format!("The heading \"{text}\" repeats an earlier heading at the same level; merge the two sections."),
            ));
        }
    }
}

fn duplicate_lines(doc: &Doc, out: &mut Vec<Finding>) {
    let mut seen: HashSet<&str> = HashSet::new();
    for line in &doc.lines {
        let content: &str = match &line.block {
            Block::Text => line.raw.trim(),
            Block::Item { text, .. } => text.as_str(),
            _ => continue,
        };
        if content.chars().count() < 12 {
            continue;
        }
        if !seen.insert(content) {
            out.push(Finding::new(
                "duplicate-line",
                Severity::Warning,
                line.number,
                "This line repeats an earlier one word for word; keep one copy so the two cannot drift apart.",
            ));
        }
    }
}

fn shouting(doc: &Doc, out: &mut Vec<Finding>) {
    let (mut emphasis, mut other) = (0usize, 0usize);
    for line in &doc.lines {
        if matches!(line.block, Block::Code | Block::Fence { .. }) {
            continue;
        }
        for word in words(&plain(line.raw)) {
            if EMPHASIS.contains(&word) {
                emphasis += 1;
            } else if word.len() >= 4
                && word.chars().all(|c| c.is_ascii_uppercase())
                && !ACRONYMS.contains(&word)
            {
                other += 1;
            }
        }
    }
    if emphasis > 5 || other > 3 {
        out.push(Finding::new(
            "shouting",
            Severity::Warning,
            0,
            format!(
                "The prompt uses {emphasis} emphasis words (MUST, NEVER, ALWAYS, IMPORTANT, CRITICAL) and {other} other all-caps words; keep capitals for the one or two rules that matter most."
            ),
        ));
    }
}

fn vague(doc: &Doc, out: &mut Vec<Finding>) {
    for line in &doc.lines {
        if matches!(line.block, Block::Code | Block::Fence { .. }) {
            continue;
        }
        let lower = plain(line.raw).to_ascii_lowercase();
        let found: Vec<&str> = VAGUE
            .iter()
            .copied()
            .filter(|p| has_phrase(&lower, p))
            .collect();
        if !found.is_empty() {
            let list = found
                .iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Finding::new(
                "vague",
                Severity::Info,
                line.number,
                format!("Vague wording ({list}); say exactly what to do and when."),
            ));
        }
    }
}

fn negative_only(doc: &Doc, out: &mut Vec<Finding>) {
    for section in doc.sections() {
        let items: Vec<&str> = section
            .body
            .iter()
            .filter_map(|&n| match doc.line(n).map(|l| &l.block) {
                Some(Block::Item { text, .. }) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let negative = items
            .iter()
            .filter(|item| {
                let lower = item.to_lowercase();
                NEGATIVE.iter().any(|&p| lower.starts_with(p))
            })
            .count();
        if items.len() >= 3 && negative * 2 > items.len() {
            let name = if section.title.is_empty() {
                "the opening lines".to_string()
            } else {
                format!("\"{}\"", section.title)
            };
            out.push(Finding::new(
                "negative-only",
                Severity::Info,
                section.line,
                format!("{negative} of the {} items under {name} say what not to do; add what to do instead.", items.len()),
            ));
        }
    }
}

fn long_lines(doc: &Doc, out: &mut Vec<Finding>) {
    for line in &doc.lines {
        if !matches!(line.block, Block::Text | Block::Item { .. }) {
            continue;
        }
        let len = line.raw.chars().count();
        if len > 400 {
            out.push(Finding::new(
                "long-line",
                Severity::Info,
                line.number,
                format!("This line is {len} characters; split it into a list so each instruction is visible."),
            ));
        }
    }
}

fn fence_language(doc: &Doc, out: &mut Vec<Finding>) {
    for line in &doc.lines {
        if let Block::Fence {
            opening: true,
            lang,
        } = &line.block
        {
            if lang.is_empty() {
                out.push(Finding::new(
                    "fence-language",
                    Severity::Info,
                    line.number,
                    "This code fence has no language; name it (```bash, ```json) so the block reads correctly.",
                ));
            }
        }
    }
}

fn trailing_space(doc: &Doc, out: &mut Vec<Finding>) {
    let hits: Vec<u32> = doc
        .lines
        .iter()
        .filter(|l| {
            !matches!(l.block, Block::Code) && (l.raw.ends_with(' ') || l.raw.ends_with('\t'))
        })
        .map(|l| l.number)
        .collect();
    if let Some(&first) = hits.first() {
        out.push(Finding::new(
            "trailing-space",
            Severity::Info,
            first,
            format!(
                "Trailing spaces or tabs on {} line(s); strip them.",
                hits.len()
            ),
        ));
    }
}

/// The line with its `code spans` blanked out, so words inside backticks do not count as prose.
fn plain(raw: &str) -> String {
    let mut in_code = false;
    raw.chars()
        .map(|c| {
            if c == '`' {
                in_code = !in_code;
                ' '
            } else if in_code {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether `phrase` occurs in `lower` (already lower-case) with a word boundary on both sides.
fn has_phrase(lower: &str, phrase: &str) -> bool {
    lower.match_indices(phrase).any(|(at, _)| {
        let before = lower[..at]
            .chars()
            .next_back()
            .map_or(true, |c| !c.is_alphanumeric());
        let after = lower[at + phrase.len()..]
            .chars()
            .next()
            .map_or(true, |c| !c.is_alphanumeric());
        before && after
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The findings of rule `id` for `body`, checked as `kind`.
    fn fired(kind: Kind, body: &str, id: &str) -> Vec<Finding> {
        check(kind, &Doc::parse(body))
            .into_iter()
            .filter(|f| f.rule == id)
            .collect()
    }

    #[test]
    fn empty_fires_on_blank_bodies_only() {
        let f = fired(Kind::Fragment, " \n\t\n", "empty");
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 0, Severity::Error));
        assert!(fired(Kind::Fragment, "Some text", "empty").is_empty());
    }

    #[test]
    fn too_long_warns_above_32k_and_errors_above_200k() {
        let warn = fired(Kind::Fragment, &"a".repeat(33_000), "too-long");
        assert_eq!((warn.len(), warn[0].severity), (1, Severity::Warning));
        let error = fired(Kind::Fragment, &"a".repeat(200_001), "too-long");
        assert_eq!((error.len(), error[0].severity), (1, Severity::Error));
        assert!(fired(Kind::Fragment, &"a".repeat(1_000), "too-long").is_empty());
    }

    #[test]
    fn secret_fires_on_keys_and_assignments_not_on_plain_words() {
        let f = fired(
            Kind::Fragment,
            "Use sk-abcdefghijklmnopqrstuvwxyz012345 here",
            "secret",
        );
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 1, Severity::Error));
        assert!(!f[0].message.contains("abcdefgh"));
        assert_eq!(
            fired(
                Kind::Fragment,
                "api_key = a1b2c3d4e5f6g7h8i9j0k1l2",
                "secret"
            )
            .len(),
            1
        );
        assert!(
            fired(
                Kind::Fragment,
                "Read the task-management-overview-document-section-two",
                "secret"
            )
            .is_empty()
        );
        assert!(
            fired(
                Kind::Fragment,
                "The key takeaway is to read the docs carefully.",
                "secret"
            )
            .is_empty()
        );
    }

    #[test]
    fn unclosed_fence_fires_at_the_last_opening_fence() {
        let f = fired(Kind::Fragment, "# T\n\n```bash\nls -la", "unclosed-fence");
        assert_eq!((f.len(), f[0].line), (1, 3));
        assert!(fired(Kind::Fragment, "```bash\nls -la\n```", "unclosed-fence").is_empty());
    }

    #[test]
    fn template_var_fires_on_unknown_names() {
        let f = fired(Kind::Fragment, "Hello\nHi {{nope}}", "template-var");
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 2, Severity::Error));
        assert!(f[0].message.starts_with("{{nope}}"));
        assert!(fired(Kind::Fragment, "Hi {{name}} from {{plaza}}", "template-var").is_empty());
    }

    #[test]
    fn heading_first_applies_to_system_prompts_only() {
        let f = fired(Kind::System, "Some text\n# Title", "heading-first");
        assert_eq!((f.len(), f[0].line), (1, 1));
        assert!(fired(Kind::System, "# Title\nSome text", "heading-first").is_empty());
        assert!(fired(Kind::Fragment, "Some text", "heading-first").is_empty());
    }

    #[test]
    fn heading_skip_fires_on_a_jump_of_two_levels() {
        let f = fired(Kind::Fragment, "# A\ntext\n### B\ntext", "heading-skip");
        assert_eq!((f.len(), f[0].line), (1, 3));
        assert!(fired(Kind::Fragment, "### Only\ntext", "heading-skip").is_empty());
        assert!(fired(Kind::Fragment, "# A\n## B\n### C\n## D", "heading-skip").is_empty());
    }

    #[test]
    fn empty_section_fires_on_a_heading_with_only_blank_lines_under_it() {
        let f = fired(Kind::Fragment, "# A\n## B\n\n## C\ntext", "empty-section");
        assert_eq!((f.len(), f[0].line), (1, 2));
        assert!(fired(Kind::Fragment, "# A\n\ntext", "empty-section").is_empty());
    }

    #[test]
    fn duplicate_heading_compares_level_and_case_insensitively() {
        let f = fired(Kind::Fragment, "# A\ntext\n# a", "duplicate-heading");
        assert_eq!((f.len(), f[0].line), (1, 3));
        assert!(fired(Kind::Fragment, "# A\n## A", "duplicate-heading").is_empty());
    }

    #[test]
    fn duplicate_line_fires_on_a_repeat_of_twelve_characters_or_more() {
        let f = fired(
            Kind::Fragment,
            "Always run the test suite first.\n\nAlways run the test suite first.",
            "duplicate-line",
        );
        assert_eq!((f.len(), f[0].line), (1, 3));
        assert!(fired(Kind::Fragment, "Short\nShort", "duplicate-line").is_empty());
    }

    #[test]
    fn shouting_counts_emphasis_outside_code() {
        let f = fired(
            Kind::Fragment,
            "MUST do a. MUST do b. MUST do c. MUST do d. MUST do e. MUST do f.",
            "shouting",
        );
        assert_eq!(
            (f.len(), f[0].line, f[0].severity),
            (1, 0, Severity::Warning)
        );
        assert!(fired(
            Kind::Fragment,
            "Use `MUST`, `NEVER`, `ALWAYS`, `IMPORTANT`, `CRITICAL` and `MUST` in code only. Call the API and read JSON over HTTP.",
            "shouting",
        )
        .is_empty());
    }

    #[test]
    fn vague_lists_the_hedges_on_a_line() {
        let f = fired(
            Kind::Fragment,
            "Try to keep it short, if possible.",
            "vague",
        );
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 1, Severity::Info));
        assert!(f[0].message.contains("\"try to\"") && f[0].message.contains("\"if possible\""));
        assert!(fired(Kind::Fragment, "Keep it short and specific.", "vague").is_empty());
        assert!(fired(Kind::Fragment, "Retry to keep it short.", "vague").is_empty());
    }

    #[test]
    fn negative_only_fires_when_most_items_are_prohibitions() {
        let f = fired(
            Kind::Fragment,
            "# Rules\n- Don't lie.\n- Never guess.\n- Avoid fluff.\n- Use JSON.",
            "negative-only",
        );
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 1, Severity::Info));
        assert!(
            fired(
                Kind::Fragment,
                "# Rules\n- Be brief.\n- Never guess.\n- Cite sources.",
                "negative-only"
            )
            .is_empty()
        );
    }

    #[test]
    fn long_line_fires_above_400_characters() {
        let f = fired(Kind::Fragment, &"a".repeat(401), "long-line");
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 1, Severity::Info));
        assert!(fired(Kind::Fragment, &"a".repeat(400), "long-line").is_empty());
    }

    #[test]
    fn fence_language_fires_on_an_unnamed_fence() {
        let f = fired(Kind::Fragment, "```\nx\n```", "fence-language");
        assert_eq!((f.len(), f[0].line), (1, 1));
        assert!(fired(Kind::Fragment, "```rust\nx\n```", "fence-language").is_empty());
    }

    #[test]
    fn trailing_space_reports_once_with_the_count() {
        let f = fired(Kind::Fragment, "first \nsecond\nthird\t", "trailing-space");
        assert_eq!((f.len(), f[0].line), (1, 1));
        assert!(f[0].message.contains("2 line"));
        assert!(fired(Kind::Fragment, "clean\nlines", "trailing-space").is_empty());
    }
}
