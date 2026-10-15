//! The launch settings of the coding engines `code_engine` runs over ACP (see `acp`): how each
//! one reaches the plaza's model gateway, what it is started with, and the shared folder and
//! install helpers the ACP path uses. The gateway routes keep every key out of the VM: each
//! engine gets a placeholder, and the gateway holds the real one.
//!
//! The engines are installed on first use by the ACP path (see `acp::catalog`): `nodejs` and
//! `npm` through apk (as root), then each engine's pinned package into the agent's global
//! prefix (as the engine user, see `acp::layout`).

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::acp::layout;

use crate::bash;
use crate::files::Workspace;

const DEFAULT_TIMEOUT_S: u64 = 900;
const MIN_TIMEOUT_S: u64 = 30;
const MAX_TIMEOUT_S: u64 = 3600;
/// npm's global prefix for the engines; their binaries are in its `bin`.
pub(crate) const NPM_PREFIX: &str = "/workspace/.npm-global";
/// An npm install of an engine may take a while on a cold cache.
pub(crate) const INSTALL_TIMEOUT_S: u64 = 600;
/// The most output a tool result returns, and the tail kept from a failure.
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

/// The engines whose model routes are built here (see [`model_route`]). The other ACP engines have
/// their own arms in [`acp_launch`].
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
            _ => Err(format!(
                "invalid input: \"engine\" must be claude, codex or opencode, not {name:?}"
            )),
        }
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

