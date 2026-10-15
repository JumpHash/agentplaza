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
pub(crate) const NPM_PREFIX: &str = "/workspace/.npm-global";
/// An npm install of an engine may take a while on a cold cache.
pub(crate) const INSTALL_TIMEOUT_S: u64 = 600;
/// The most output a run returns, and the tail kept from a failure.
const TAIL_BYTES: usize = 16 * 1024;
pub(crate) const ENV_PROGRAM: &str = "/usr/bin/env";
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
/// The plaza gateway as a VM names it. The hub relays it to apid, which adds the agent's
/// id, so the VM never needs its own id.
const GATEWAY: &str = "http://plaza.gateway";
/// The models the gateway's `opencode` responses route serves (the Codex wire protocol).
const RESPONSES_PREFIXES: [&str; 3] = ["gpt-", "grok-", "muse-spark-"];

/// Runs one job through the chosen engine and returns its final report.
pub fn run(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let job = Job::parse(workspace, input)?;
    ensure_installed(workspace, job.engine)?;
    let argv = command_args(job.engine, &job.prompt, workspace.dir(), &job.route);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let outcome = bash::execute(
        &job.dir,
        ENV_PROGRAM,
        &refs,
        job.timeout_s,
        Privilege::Agent,
    );
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
            // Gemini CLI, Qwen Code and Kilo Code have no headless run: their ACP adapters are the
            // only way they run.
            "gemini" => Err(
                "invalid input: \"engine\" is gemini: Gemini CLI runs over ACP only; switch engines_mode to acp"
                    .to_string(),
            ),
            "qwen" => Err(
                "invalid input: \"engine\" is qwen: Qwen Code runs over ACP only; switch engines_mode to acp"
                    .to_string(),
            ),
            "kilo" => Err(
                "invalid input: \"engine\" is kilo: Kilo Code runs over ACP only; switch engines_mode to acp"
                    .to_string(),
            ),
            _ => Err(format!(
                "invalid input: \"engine\" must be claude, codex or opencode, not {name:?}"
            )),
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

    /// The arguments of one headless run; the prompt is a single argument. `model` goes on
    /// the engine's model flag; OpenCode takes its model from its config instead.
    fn args(self, prompt: &str, model: Option<&str>) -> Vec<String> {
        let mut words: Vec<&str> = match self {
            // Inside its own VM an agent may do anything (the owner's choice), so the engines run
            // without their own approval prompts or sandboxes. That never widens what the agent
            // can reach: the engine runs inside the agent's microVM (no NIC, vsock only, internet
            // only via the hub's audited egress, keys held by apid), where bash runs as root too.
            Self::Claude => vec![
                "-p",
                prompt,
                "--output-format",
                "json",
                "--permission-mode",
                "bypassPermissions",
            ],
            Self::Codex => vec![
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
            ],
            Self::OpenCode => vec!["run", prompt],
        };
        if let (Self::Codex, Some(model)) = (self, model) {
            words.extend(["-m", model]);
        }
        if self == Self::Codex {
            words.push(prompt);
        }
        if let (Self::Claude, Some(model)) = (self, model) {
            words.extend(["--model", model]);
        }
        words.into_iter().map(str::to_string).collect()
    }
}

/// A validated job: the engine, its prompt, the folder it runs in, its timeout and the
/// route to its model.
struct Job {
    engine: Engine,
    prompt: String,
    dir: PathBuf,
    timeout_s: u64,
    route: Route,
}

