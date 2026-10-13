//! The rules for one kind: system prompts and AGENTS.md (docs/design/prompt-library.md, "Rules").

use crate::doc::{Block, Doc};
use crate::{Finding, Kind, Rule, Severity};

pub const RULES: &[Rule] = &[
    Rule { id: "system-sections", severity: Severity::Warning, kinds: &[Kind::System], title: "Missing house section",
        why: "Role prompts in this plaza share one outline: Purpose, Responsibilities, Deliverables, Boundaries. A missing one is usually a missing piece of the role." },
    Rule { id: "second-person", severity: Severity::Info, kinds: &[Kind::System], title: "Address the model as \"you\"",
        why: "A system prompt is spoken to the model. \"You review the code\" is clearer than \"The agent should review the code\"." },
    Rule { id: "agents-md-sections", severity: Severity::Warning, kinds: &[Kind::AgentsMd], title: "Missing AGENTS.md section",
        why: "The agents.md convention: Project overview, Setup or Build commands, Testing, Code style, Commit or PR guidelines, Security. Build and test commands are the ones agents need most (warning); the rest are info." },
    Rule { id: "bare-command", severity: Severity::Warning, kinds: &[Kind::AgentsMd], title: "Command outside code formatting",
        why: "Commands in `backticks` or a ```bash fence are copied exactly; in prose they get paraphrased or mangled." },
];

/// The findings of the rules above for `kind`: each catalogue rule that applies to `kind` runs once.
pub fn check(kind: Kind, doc: &Doc) -> Vec<Finding> {
    let mut out = Vec::new();
    for rule in RULES.iter().filter(|r| r.applies_to(kind)) {
        match rule.id {
            "system-sections" => system_sections(doc, &mut out),
            "second-person" => second_person(doc, &mut out),
            "agents-md-sections" => agents_md_sections(doc, &mut out),
            "bare-command" => bare_command(doc, &mut out),
            _ => {}
        }
    }
    out
}

/// Whether any heading's text has one of `words` at the start of a word, ignoring case: "test" matches "Testing"
/// but not "Latest", and "git" does not match "digital".
fn has_heading(doc: &Doc, words: &[&str]) -> bool {
    doc.headings().any(|(_, _, text)| {
        let text = format!(" {}", text.to_lowercase());
        words.iter().any(|w| {
            text.match_indices(w.trim_end())
                .any(|(at, _)| text[..at].ends_with(|c: char| !c.is_alphanumeric()))
        })
    })
}

fn system_sections(doc: &Doc, out: &mut Vec<Finding>) {
    const WANTED: [(&str, &[&str], &str); 4] = [
        ("Purpose", &["purpose", "goal", "mission"], "what the role exists to achieve"),
        ("Responsibilities", &["responsibilit", "duties", "what you do"], "what the role owns"),
        ("Deliverables", &["deliverable", "output", "done when", "result"], "what is handed over when a task is finished"),
        ("Boundaries", &["boundar", "constraint", "limit", "rules", "never"], "what the role must not do"),
    ];
    for (label, words, what) in WANTED {
        if !has_heading(doc, words) {
            out.push(Finding::new(
                "system-sections",
                Severity::Warning,
                0,
                format!("No heading names the {label} section: add a \"## {label}\" heading that says {what}."),
            ));
        }
    }
}

fn second_person(doc: &Doc, out: &mut Vec<Finding>) {
    const PHRASES: [&str; 4] = ["the agent ", "the assistant ", "the model should", "this agent "];
    let hits: Vec<u32> = doc
        .prose()
        .filter(|l| {
            let raw = l.raw.to_lowercase();
            PHRASES.iter().any(|p| raw.contains(*p))
        })
        .map(|l| l.number)
        .collect();
    if let Some(&first) = hits.first() {
        out.push(Finding::new(
            "second-person",
            Severity::Info,
            first,
            format!(
                "{} prose line(s) refer to the model in the third person, starting at this one: address it as \"you\" instead.",
                hits.len()
            ),
        ));
    }
}