/// The folder a session starts in: the workspace, or `path` under it. A `path` that is
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
/// pairs (see [`engine_env`]) and the model id the engine is given. OpenCode's model id is its provider block's `<id>/<model>`, which its config
/// names as the default model. `HOME` is the engine's own folder (see `acp::layout::engine_home`), not
/// the workspace.
pub(crate) fn acp_launch(
    workspace: &Workspace,
    engine_id: &str,
    model: Option<&str>,
) -> Result<(Vec<String>, Option<String>), String> {
    let home = layout::engine_home(workspace, engine_id);
    let home = home.as_path();
    match engine_id {
        // Gemini CLI and Qwen Code read their model from the environment, so the session is given none.
        "gemini" => return Ok((engine_env(home, &gemini_route(model)?), None)),
        "qwen" => return Ok((engine_env(home, &qwen_route(model)?), None)),
        // Cline's model is in its saved settings (see `setup_args`), so the session is given none.
        "cline" => return Ok((engine_env(home, &Route::default()), None)),
        // Copilot and Goose take their provider and model from the environment too.
        "copilot" => return Ok((engine_env(home, &copilot_route(model)?), None)),
        "goose" => return Ok((engine_env(home, &goose_route(model)?), None)),
        "kimi" => return Ok((engine_env(home, &kimi_route(model)?), None)),
        // Auggie's session token is the placeholder the vendor relay answers for; its model is on the
        // command line (`acp_args`).
        "auggie" => return Ok((engine_env(home, &auggie_route()), None)),
        // Vibe reads its model from the config file `write_vibe_config` wrote before this launch.
        "vibe" => return Ok((engine_env(home, &vibe_route(workspace, model)?), None)),
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

/// The folder Mistral Vibe keeps its config in: `vibe` under the engine's own home
/// (`/workspace/.engines/home/vibe/vibe`). It is the engine's, so the engine can edit its own
/// settings; it is outside `.plaza`, which the engine cannot read. The config holds only
/// the gateway route and a placeholder key.
pub(crate) fn vibe_home(workspace: &Workspace) -> PathBuf {
    layout::engine_home(workspace, "vibe").join("vibe")
}

/// Writes Mistral Vibe's `config.toml` for the call's model, before the launch, so a new model takes
/// effect on every call (the way Cline's saved settings do). Vibe reads it from `VIBE_HOME`.
pub(crate) fn write_vibe_config(workspace: &Workspace, model: Option<&str>) -> Result<(), String> {
    let config = vibe_config(model)?;
    let home = vibe_home(workspace);
    std::fs::create_dir_all(&home)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", home.display()))?;
    std::fs::write(home.join("config.toml"), config)
        .map_err(|err| format!("unavailable: cannot write {}: {err}", home.display()))?;
    // The harness wrote it as root; the engine keeps its own settings folder (see `acp::layout`).
    layout::give_to_engine(workspace.dir(), &home)
}

/// The route for Mistral Vibe, which runs over ACP only and reads its provider and model from the file
/// [`write_vibe_config`] writes: `VIBE_HOME` names that file's folder, and the placeholder key comes from
/// `PLAZA_API_KEY`, which the file names. The model is in the file, so the session is given none.
fn vibe_route(workspace: &Workspace, model: Option<&str>) -> Result<Route, String> {
    vibe_config(model)?;
    Ok(Route {
        env: vec![
            (
                "VIBE_HOME".to_string(),
                vibe_home(workspace).display().to_string(),
            ),
            ("PLAZA_API_KEY".to_string(), "plaza".to_string()),
        ],
        model: None,
    })
}

/// Mistral Vibe's `config.toml` for `model`: the gateway route of the model's provider as its one provider
/// (`openai`, `openrouter` or `opencode`, all chat completions), the model without its provider, and
/// `bypass_tool_permissions = false`, so the engine asks before its tools act. The gateway has no Mistral
/// route, so a Mistral model goes through `openrouter/mistralai/<model>`.
fn vibe_config(model: Option<&str>) -> Result<String, String> {
    let Some(model) = model else {
        return Err("invalid input: Vibe needs \"model\": its config names the model".to_string());
    };
    let (provider, rest) = split_model(model)?;
    let route = match provider {
        "openai" | "openrouter" | "opencode" => provider,
        _ => {
            return Err(format!(
                "invalid input: \"model\" is {model:?}; Vibe takes openai/..., openrouter/... or opencode/... models (Mistral models go as openrouter/mistralai/<model>: the gateway has no Mistral route)"
            ));
        }
    };
    if rest.chars().any(char::is_control) {
        return Err(format!(
            "invalid input: \"model\" is {model:?}; a model name has no control characters"
        ));
    }
    Ok(format!(
        "active_model = \"plaza\"\n\
         bypass_tool_permissions = false\n\
         \n\
         [[providers]]\n\
         name = \"plaza\"\n\
         api_base = \"{GATEWAY}/{route}/v1\"\n\
         api_key_env_var = \"PLAZA_API_KEY\"\n\
         api_style = \"openai\"\n\
         backend = \"generic\"\n\
         \n\
         [[models]]\n\
         name = \"{name}\"\n\
         provider = \"plaza\"\n\
         alias = \"plaza\"\n",
        name = toml_escape(rest),
    ))
}

/// `text` as the contents of a TOML basic string: a backslash and a double quote are escaped.
fn toml_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The command that writes an engine's saved configuration before each call, for the engines that
/// are configured by a command rather than by environment: Cline keeps its provider, base URL, key
/// and model under HOME, so `cline auth` runs first and rewrites them. A call with a new model
/// therefore takes effect. Cline's provider is the plaza model's provider: `anthropic`, and
/// `openrouter` (both Anthropic-shaped on the gateway, as Claude Code reaches them), or `openai` and
/// `opencode` (chat completions), with `opencode/claude-` on the Anthropic route as OpenCode's own
/// blocks are. The key is the gateway's placeholder, as for every engine.
pub(crate) fn setup_args(
    engine_id: &str,
    model: Option<&str>,
) -> Result<Option<Vec<String>>, String> {
    if engine_id != "cline" {
        return Ok(None);
    }
    let Some(model) = model else {
        return Err("invalid input: Cline needs \"model\"".to_string());
    };
    let (provider, rest) = split_model(model)?;
    let (cline_provider, base_url) = match provider {
        "anthropic" | "openrouter" => ("anthropic", format!("{GATEWAY}/{provider}")),
        "opencode" if rest.starts_with("claude-") => ("anthropic", format!("{GATEWAY}/opencode")),
        "openai" | "opencode" => ("openai", format!("{GATEWAY}/{provider}/v1")),
        _ => {
            return Err(format!(
                "invalid input: \"model\" is {model:?}; Cline takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ));
        }
    };
    Ok(Some(
        [
            "auth",
            "-p",
            cline_provider,
            "-k",
            "plaza",
            "-b",
            base_url.as_str(),
            "-m",
            rest,
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect(),
    ))
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

/// The route for GitHub Copilot CLI in its bring-your-own-key mode, which runs over ACP only: the
/// gateway route of the model's provider (Anthropic's route for `anthropic`, the chat-completions route
/// for `openai`, `openrouter` and `opencode`), the placeholder key, and the model without its provider
/// as `COPILOT_MODEL`. The mode names no default model, so a call needs one. No GitHub credential is
/// ever given: `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` and `COPILOT_ALLOW_ALL` are never set
/// by the harness, and the test below holds the launch to that.
fn copilot_route(model: Option<&str>) -> Result<Route, String> {
    let Some(model) = model else {
        return Err(
            "invalid input: Copilot needs \"model\": its bring-your-own-key mode names the model"
                .to_string(),
        );
    };
    let (provider, rest) = split_model(model)?;
    let (provider_type, base_url) = match provider {
        "anthropic" => ("anthropic", format!("{GATEWAY}/anthropic")),
        "openai" | "openrouter" | "opencode" => ("openai", format!("{GATEWAY}/{provider}/v1")),
        _ => {
            return Err(format!(
                "invalid input: \"model\" is {model:?}; Copilot takes anthropic/..., openai/..., openrouter/... or opencode/... models"
            ));
        }
    };
    let env = [
        ("COPILOT_OFFLINE", "true".to_string()),
        ("COPILOT_PROVIDER_TYPE", provider_type.to_string()),
        ("COPILOT_PROVIDER_BASE_URL", base_url),
        ("COPILOT_PROVIDER_API_KEY", "plaza".to_string()),
        ("COPILOT_MODEL", rest.to_string()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    Ok(Route { env, model: None })
}

/// The route for Goose, which runs over ACP only: the placeholder key, the gateway route of the model's
/// provider, and the model without its provider as `GOOSE_MODEL`. `anthropic` and `openrouter` are
/// Anthropic-shaped on the gateway, so Goose's Anthropic provider takes them; `openai` and `opencode`
/// speak chat completions, so its OpenAI provider does. The keyring is off (the VM has none) and the
/// mode is `approve`, so Goose asks before it acts. A call needs a model: Goose names none by default.
fn goose_route(model: Option<&str>) -> Result<Route, String> {
    let Some(model) = model else {
        return Err(
            "invalid input: Goose needs \"model\": its provider and model come from it".to_string(),
        );
    };
    let (provider, rest) = split_model(model)?;
    let mut env: Vec<(String, String)> = [
        ("GOOSE_DISABLE_KEYRING", "1"),
        ("GOOSE_MODE", "approve"),
        ("GOOSE_MODEL", rest),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    let provider_pairs: Vec<(&str, String)> = match provider {
        "anthropic" | "openrouter" => vec![
            ("GOOSE_PROVIDER", "anthropic".to_string()),
            ("ANTHROPIC_HOST", format!("{GATEWAY}/{provider}")),
            ("ANTHROPIC_API_KEY", "plaza".to_string()),
        ],
        "openai" | "opencode" => vec![
            ("GOOSE_PROVIDER", "openai".to_string()),
            ("OPENAI_HOST", format!("{GATEWAY}/{provider}")),
            ("OPENAI_BASE_PATH", "v1/chat/completions".to_string()),
            ("OPENAI_API_KEY", "plaza".to_string()),
        ],
        _ => {
            return Err(format!(
                "invalid input: \"model\" is {model:?}; Goose takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ));
        }
    };
    env.extend(
        provider_pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    );
    Ok(Route { env, model: None })
}

/// The route for Kimi Code, which runs over ACP only, in its env-only provider mode: the placeholder
/// key, the gateway route of the model's provider, the model without its provider as
/// `KIMI_MODEL_NAME`, and the context window it is given. `anthropic` and `openrouter` are
/// Anthropic-shaped on the gateway; `openai` and `opencode` speak chat completions. A call needs a
/// model: Kimi Code names none by default.
fn kimi_route(model: Option<&str>) -> Result<Route, String> {
    let Some(model) = model else {
        return Err(
            "invalid input: Kimi Code needs \"model\": its provider and model come from it"
                .to_string(),
        );
    };
    let (provider, rest) = split_model(model)?;
    let (provider_type, base_url) = match provider {
        "anthropic" | "openrouter" => ("anthropic", format!("{GATEWAY}/{provider}")),
        "openai" | "opencode" => ("openai", format!("{GATEWAY}/{provider}/v1")),
        _ => {
            return Err(format!(
                "invalid input: \"model\" is {model:?}; Kimi Code takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ));
        }
    };
    let env = [
        ("KIMI_MODEL_NAME", rest.to_string()),
        ("KIMI_MODEL_PROVIDER_TYPE", provider_type.to_string()),
        ("KIMI_MODEL_BASE_URL", base_url),
        ("KIMI_MODEL_API_KEY", "plaza".to_string()),
        ("KIMI_MODEL_MAX_CONTEXT_SIZE", "262144".to_string()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    Ok(Route { env, model: None })
}

/// The session token Auggie starts with inside the VM: a placeholder, and a tenant URL on the hub's
/// Augment relay (`/vendor/augment/`, see apid's vendor relay), which swaps in the owner's real token
/// and tenant. The VM never holds the owner's token.
const AUGGIE_SESSION_AUTH: &str = r#"{"accessToken":"plaza","tenantURL":"http://plaza.gateway/vendor/augment/","scopes":["email"]}"#;

/// The route for Auggie: its placeholder session token, and no model (a call's model is on the command
/// line, see [`acp_args`]).
fn auggie_route() -> Route {
    Route {
        env: vec![(
            "AUGMENT_SESSION_AUTH".to_string(),
            AUGGIE_SESSION_AUTH.to_string(),
        )],
        model: None,
    }
}

/// The arguments an ACP engine's command takes for a call's model, beyond its catalogue arguments.
/// Auggie takes its model on the command line (`--model`), and its models are Augment's own: a call
/// names them `augment/<model>`, and the provider is dropped. The other engines take none here.
pub(crate) fn acp_args(engine_id: &str, model: Option<&str>) -> Result<Vec<String>, String> {
    if engine_id != "auggie" {
        return Ok(Vec::new());
    }
    let Some(model) = model else {
        return Ok(Vec::new());
    };
    match split_model(model)? {
        ("augment", rest) => Ok(vec!["--model".to_string(), rest.to_string()]),
        _ => Err(format!(
            "invalid input: \"model\" is {model:?}; Auggie runs Augment's own models: use augment/<model>"
        )),
    }
}

/// How a session reaches its model: the environment settings the engine needs, and the model
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
    fn the_launch_sets_the_home_and_the_gateway_environment() {
        let pairs = engine_env(Path::new("/workspace"), &Route::default());
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
            assert!(pairs.iter().any(|arg| arg == pair), "{pair}");
        }
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
    fn bad_input_is_refused_before_the_launch() {
        let workspace = scratch("input");
        let err = acp_launch(&workspace, "vim", None).expect_err("refused");
        assert!(err.contains("engine"), "{err}");
        for timeout in [json!(5), json!(3601)] {
            let input = json!({ "timeout_s": timeout });
            let err = timeout_arg(&input).expect_err("refused");
            assert!(err.contains("timeout_s"), "{err}");
        }
        let err = job_folder(&workspace, Some(&json!("../up"))).expect_err("refused");
        assert!(err.contains("path"), "{err}");
        let err = acp_launch(&workspace, "codex", Some("opencode/claude-x")).expect_err("refused");
        assert!(err.contains("model"), "{err}");
        let err = acp_launch(&workspace, "claude", Some("opencode/deepseek-v4-pro"))
            .expect_err("refused");
        assert!(err.contains("Claude Code"), "{err}");
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
    fn model_route_follows_each_engine_to_the_route_that_serves_its_model() {
        // Claude Code points at the Anthropic-shaped gateway routes, names the model after the route prefix, and the default route is empty.
        {
            let route = model_route(Engine::Claude, Some("openrouter/anthropic/claude-x"))
                .expect("a route");
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
        // Claude Code refuses an OpenCode model that its Anthropic-shaped route does not serve.
        {
            let err =
                model_route(Engine::Claude, Some("opencode/deepseek-v4-pro")).expect_err("refused");
            assert!(
                err.contains("Claude Code needs an Anthropic-shaped model"),
                "{err}"
            );
        }
        // Codex takes only the models its OpenAI wire protocol serves and refuses the rest.
        {
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
    }

    #[test]
    fn gemini_launch_follows_its_gateway_route_and_refuses_another_provider() {
        // Gemini launch points at the gateway and takes its model from the environment only when a gemini/ model is named.
        {
            let workspace = scratch("gemini-launch");
            let (env, model) = acp_launch(&workspace, "gemini", None).expect("gemini");
            assert_eq!(
                env[0],
                format!(
                    "HOME={}",
                    workspace.dir().join(".engines/home/gemini").display()
                )
            );
            assert!(
                env.contains(&"GOOGLE_GEMINI_BASE_URL=http://plaza.gateway/gemini".to_string())
            );
            assert!(env.contains(&"GEMINI_API_KEY=plaza".to_string()));
            assert!(env.contains(&"GEMINI_CLI_TRUST_WORKSPACE=true".to_string()));
            assert!(!env.iter().any(|pair| pair.starts_with("GEMINI_MODEL=")));
            assert_eq!(model, None);

            let (env, model) =
                acp_launch(&workspace, "gemini", Some("gemini/gemini-2.5-pro")).expect("a model");
            assert!(env.contains(&"GEMINI_MODEL=gemini-2.5-pro".to_string()));
            assert_eq!(model, None);
        }
        // Gemini refuses a model from another provider, with the Gemini CLI hint.
        {
            let workspace = scratch("gemini-refuse");
            let err =
                acp_launch(&workspace, "gemini", Some("anthropic/claude-x")).expect_err("refused");
            assert_eq!(
                err,
                "invalid input: \"model\" is \"anthropic/claude-x\"; Gemini CLI takes gemini/<model> models"
            );
        }
    }

    #[test]
    fn vibe_names_its_model_in_a_config_file_under_its_own_folder() {
        let workspace = scratch("vibe-config");
        let config = vibe_config(Some("openrouter/mistralai/mistral-large")).expect("a model");
        assert_eq!(
            config,
            "active_model = \"plaza\"\n\
             bypass_tool_permissions = false\n\
             \n\
             [[providers]]\n\
             name = \"plaza\"\n\
             api_base = \"http://plaza.gateway/openrouter/v1\"\n\
             api_key_env_var = \"PLAZA_API_KEY\"\n\
             api_style = \"openai\"\n\
             backend = \"generic\"\n\
             \n\
             [[models]]\n\
             name = \"mistralai/mistral-large\"\n\
             provider = \"plaza\"\n\
             alias = \"plaza\"\n"
        );
        assert!(config.contains("bypass_tool_permissions = false\n"));
        // Under the engine's own home, and never under the harness's own state.
        assert_eq!(
            vibe_home(&workspace),
            workspace
                .dir()
                .join(".engines")
                .join("home")
                .join("vibe")
                .join("vibe")
        );
        assert!(!vibe_home(&workspace).starts_with(workspace.plaza_dir()));

        let (env, model) = acp_launch(&workspace, "vibe", Some("openai/gpt-x")).expect("vibe");
        assert_eq!(model, None);
        assert_eq!(
            env[0],
            format!(
                "HOME={}",
                workspace.dir().join(".engines/home/vibe").display()
            )
        );
        assert!(env.contains(&format!("VIBE_HOME={}", vibe_home(&workspace).display())));
        assert!(env.contains(&"PLAZA_API_KEY=plaza".to_string()));
    }

    #[test]
    fn vibe_routes_follow_its_provider_and_refuse_the_rest_with_the_mistral_hint() {
        assert!(
            vibe_config(Some("opencode/gpt-x"))
                .unwrap()
                .contains("http://plaza.gateway/opencode/v1")
        );
        assert!(
            vibe_config(Some("openai/gpt-x"))
                .unwrap()
                .contains("name = \"gpt-x\"")
        );
        assert!(vibe_config(None).is_err());
        let err = vibe_config(Some("anthropic/claude-x")).expect_err("refused");
        assert!(err.contains("openrouter/mistralai/<model>"), "{err}");
        assert!(vibe_config(Some("openai/a\nb")).is_err());
        assert!(acp_launch(&scratch("vibe-refused"), "vibe", None).is_err());
    }

    #[test]
    fn a_vibe_model_name_is_escaped_for_a_toml_basic_string() {
        assert_eq!(toml_escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(toml_escape("plain/name-1.0"), "plain/name-1.0");
    }

    #[test]
    fn qwen_launch_follows_the_model_provider_to_its_gateway_route() {
        // Qwen launch with no model uses the OpenAI gateway route and sets no model; openrouter/ and opencode/ models follow their own routes.
        {
            let workspace = scratch("qwen-launch");
            let (env, model) = acp_launch(&workspace, "qwen", None).expect("qwen");
            assert_eq!(
                env[0],
                format!(
                    "HOME={}",
                    workspace.dir().join(".engines/home/qwen").display()
                )
            );
            assert!(env.contains(&"OPENAI_API_KEY=plaza".to_string()));
            assert!(env.contains(&"OPENAI_BASE_URL=http://plaza.gateway/openai/v1".to_string()));
            assert!(!env.iter().any(|pair| pair.starts_with("OPENAI_MODEL=")));
            assert_eq!(model, None);

            let (env, model) =
                acp_launch(&workspace, "qwen", Some("openrouter/anthropic/claude-x"))
                    .expect("a model");
            assert!(
                env.contains(&"OPENAI_BASE_URL=http://plaza.gateway/openrouter/v1".to_string())
            );
            assert!(env.contains(&"OPENAI_MODEL=anthropic/claude-x".to_string()));
            assert_eq!(model, None);

            let (env, _) =
                acp_launch(&workspace, "qwen", Some("opencode/deepseek-v4-pro")).expect("oc");
            assert!(env.contains(&"OPENAI_BASE_URL=http://plaza.gateway/opencode/v1".to_string()));
            assert!(env.contains(&"OPENAI_MODEL=deepseek-v4-pro".to_string()));
        }
        // Qwen refuses a model from a provider it cannot reach, and names the providers it takes.
        {
            let workspace = scratch("qwen-refuse");
            let err =
                acp_launch(&workspace, "qwen", Some("anthropic/claude-x")).expect_err("refused");
            assert_eq!(
                err,
                "invalid input: \"model\" is \"anthropic/claude-x\"; Qwen Code takes openai/<model>, openrouter/<model> or opencode/<model> models"
            );
        }
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

    /// The value of `key` in a launch's `KEY=VALUE` pairs.
    fn value<'a>(env: &'a [String], key: &str) -> Option<&'a str> {
        env.iter().find_map(|pair| {
            pair.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
        })
    }

    #[test]
    fn copilot_runs_byok_through_the_gateway_and_needs_a_reachable_model() {
        // Copilot BYOK mode runs through the gateway with no GitHub credential, for an openrouter/ model and an anthropic/ model.
        {
            let workspace = scratch("copilot-launch");
            let (env, model) =
                acp_launch(&workspace, "copilot", Some("openrouter/anthropic/claude-x"))
                    .expect("copilot");
            assert_eq!(model, None);
            assert_eq!(value(&env, "COPILOT_OFFLINE"), Some("true"));
            assert_eq!(value(&env, "COPILOT_PROVIDER_TYPE"), Some("openai"));
            assert_eq!(
                value(&env, "COPILOT_PROVIDER_BASE_URL"),
                Some("http://plaza.gateway/openrouter/v1")
            );
            assert_eq!(value(&env, "COPILOT_PROVIDER_API_KEY"), Some("plaza"));
            assert_eq!(value(&env, "COPILOT_MODEL"), Some("anthropic/claude-x"));
            // Copilot must never see a GitHub credential or the allow-all switch: the harness never sets
            // them, and the bash environment it inherits is cleared of them.
            for key in [
                "COPILOT_GITHUB_TOKEN",
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "COPILOT_ALLOW_ALL",
            ] {
                assert_eq!(value(&env, key), None, "{key} is never set");
            }
            let (env, _) =
                acp_launch(&workspace, "copilot", Some("anthropic/claude-x")).expect("claude");
            assert_eq!(value(&env, "COPILOT_PROVIDER_TYPE"), Some("anthropic"));
            assert_eq!(
                value(&env, "COPILOT_PROVIDER_BASE_URL"),
                Some("http://plaza.gateway/anthropic")
            );
            assert_eq!(value(&env, "COPILOT_MODEL"), Some("claude-x"));
        }
        // Copilot needs a model, and refuses a model on a provider it cannot reach.
        {
            let workspace = scratch("copilot-refused");
            let err = acp_launch(&workspace, "copilot", None).expect_err("refused");
            assert!(err.contains("Copilot needs \"model\""), "{err}");
            let err = acp_launch(&workspace, "copilot", Some("gemini/x")).expect_err("refused");
            assert!(err.contains("Copilot takes"), "{err}");
        }
    }

    #[test]
    fn goose_takes_its_provider_and_model_from_the_environment() {
        let workspace = scratch("goose-launch");
        let (env, model) =
            acp_launch(&workspace, "goose", Some("anthropic/claude-x")).expect("goose");
        assert_eq!(model, None);
        assert_eq!(value(&env, "GOOSE_PROVIDER"), Some("anthropic"));
        assert_eq!(
            value(&env, "ANTHROPIC_HOST"),
            Some("http://plaza.gateway/anthropic")
        );
        assert_eq!(value(&env, "ANTHROPIC_API_KEY"), Some("plaza"));
        assert_eq!(value(&env, "GOOSE_MODEL"), Some("claude-x"));
        assert_eq!(value(&env, "GOOSE_DISABLE_KEYRING"), Some("1"));
        assert_eq!(value(&env, "GOOSE_MODE"), Some("approve"));
        let (env, _) = acp_launch(&workspace, "goose", Some("opencode/gpt-x")).expect("goose");
        assert_eq!(value(&env, "GOOSE_PROVIDER"), Some("openai"));
        assert_eq!(
            value(&env, "OPENAI_HOST"),
            Some("http://plaza.gateway/opencode")
        );
        assert_eq!(value(&env, "OPENAI_BASE_PATH"), Some("v1/chat/completions"));
        assert_eq!(value(&env, "GOOSE_MODEL"), Some("gpt-x"));
        let err = acp_launch(&workspace, "goose", None).expect_err("refused");
        assert!(err.contains("Goose needs \"model\""), "{err}");
        let err = acp_launch(&workspace, "goose", Some("gemini/x")).expect_err("refused");
        assert!(err.contains("Goose takes"), "{err}");
    }

    #[test]
    fn kimi_runs_its_env_only_provider_mode_through_the_gateway() {
        let workspace = scratch("kimi-launch");
        let (env, model) =
            acp_launch(&workspace, "kimi", Some("anthropic/claude-x")).expect("kimi");
        assert_eq!(model, None);
        assert_eq!(value(&env, "KIMI_MODEL_NAME"), Some("claude-x"));
        assert_eq!(value(&env, "KIMI_MODEL_PROVIDER_TYPE"), Some("anthropic"));
        assert_eq!(
            value(&env, "KIMI_MODEL_BASE_URL"),
            Some("http://plaza.gateway/anthropic")
        );
        assert_eq!(value(&env, "KIMI_MODEL_API_KEY"), Some("plaza"));
        assert_eq!(value(&env, "KIMI_MODEL_MAX_CONTEXT_SIZE"), Some("262144"));
        let (env, _) =
            acp_launch(&workspace, "kimi", Some("openrouter/anthropic/claude-x")).expect("kimi");
        assert_eq!(value(&env, "KIMI_MODEL_PROVIDER_TYPE"), Some("anthropic"));
        assert_eq!(
            value(&env, "KIMI_MODEL_BASE_URL"),
            Some("http://plaza.gateway/openrouter")
        );
        assert_eq!(value(&env, "KIMI_MODEL_NAME"), Some("anthropic/claude-x"));
        let (env, _) = acp_launch(&workspace, "kimi", Some("opencode/gpt-x")).expect("kimi");
        assert_eq!(value(&env, "KIMI_MODEL_PROVIDER_TYPE"), Some("openai"));
        assert_eq!(
            value(&env, "KIMI_MODEL_BASE_URL"),
            Some("http://plaza.gateway/opencode/v1")
        );
        assert_eq!(value(&env, "KIMI_MODEL_NAME"), Some("gpt-x"));
        let err = acp_launch(&workspace, "kimi", None).expect_err("refused");
        assert!(err.contains("Kimi Code needs \"model\""), "{err}");
        let err = acp_launch(&workspace, "kimi", Some("gemini/x")).expect_err("refused");
        assert!(err.contains("Kimi Code takes"), "{err}");
    }

    #[test]
    fn auggie_starts_on_a_placeholder_session_and_takes_its_model_on_the_command_line() {
        let workspace = scratch("auggie-launch");
        let (env, model) = acp_launch(&workspace, "auggie", None).expect("auggie");
        assert_eq!(model, None);
        let auth = value(&env, "AUGMENT_SESSION_AUTH").expect("the placeholder session");
        let session: serde_json::Value = serde_json::from_str(auth).expect("valid JSON");
        assert_eq!(session["accessToken"], "plaza");
        assert_eq!(session["tenantURL"], "http://plaza.gateway/vendor/augment/");
        assert_eq!(session["scopes"], json!(["email"]));
        // The owner's token and tenant never enter the VM: the relay swaps them in.
        assert!(!auth.contains("augmentcode.com"), "{auth}");
        assert_eq!(
            acp_args("auggie", None).expect("no model"),
            Vec::<String>::new()
        );
        assert_eq!(
            acp_args("auggie", Some("augment/claude-x")).expect("a model"),
            ["--model", "claude-x"]
        );
        let err = acp_args("auggie", Some("anthropic/claude-x")).expect_err("refused");
        assert!(err.contains("use augment/<model>"), "{err}");
        assert!(
            acp_args("claude", Some("anthropic/claude-x"))
                .expect("claude")
                .is_empty()
        );
    }

    #[test]
    fn cline_launch_is_given_no_model_and_its_home() {
        let workspace = scratch("cline-launch");
        let (env, model) = acp_launch(&workspace, "cline", None).expect("cline");
        assert_eq!(
            env[0],
            format!(
                "HOME={}",
                workspace.dir().join(".engines/home/cline").display()
            )
        );
        assert_eq!(model, None);
        let (_, model) =
            acp_launch(&workspace, "cline", Some("anthropic/claude-x")).expect("a model");
        assert_eq!(model, None);
    }

    #[test]
    fn cline_setup_follows_the_model_provider_and_needs_a_reachable_model() {
        // Cline setup follows each model's provider to its gateway route (anthropic, openai and opencode routes).
        {
            let setup = |model: &str| {
                setup_args("cline", Some(model))
                    .expect("a setup")
                    .expect("Cline has one")
                    .join(" ")
            };
            assert_eq!(
                setup("anthropic/claude-x"),
                "auth -p anthropic -k plaza -b http://plaza.gateway/anthropic -m claude-x"
            );
            assert_eq!(
                setup("openrouter/anthropic/claude-x"),
                "auth -p anthropic -k plaza -b http://plaza.gateway/openrouter -m anthropic/claude-x"
            );
            assert_eq!(
                setup("opencode/claude-x"),
                "auth -p anthropic -k plaza -b http://plaza.gateway/opencode -m claude-x"
            );
            assert_eq!(
                setup("openai/gpt-x"),
                "auth -p openai -k plaza -b http://plaza.gateway/openai/v1 -m gpt-x"
            );
            assert_eq!(
                setup("opencode/deepseek-v4-pro"),
                "auth -p openai -k plaza -b http://plaza.gateway/opencode/v1 -m deepseek-v4-pro"
            );
        }
        // Cline setup needs a model and refuses a provider it cannot reach; Qwen has no Cline setup.
        {
            assert_eq!(
                setup_args("cline", None).expect_err("refused"),
                "invalid input: Cline needs \"model\""
            );
            let err = setup_args("cline", Some("gemini/x")).expect_err("refused");
            assert!(err.contains("Cline takes anthropic/..."), "{err}");
            assert_eq!(
                setup_args("qwen", Some("openai/x")).expect("no setup"),
                None
            );
        }
    }

    #[test]
    fn opencode_config_names_plaza_blocks_at_the_gateway_and_each_model_names_its_block() {
        // OpenCode config names a plaza provider block at the gateway, selects it as the model, and refuses a bad model name.
        {
            let route = model_route(Engine::OpenCode, Some("openrouter/anthropic/claude-x"))
                .expect("a route");
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
            let pairs = engine_env(Path::new("/workspace"), &route);
            assert!(
                pairs.contains(&format!("OPENCODE_CONFIG_CONTENT={}", route.env[0].1)),
                "{pairs:?}"
            );
            assert!(model_route(Engine::OpenCode, Some("bad_name/x")).is_err());
        }
        // OpenCode config parses, and each model names the block whose API shape serves it.
        {
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
}