impl Job {
    fn parse(workspace: &Workspace, input: &Value) -> Result<Self, String> {
        let engine = Engine::parse(
            input
                .get("engine")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?;
        let prompt = match input.get("prompt").and_then(Value::as_str) {
            Some(prompt) if !prompt.trim().is_empty() => prompt.to_string(),
            _ => return Err("invalid input: \"prompt\" must be a non-empty string".to_string()),
        };
        let timeout_s = timeout_arg(input)?;
        let model = optional_text(input, "model")?;
        let route = model_route(engine, model)?;
        let dir = job_dir(workspace, input.get("path"))?;
        Ok(Self {
            engine,
            prompt,
            dir,
            timeout_s,
            route,
        })
    }
}

/// The `timeout_s` of a call: a whole number of seconds from `MIN_TIMEOUT_S` to `MAX_TIMEOUT_S`,
/// `DEFAULT_TIMEOUT_S` when absent. The ACP engines take the same limits.
pub(crate) fn timeout_arg(input: &Value) -> Result<u64, String> {
    match input.get("timeout_s") {
        None | Some(Value::Null) => Ok(DEFAULT_TIMEOUT_S),
        Some(value) => value
            .as_u64()
            .filter(|secs| (MIN_TIMEOUT_S..=MAX_TIMEOUT_S).contains(secs))
            .ok_or_else(|| {
                format!("invalid input: \"timeout_s\" must be a whole number of seconds from {MIN_TIMEOUT_S} to {MAX_TIMEOUT_S}")
            }),
    }
}

/// An optional string argument: absent or null is `None`; anything else must be a non-empty string.
pub(crate) fn optional_text<'a>(input: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(Some)
            .ok_or_else(|| format!("invalid input: {key:?} must be a non-empty string")),
    }
}

/// The folder an ACP engine session starts in: [`job_dir`]'s rules for `path`.
pub(crate) fn job_folder(workspace: &Workspace, path: Option<&Value>) -> Result<PathBuf, String> {
    job_dir(workspace, path)
}

/// The folder a run starts in: the workspace, or `path` under it. A `path` that is
/// absolute, has a `..` part, is not a folder, or leads out of the workspace through a
/// symlink is refused.
fn job_dir(workspace: &Workspace, path: Option<&Value>) -> Result<PathBuf, String> {
    let root = workspace.dir();
    let raw = match path {
        None | Some(Value::Null) => return Ok(root.to_path_buf()),
        Some(value) => value
            .as_str()
            .ok_or_else(|| "invalid input: \"path\" must be a string".to_string())?,
    };
    let refused =
        || format!("invalid input: \"path\" must be a folder inside the workspace, not {raw:?}");
    let plain = Path::new(raw)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if !plain {
        return Err(refused());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|err| format!("unavailable: cannot open the workspace: {err}"))?;
    let canonical = root.join(raw).canonicalize().map_err(|_| refused())?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
        return Err(refused());
    }
    Ok(canonical)
}

/// Installs `nodejs` and `npm` through apk; the engines' npm installs need both.
pub(crate) fn ensure_node(workspace: &Workspace) -> Result<(), String> {
    bash::install_packages(workspace, &json!({ "packages": ["nodejs", "npm"] })).map(|_| ())
}

/// Installs the engine when its binary is missing: `nodejs` and `npm` through apk, then
/// the engine's package into the agent's npm prefix. A failure says which step failed.
fn ensure_installed(workspace: &Workspace, engine: Engine) -> Result<(), String> {
    let binary = Path::new(NPM_PREFIX).join("bin").join(engine.binary());
    if binary.exists() {
        return Ok(());
    }
    ensure_node(workspace).map_err(|err| {
        format!(
            "cannot install nodejs and npm for {}: {err}",
            engine.binary()
        )
    })?;
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
        Err(format!(
            "npm installed {} but {} is missing",
            engine.package(),
            binary.display()
        ))
    }
}