fn agents_md_sections(doc: &Doc, out: &mut Vec<Finding>) {
    const WANTED: [(&str, &[&str], Severity, &str); 6] = [
        (
            "setup or build commands",
            &["setup", "build", "install", "getting started", "development"],
            Severity::Warning,
            "a \"## Setup\" section with the commands that install and build the project",
        ),
        ("testing", &["test"], Severity::Warning, "a \"## Testing\" section with the command that runs the suite"),
        (
            "the project overview",
            &["overview", "about", "project"],
            Severity::Info,
            "a \"## Project overview\" section saying what the project is and how its code is laid out",
        ),
        (
            "code style",
            &["style", "convention", "format", "lint"],
            Severity::Info,
            "a \"## Code style\" section with the conventions the code follows",
        ),
        (
            "commit or pull request guidelines",
            &["commit", "pull request", "pr ", "git", "review"],
            Severity::Info,
            "a \"## Commits and pull requests\" section saying how changes are committed and reviewed",
        ),
        (
            "security",
            &["security", "secret"],
            Severity::Info,
            "a \"## Security\" section saying how keys and secrets are handled",
        ),
    ];
    for (topic, words, severity, add) in WANTED {
        if !has_heading(doc, words) {
            out.push(Finding::new("agents-md-sections", severity, 0, format!("No heading covers {topic}: add {add}.")));
        }
    }
}

fn bare_command(doc: &Doc, out: &mut Vec<Finding>) {
    for line in &doc.lines {
        let (text, is_item) = match &line.block {
            Block::Text => (line.raw.trim(), false),
            Block::Item { text, .. } => (text.as_str(), true),
            _ => continue,
        };
        if is_bare_command(&strip_code(text), is_item) {
            out.push(Finding::new(
                "bare-command",
                Severity::Warning,
                line.number,
                "Put the command in backticks or a ```bash block so it is copied exactly.",
            ));
        }
    }
}

