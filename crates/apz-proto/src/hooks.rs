//! A project's hooks: checks and formatters that run around an agent's tool calls, without a model turn.
//!
//! A hook says when it runs (`on`: an `edit` of a file, a `run` of a command, the `land` of a change), what it
//! covers (`paths`, globs over the workspace's paths, and `languages`; an empty list covers everything), what it
//! does (`action`) and whether a finding only reports it or refuses the call (`mode`). The hub declares them per
//! project (`apid`'s project settings) and pushes the effective ones in each agent's profile; the harness runs
//! them, and the engines that have hooks of their own get them rendered from the same list.
//!
//! This module is pure: it says what a hook covers and what a text's findings are. Running a formatter or a
//! linter is the harness's (`agent-harness`'s `hooks`), since only the VM has the tools.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// When a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "store", derive(apz_store::Column))]
pub enum On {
    /// After an edit writes a file the hook covers.
    Edit,
    /// Before a command runs.
    Run,
    /// When a change is about to join its project's landing queue (the hub runs these on the sealed revision).
    Land,
}

/// Whether a hook's findings only report, or refuse the call they came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "store", derive(apz_store::Column))]
pub enum Mode {
    /// The findings are reported with the call's result, which still happens.
    #[default]
    Report,
    /// Any finding fails the call: an edit is rolled back, a command does not run, a change goes back to its builder.
    Block,
}

/// What a hook does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Formats the file with its language's formatter (`edit` only).
    Format,
    /// Lints the file with its language's linter (`edit` only).
    Lint,
    /// Checks the text against the named rule of [`RULES`].
    Rule { id: String },
    /// Scans the text for secrets by their shape (`apz_secrets`).
    SecretScan,
}

/// The kind of an [`Action`], as a column: the rule's id is kept beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "store", derive(apz_store::Column))]
pub enum ActionKind {
    Format,
    Lint,
    Rule,
    SecretScan,
}

impl Action {
    /// The kind of this action, and the rule id when it is a rule.
    pub fn split(&self) -> (ActionKind, Option<&str>) {
        match self {
            Action::Format => (ActionKind::Format, None),
            Action::Lint => (ActionKind::Lint, None),
            Action::Rule { id } => (ActionKind::Rule, Some(id)),
            Action::SecretScan => (ActionKind::SecretScan, None),
        }
    }

    /// The action of a kind and its rule id (`None` for a rule that names none, which is not an action).
    pub fn join(kind: ActionKind, rule: Option<String>) -> Option<Action> {
        Some(match kind {
            ActionKind::Format => Action::Format,
            ActionKind::Lint => Action::Lint,
            ActionKind::Rule => Action::Rule { id: rule? },
            ActionKind::SecretScan => Action::SecretScan,
        })
    }

    /// The name the findings and results use for the action.
    pub fn name(&self) -> String {
        match self {
            Action::Format => "format".into(),
            Action::Lint => "lint".into(),
            Action::Rule { id } => format!("rule {id}"),
            Action::SecretScan => "secret scan".into(),
        }
    }
}

/// One hook of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub on: On,
    /// Globs over the workspace's relative paths (`*` within a folder, `**` across folders); empty covers every path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Languages by their names ([`LANGUAGES`]); empty covers every language.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    pub action: Action,
    #[serde(default)]
    pub mode: Mode,
}

/// The languages a hook can name: the ones the code-intelligence tools answer for.
pub const LANGUAGES: &[&str] = &[
    "rust",
    "go",
    "python",
    "typescript",
    "javascript",
    "swift",
    "kotlin",
    "java",
    "csharp",
    "html",
    "css",
    "bash",
    "powershell",
    "yaml",
    "toml",
    "json",
];

/// The languages the default hooks format. The harness has a formatter for each (its test checks it).
pub const FORMATTED: &[&str] = &[
    "rust",
    "go",
    "python",
    "typescript",
    "javascript",
    "swift",
    "kotlin",
    "java",
    "csharp",
    "bash",
    "toml",
];

/// The languages the default hooks lint. The harness has a linter for each (its test checks it).
pub const LINTED: &[&str] = &["python", "typescript", "javascript", "bash"];

/// The rules a hook can check, by id, with what a finding says.
pub const RULES: &[(&str, &str)] = &[(
    "conflict-markers",
    "a merge conflict marker is left in the text",
)];

/// The hooks a project gets when it sets `hooks: "defaults"`: format and lint each language that has a tool, and
/// scan for secrets on every edit and run, refusing them.
pub fn default_hooks() -> Vec<Hook> {
    let mut hooks: Vec<Hook> = FORMATTED
        .iter()
        .map(|language| Hook {
            on: On::Edit,
            paths: Vec::new(),
            languages: vec![(*language).to_string()],
            action: Action::Format,
            mode: Mode::Report,
        })
        .collect();
    hooks.extend(LINTED.iter().map(|language| Hook {
        on: On::Edit,
        paths: Vec::new(),
        languages: vec![(*language).to_string()],
        action: Action::Lint,
        mode: Mode::Report,
    }));
    for on in [On::Edit, On::Run] {
        hooks.push(Hook {
            on,
            paths: Vec::new(),
            languages: Vec::new(),
            action: Action::SecretScan,
            mode: Mode::Block,
        });
    }
    hooks
}

