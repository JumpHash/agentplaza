//! The project's hooks, run around the agent's edits and commands without a model turn (`apz_proto::hooks`).
//!
//! The hub pushes a project's effective hooks in the agent's profile, and [`set`] keeps them in the workspace's lane.
//! An edit runs its `edit` hooks once the file is written ([`after_edit`]): a formatter rewrites the file, a linter
//! reports, and a secret or rule check reads the text the edit added. A finding of a hook in `block` mode refuses the
//! edit, and the edit rolls back (`v2::edit`). A command runs its `run` hooks before it starts ([`before_run`]): a
//! finding of a `block` hook refuses it. Each tool runs from the workspace root with its path as its last argument,
//! and never through a shell.
//!
//! A tool that is not installed skips its hook and is reported once per lane, not as an error.
//!
//! Claude Code has hooks of its own: `apz_proto::hooks::claude_code_settings` renders the same list into its settings,
//! which call the harness's `apz-hook` entry point ([`hook_main`]) to run them.

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use apz_lsp::servers::Language;
use apz_proto::hooks::{Action, Hook, Mode, On, findings};
use serde_json::Value;

use crate::files::Workspace;
use crate::tools::ToolError;
use crate::v2::{Lane, locked};

/// The most lint lines a result carries.
const LINT_LINES: usize = 5;

/// A tool a hook runs on a file: its program, and the arguments before the file's path.
struct Tool {
    program: &'static str,
    args: &'static [&'static str],
}

/// The formatter of a language (a name from `apz_proto::hooks::LANGUAGES`), if it has one.
fn formatter(language: &str) -> Option<Tool> {
    let (program, args): (&'static str, &'static [&'static str]) = match language {
        "rust" => ("rustfmt", &[]),
        "go" => ("gofmt", &["-w"]),
        "python" => ("ruff", &["format"]),
        "typescript" | "javascript" => ("biome", &["format", "--write"]),
        "swift" => ("swift-format", &["-i"]),
        "kotlin" => ("ktfmt", &[]),
        "java" => ("google-java-format", &["-i"]),
        "csharp" => ("dotnet", &["format", "--include"]),
        "bash" => ("shfmt", &["-w"]),
        "toml" => ("taplo", &["fmt"]),
        _ => return None,
    };
    Some(Tool { program, args })
}

/// The linter of a language, if it has one. A linter exits non-zero when it finds something.
fn linter(language: &str) -> Option<Tool> {
    let (program, args): (&'static str, &'static [&'static str]) = match language {
        "python" => (
            "ruff",
            &["check", "--no-cache", "--output-format", "concise"],
        ),
        "typescript" | "javascript" => ("biome", &["lint"]),
        "bash" => ("shellcheck", &["--format", "gcc"]),
        _ => return None,
    };
    Some(Tool { program, args })
}

/// Keeps the hooks of the workspace's lane: the agent's profile sets them, and they change only with it.
pub(crate) fn set(lane: &Lane, hooks: Vec<Hook>) {
    *locked(&lane.hooks) = hooks;
}

/// Sets the hooks of `workspace`'s lane, as the agent's profile gives them.
pub(crate) fn set_for(workspace: &Workspace, hooks: Vec<Hook>) {
    set(&crate::v2::lane_of(workspace), hooks);
}

/// What the hooks found for one edited file.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct EditCheck {
    /// What the formatter did, in words ("formatted with rustfmt"), and what could not run.
    pub(crate) done: Vec<String>,
    /// The warnings of the linter, one line each.
    pub(crate) lint: Vec<String>,
    /// The findings of the secret and rule checks, each naming its hook.
    pub(crate) findings: Vec<String>,
    /// Whether a finding of a `block` hook refuses the edit.
    pub(crate) blocked: bool,
}

impl EditCheck {
    /// Whether no hook said anything about the file.
    pub(crate) fn is_empty(&self) -> bool {
        self.done.is_empty() && self.lint.is_empty() && self.findings.is_empty()
    }

    /// The hooks' outcome in words for a result: "formatted with rustfmt; 2 lint warnings: a; b".
    pub(crate) fn summary(&self) -> String {
        let mut parts = self.done.clone();
        if !self.lint.is_empty() {
            parts.push(format!(
                "{} lint warnings: {}",
                self.lint.len(),
                self.lint.join("; ")
            ));
        }
        if !self.findings.is_empty() {
            parts.push(format!(
                "{} findings: {}",
                self.findings.len(),
                self.findings.join("; ")
            ));
        }
        parts.join("; ")
    }
}

/// The hooks of `lane` that run on `on` for a file at `label` (workspace-relative, in `language`).
fn covering(lane: &Lane, on: On, label: &str, language: Option<&str>) -> Vec<Hook> {
    locked(&lane.hooks)
        .iter()
        .filter(|hook| hook.on == on && hook.covers(label, language))
        .cloned()
        .collect()
}

/// The language of a workspace-relative path, by the name the hooks use.
fn language_of(label: &str) -> Option<&'static str> {
    Language::from_path(label).map(Language::name)
}