/// The text with its inline code spans removed. An unpaired final backtick leaves the text after it as prose.
fn strip_code(text: &str) -> String {
    let backticks = text.matches('`').count();
    text.split('`')
        .enumerate()
        .filter_map(|(i, part)| (!(i % 2 == 1 && i < backticks)).then_some(part))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Command words and the subcommands that make `word subcommand` a command.
const COMMANDS: [(&str, &[&str]); 7] = [
    ("cargo", &["build", "test", "run", "check", "clippy", "fmt", "install"]),
    ("npm", &["install", "run", "test", "ci"]),
    ("pnpm", &["install", "run", "test", "add"]),
    ("yarn", &["install", "run", "test", "add"]),
    ("go", &["test", "build", "run", "vet", "mod"]),
    ("docker", &["build", "run", "compose", "push"]),
    ("git", &["commit", "push", "pull", "checkout", "rebase", "clone", "merge"]),
];

/// Whether `plain` (a line with its code spans removed) reads as a command typed out in prose.
fn is_bare_command(plain: &str, is_item: bool) -> bool {
    let plain = plain.trim_start();
    if plain.starts_with("$ ") {
        return true;
    }
    let punct = |c: char| c.is_ascii_punctuation();
    let raw_words: Vec<&str> = plain.split_whitespace().collect();
    let words: Vec<String> = raw_words.iter().map(|w| w.trim_end_matches(punct).to_lowercase()).collect();
    if words.iter().any(|w| w.as_str() == "pytest") {
        return true;
    }
    if words.windows(2).any(|p| {
        COMMANDS.iter().any(|(cmd, subs)| p[0] == *cmd && subs.contains(&p[1].as_str()))
    }) {
        return true;
    }
    // `make` opening a list item, then a lowercase target or nothing at all.
    is_item
        && raw_words.first().copied().map(|w| w.trim_end_matches(punct)) == Some("make")
        && raw_words.get(1).map_or(true, |w| w.starts_with(char::is_lowercase) && *w != "sure")
}

#[cfg(test)]
mod tests {
    #[test]
    fn section_words_match_at_word_starts_only() {
        let body = "# Notes\n\n## Latest digital news\n\nText.\n";
        let missing: Vec<_> = found(Kind::AgentsMd, body, "agents-md-sections").into_iter().map(|f| f.message).collect();
        assert!(missing.iter().any(|m| m.contains("testing")), "{missing:?}");
        assert!(found(Kind::AgentsMd, "# A\n\n- make sure the suite passes\n", "bare-command").is_empty());
    }

    use super::*;

    /// The findings of `rule` that `check` reports for `body` as `kind`.
    fn found(kind: Kind, body: &str, rule: &str) -> Vec<Finding> {
        check(kind, &Doc::parse(body)).into_iter().filter(|f| f.rule == rule).collect()
    }

    #[test]
    fn system_sections_fire_for_each_missing_section() {
        let f = found(Kind::System, "# Role\n\n## Purpose\nDo the job.\n", "system-sections");
        assert_eq!(f.len(), 3);
        assert!(f.iter().all(|x| x.line == 0 && x.severity == Severity::Warning));
        assert!(f[0].message.contains("Responsibilities"));
        assert!(f[1].message.contains("Deliverables"));
        assert!(f[2].message.contains("Boundaries"));
    }

    #[test]
    fn system_sections_accept_synonyms_in_any_case() {
        let body = "# Role\n\n## MISSION\n\n## Duties\n\n## Results\n\n## Constraints\n";
        assert!(found(Kind::System, body, "system-sections").is_empty());
    }

    #[test]
    fn second_person_fires_once_at_the_first_third_person_line() {
        let body = "# Role\n\nYou review code.\n\n- The agent should check the diff.\n- The assistant replies in English.\n";
        let f = found(Kind::System, body, "second-person");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 5);
        assert_eq!(f[0].severity, Severity::Info);
        assert!(f[0].message.contains('2'));
    }

    #[test]
    fn second_person_ignores_code_blocks() {
        let body = "You review the code.\n\n```\nthe agent should not count\n```\n";
        assert!(found(Kind::System, body, "second-person").is_empty());
    }

    #[test]
    fn agents_md_sections_warn_on_missing_setup_and_testing_and_inform_on_the_rest() {
        let f = found(Kind::AgentsMd, "# Repo\n\n## Security\nKeep keys out of git.\n", "agents-md-sections");
        assert_eq!(f.len(), 5);
        assert_eq!(f[0].severity, Severity::Warning);
        assert!(f[0].message.contains("setup or build"));
        assert_eq!(f[1].severity, Severity::Warning);
        assert!(f[1].message.contains("testing"));
        assert!(f[2..].iter().all(|x| x.severity == Severity::Info));
    }

    #[test]
    fn agents_md_sections_accept_every_section_in_any_case() {
        let body = "## Overview\n## Getting started\n## TESTING\n## Lint rules\n## Pull request guidelines\n## Secrets\n";
        assert!(found(Kind::AgentsMd, body, "agents-md-sections").is_empty());
    }

    #[test]
    fn bare_command_fires_on_commands_outside_formatting() {
        let body = "# Repo\n\n- cargo test before you hand over\n$ npm start\n- make build\npytest runs the suite\nShare it with git push\n- make\n";
        let f = found(Kind::AgentsMd, body, "bare-command");
        let lines: Vec<u32> = f.iter().map(|x| x.line).collect();
        assert_eq!(lines, vec![3, 4, 5, 6, 7, 8]);
        assert!(f.iter().all(|x| x.severity == Severity::Warning));
        assert_eq!(f[0].message, "Put the command in backticks or a ```bash block so it is copied exactly.");
    }

    #[test]
    fn bare_command_leaves_formatted_and_plain_prose_alone() {
        let body = "- Install the dependencies: `npm install`\n- Run the suite with `cargo test`\n- `$ cargo test` runs everything\n```bash\ncargo test\n```\n- The cargo workspace has three crates.\nmake test\n- Make sure the suite passes\n";
        assert!(found(Kind::AgentsMd, body, "bare-command").is_empty());
    }

    #[test]
    fn an_unpaired_backtick_does_not_hide_a_command() {
        let f = found(Kind::AgentsMd, "Run `cargo test\n", "bare-command");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 1);
    }
}