impl Hook {
    /// Whether the hook covers a file at `path` (workspace-relative) in `language` (a name from [`LANGUAGES`], or
    /// `None` when the path's language is none of them). A hook with languages covers no file of another language.
    pub fn covers(&self, path: &str, language: Option<&str>) -> bool {
        let path_ok =
            self.paths.is_empty() || self.paths.iter().any(|pattern| glob_matches(pattern, path));
        let language_ok = self.languages.is_empty()
            || language.is_some_and(|l| self.languages.iter().any(|x| x == l));
        path_ok && language_ok
    }
}

/// The findings of a text under a hook's action, one line each: the secret shapes it carries, or the rule's hits.
/// A format or lint is not a finding of the text (the harness runs the tool), so it has none here.
pub fn findings(action: &Action, text: &str) -> Vec<String> {
    match action {
        Action::Format | Action::Lint => Vec::new(),
        Action::SecretScan => text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                apz_secrets::shape(line).map(|shape| {
                    format!("line {}: a value shaped like a {shape} secret", index + 1)
                })
            })
            .collect(),
        Action::Rule { id } => match RULES.iter().find(|(known, _)| known == id) {
            None => vec![format!("rule {id} is not in the catalogue")],
            Some((_, what)) => text
                .lines()
                .enumerate()
                .filter(|(_, line)| rule_hit(id, line))
                .map(|(index, _)| format!("line {}: {what}", index + 1))
                .collect(),
        },
    }
}

/// Whether one line breaks the rule `id` (the ids of [`RULES`]).
fn rule_hit(id: &str, line: &str) -> bool {
    match id {
        "conflict-markers" => {
            line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> ") || line == "======="
        }
        _ => false,
    }
}

/// The lines a diff adds: the `+` lines, not the `+++` header of a file. A landing's hooks scan these.
pub fn added_lines(diff: &str) -> String {
    let mut out = String::new();
    for line in diff.lines() {
        if let Some(added) = line.strip_prefix('+')
            && !added.starts_with("++")
        {
            out.push_str(added);
            out.push('\n');
        }
    }
    out
}

/// The `hooks` of Claude Code's settings for `hooks`, calling the harness at `program` with its `apz-hook` entry point
/// and the hooks in `hooks_file` (a JSON list the adapter writes beside the settings).
///
/// Before a command, a secret or rule check on the command (`PreToolUse` on `Bash`); before an edit, the same checks on
/// the text it writes (`PreToolUse` on the file tools), so Claude Code's own write is refused before it lands; after an
/// edit, the formatter and linter (`PostToolUse`). Only the events that have a hook get an entry.
pub fn claude_code_settings(hooks: &[Hook], program: &str, hooks_file: &str) -> Value {
    let checks = |on: On| {
        hooks.iter().any(|hook| {
            hook.on == on && matches!(hook.action, Action::SecretScan | Action::Rule { .. })
        })
    };
    let formats = hooks
        .iter()
        .any(|hook| hook.on == On::Edit && matches!(hook.action, Action::Format | Action::Lint));
    let call = |event: &str| format!("{program} apz-hook {event} --hooks {hooks_file}");
    let mut settings = serde_json::Map::new();
    let mut pre = Vec::new();
    if checks(On::Run) {
        pre.push(
            json!({"matcher": "Bash", "hooks": [{"type": "command", "command": call("run")}]}),
        );
    }
    if checks(On::Edit) {
        pre.push(json!({"matcher": "Edit|Write|MultiEdit", "hooks": [{"type": "command", "command": call("pre")}]}));
    }
    if !pre.is_empty() {
        settings.insert("PreToolUse".into(), Value::Array(pre));
    }
    if formats {
        settings.insert(
            "PostToolUse".into(),
            json!([{"matcher": "Edit|Write|MultiEdit", "hooks": [{"type": "command", "command": call("post")}]}]),
        );
    }
    Value::Object(settings)
}

/// Whether a workspace-relative `path` matches the glob `pattern`: `*` matches within one folder's name, `**` any
/// number of whole folders, `?` one character that is not a `/`. Everything else matches itself.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let path: Vec<char> = path.chars().collect();
    matches_at(&pattern, &path)
}