/// Runs the `edit` hooks of the file at `label` (absolute `file`, written with `added` as its new text). The formatter
/// rewrites the file in place; the linter and the checks read it. The caller rolls the edit back when the check
/// `blocked`s it.
pub(crate) fn after_edit(
    workspace: &Workspace,
    lane: &Lane,
    label: &str,
    file: &Path,
    added: &str,
) -> EditCheck {
    let language = language_of(label);
    let hooks = covering(lane, On::Edit, label, language);
    let mut check = EditCheck::default();
    if hooks.is_empty() {
        return check;
    }
    if let Some(language) = language
        && hooks.iter().any(|hook| hook.action == Action::Format)
        && let Some(tool) = formatter(language)
    {
        format_file(workspace, lane, &tool, file, &mut check);
    }
    for hook in &hooks {
        match &hook.action {
            Action::Lint => {
                if let Some(language) = language
                    && let Some(tool) = linter(language)
                {
                    lint_file(workspace, lane, &tool, file, hook.mode, &mut check);
                }
            }
            Action::SecretScan | Action::Rule { .. } => {
                let found = findings(&hook.action, added);
                if found.is_empty() {
                    continue;
                }
                let name = hook.action.name();
                check
                    .findings
                    .extend(found.iter().map(|line| format!("{name}: {line}")));
                check.blocked |= hook.mode == Mode::Block;
            }
            Action::Format => {}
        }
    }
    check
}

/// Runs the formatter on `file` and says what it did. Its output changes the file only when it formats it.
fn format_file(
    workspace: &Workspace,
    lane: &Lane,
    tool: &Tool,
    file: &Path,
    check: &mut EditCheck,
) {
    let before = fs::read(file).ok();
    match run_tool(workspace, tool, file) {
        Err(err) => missing_or_failed(lane, tool, "formatting", err, check),
        Ok(output) if !output.status.success() => check.done.push(format!(
            "{} could not format it: {}",
            tool.program,
            tail(&output)
        )),
        Ok(_) => {
            if fs::read(file).ok() != before {
                check.done.push(format!("formatted with {}", tool.program));
            }
        }
    }
}

/// Runs the linter on `file`: its warnings are the lines it prints when it exits non-zero. A block-mode linter's
/// warnings refuse the edit.
fn lint_file(
    workspace: &Workspace,
    lane: &Lane,
    tool: &Tool,
    file: &Path,
    mode: Mode,
    check: &mut EditCheck,
) {
    match run_tool(workspace, tool, file) {
        Err(err) => missing_or_failed(lane, tool, "linting", err, check),
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr);
            let warnings: Vec<String> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .take(LINT_LINES)
                .map(str::to_string)
                .collect();
            if mode == Mode::Block {
                check.blocked = true;
                check
                    .findings
                    .extend(warnings.iter().map(|w| format!("lint: {w}")));
            }
            check.lint.extend(warnings);
        }
    }
}

/// Runs `tool` on `file` from the workspace root, its output captured.
fn run_tool(workspace: &Workspace, tool: &Tool, file: &Path) -> io::Result<std::process::Output> {
    Command::new(tool.program)
        .args(tool.args)
        .arg(file)
        .current_dir(workspace.dir())
        .stdin(Stdio::null())
        .output()
}