/// The `KEY=VALUE` pairs that start an engine: `HOME` is the workspace (the engine's own config
/// lives there, not in a `path` subfolder), then the engine's environment, then the route's
/// settings (`env` applies its pairs in order, so they override the defaults).
fn engine_env(home: &Path, route: &Route) -> Vec<String> {
    let mut pairs = vec![format!("HOME={}", home.display())];
    pairs.extend(
        ENGINE_ENV
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    pairs.extend(
        route
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    pairs
}

/// What an ACP engine session starts with, for the catalogue's engine `engine_id`: its `KEY=VALUE`
/// pairs (see [`engine_env`]) and the model id the engine is given, from the same routes as the
/// headless runs. OpenCode's model id is its provider block's `<id>/<model>`, which its config
/// names as the default model.
pub(crate) fn acp_launch(
    workspace: &Workspace,
    engine_id: &str,
    model: Option<&str>,
) -> Result<(Vec<String>, Option<String>), String> {
    let home = workspace.dir();
    match engine_id {
        // Gemini CLI and Qwen Code read their model from the environment, so the session is given none.
        "gemini" => return Ok((engine_env(home, &gemini_route(model)?), None)),
        "qwen" => return Ok((engine_env(home, &qwen_route(model)?), None)),
        "opencode" => {
            let (route, model_id) = opencode_schema_launch("OPENCODE_CONFIG_CONTENT", model)?;
            return Ok((engine_env(home, &route), model_id));
        }
        "kilo" => {
            let (route, model_id) = opencode_schema_launch("KILO_CONFIG_CONTENT", model)?;
            return Ok((engine_env(home, &route), model_id));
        }
        _ => {}
    }
    let engine = Engine::parse(engine_id)?;
    let route = model_route(engine, model)?;
    Ok((engine_env(home, &route), route.model.clone()))
}

/// The route for Qwen Code, which runs over ACP only and speaks chat completions: the placeholder
/// key, the gateway route of the model's provider (`openai`, `openrouter` or `opencode`, as Codex's
/// routes name them), and the model without its provider as `OPENAI_MODEL`. No model keeps the
/// `openai` route and the engine's own default.
fn qwen_route(model: Option<&str>) -> Result<Route, String> {
    let (route, model_name) = match model {
        None => ("openai", None),
        Some(model) => {
            let (provider, rest) = split_model(model)?;
            match provider {
                "openai" | "openrouter" | "opencode" => (provider, Some(rest)),
                _ => {
                    return Err(format!(
                        "invalid input: \"model\" is {model:?}; Qwen Code takes openai/<model>, openrouter/<model> or opencode/<model> models"
                    ));
                }
            }
        }
    };
    let mut env = vec![
        ("OPENAI_API_KEY".to_string(), "plaza".to_string()),
        (
            "OPENAI_BASE_URL".to_string(),
            format!("{GATEWAY}/{route}/v1"),
        ),
    ];
    if let Some(model_name) = model_name {
        env.push(("OPENAI_MODEL".to_string(), model_name.to_string()));
    }
    Ok(Route { env, model: None })
}

/// The route and the session model of an engine that reads an OpenCode-schema config from `env_key`
/// (OpenCode its `OPENCODE_CONFIG_CONTENT`, Kilo Code its `KILO_CONFIG_CONTENT`). The session names
/// the model as its provider block's `<id>/<model>`. No model is the engine's own default.
fn opencode_schema_launch(
    env_key: &str,
    model: Option<&str>,
) -> Result<(Route, Option<String>), String> {
    let Some(model) = model else {
        return Ok((Route::default(), None));
    };
    let (provider, rest) = split_model(model)?;
    let route = opencode_config_route(env_key, provider, rest)?;
    let (id, _, _) = opencode_provider_for(provider, rest);
    Ok((route, Some(format!("{id}/{rest}"))))
}

/// The route for Gemini CLI, which runs over ACP only: the gateway's Gemini route, the placeholder
/// key, and the model as `GEMINI_MODEL`. The model is `gemini/<model>`; no other provider is served
/// to the CLI. The workspace is trusted because it is the agent's own folder inside its VM.
fn gemini_route(model: Option<&str>) -> Result<Route, String> {
    let mut env = vec![
        (
            "GOOGLE_GEMINI_BASE_URL".to_string(),
            format!("{GATEWAY}/gemini"),
        ),
        ("GEMINI_API_KEY".to_string(), "plaza".to_string()),
        ("GEMINI_CLI_TRUST_WORKSPACE".to_string(), "true".to_string()),
    ];
    if let Some(model) = model {
        let (provider, rest) = split_model(model)?;
        if provider != "gemini" {
            return Err(format!(
                "invalid input: \"model\" is {model:?}; Gemini CLI takes gemini/<model> models"
            ));
        }
        env.push(("GEMINI_MODEL".to_string(), rest.to_string()));
    }
    Ok(Route { env, model: None })
}

/// The `env` arguments that start the headless engine: its pairs (see [`engine_env`]), then the
/// binary and its arguments.
fn command_args(engine: Engine, prompt: &str, home: &Path, route: &Route) -> Vec<String> {
    let mut argv = engine_env(home, route);
    argv.push(engine.binary().to_string());
    argv.extend(engine.args(prompt, route.model.as_deref()));
    argv
}

/// How a run reaches its model: the environment settings the engine needs, and the model
/// name the engine is given (the plaza id after its provider, as the gateway names it).
#[derive(Debug, Default, PartialEq, Eq)]
struct Route {
    env: Vec<(String, String)>,
    model: Option<String>,
}

/// Splits a plaza model id `<provider>/<rest>` on its first `/`.
fn split_model(model: &str) -> Result<(&str, &str), String> {
    match model.split_once('/') {
        Some((provider, rest)) if !provider.is_empty() && !rest.is_empty() => Ok((provider, rest)),
        _ => Err(format!(
            "invalid input: \"model\" must be <provider>/<model>, not {model:?}"
        )),
    }
}

/// The OpenCode provider block that serves `model` from `provider`: its id, its npm package
/// and its base URL. The gateway's opencode routes differ by model shape, so each shape gets
/// its own block: Responses for `gpt-`, `grok-` and `muse-spark-`, Messages for `claude-`,
/// and chat for the rest. Every other provider (openrouter included) gets one
/// OpenAI-compatible chat block, `plaza-<provider>`.
fn opencode_provider_for(provider: &str, model: &str) -> (String, &'static str, String) {
    if provider == "opencode" {
        let base_url = format!("{GATEWAY}/opencode/v1");
        if model.starts_with("claude-") {
            return (
                "plaza-opencode-anthropic".to_string(),
                "@ai-sdk/anthropic",
                base_url,
            );
        }
        if RESPONSES_PREFIXES
            .iter()
            .any(|prefix| model.starts_with(prefix))
        {
            return (
                "plaza-opencode-responses".to_string(),
                "@ai-sdk/openai",
                base_url,
            );
        }
        return (
            "plaza-opencode".to_string(),
            "@ai-sdk/openai-compatible",
            base_url,
        );
    }
    match provider {
        "openai" => (
            "plaza-openai".to_string(),
            "@ai-sdk/openai",
            format!("{GATEWAY}/openai/v1"),
        ),
        _ => (
            format!("plaza-{provider}"),
            "@ai-sdk/openai-compatible",
            format!("{GATEWAY}/{provider}/v1"),
        ),
    }
}

/// The route for `model` under `engine`; no model keeps the engine's own defaults.
/// Claude Code speaks Anthropic Messages, so it takes the anthropic, openrouter and
/// opencode `claude-` routes. Codex speaks the Responses API, so it takes openai and the
/// opencode `gpt-`, `grok-` and `muse-spark-` models. OpenCode gets its model from the
/// provider block that `opencode_provider_for` picks for it.
fn model_route(engine: Engine, model: Option<&str>) -> Result<Route, String> {
    let Some(model) = model else {
        return Ok(Route::default());
    };
    let (provider, rest) = split_model(model)?;
    let gateway_env = |key: &str, path: &str| vec![(key.to_string(), format!("{GATEWAY}/{path}"))];
    match engine {
        Engine::Claude => {
            let path = match provider {
                "anthropic" | "openrouter" => provider,
                "opencode" if rest.starts_with("claude-") => provider,
                "opencode" => {
                    return Err(format!(
                        "invalid input: \"model\" is {model:?}. Claude Code needs an Anthropic-shaped model; OpenCode serves this one on its chat endpoint, so use OpenCode or Codex for it"
                    ));
                }
                _ => {
                    return Err(format!(
                        "invalid input: \"model\" is {model:?}. Claude Code needs an Anthropic-shaped model: anthropic/..., openrouter/... or opencode/claude-..."
                    ));
                }
            };
            Ok(Route {
                env: gateway_env("ANTHROPIC_BASE_URL", path),
                model: Some(rest.to_string()),
            })
        }
        Engine::Codex => match provider {
            "openai" => Ok(Route {
                env: Vec::new(),
                model: Some(rest.to_string()),
            }),
            "opencode"
                if RESPONSES_PREFIXES
                    .iter()
                    .any(|prefix| rest.starts_with(prefix)) =>
            {
                Ok(Route {
                    env: gateway_env("OPENAI_BASE_URL", "opencode/v1"),
                    model: Some(rest.to_string()),
                })
            }
            "opencode" => Err(format!(
                "invalid input: \"model\" is {model:?}. Codex needs a Responses model on OpenCode (gpt-, grok- or muse-spark-); use OpenCode or Claude Code for this one"
            )),
            "openrouter" => Err(format!(
                "invalid input: \"model\" is {model:?}. Codex needs the OpenAI Responses API, which the gateway's openrouter route does not serve; use OpenCode or Claude Code for it"
            )),
            _ => Err(format!(
                "invalid input: \"model\" is {model:?}. Codex serves openai/... models and opencode gpt-, grok- or muse-spark- models"
            )),
        },
        Engine::OpenCode => opencode_config_route("OPENCODE_CONFIG_CONTENT", provider, rest),
    }
}

/// The route of an engine that reads an OpenCode-schema config from `env_key`: one provider block
/// at the gateway (see [`opencode_provider_for`]), with the model as its default. OpenCode and its
/// fork Kilo Code share this config; only the variable that carries it differs.
fn opencode_config_route(env_key: &str, provider: &str, rest: &str) -> Result<Route, String> {
    if !provider
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(format!(
            "invalid input: \"model\" provider must be letters, digits and dashes, not {provider:?}"
        ));
    }
    let (id, npm, base_url) = opencode_provider_for(provider, rest);
    let config = json!({
        "$schema": "https://opencode.ai/config.json",
        "model": format!("{id}/{rest}"),
        "provider": {
            id.clone(): {
                "npm": npm,
                "name": format!("AgentPlaza {provider}"),
                "options": { "baseURL": base_url, "apiKey": "plaza" },
                "models": { rest: {} }
            }
        }
    });
    Ok(Route {
        env: vec![(env_key.to_string(), config.to_string())],
        model: None,
    })
}

/// Claude's `--output-format json` report: its `result`, then a footer with the turn
/// count and cost when it has them. An error report is an `Err`; output that is not
/// JSON (cut or unexpected) is returned as it is, tail-capped.
fn claude_report(body: &str) -> Result<String, String> {
    let Ok(report) = serde_json::from_str::<Value>(body.trim()) else {
        return Ok(tail(body));
    };
    let result = report
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if report.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(if result.is_empty() {
            tail(body)
        } else {
            tail(result)
        });
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
pub(crate) fn tail(text: &str) -> String {
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
        let dir =
            std::env::temp_dir().join(format!("apz-harness-engines-{name}-{}", std::process::id()));
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
            Engine::Claude.args("fix the bug", None),
            [
                "-p",
                "fix the bug",
                "--output-format",
                "json",
                "--permission-mode",
                "bypassPermissions"
            ]
        );
        assert_eq!(
            Engine::Codex.args("fix the bug", None),
            [
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "fix the bug"
            ]
        );
        assert_eq!(
            Engine::OpenCode.args("fix the bug", None),
            ["run", "fix the bug"]
        );
        assert_eq!(
            Engine::Claude.args("fix", Some("claude-x"))[6..],
            ["--model", "claude-x"]
        );
        assert_eq!(
            Engine::Codex.args("fix", Some("gpt-5")),
            [
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "-m",
                "gpt-5",
                "fix"
            ]
        );
        assert_eq!(
            Engine::OpenCode.args("fix", Some("claude-x")),
            ["run", "fix"]
        );
    }

    #[test]
    fn the_command_sets_the_gateway_environment_and_runs_the_binary_last() {
        let argv = command_args(
            Engine::Claude,
            "hi",
            Path::new("/workspace"),
            &Route::default(),
        );
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
        let at = argv
            .iter()
            .position(|arg| arg == "claude")
            .expect("the binary");
        assert_eq!(
            argv[at + 1..],
            [
                "-p",
                "hi",
                "--output-format",
                "json",
                "--permission-mode",
                "bypassPermissions"
            ]
        );
    }

    #[test]
    fn a_claude_report_returns_its_result_with_the_turns_and_cost() {
        let body = r#"{"type":"result","subtype":"success","is_error":false,"result":"Fixed it.","num_turns":4,"total_cost_usd":0.0123}"#;
        assert_eq!(
            claude_report(body).expect("a report"),
            "Fixed it.\n\n[num_turns: 4, total_cost_usd: 0.0123]"
        );
        assert_eq!(
            claude_report(r#"{"result":"Done."}"#).expect("a report"),
            "Done."
        );
    }

    #[test]
    fn a_claude_error_report_is_an_error_and_plain_output_is_kept() {
        let body = r#"{"is_error":true,"result":"Context is full."}"#;
        assert_eq!(
            claude_report(body).expect_err("an error report"),
            "Context is full."
        );
        assert_eq!(
            claude_report("not json\n").expect("kept as it is"),
            "not json\n"
        );
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
        assert!(
            kept.ends_with("end") && kept.len() <= TAIL_BYTES + 32,
            "{}",
            kept.len()
        );
        assert_eq!(tail("short"), "short");
    }

    #[test]
    fn bad_input_is_refused_before_anything_is_installed() {
        let workspace = scratch("input");
        for (input, reason) in [
            (json!({ "engine": "vim", "prompt": "x" }), "engine"),
            (json!({ "prompt": "x" }), "engine"),
            (json!({ "engine": "codex", "prompt": "  " }), "prompt"),
            (
                json!({ "engine": "codex", "prompt": "x", "timeout_s": 5 }),
                "timeout_s",
            ),
            (
                json!({ "engine": "codex", "prompt": "x", "timeout_s": 3601 }),
                "timeout_s",
            ),
            (
                json!({ "engine": "codex", "prompt": "x", "path": "../up" }),
                "path",
            ),
            (
                json!({ "engine": "codex", "prompt": "x", "model": "opencode/claude-x" }),
                "model",
            ),
            (
                json!({ "engine": "claude", "prompt": "x", "model": "opencode/deepseek-v4-pro" }),
                "Claude Code",
            ),
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
        assert!(
            job_dir(&workspace, Some(&json!("project")))
                .expect("a folder")
                .ends_with("project")
        );
        for path in [
            "/etc",
            "../outside",
            "project/../../outside",
            "escape",
            "missing",
        ] {
            assert!(job_dir(&workspace, Some(&json!(path))).is_err(), "{path}");
        }
    }

    #[test]
    fn a_model_id_splits_on_its_first_slash() {
        assert_eq!(
            split_model("openrouter/anthropic/claude-x").expect("a split"),
            ("openrouter", "anthropic/claude-x")
        );
        assert_eq!(
            split_model("opencode/claude-sonnet-5-5").expect("a split"),
            ("opencode", "claude-sonnet-5-5")
        );
        for bad in ["claude-sonnet-5-5", "/claude", "openrouter/"] {
            assert!(split_model(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn claude_code_points_at_the_anthropic_shaped_gateway_routes() {
        let route =
            model_route(Engine::Claude, Some("openrouter/anthropic/claude-x")).expect("a route");
        assert_eq!(
            route.env,
            [(
                "ANTHROPIC_BASE_URL".to_string(),
                "http://plaza.gateway/openrouter".to_string()
            )]
        );
        assert_eq!(route.model.as_deref(), Some("anthropic/claude-x"));
        let route =
            model_route(Engine::Claude, Some("opencode/claude-sonnet-5-5")).expect("a route");
        assert_eq!(
            route.env,
            [(
                "ANTHROPIC_BASE_URL".to_string(),
                "http://plaza.gateway/opencode".to_string()
            )]
        );
        assert_eq!(route.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(
            model_route(Engine::Claude, None).expect("the default"),
            Route::default()
        );
    }

    #[test]
    fn claude_code_refuses_an_opencode_model_its_route_does_not_serve() {
        let err =
            model_route(Engine::Claude, Some("opencode/deepseek-v4-pro")).expect_err("refused");
        assert!(
            err.contains("Claude Code needs an Anthropic-shaped model"),
            "{err}"
        );
    }

    #[test]
    fn codex_takes_only_the_models_its_wire_protocol_serves() {
        let route = model_route(Engine::Codex, Some("opencode/gpt-5")).expect("a route");
        assert_eq!(
            route.env,
            [(
                "OPENAI_BASE_URL".to_string(),
                "http://plaza.gateway/opencode/v1".to_string()
            )]
        );
        assert_eq!(route.model.as_deref(), Some("gpt-5"));
        let openai = model_route(Engine::Codex, Some("openai/gpt-5")).expect("a route");
        assert!(openai.env.is_empty());
        assert_eq!(openai.model.as_deref(), Some("gpt-5"));
        for refused in [
            "openrouter/anthropic/claude-x",
            "opencode/deepseek-v4-pro",
            "google/gemini-x",
        ] {
            assert!(
                model_route(Engine::Codex, Some(refused)).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn gemini_launch_points_at_the_gateway_and_takes_its_model_from_the_environment() {
        let workspace = scratch("gemini-launch");
        let (env, model) = acp_launch(&workspace, "gemini", None).expect("gemini");
        assert_eq!(env[0], format!("HOME={}", workspace.dir().display()));
        assert!(env.contains(&"GOOGLE_GEMINI_BASE_URL=http://plaza.gateway/gemini".to_string()));
        assert!(env.contains(&"GEMINI_API_KEY=plaza".to_string()));
        assert!(env.contains(&"GEMINI_CLI_TRUST_WORKSPACE=true".to_string()));
        assert!(!env.iter().any(|pair| pair.starts_with("GEMINI_MODEL=")));
        assert_eq!(model, None);

        let (env, model) =
            acp_launch(&workspace, "gemini", Some("gemini/gemini-2.5-pro")).expect("a model");
        assert!(env.contains(&"GEMINI_MODEL=gemini-2.5-pro".to_string()));
        assert_eq!(model, None);
    }

    #[test]
    fn gemini_refuses_a_model_from_another_provider() {
        let workspace = scratch("gemini-refuse");
        let err =
            acp_launch(&workspace, "gemini", Some("anthropic/claude-x")).expect_err("refused");
        assert_eq!(
            err,
            "invalid input: \"model\" is \"anthropic/claude-x\"; Gemini CLI takes gemini/<model> models"
        );
    }

    #[test]
    fn gemini_is_acp_only_under_the_headless_engines() {
        let workspace = scratch("gemini-headless");
        let input = json!({"engine": "gemini", "prompt": "p"});
        let err = Job::parse(&workspace, &input).err().expect("refused");
        assert!(
            err.contains("Gemini CLI runs over ACP only; switch engines_mode to acp"),
            "{err}"
        );
    }

    #[test]
    fn qwen_launch_follows_the_model_provider_to_its_gateway_route() {
        let workspace = scratch("qwen-launch");
        let (env, model) = acp_launch(&workspace, "qwen", None).expect("qwen");
        assert_eq!(env[0], format!("HOME={}", workspace.dir().display()));
        assert!(env.contains(&"OPENAI_API_KEY=plaza".to_string()));
        assert!(env.contains(&"OPENAI_BASE_URL=http://plaza.gateway/openai/v1".to_string()));
        assert!(!env.iter().any(|pair| pair.starts_with("OPENAI_MODEL=")));
        assert_eq!(model, None);

        let (env, model) =
            acp_launch(&workspace, "qwen", Some("openrouter/anthropic/claude-x")).expect("a model");
        assert!(env.contains(&"OPENAI_BASE_URL=http://plaza.gateway/openrouter/v1".to_string()));
        assert!(env.contains(&"OPENAI_MODEL=anthropic/claude-x".to_string()));
        assert_eq!(model, None);

        let (env, _) =
            acp_launch(&workspace, "qwen", Some("opencode/deepseek-v4-pro")).expect("oc");
        assert!(env.contains(&"OPENAI_BASE_URL=http://plaza.gateway/opencode/v1".to_string()));
        assert!(env.contains(&"OPENAI_MODEL=deepseek-v4-pro".to_string()));
    }

    #[test]
    fn qwen_refuses_a_model_from_a_provider_it_cannot_reach() {
        let workspace = scratch("qwen-refuse");
        let err = acp_launch(&workspace, "qwen", Some("anthropic/claude-x")).expect_err("refused");
        assert_eq!(
            err,
            "invalid input: \"model\" is \"anthropic/claude-x\"; Qwen Code takes openai/<model>, openrouter/<model> or opencode/<model> models"
        );
    }

    #[test]
    fn kilo_launch_names_the_provider_block_as_its_session_model() {
        let workspace = scratch("kilo-launch");
        let (env, model) = acp_launch(&workspace, "kilo", None).expect("kilo");
        assert!(
            !env.iter()
                .any(|pair| pair.starts_with("KILO_CONFIG_CONTENT="))
        );
        assert_eq!(model, None);

        let (env, model) =
            acp_launch(&workspace, "kilo", Some("openrouter/anthropic/claude-x")).expect("a model");
        assert_eq!(
            model.as_deref(),
            Some("plaza-openrouter/anthropic/claude-x")
        );
        assert!(
            !env.iter()
                .any(|pair| pair.starts_with("OPENCODE_CONFIG_CONTENT="))
        );
        let config = env
            .iter()
            .find_map(|pair| pair.strip_prefix("KILO_CONFIG_CONTENT="))
            .expect("the Kilo config");
        let config: Value = serde_json::from_str(config).expect("JSON");
        assert_eq!(config["model"], "plaza-openrouter/anthropic/claude-x");
        assert_eq!(
            config["provider"]["plaza-openrouter"]["options"]["baseURL"],
            "http://plaza.gateway/openrouter/v1"
        );
    }

    #[test]
    fn qwen_and_kilo_are_acp_only_under_the_headless_engines() {
        let workspace = scratch("acp-only-headless");
        for (engine, name) in [("qwen", "Qwen Code"), ("kilo", "Kilo Code")] {
            let input = json!({"engine": engine, "prompt": "p"});
            let err = Job::parse(&workspace, &input).err().expect("refused");
            assert!(
                err.contains(&format!(
                    "{name} runs over ACP only; switch engines_mode to acp"
                )),
                "{err}"
            );
        }
    }

    #[test]
    fn opencode_config_names_a_plaza_provider_at_the_gateway() {
        let route =
            model_route(Engine::OpenCode, Some("openrouter/anthropic/claude-x")).expect("a route");
        assert_eq!(route.model, None);
        assert_eq!(route.env[0].0, "OPENCODE_CONFIG_CONTENT");
        let config: Value = serde_json::from_str(&route.env[0].1).expect("JSON");
        assert_eq!(config["model"], "plaza-openrouter/anthropic/claude-x");
        assert_eq!(
            config["provider"]["plaza-openrouter"]["options"]["baseURL"],
            "http://plaza.gateway/openrouter/v1"
        );
        assert!(
            config["provider"]["plaza-openrouter"]["models"]
                .get("anthropic/claude-x")
                .is_some()
        );
        let argv = command_args(Engine::OpenCode, "hi", Path::new("/workspace"), &route);
        assert!(
            argv.contains(&format!("OPENCODE_CONFIG_CONTENT={}", route.env[0].1)),
            "{argv:?}"
        );
        assert!(model_route(Engine::OpenCode, Some("bad_name/x")).is_err());
    }

    #[test]
    fn opencode_config_parses_and_each_model_names_the_block_whose_api_shape_serves_it() {
        let opencode = "http://plaza.gateway/opencode/v1";
        for (model, block, npm, base) in [
            (
                "opencode/gpt-5.6-terra",
                "plaza-opencode-responses",
                "@ai-sdk/openai",
                opencode,
            ),
            (
                "opencode/claude-sonnet-5-5",
                "plaza-opencode-anthropic",
                "@ai-sdk/anthropic",
                opencode,
            ),
            (
                "opencode/deepseek-v4-pro",
                "plaza-opencode",
                "@ai-sdk/openai-compatible",
                opencode,
            ),
            (
                "openai/gpt-5.6-terra",
                "plaza-openai",
                "@ai-sdk/openai",
                "http://plaza.gateway/openai/v1",
            ),
            (
                "openrouter/anthropic/claude-x",
                "plaza-openrouter",
                "@ai-sdk/openai-compatible",
                "http://plaza.gateway/openrouter/v1",
            ),
        ] {
            let route = model_route(Engine::OpenCode, Some(model)).expect("a route");
            let config: Value = serde_json::from_str(&route.env[0].1).expect("JSON");
            let selected = config["model"].as_str().expect("a model");
            let (selected_block, served) = selected.split_once('/').expect("<block>/<model>");
            assert_eq!(selected_block, block, "{model}: {config}");
            assert_eq!(config["provider"][block]["npm"], npm, "{model}: {config}");
            assert_eq!(
                config["provider"][block]["options"]["baseURL"], base,
                "{model}: {config}"
            );
            assert!(
                config["provider"][block]["models"].get(served).is_some(),
                "{model}: {config}"
            );
            assert_eq!(
                config["provider"].as_object().map(|blocks| blocks.len()),
                Some(1),
                "{model}: {config}"
            );
        }
    }
}
