//! `code_engine`: a full coding agent (Claude Code, Codex or OpenCode) run headless
//! inside the VM, pointed at the plaza's model gateway so no key ever enters the VM.
//!
//! The engine is installed on first use: `nodejs` and `npm` through apk (as root), then
//! the engine's npm package into the agent's global prefix (as the agent). Each run goes
//! through `/usr/bin/env`, which sets the engine's environment on the process and starts
//! the engine with the prompt as one argument, never through a shell string.

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::bash::{self, Privilege};
use crate::files::Workspace;

const DEFAULT_TIMEOUT_S: u64 = 900;
const MIN_TIMEOUT_S: u64 = 30;
const MAX_TIMEOUT_S: u64 = 3600;
/// npm's global prefix for the engines; their binaries are in its `bin`.
const NPM_PREFIX: &str = "/workspace/.npm-global";
/// An npm install of an engine may take a while on a cold cache.
const INSTALL_TIMEOUT_S: u64 = 600;
/// The most output a run returns, and the tail kept from a failure.
const TAIL_BYTES: usize = 16 * 1024;
const ENV_PROGRAM: &str = "/usr/bin/env";
/// Set for the engine on top of the bash tool's environment. `plaza.gateway` is relayed
/// to the plaza's model gateway, so the placeholder keys are not real credentials.
const ENGINE_ENV: &[(&str, &str)] = &[
    ("ANTHROPIC_BASE_URL", "http://plaza.gateway/anthropic"),
    ("ANTHROPIC_API_KEY", "plaza"),
    ("OPENAI_BASE_URL", "http://plaza.gateway/openai/v1"),
    ("OPENAI_API_KEY", "plaza"),
    ("NO_PROXY", ""),
    ("DISABLE_AUTOUPDATER", "1"),
    ("CI", "1"),
];

/// Runs one job through the chosen engine and returns its final report.
pub fn run(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let job = Job::parse(workspace, input)?;
    ensure_installed(workspace, job.engine)?;
    let argv = command_args(job.engine, &job.prompt, workspace.dir());
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let outcome = bash::execute(&job.dir, ENV_PROGRAM, &refs, job.timeout_s, Privilege::Agent);
    match (job.engine, outcome) {
        (Engine::Claude, Ok(text)) => claude_report(strip_exit_line(&text)),
        (_, Ok(text)) => Ok(tail(&text)),
        (_, Err(text)) => Err(tail(&text)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Claude,
    Codex,
    OpenCode,
}

impl Engine {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            _ => Err(format!("invalid input: \"engine\" must be claude, codex or opencode, not {name:?}")),
        }
    }

    /// The npm package that provides the engine.
    fn package(self) -> &'static str {
        match self {
            Self::Claude => "@anthropic-ai/claude-code",
            Self::Codex => "@openai/codex",
            Self::OpenCode => "opencode-ai",
        }
    }

    /// The command the package installs.
    fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }

    /// The arguments of one headless run; the prompt is a single argument.
    fn args(self, prompt: &str) -> Vec<String> {
        let words: Vec<&str> = match self {
            // Headless engines can't ask a human for approval, so their own per-command prompts
            // are off. That never widens what the agent can reach: the engine runs inside the
            // agent's own microVM (no NIC, vsock only, internet only via the hub's audited
            // egress, keys held by apid), the same place the agent's bash tool runs. Codex keeps
            // its own kernel sandbox on top (writes confined to the workspace).
            Self::Claude => vec![
                "-p",
                prompt,
                "--output-format",
                "json",
                "--permission-mode",
                "bypassPermissions",
            ],
            Self::Codex => vec!["exec", "--skip-git-repo-check", "--sandbox", "workspace-write", prompt],
            Self::OpenCode => vec!["run", prompt],
        };
        words.into_iter().map(str::to_string).collect()
    }
}

/// A validated job: the engine, its prompt, the folder it runs in and its timeout.
struct Job {
    engine: Engine,
    prompt: String,
    dir: PathBuf,
    timeout_s: u64,
}

impl Job {
    fn parse(workspace: &Workspace, input: &Value) -> Result<Self, String> {
        let engine = Engine::parse(input.get("engine").and_then(Value::as_str).unwrap_or_default())?;
        let prompt = match input.get("prompt").and_then(Value::as_str) {
            Some(prompt) if !prompt.trim().is_empty() => prompt.to_string(),
            _ => return Err("invalid input: \"prompt\" must be a non-empty string".to_string()),
        };
        let timeout_s = match input.get("timeout_s") {
            None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
            Some(value) => value
                .as_u64()
                .filter(|secs| (MIN_TIMEOUT_S..=MAX_TIMEOUT_S).contains(secs))
                .ok_or_else(|| {
                    format!("invalid input: \"timeout_s\" must be a whole number of seconds from {MIN_TIMEOUT_S} to {MAX_TIMEOUT_S}")
                })?,
        };
        let dir = job_dir(workspace, input.get("path"))?;
        Ok(Self { engine, prompt, dir, timeout_s })
    }
}