fn matches_at(pattern: &[char], path: &[char]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some(('*', rest)) if rest.first() == Some(&'*') => {
            // `**`: any run of whole path segments, and a `/` after it when it stands for some. At the end of the
            // pattern it matches whatever is left.
            let tail = &rest[1..];
            if tail.is_empty() {
                return true;
            }
            let tail = tail.strip_prefix(&['/']).unwrap_or(tail);
            (0..=path.len())
                .any(|at| (at == 0 || path[at - 1] == '/') && matches_at(tail, &path[at..]))
        }
        Some(('*', rest)) => (0..=path.len())
            .take_while(|&at| at == 0 || path[at - 1] != '/')
            .any(|at| matches_at(rest, &path[at..])),
        Some(('?', rest)) => {
            path.first().is_some_and(|c| *c != '/') && matches_at(rest, &path[1..])
        }
        Some((c, rest)) => path.first() == Some(c) && matches_at(rest, &path[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(on: On, action: Action) -> Hook {
        Hook {
            on,
            paths: Vec::new(),
            languages: Vec::new(),
            action,
            mode: Mode::Block,
        }
    }

    #[test]
    fn a_glob_matches_one_folder_or_many() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(
            !glob_matches("*.rs", "src/main.rs"),
            "a * stays in its folder"
        );
        assert!(glob_matches("src/**/*.rs", "src/a/b/c.rs"));
        assert!(glob_matches("src/**/*.rs", "src/c.rs"));
        assert!(glob_matches("**/*.rs", "c.rs"));
        assert!(!glob_matches("src/**", "lib/a.rs"));
        assert!(glob_matches("a?c.txt", "abc.txt") && !glob_matches("a?c.txt", "a/c.txt"));
    }

    #[test]
    fn a_hook_covers_its_paths_and_languages_and_nothing_else() {
        let mut only_rust = hook(On::Edit, Action::Format);
        only_rust.languages = vec!["rust".into()];
        assert!(only_rust.covers("src/lib.rs", Some("rust")));
        assert!(!only_rust.covers("src/app.py", Some("python")));
        assert!(!only_rust.covers("notes", None));
        let mut in_src = hook(On::Edit, Action::SecretScan);
        in_src.paths = vec!["src/**".into()];
        assert!(in_src.covers("src/a.txt", None));
        assert!(!in_src.covers("docs/a.txt", None));
        assert!(hook(On::Run, Action::SecretScan).covers("anything", None));
    }

    #[test]
    fn a_secret_is_found_by_its_line_and_its_shape_not_its_value() {
        let key = format!("sk-ant-{}", "a1".repeat(12));
        let text = format!("fine\nkey = {key}\n");
        let found = findings(&Action::SecretScan, &text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("line 2:") && !found[0].contains(&key),
            "{found:?}"
        );
        assert!(findings(&Action::SecretScan, "the password field\n").is_empty());
    }

    #[test]
    fn a_conflict_marker_breaks_the_rule_and_an_unknown_rule_is_a_finding() {
        let text = "a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> topic\n";
        let found = findings(
            &Action::Rule {
                id: "conflict-markers".into(),
            },
            text,
        );
        assert_eq!(found.len(), 3, "{found:?}");
        let unknown = findings(&Action::Rule { id: "nope".into() }, text);
        assert_eq!(
            unknown,
            vec!["rule nope is not in the catalogue".to_string()]
        );
    }

    #[test]
    fn a_diff_adds_only_its_plus_lines() {
        let diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new key\n context\n";
        assert_eq!(added_lines(diff), "new key\n");
    }

    #[test]
    fn the_defaults_format_lint_and_scan_and_refuse_secrets() {
        let hooks = default_hooks();
        assert!(
            hooks
                .iter()
                .any(|h| h.action == Action::Format && h.languages == ["rust".to_string()])
        );
        let scans: Vec<&Hook> = hooks
            .iter()
            .filter(|h| h.action == Action::SecretScan)
            .collect();
        assert_eq!(scans.len(), 2);
        assert!(scans.iter().all(|h| h.mode == Mode::Block));
        for language in FORMATTED.iter().chain(LINTED) {
            assert!(LANGUAGES.contains(language), "{language} is not a language");
        }
    }

    #[test]
    fn a_hook_reads_from_json_with_its_defaults() {
        let hook: Hook = serde_json::from_str(
            r#"{"on":"edit","action":{"kind":"rule","id":"conflict-markers"}}"#,
        )
        .expect("a hook");
        assert_eq!(hook.mode, Mode::Report);
        assert!(hook.paths.is_empty() && hook.languages.is_empty());
        assert!(
            serde_json::from_str::<Hook>(r#"{"on":"edit","action":{"kind":"format"},"colour":1}"#)
                .is_err()
        );
    }

    #[test]
    fn claude_code_gets_only_the_events_its_hooks_need() {
        let hooks = vec![Hook {
            mode: Mode::Block,
            ..hook(On::Edit, Action::SecretScan)
        }];
        let settings = claude_code_settings(&hooks, "/usr/bin/agent-harness", "/s/hooks.json");
        assert!(settings.get("PreToolUse").is_some());
        assert!(settings.get("PostToolUse").is_none(), "no formatter here");
        assert_eq!(
            settings["PreToolUse"][0]["hooks"][0]["command"],
            "/usr/bin/agent-harness apz-hook pre --hooks /s/hooks.json"
        );
        let formats = vec![hook(On::Edit, Action::Format)];
        let settings = claude_code_settings(&formats, "h", "f");
        assert!(settings.get("PreToolUse").is_none());
        assert_eq!(
            settings["PostToolUse"][0]["hooks"][0]["command"],
            "h apz-hook post --hooks f"
        );
    }
}