/// A tool that is not installed is reported once per lane; any other failure to run it is reported as it is.
fn missing_or_failed(lane: &Lane, tool: &Tool, step: &str, err: io::Error, check: &mut EditCheck) {
    if err.kind() == io::ErrorKind::NotFound {
        let key = format!("missing:{}", tool.program);
        if locked(&lane.reported).insert(key) {
            check.done.push(format!(
                "{} is not installed, so {step} was skipped",
                tool.program
            ));
        }
    } else {
        check
            .done
            .push(format!("{} did not run: {err}", tool.program));
    }
}

/// The last few lines of a tool's output, for a message.
fn tail(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr).into_owned()
        + &String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" | ")
}

/// Runs the `run` hooks before a command starts. Returns the findings of the report-mode hooks, each naming its
/// hook; a finding of a `block` hook refuses the command.
pub(crate) fn before_run(lane: &Lane, command: &str) -> Result<Vec<String>, ToolError> {
    let mut reported = Vec::new();
    // A command has no path, so a run hook's paths do not apply to it.
    let runs: Vec<Hook> = locked(&lane.hooks)
        .iter()
        .filter(|hook| hook.on == On::Run)
        .cloned()
        .collect();
    for hook in runs {
        let found = findings(&hook.action, command);
        if found.is_empty() {
            continue;
        }
        let name = hook.action.name();
        let lines: Vec<String> = found.iter().map(|line| format!("{name}: {line}")).collect();
        if hook.mode == Mode::Block {
            return Err(ToolError::Denied {
                reason: format!(
                    "refused by a hook before the command ran, so it did not run: {}",
                    lines.join("; ")
                ),
            });
        }
        reported.extend(lines);
    }
    Ok(reported)
}