/// The folder a run starts in: the workspace, or `path` under it. A `path` that is
/// absolute, has a `..` part, is not a folder, or leads out of the workspace through a
/// symlink is refused.
fn job_dir(workspace: &Workspace, path: Option<&Value>) -> Result<PathBuf, String> {
    let root = workspace.dir();
    let raw = match path {
        None | Some(Value::Null) => return Ok(root.to_path_buf()),
        Some(value) => value.as_str().ok_or_else(|| "invalid input: \"path\" must be a string".to_string())?,
    };
    let refused = || format!("invalid input: \"path\" must be a folder inside the workspace, not {raw:?}");
    let plain = Path::new(raw)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if !plain {
        return Err(refused());
    }
    let canonical_root = root.canonicalize().map_err(|err| format!("unavailable: cannot open the workspace: {err}"))?;
    let canonical = root.join(raw).canonicalize().map_err(|_| refused())?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
        return Err(refused());
    }
    Ok(canonical)
}

/// Installs the engine when its binary is missing: `nodejs` and `npm` through apk, then
/// the engine's package into the agent's npm prefix. A failure says which step failed.
fn ensure_installed(workspace: &Workspace, engine: Engine) -> Result<(), String> {
    let binary = Path::new(NPM_PREFIX).join("bin").join(engine.binary());
    if binary.exists() {
        return Ok(());
    }
    bash::install_packages(workspace, &json!({ "packages": ["nodejs", "npm"] }))
        .map_err(|err| format!("cannot install nodejs and npm for {}: {err}", engine.binary()))?;
    bash::execute(
        workspace.dir(),
        "npm",
        &["install", "-g", "--prefix", NPM_PREFIX, engine.package()],
        INSTALL_TIMEOUT_S,
        Privilege::Agent,
    )
    .map_err(|err| format!("cannot install {}: {}", engine.package(), tail(&err)))?;
    if binary.exists() {
        Ok(())
    } else {
        Err(format!("npm installed {} but {} is missing", engine.package(), binary.display()))
    }
}

/// The `env` arguments that start the engine: `HOME` is the workspace (the engine's own
/// config lives there, not in a `path` subfolder), then the engine's environment, then
/// the binary and its arguments.
fn command_args(engine: Engine, prompt: &str, home: &Path) -> Vec<String> {
    let mut argv = vec![format!("HOME={}", home.display())];
    argv.extend(ENGINE_ENV.iter().map(|(key, value)| format!("{key}={value}")));
    argv.push(engine.binary().to_string());
    argv.extend(engine.args(prompt));
    argv
}

/// Claude's `--output-format json` report: its `result`, then a footer with the turn
/// count and cost when it has them. An error report is an `Err`; output that is not
/// JSON (cut or unexpected) is returned as it is, tail-capped.
fn claude_report(body: &str) -> Result<String, String> {
    let Ok(report) = serde_json::from_str::<Value>(body.trim()) else {
        return Ok(tail(body));
    };
    let result = report.get("result").and_then(Value::as_str).unwrap_or_default();
    if report.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(if result.is_empty() { tail(body) } else { tail(result) });
    }
    let mut text = tail(result);
    let mut footer = Vec::new();
    if let Some(turns) = report.get("num_turns").and_then(Value::as_u64) {
        footer.push(format!("num_turns: {turns}"));
    }
    if let Some(cost) = report.get("total_cost_usd").and_then(Value::as_f64) {
        footer.push(format!("total_cost_usd: {cost}"));
    }
    if !footer.is_empty() {
        text.push_str(&format!("\n\n[{}]", footer.join(", ")));
    }
    Ok(text)
}

/// The output without the `exit <code>` line that the bash helper appends.
fn strip_exit_line(text: &str) -> &str {
    let last_line = text.rfind('\n').map_or(0, |at| at + 1);
    if text[last_line..].starts_with("exit") {
        &text[..last_line]
    } else {
        text
    }
}