/// The `apz-hook` entry point: a Claude Code hook call. Its input is the hook's JSON on stdin; `--hooks <file>` names
/// the project's hooks (a JSON list). `run` reads the command, `pre` the text an edit writes, `post` the file an edit
/// wrote. A refusal exits 2 with its reason on stderr, which Claude Code shows the model.
pub fn hook_main(args: &[String]) -> std::process::ExitCode {
    let refuse = |reason: String| {
        eprintln!("{reason}");
        std::process::ExitCode::from(2)
    };
    let fail = |reason: String| {
        eprintln!("agent-harness apz-hook: {reason}");
        std::process::ExitCode::FAILURE
    };
    let [event, flag, file] = args else {
        return fail("usage: apz-hook <run|pre|post> --hooks <file>".into());
    };
    if flag != "--hooks" {
        return fail("usage: apz-hook <run|pre|post> --hooks <file>".into());
    }
    let hooks: Vec<Hook> = match fs::read_to_string(file)
        .map_err(|err| err.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|err| err.to_string()))
    {
        Ok(hooks) => hooks,
        Err(err) => return fail(format!("cannot read the hooks in {file}: {err}")),
    };
    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        return fail(format!("cannot read the hook's input: {err}"));
    }
    let input: Value = match serde_json::from_str(&input) {
        Ok(input) => input,
        Err(err) => return fail(format!("the hook's input is not JSON: {err}")),
    };
    let tool_input = &input["tool_input"];
    let on = match event.as_str() {
        "run" => On::Run,
        "pre" => On::Edit,
        "post" => On::Edit,
        other => return fail(format!("unknown event {other}")),
    };
    // A pre-edit check reads the text the edit writes; a run reads the command.
    let text = match event.as_str() {
        "run" => tool_input["command"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        "pre" => [
            tool_input["content"].as_str(),
            tool_input["new_string"].as_str(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n"),
        _ => String::new(),
    };
    if event == "pre" || event == "run" {
        for hook in hooks.iter().filter(|hook| hook.on == on) {
            let found = findings(&hook.action, &text);
            if found.is_empty() || hook.mode != Mode::Block {
                continue;
            }
            return refuse(format!(
                "refused by a hook: {}",
                found
                    .iter()
                    .map(|line| format!("{}: {line}", hook.action.name()))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        return std::process::ExitCode::SUCCESS;
    }
    // `post`: the formatter and linter on the file the edit wrote.
    let Some(path) = tool_input["file_path"].as_str() else {
        return std::process::ExitCode::SUCCESS;
    };
    let file_path = Path::new(path);
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(err) => return fail(format!("no working directory: {err}")),
    };
    let label = file_path
        .strip_prefix(&root)
        .unwrap_or(file_path)
        .display()
        .to_string();
    let workspace = Workspace::at(root);
    let lane = Lane::new();
    set(&lane, hooks);
    let check = after_edit(&workspace, &lane, &label, file_path, "");
    if check.is_empty() {
        return std::process::ExitCode::SUCCESS;
    }
    if check.blocked {
        return refuse(check.summary());
    }
    println!("{}", check.summary());
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_proto::hooks::Action;
    use serde_json::json;

    /// A scratch workspace, with one file in it.
    fn workspace(name: &str) -> Workspace {
        let root = std::env::temp_dir().join(format!("apz-hooks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("a scratch workspace");
        Workspace::at(root)
    }

    fn hook(on: On, action: Action, mode: Mode) -> Hook {
        Hook {
            on,
            paths: Vec::new(),
            languages: Vec::new(),
            action,
            mode,
        }
    }

    /// Whether `tool` runs here: the formatter of a test is one the container has.
    fn installed(tool: &str) -> bool {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn every_default_formatted_or_linted_language_has_its_tool() {
        for language in apz_proto::hooks::FORMATTED {
            assert!(formatter(language).is_some(), "no formatter for {language}");
        }
        for language in apz_proto::hooks::LINTED {
            assert!(linter(language).is_some(), "no linter for {language}");
        }
        let names: Vec<&str> = Language::ALL.iter().map(|l| l.name()).collect();
        assert_eq!(names, apz_proto::hooks::LANGUAGES);
    }

    #[test]
    fn a_secret_in_an_edit_is_flagged_or_refused_by_its_mode() {
        let ws = workspace("edit-secret");
        let lane = Lane::new();
        let key = format!("sk-ant-{}", "a1".repeat(12));
        let file = ws.dir().join("notes.txt");
        fs::write(&file, "keep\n").expect("a file");

        set(
            &lane,
            vec![hook(On::Edit, Action::SecretScan, Mode::Report)],
        );
        let reported = after_edit(&ws, &lane, "notes.txt", &file, &format!("key = {key}\n"));
        assert!(!reported.blocked);
        assert_eq!(reported.findings.len(), 1, "{reported:?}");

        set(&lane, vec![hook(On::Edit, Action::SecretScan, Mode::Block)]);
        let refused = after_edit(&ws, &lane, "notes.txt", &file, &format!("key = {key}\n"));
        assert!(refused.blocked, "{refused:?}");
        assert!(
            !refused.findings[0].contains(&key),
            "the value is never reported"
        );
        let clean = after_edit(&ws, &lane, "notes.txt", &file, "nothing here\n");
        assert!(clean.is_empty() && !clean.blocked);
    }

    #[test]
    fn a_run_with_a_secret_is_refused_in_block_mode_and_reported_otherwise() {
        let lane = Lane::new();
        let key = format!("ghp_{}", "x9".repeat(20));
        let command = format!("curl -H 'token: {key}' https://example.com");
        set(&lane, vec![hook(On::Run, Action::SecretScan, Mode::Block)]);
        let refused = before_run(&lane, &command).expect_err("a block refuses");
        assert!(refused.to_string().contains("did not run"), "{refused}");
        set(&lane, vec![hook(On::Run, Action::SecretScan, Mode::Report)]);
        let reported = before_run(&lane, &command).expect("a report lets it run");
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert!(before_run(&lane, "ls -la").expect("clean").is_empty());
    }

    #[test]
    fn a_rust_file_written_badly_formatted_comes_back_formatted_and_says_so() {
        if !installed("rustfmt") {
            eprintln!("skipped: rustfmt is not installed here");
            return;
        }
        let ws = workspace("format");
        let lane = Lane::new();
        set(&lane, vec![hook(On::Edit, Action::Format, Mode::Report)]);
        let file = ws.dir().join("lib.rs");
        fs::write(&file, "fn main(){let x=1;println!(\"{}\",x);}\n").expect("a file");
        let check = after_edit(&ws, &lane, "lib.rs", &file, "");
        assert!(
            check.done.iter().any(|d| d == "formatted with rustfmt"),
            "{check:?}"
        );
        let formatted = fs::read_to_string(&file).expect("the file");
        assert!(formatted.contains("fn main() {"), "{formatted}");
        assert!(check.summary().contains("formatted with rustfmt"));
    }

    #[test]
    fn a_missing_formatter_is_reported_once_and_not_an_error() {
        let ws = workspace("missing");
        let lane = Lane::new();
        set(&lane, vec![hook(On::Edit, Action::Format, Mode::Report)]);
        let file = ws.dir().join("a.kt");
        fs::write(&file, "fun main() {}\n").expect("a file");
        let tool = Tool {
            program: "apz-no-such-formatter",
            args: &[],
        };
        let mut first = EditCheck::default();
        missing_or_failed(
            &lane,
            &tool,
            "formatting",
            io::Error::from(io::ErrorKind::NotFound),
            &mut first,
        );
        let mut second = EditCheck::default();
        missing_or_failed(
            &lane,
            &tool,
            "formatting",
            io::Error::from(io::ErrorKind::NotFound),
            &mut second,
        );
        assert_eq!(first.done.len(), 1, "{first:?}");
        assert!(second.done.is_empty(), "{second:?}");
        assert!(!first.blocked);
    }

    /// `serve`'s reply for `tool`, parsed.
    fn serve_json(ws: &Workspace, tool: &str, input: &Value) -> Result<Value, ToolError> {
        let text = crate::v2::serve(ws, 1, tool, input)?;
        Ok(serde_json::from_str(&text).expect("JSON"))
    }

    #[test]
    fn an_edit_that_writes_a_secret_in_block_mode_is_rolled_back_and_says_why() {
        let ws = workspace("edit-block");
        set_for(&ws, vec![hook(On::Edit, Action::SecretScan, Mode::Block)]);
        let file = ws.dir().join("cfg.txt");
        fs::write(&file, "old\n").expect("a file");
        let key = format!("sk-ant-{}", "a1".repeat(12));
        let reply = serve_json(
            &ws,
            "edit",
            &json!({"edits": [{"path": "cfg.txt", "old": "old", "new": format!("token = {key}")}]}),
        )
        .expect("the call answers per file");
        let entry = &reply["files"][0];
        assert_eq!(entry["ok"], json!(false), "{reply}");
        assert!(
            entry["error"]
                .as_str()
                .unwrap_or_default()
                .contains("refused by a hook"),
            "{reply}"
        );
        assert_eq!(
            fs::read_to_string(&file).expect("the file"),
            "old\n",
            "the file keeps its text"
        );
        let created = ws.dir().join("new.txt");
        let reply = serve_json(
            &ws,
            "edit",
            &json!({"create": [{"path": "new.txt", "content": format!("{key}\n")}]}),
        )
        .expect("the call answers per file");
        assert_eq!(reply["files"][0]["ok"], json!(false), "{reply}");
        assert!(!created.exists(), "a refused create leaves no file");
    }

    #[test]
    fn an_edit_that_writes_a_secret_in_report_mode_goes_through_and_is_flagged() {
        let ws = workspace("edit-report");
        set_for(&ws, vec![hook(On::Edit, Action::SecretScan, Mode::Report)]);
        let key = format!("sk-ant-{}", "a1".repeat(12));
        let reply = serve_json(
            &ws,
            "edit",
            &json!({"create": [{"path": "notes.txt", "content": format!("key = {key}\n")}]}),
        )
        .expect("the call answers");
        let entry = &reply["files"][0];
        assert_eq!(entry["ok"], json!(true), "{reply}");
        assert!(
            entry["check"]["summary"]
                .as_str()
                .unwrap_or_default()
                .contains("secret scan"),
            "{reply}"
        );
        assert!(ws.dir().join("notes.txt").exists());
    }

    #[test]
    fn a_command_with_a_secret_in_block_mode_is_refused_through_the_run_tool() {
        let ws = workspace("run-block");
        set_for(&ws, vec![hook(On::Run, Action::SecretScan, Mode::Block)]);
        let key = format!("ghp_{}", "x9".repeat(20));
        let refused = serve_json(&ws, "run", &json!({"command": format!("echo {key}")}));
        assert!(refused.is_err(), "a block-mode secret refuses the command");
        let ran =
            serve_json(&ws, "run", &json!({"command": "echo fine"})).expect("a clean command runs");
        assert_eq!(ran["exit"], json!(0), "{ran}");
    }
}