/// The last `TAIL_BYTES` of `text`, on a character boundary, with a note when cut.
fn tail(text: &str) -> String {
    if text.len() <= TAIL_BYTES {
        return text.to_string();
    }
    let mut start = text.len() - TAIL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[earlier output cut]\n{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("apz-harness-engines-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    #[test]
    fn each_engine_has_its_package_binary_and_run_arguments() {
        assert_eq!(Engine::Claude.package(), "@anthropic-ai/claude-code");
        assert_eq!(Engine::Codex.package(), "@openai/codex");
        assert_eq!(Engine::OpenCode.package(), "opencode-ai");
        assert_eq!(Engine::Claude.binary(), "claude");
        assert_eq!(Engine::Codex.binary(), "codex");
        assert_eq!(Engine::OpenCode.binary(), "opencode");
        assert_eq!(
            Engine::Claude.args("fix the bug"),
            ["-p", "fix the bug", "--output-format", "json", "--permission-mode", "bypassPermissions"]
        );
        assert_eq!(Engine::Codex.args("fix the bug"), ["exec", "--skip-git-repo-check", "--sandbox", "workspace-write", "fix the bug"]);
        assert_eq!(Engine::OpenCode.args("fix the bug"), ["run", "fix the bug"]);
    }

    #[test]
    fn the_command_sets_the_gateway_environment_and_runs_the_binary_last() {
        let argv = command_args(Engine::Claude, "hi", Path::new("/workspace"));
        for pair in [
            "HOME=/workspace",
            "ANTHROPIC_BASE_URL=http://plaza.gateway/anthropic",
            "ANTHROPIC_API_KEY=plaza",
            "OPENAI_BASE_URL=http://plaza.gateway/openai/v1",
            "OPENAI_API_KEY=plaza",
            "NO_PROXY=",
            "DISABLE_AUTOUPDATER=1",
            "CI=1",
        ] {
            assert!(argv.iter().any(|arg| arg == pair), "{pair}");
        }
        let at = argv.iter().position(|arg| arg == "claude").expect("the binary");
        assert_eq!(argv[at + 1..], ["-p", "hi", "--output-format", "json", "--permission-mode", "bypassPermissions"]);
    }

    #[test]
    fn a_claude_report_returns_its_result_with_the_turns_and_cost() {
        let body = r#"{"type":"result","subtype":"success","is_error":false,"result":"Fixed it.","num_turns":4,"total_cost_usd":0.0123}"#;
        assert_eq!(claude_report(body).expect("a report"), "Fixed it.\n\n[num_turns: 4, total_cost_usd: 0.0123]");
        assert_eq!(claude_report(r#"{"result":"Done."}"#).expect("a report"), "Done.");
    }

    #[test]
    fn a_claude_error_report_is_an_error_and_plain_output_is_kept() {
        let body = r#"{"is_error":true,"result":"Context is full."}"#;
        assert_eq!(claude_report(body).expect_err("an error report"), "Context is full.");
        assert_eq!(claude_report("not json\n").expect("kept as it is"), "not json\n");
    }

    #[test]
    fn the_shell_helper_exit_line_is_stripped() {
        assert_eq!(strip_exit_line("{\"a\":1}\nexit 0"), "{\"a\":1}\n");
        assert_eq!(strip_exit_line("one\ntwo\nexit 0"), "one\ntwo\n");
        assert_eq!(strip_exit_line("exit 0"), "");
    }

    #[test]
    fn a_long_output_keeps_its_end_on_a_character_boundary() {
        let text = format!("{}end", "é".repeat(TAIL_BYTES));
        let kept = tail(&text);
        assert!(kept.ends_with("end") && kept.len() <= TAIL_BYTES + 32, "{}", kept.len());
        assert_eq!(tail("short"), "short");
    }

    #[test]
    fn bad_input_is_refused_before_anything_is_installed() {
        let workspace = scratch("input");
        for (input, reason) in [
            (json!({ "engine": "vim", "prompt": "x" }), "engine"),
            (json!({ "prompt": "x" }), "engine"),
            (json!({ "engine": "codex", "prompt": "  " }), "prompt"),
            (json!({ "engine": "codex", "prompt": "x", "timeout_s": 5 }), "timeout_s"),
            (json!({ "engine": "codex", "prompt": "x", "timeout_s": 3601 }), "timeout_s"),
            (json!({ "engine": "codex", "prompt": "x", "path": "../up" }), "path"),
        ] {
            let err = run(&workspace, &input).expect_err("refused");
            assert!(err.contains(reason), "{input}: {err}");
        }
    }

    #[test]
    fn a_path_must_be_a_folder_inside_the_workspace() {
        let workspace = scratch("paths");
        std::fs::create_dir_all(workspace.dir().join("project")).expect("a folder");
        let outside = scratch("outside");
        std::os::unix::fs::symlink(outside.dir(), workspace.dir().join("escape")).expect("a link");
        assert!(job_dir(&workspace, None).is_ok());
        assert!(job_dir(&workspace, Some(&json!("project"))).expect("a folder").ends_with("project"));
        for path in ["/etc", "../outside", "project/../../outside", "escape", "missing"] {
            assert!(job_dir(&workspace, Some(&json!(path))).is_err(), "{path}");
        }
    }
}
