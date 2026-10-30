//! The tools agents are offered: their specs, who runs them, and the host
//! tools `apid` runs itself.
//!
//! OWNER: slice "loop". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use apz_cache::Generations;
use apz_plaza::ConnectedAgent;
use apz_plaza::social::PostRequest;
use apz_plaza::tasks::{CreateTask, Task};
use apz_proto::chat::ToolSpec;
use apz_proto::wire::WorkKind;
use serde::Serialize;
use serde_json::{Value, json};

use crate::agents::{self, Agent};
use crate::laws::Laws;
use crate::scheduler::Origin;
use crate::settings::tier_of;
use crate::state::App;

/// Where a tool runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// In the agent's VM (the harness).
    Guest,
    /// In apid, with the agent's identity.
    Host,
}

/// Tools every agent is offered: host tools, plus the guest internet tools,
/// which run in the agent's VM and reach the internet through the hub's egress proxy.
const ALWAYS: &[&str] = &[
    "tools_list",
    "tools_describe",
    "tools_search",
    "tools_call",
    "plaza_toolbox",
    "plaza_toolbox_share",
    "plaza_projects",
    "plaza_project_attach",
    "plaza_project_critique",
    "submit_claim",
    "transcript_search",
    "plaza_tasks",
    "plaza_board_read",
    "plaza_board_post",
    "plaza_board_comment",
    "plaza_send_message",
    "plaza_message_seat",
    "team_set",
    "thread_read",
    "thread_answer",
    "share_file",
    "plaza_get_file",
    "fetch_url",
    "download",
    "install_packages",
    "todo_write",
    "code_engine",
    "deliver_file",
    "artifact_create",
    "artifact_update",
    "artifact_attach_data",
    "artifact_share_request",
    "tool_package",
    "checkpoint_diff",
    "checkpoint_restore",
    "checkpoint_list",
    "plaza_connector_propose",
    "plaza_skills",
    "plaza_skill_read",
    "review_tree",
    "review_score",
    "review_verdict",
    "review_spawn",
    "finding_verify",
    "finding_contest",
    "finding_arbitrate",
    "weft_status",
    "weft_diff",
    "weft_describe",
    "weft_log",
    "weft_show",
    "weft_blame",
    "weft_map",
    "weft_change_tree",
    "weft_touches",
    "tests_select",
    "logs_query",
    "metrics_query",
    "traces_query",
    "plaza_maintenance",
    "plaza_maintenance_map",
    "plaza_maintenance_propose",
    "plaza_maintenance_signal",
    "plaza_environments",
    "plaza_deploy",
    "journal_note",
    "journal_read",
    "handover_write",
    "brief",
    "memory_search",
    "memory_get",
    "memory_propose",
    "memory_record_decision",
    "memory_land",
    "memory_anchored",
    "memory_verify",
];

/// The tools a seat uses on most steps: offered with their schemas. Every other tool is deferred: the model sees its name
/// and one line, and fetches its schema through `tool_search` before it calls the tool.
const CORE: &[&str] = &[
    "tools_list",
    "tools_describe",
    "tools_search",
    "tools_call",
    "run",
    "run_tests",
    "run_script",
    "read",
    "edit",
    "find",
    "search",
    "submit_claim",
    "plaza_tasks",
    "plaza_board_read",
    "plaza_send_message",
    "plaza_message_seat",
    "plaza_task_result",
];

/// The tool the harness serves itself: it returns the full schemas of the deferred tools that match its query.
pub const TOOL_SEARCH: &str = "tool_search";

/// The guest tools that act only inside the agent's own VM, offered to every role.
const IN_VM: &[&str] = &[
    "read",
    "edit",
    "find",
    "search",
    "run",
    "run_tests",
    "run_script",
    "symbols",
    "definition",
    "references",
    "diagnostics",
    "outline",
    "spawn_helper",
    "await_helper",
];

/// Host tools offered only by tier (see [`permitted`]).
const GATED: &[&str] = &[
    "plaza_repository",
    "plaza_agent_propose",
    "plaza_project_draft",
    "plan_post",
    "plaza_project_open",
    "plaza_project_advance",
    "plaza_project_approve",
    "plaza_agent_control",
    "plaza_loop",
    "plaza_agents",
    "plaza_post_task",
    "plaza_tasks_post",
    "plaza_task_link",
    "plaza_assign",
    "plaza_cancel_task",
    "plaza_task_result",
    "plaza_verdict",
    "foundry_datasets",
    "foundry_export",
    "foundry_submit_job",
    "foundry_job_status",
    "foundry_request_shadow",
    "foundry_hf_search",
    "invoke_model",
];

/// Where `name` runs, or `None` for an unknown tool.
pub fn runs(name: &str) -> Option<Runs> {
    // contract
    match name {
        "read"
        | "edit"
        | "find"
        | "search"
        | "run"
        | "run_tests"
        | "share_file"
        | "fetch_url"
        | "download"
        | "install_packages"
        | "todo_write"
        | "code_engine"
        | "run_script"
        | "deliver_file"
        | "artifact_create"
        | "artifact_update"
        | "artifact_attach_data"
        | "artifact_share_request"
        | "tool_package"
        | "checkpoint_diff"
        | "checkpoint_restore"
        | "checkpoint_list"
        | "symbols"
        | "definition"
        | "references"
        | "diagnostics"
        | "outline"
        | "spawn_helper"
        | "await_helper" => Some(Runs::Guest),
        "transcript_search"
        | "submit_claim"
        | "plaza_tasks"
        | "plaza_board_read"
        | "plaza_board_post"
        | "plaza_board_comment"
        | "plaza_send_message"
        | "plaza_message_seat"
        | "team_set"
        | "thread_read"
        | "thread_answer"
        | "plaza_agents"
        | "plaza_post_task"
        | "plaza_tasks_post"
        | "plaza_task_link"
        | "plaza_assign"
        | "plaza_cancel_task"
        | "plaza_task_result"
        | "plaza_get_file"
        | "plaza_verdict" => Some(Runs::Host),
        name if crate::skills::agent_tools::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::toolbox::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::projects::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::maintenance::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::environments::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::harness_control::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::agent_proposals::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::project_memory::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::curator::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::verifier::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::drafts::TOOLS.contains(&name) => Some(Runs::Host),
        name if name == crate::platforms::repositories::TOOL => Some(Runs::Host),
        name if crate::review_graph::tools::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::test_select::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::lab_observe::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::journal::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::planning::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::stations::tools::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::foundry::breeder::TOOLS.contains(&name) => Some(Runs::Host),
        "plaza_connector_propose" => Some(Runs::Host),
        name if crate::tool_library::TOOLS.contains(&name) => Some(Runs::Host),
        name if crate::connectors::by_tool(name).is_some() => Some(Runs::Host),
        name if name.starts_with(crate::mcp::PREFIX) => Some(Runs::Host),
        name if name.starts_with(crate::library::PREFIX) => Some(Runs::Guest),
        _ => None,
    }
}

/// The tool specs an agent of `role` on `project` is offered (its grants + plaza tools + the connectors that let it in).
pub fn specs_for(app: &App, role: &str, project: Option<&str>) -> Vec<ToolSpec> {
    // contract
    let granted = crate::roles_custom::tools_of(app, role);
    let mut specs: Vec<ToolSpec> = offered(role, &granted)
        .iter()
        .filter_map(|name| spec(name))
        .collect();
    // A connector tool is offered only when the laws permit it (C-004a holds back the irreversible ones).
    specs.extend(
        crate::connectors::specs_for(app, role, project)
            .into_iter()
            .filter(|spec| permitted(role, &spec.name)),
    );
    specs
}

/// A session's tools section is kept this long at most, whatever its sources say.
const SECTION_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// What a session's tools section is built from besides its sources: the role it runs as, the project it works on and
/// the node it runs on (the MCP servers offered depend on the node).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SectionKey {
    pub role: String,
    pub project: Option<String>,
    pub node: Option<String>,
}

/// A session's tools section, built from its sources: the tools it is offered except the library's (those are per agent)
/// and its `## Skills` prompt section.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// The role's tools: grants, plaza tools, connectors, with the maintenance and environment tools where they apply.
    pub tools: Vec<ToolSpec>,
    /// The MCP servers' tools the session may use on its node.
    pub mcp: Vec<ToolSpec>,
    /// The `## Skills` section of its system prompt, when it has skills.
    pub skills: Option<String>,
}

/// The sections of the sessions, each built once per key and rebuilt when a source it was built from moves.
pub struct Sections {
    cache: apz_cache::Cache<SectionKey, Section>,
}

impl Default for Sections {
    fn default() -> Self {
        Sections::new()
    }
}

impl Sections {
    pub fn new() -> Sections {
        Sections {
            cache: apz_cache::Cache::new(SECTION_AGE),
        }
    }

    /// The section for `key`: answered from the cache while `current` (the generation of its sources now) is the one it was
    /// built at, else built by `build` (once for every caller that misses, see `apz_cache::Cache`).
    pub fn get(
        &self,
        key: &SectionKey,
        current: impl Fn() -> u64,
        build: impl Fn() -> Section,
    ) -> Arc<Section> {
        self.cache
            .get(key, current, || Ok::<_, String>(build()))
            .expect("a section is built from its sources, which cannot fail")
    }

    /// How many sections were built (a test counts them).
    #[cfg(test)]
    pub fn builds(&self) -> u64 {
        self.cache.stats().1
    }
}

/// The generation of the sources `key`'s section is built from: the generations of the connectors, the vault, the MCP
/// servers, the settings, the engines, the skills, the role and the project, and the laws. Each only moves up, so their sum
/// moves with any of them.
pub fn section_generation(gens: &Generations<String>, laws: &Laws, key: &SectionKey) -> u64 {
    let mut keys = vec![
        crate::sources::CONNECTORS.to_string(),
        crate::sources::VAULT.to_string(),
        crate::sources::MCP.to_string(),
        crate::sources::SETTINGS.to_string(),
        crate::sources::ENGINES.to_string(),
        crate::skills::LIBRARY.to_string(),
        crate::sources::role(&key.role),
    ];
    if let Some(project) = &key.project {
        keys.push(crate::sources::project(project));
    }
    keys.iter().fold(laws.generation(), |sum, key| {
        sum.wrapping_add(gens.get(key))
    })
}

/// The section of the tools `agent` is offered (its role's, on its project and node), from the cache while its sources
/// have not moved.
pub fn section(app: &App, agent: &Agent) -> Arc<Section> {
    let key = SectionKey {
        role: agent.role.clone(),
        project: crate::projects::project_of(app, agent.cid),
        node: app.nodes.owner_of_cid(agent.cid).map(|n| n.id),
    };
    app.tool_sections.get(
        &key,
        || section_generation(&app.generations, app.laws, &key),
        || build_section(app, &key),
    )
}

/// Builds the section of `key` from the stores: the same specs `specs_for_agent` offered before the cache.
fn build_section(app: &App, key: &SectionKey) -> Section {
    let project = key.project.as_deref();
    let mut tools = specs_for(app, &key.role, project);
    // A project's maintenance and environment tools are offered only where they apply: the maintenance tools on a
    // project in maintenance, the environment tools on one that has environments.
    let found = project.and_then(|id| app.projects.get(id));
    let maintained = found.as_ref().is_some_and(|p| p.in_maintenance());
    let has_envs = project.is_some_and(|id| !crate::environments::list(app, id).is_empty());
    tools.retain(|spec| {
        (maintained || !crate::maintenance::TOOLS.contains(&spec.name.as_str()))
            && (has_envs || !crate::environments::TOOLS.contains(&spec.name.as_str()))
    });
    // The engines offered are the owner's enabled ones: the built-in ones in `engines_enabled`, then the
    // instances the owner added (`engine_instances.rs`).
    let enabled = enabled_engines(app);
    for spec in tools.iter_mut().filter(|spec| spec.name == "code_engine") {
        restrict_engines(&mut spec.input_schema, &enabled);
    }
    Section {
        tools,
        mcp: crate::mcp::specs_for(app, &key.role, key.node.as_deref(), project),
        skills: crate::skills::agent_tools::prompt_section(app, &key.role, project),
    }
}

/// The tool specs agent `agent` is offered: its role's, plus the library tools it may use.
pub fn specs_for_agent(app: &App, agent: &Agent) -> Vec<ToolSpec> {
    with_library(app, agent, &section(app, agent))
}

/// The tools of `agent` from its `section`: the section's role tools, then the library tools it may use (per agent, so
/// not cached), then the MCP tools.
pub fn with_library(app: &App, agent: &Agent, section: &Section) -> Vec<ToolSpec> {
    let mut specs = section.tools.clone();
    specs.extend(crate::library::specs_for(app, agent.cid));
    specs.extend(section.mcp.iter().cloned());
    specs
}

/// The tools agent `agent` is offered, as its profile carries them: `specs_for_agent`, with every tool outside `CORE`
/// deferred, and `tool_search` for the deferred ones.
pub fn offered_to(app: &App, agent: &Agent) -> Vec<ToolSpec> {
    let mut specs = specs_for_agent(app, agent);
    for spec in &mut specs {
        spec.deferred = !CORE.contains(&spec.name.as_str());
    }
    specs.push(ToolSpec {
        deferred: false,
        name: TOOL_SEARCH.to_string(),
        description: "Fetch the full schemas of tools you were offered by name only. Give a query that matches their names or \
            what they do; each match comes back with its input schema, and from then on you may call it. Call it before you \
            call a deferred tool."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {"query": {"type": "string", "minLength": 1}},
            "required": ["query"],
        }),
    });
    specs
}

/// Narrows a `code_engine` schema's `engine` enum to the owner's enabled engines.
fn restrict_engines(schema: &mut Value, enabled: &[String]) {
    schema["properties"]["engine"]["enum"] = json!(enabled);
}

/// Runs host tool `name` for `agent`; `(is_error, output)`.
pub fn run_host(app: &App, agent: &Agent, work: u64, name: &str, input: &Value) -> (bool, String) {
    // contract
    if let Some(why) = refusal(&agent.role, name) {
        return (true, why);
    }
    match host_call(app, agent, work, name, input) {
        Ok(out) => (false, out),
        Err(why) => (true, why),
    }
}

/// The `model` a `code_engine` call asks for, checked against the calling agent's tier through
/// `Settings::allowed`, the rule the model gateway enforces on every call: `None` keeps the engine's
/// default (no `model`, or an empty one). Called by the guest arm of `internal::dispatch`, before the
/// call is forwarded to the VM.
pub(crate) fn code_engine_model(
    app: &App,
    role: &str,
    input: &Value,
) -> Result<Option<String>, String> {
    let Some(model) = model_input(input)? else {
        model_required(input)?;
        return Ok(None);
    };
    engine_provider(input, model)?;
    // Auggie's `augment/` models too: workers may run any model, and the orchestrator only the ones
    // the owner listed for it, Augment's own included.
    app.settings.allowed(tier_of(role), model)?;
    Ok(Some(model.to_string()))
}

/// Refuses a call without a model where its engine needs one: Cline is configured with its model
/// before the call, so it has no default to fall back on.
fn model_required(input: &Value) -> Result<(), String> {
    match input.get("engine").and_then(Value::as_str) {
        Some("cline") => Err("invalid input: Cline needs \"model\"".to_string()),
        Some("kimi") => Err(
            "invalid input: Kimi Code needs \"model\": its provider and model come from it"
                .to_string(),
        ),
        Some("copilot") => Err(
            "invalid input: Copilot needs \"model\": its bring-your-own-key mode names the model"
                .to_string(),
        ),
        Some("goose") => Err(
            "invalid input: Goose needs \"model\": its provider and model come from it".to_string(),
        ),
        Some("vibe") => {
            Err("invalid input: Vibe needs \"model\": its config names the model".to_string())
        }
        _ => Ok(()),
    }
}

/// The provider an engine's model must name. Gemini CLI reaches only the gateway's Gemini route, so
/// `gemini/<model>` is the one shape it takes. Qwen Code speaks chat completions and takes the three
/// routes that serve them. Cline takes the four routes its setup maps (Anthropic-shaped
/// `anthropic/` and `openrouter/`, and `openai/` and `opencode/`). Kilo Code takes what OpenCode
/// takes, so it has no rule here (the route checks the provider's shape). GitHub Copilot takes the
/// four routes its bring-your-own-key mode maps, and Goose the four its providers map. Kimi Code takes
/// the four routes its provider mode maps, Auggie only its own `augment/` models, and Mistral Vibe the
/// three its config maps. The other
/// engines keep their own routes.
fn engine_provider(input: &Value, model: &str) -> Result<(), String> {
    let provider = model.split_once('/').map(|(provider, _)| provider);
    match input.get("engine").and_then(Value::as_str) {
        Some("gemini") if !model.starts_with("gemini/") => Err(format!(
            "invalid input: \"model\" is {model:?}; Gemini CLI takes gemini/<model> models"
        )),
        Some("qwen") if !matches!(provider, Some("openai" | "openrouter" | "opencode")) => {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Qwen Code takes openai/<model>, openrouter/<model> or opencode/<model> models"
            ))
        }
        Some("cline")
            if !matches!(
                provider,
                Some("anthropic" | "openrouter" | "openai" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Cline takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ))
        }
        Some("copilot")
            if !matches!(
                provider,
                Some("anthropic" | "openai" | "openrouter" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Copilot takes anthropic/..., openai/..., openrouter/... or opencode/... models"
            ))
        }
        Some("vibe") if !matches!(provider, Some("openai" | "openrouter" | "opencode")) => {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Vibe takes openai/..., openrouter/... or opencode/... models"
            ))
        }
        Some("goose")
            if !matches!(
                provider,
                Some("anthropic" | "openrouter" | "openai" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Goose takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ))
        }
        Some("kimi")
            if !matches!(
                provider,
                Some("anthropic" | "openrouter" | "openai" | "opencode")
            ) =>
        {
            Err(format!(
                "invalid input: \"model\" is {model:?}; Kimi Code takes anthropic/..., openrouter/..., openai/... or opencode/... models"
            ))
        }
        Some("auggie") if provider != Some("augment") => Err(format!(
            "invalid input: \"model\" is {model:?}; Auggie runs Augment's own models: use augment/<model>"
        )),
        _ => Ok(()),
    }
}

/// Refuses a `code_engine` call that names an engine the owner has not enabled (`engines_enabled`).
/// The guest arm of `internal::dispatch` calls it beside `code_engine_model`.
pub(crate) fn code_engine_enabled(app: &App, input: &Value) -> Result<(), String> {
    engine_enabled(&enabled_engines(app), input)
}

/// The engine ids a `code_engine` call may name: the enabled built-in engines, then the enabled instances.
fn enabled_engines(app: &App) -> Vec<String> {
    let mut enabled = app.settings.get().engines_enabled;
    enabled.extend(app.engines.enabled_extras());
    enabled
}

/// Whether the `engine` named by `input` is one of `enabled`.
fn engine_enabled(enabled: &[String], input: &Value) -> Result<(), String> {
    let engine = input
        .get("engine")
        .and_then(Value::as_str)
        .ok_or("invalid input: \"engine\" must name an engine")?;
    if enabled.iter().any(|id| id == engine) {
        Ok(())
    } else {
        Err(format!("the owner has not enabled the {engine} engine"))
    }
}

/// The shape of the `model` input: `None` when absent or empty, else the trimmed `<provider>/<model>`
/// (the `@effort` suffix allowed).
fn model_input(input: &Value) -> Result<Option<&str>, String> {
    let model = match input.get("model") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(model)) if model.trim().is_empty() => return Ok(None),
        Some(Value::String(model)) => model.trim(),
        Some(_) => return Err("invalid input: \"model\" must be a string".to_string()),
    };
    let id = model.split_once('@').map_or(model, |(id, _)| id);
    if !id.contains('/') {
        return Err(format!(
            "invalid input: \"model\" must be <provider>/<model>, not {model:?}"
        ));
    }
    Ok(Some(model))
}

/// Whether `role` may be offered or run tool `name` (host or guest): the JHRL laws decide (`tools.jhrl`).
fn permitted(role: &str, name: &str) -> bool {
    crate::laws::global().permits(role, name)
}

/// Why host tool `name` may not run for `role`, or `None` when it may.
fn refusal(role: &str, name: &str) -> Option<String> {
    match runs(name) {
        Some(Runs::Host) if permitted(role, name) => None,
        Some(Runs::Host) => Some(format!("{name} is not available to the {role} role")),
        _ => Some(format!("{name} is not a host tool")),
    }
}

/// The tool names offered to `role` with no manifest grants; the law tests check each one.
#[cfg(test)]
pub(crate) fn offered_names(role: &str) -> Vec<String> {
    offered(role, &[])
}

/// The sorted, de-duplicated tool names offered to `role` given its manifest
/// `granted` list. Guest tools appear only when granted.
fn offered(role: &str, granted: &[String]) -> Vec<String> {
    // Every role gets every in-VM tool: inside its own VM an agent may do anything.
    let mut names: Vec<String> = granted
        .iter()
        .map(String::as_str)
        .chain(IN_VM.iter().copied())
        .chain(ALWAYS.iter().copied())
        .chain(GATED.iter().copied())
        .filter(|name| runs(name).is_some() && permitted(role, name))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Each tool's spec by name, built on first use. A spec depends on the name alone: the agent, role and
/// project only choose which specs are offered (`specs_for` and `specs_for_agent` do that), so a built spec
/// never goes stale. A name no tool has is cached as `None`.
static SPECS: LazyLock<Mutex<HashMap<String, Option<ToolSpec>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The spec for `name`: a one-line description and a JSON Schema input. Built once per name; later calls
/// return a clone from the cache.
pub(crate) fn spec(name: &str) -> Option<ToolSpec> {
    if let Some(hit) = SPECS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
    {
        return hit.clone();
    }
    // Built without the lock held, so a sub-spec can never deadlock on the table.
    let built = build_spec(name);
    SPECS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.to_string(), built.clone());
    built
}

/// One built-in spec as `builtin_specs.json` holds it.
#[derive(serde::Deserialize)]
struct BuiltinSpec {
    description: String,
    input_schema: Value,
}

/// The built-in tools' specs by name, parsed once from `builtin_specs.json`.
static BUILTIN: LazyLock<HashMap<String, BuiltinSpec>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("builtin_specs.json")).expect("builtin_specs.json is valid")
});

/// Builds the spec for `name` (see `spec`).
fn build_spec(name: &str) -> Option<ToolSpec> {
    if let Some(tool) = crate::foundry::breeder::spec(name) {
        return Some(tool);
    }
    if let Some(builtin) = BUILTIN.get(name) {
        return Some(ToolSpec {
            deferred: false,
            name: name.to_string(),
            description: builtin.description.clone(),
            input_schema: builtin.input_schema.clone(),
        });
    }
    let (description, input_schema) = crate::toolbox::spec(name)
        .or_else(|| crate::projects::spec(name))
        .or_else(|| crate::maintenance::spec(name))
        .or_else(|| crate::environments::spec(name))
        .or_else(|| crate::harness_control::spec(name))
        .or_else(|| crate::agent_proposals::spec(name))
        .or_else(|| crate::drafts::spec(name))
        .or_else(|| crate::platforms::repositories::spec(name))
        .or_else(|| crate::review_graph::tools::spec(name))
        .or_else(|| crate::test_select::spec(name))
        .or_else(|| crate::skills::agent_tools::spec(name))
        .or_else(|| crate::stations::tools::spec(name))
        .or_else(|| crate::planning::spec(name))
        .or_else(|| crate::tool_library::spec(name))?;
    Some(ToolSpec {
        deferred: false,
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

/// Every name the hub runs itself as a host tool, from its registries (the tables above and each module's `TOOLS`).
#[cfg(test)]
pub(crate) fn host_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = ALWAYS
        .iter()
        .chain(GATED.iter())
        .chain(CORE.iter())
        .chain(IN_VM.iter())
        .copied()
        .collect();
    for list in [
        crate::skills::agent_tools::TOOLS,
        crate::toolbox::TOOLS,
        crate::projects::TOOLS,
        crate::maintenance::TOOLS,
        crate::environments::TOOLS,
        crate::harness_control::TOOLS,
        crate::agent_proposals::TOOLS,
        crate::project_memory::TOOLS,
        crate::curator::TOOLS,
        crate::verifier::TOOLS,
        crate::drafts::TOOLS,
        crate::review_graph::tools::TOOLS,
        crate::test_select::TOOLS,
        crate::lab_observe::TOOLS,
        crate::journal::TOOLS,
        crate::planning::TOOLS,
        crate::stations::tools::TOOLS,
        crate::foundry::breeder::TOOLS,
        crate::tool_library::TOOLS,
    ] {
        names.extend(list.iter().copied());
    }
    names.retain(|name| runs(name) == Some(Runs::Host));
    names.sort_unstable();
    names.dedup();
    names
}

/// Plaza task tools (posting, assignment, results); `None` when `name` is not one of them.
fn host_tasks(
    app: &App,
    agent: &Agent,
    name: &str,
    input: &Value,
) -> Option<Result<String, String>> {
    match name {
        "plaza_tasks" => Some(Ok(Value::Array(
            app.plaza
                .tasks
                .list()
                .iter()
                .map(|t| task_json(t))
                .collect(),
        )
        .to_string())),
        "plaza_task_result" => Some((|| -> Result<String, String> {
            let task = task_arg(app, input)?;
            Ok(task_json(&task).to_string())
        })()),
        "plaza_post_task" => Some((|| -> Result<String, String> {
            let title = str_arg(input, "title")?;
            let body = str_arg(input, "body")?;
            let after = ids_arg(input, "after")?;
            // An assignee starts at once, so a task that still waits on its links cannot have one.
            if input.get("assignee").and_then(Value::as_u64).is_some() {
                let waiting = app.plaza.tasks.not_done(&after);
                if !waiting.is_empty() {
                    let verb = if waiting.len() == 1 { "is" } else { "are" };
                    return Err(format!(
                        "the assignee cannot start it yet: it runs after {}, which {verb} not done; post it without an assignee and the plaza hands it out once they are",
                        waiting.join(", ")
                    ));
                }
            }
            let role = input
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string);
            // Every task belongs to a project: the one named, else the poster's own. A project's agent posts
            // only to its own project; the orchestrator names the project it posts to.
            let named = input
                .get("project")
                .and_then(Value::as_str)
                .filter(|p| !p.trim().is_empty())
                .map(str::to_string);
            let project = own_project(app, agent, named)?;
            let pid = project.as_str();
            // A project's task is refused until the project's plan is approved, and once it is archived.
            takes_tasks(app, pid)?;
            let p = app
                .projects
                .get(pid)
                .ok_or_else(|| format!("no project {pid}"))?;
            if !matches!(p.stage.as_str(), "implementation" | "review") {
                return Err(format!(
                    "project {pid} is at {}: tasks can be posted only once its plan is approved",
                    p.stage
                ));
            }
            // The assignee works on the project: checked before the task exists, so a refusal posts nothing.
            if let Some(cid) = input.get("assignee").and_then(Value::as_u64) {
                let target = app
                    .agents
                    .get(cid)
                    .ok_or_else(|| format!("no connected agent {cid}"))?;
                crate::projects::may_take_task(app, &target, Some(pid))?;
            }
            let task = app
                .plaza
                .tasks
                .create(&CreateTask {
                    goal: format!("{title}\n\n{body}"),
                    acceptance: input
                        .get("acceptance")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map(str::to_string),
                    role,
                    after,
                    project: Some(project.clone()),
                })
                .map_err(|e| format!("{e:?}"))?;
            app.projects.link_task(pid, task.id())?;
            app.work.note_poster(task.id(), agent.cid);
            if let Some(cid) = input.get("assignee").and_then(Value::as_u64) {
                assign(app, &task, cid)?;
            }
            Ok(task_json(&task).to_string())
        })()),
        "plaza_tasks_post" => Some((|| -> Result<String, String> {
            let project = str_arg(input, "project")?;
            own_project(app, agent, Some(project.to_string()))?;
            takes_tasks(app, project)?;
            let tasks: Vec<crate::task_plan::Planned> = serde_json::from_value(
                input.get("tasks").cloned().unwrap_or(Value::Null),
            )
            .map_err(|e| {
                format!("`tasks` must be a list of {{key, goal, acceptance?, role?, after?}}: {e}")
            })?;
            let posted = crate::task_plan::post(&app.projects, &app.plaza.tasks, project, &tasks)?;
            for task in &posted {
                app.work.note_poster(&task.id, agent.cid);
            }
            Ok(format!(
                "{}\nNext: each task goes to a worker as soon as everything it runs after is done; follow them with plaza_task_result.",
                json!({ "project": project, "tasks": posted })
            ))
        })()),
        "plaza_task_link" => Some((|| -> Result<String, String> {
            let task = str_arg(input, "task")?;
            if input.get("after").is_none() {
                return Err(
                    "missing list `after`: the whole new list of the tasks it runs after ([] for none)"
                        .to_string(),
                );
            }
            let after = ids_arg(input, "after")?;
            let task = app
                .plaza
                .tasks
                .relink(task, &after)
                .map_err(|e| e.to_string())?;
            Ok(task_json(&task).to_string())
        })()),
        "plaza_assign" => Some((|| -> Result<String, String> {
            let task = task_arg(app, input)?;
            let cid = u64_arg(input, "agent")?;
            assign(app, &task, cid)?;
            Ok(format!("assigned {} to {cid}", task.id()))
        })()),
        "plaza_cancel_task" => Some((|| -> Result<String, String> {
            task_arg(app, input)?
                .withdraw()
                .map_err(|e| format!("{e:?}"))?;
            Ok("cancelled".into())
        })()),
        _ => None,
    }
}

/// Plaza board, message and agent tools; `None` when `name` is not one of them.
fn host_social(
    app: &App,
    agent: &Agent,
    me: &ConnectedAgent,
    name: &str,
    input: &Value,
) -> Option<Result<String, String>> {
    match name {
        "plaza_board_read" => Some(to_text(&app.plaza.social.board(&[], 50))),
        "plaza_board_comment" => Some((|| -> Result<String, String> {
            let post = str_arg(input, "post")?;
            let text = str_arg(input, "text")?;
            if text.chars().count() > 280 {
                return Err("a comment must be at most 280 characters".to_string());
            }
            let files = attachments_arg(app, input, agent.cid)?;
            publish_files(app, &files, agent.cid)?;
            let who = agent.label();
            let mut comment = app
                .plaza
                .social
                .comment(post, Some(agent.cid), &who, text)
                .map_err(|e| e.message().to_string())?;
            if !files.is_empty() {
                app.plaza
                    .social
                    .attach("comment", &comment.id, &files)
                    .map_err(|e| e.message().to_string())?;
                comment.attachments = files.clone();
            }
            crate::plaza::notify_post_author(app, post, Some(agent.cid), &who, text);
            to_text(&comment)
        })()),
        "plaza_board_post" => Some((|| -> Result<String, String> {
            let text = str_arg(input, "text")?;
            let files = attachments_arg(app, input, agent.cid)?;
            publish_files(app, &files, agent.cid)?;
            let subject: String = text.chars().take(60).collect();
            let request: PostRequest =
                from_req(json!({"agent_cid": agent.cid, "subject": subject, "body": text}))?;
            // The post is on the author's project's board (the orchestrator's on the whole board).
            let mut post = request.checked()?;
            post.project = crate::projects::project_of(app, agent.cid);
            let mut view = app.plaza.social.post(me, post).map_err(|e| e.to_string())?;
            if !files.is_empty() {
                app.plaza
                    .social
                    .attach("post", &view.id, &files)
                    .map_err(|e| e.message().to_string())?;
                view.attachments = files.clone();
            }
            to_text(&view)
        })()),
        "plaza_send_message" => Some((|| -> Result<String, String> {
            let to_cid = u64_arg(input, "to_cid")?;
            let text = str_arg(input, "text")?;
            let files = attachments_arg(app, input, agent.cid)?;
            let recipient = app
                .agents
                .get(to_cid)
                .ok_or_else(|| format!("no connected agent {to_cid}"))?;
            may_message(app, agent, &recipient)?;
            // A message to oneself is refused.
            if me.cid == to_cid {
                return Err(format!("guest CID {} cannot speak to itself", me.cid));
            }
            grant_files(app, &files, agent.cid, to_cid)?;
            let mut suffix = String::new();
            for id in &files {
                if let Some(meta) = app.deliverables.meta(id) {
                    suffix.push_str(&format!(
                        "\n[attached file {id}: {} ({}, {} bytes) — fetch http://plaza.gateway/files/{id} with download or bash curl via the proxy]",
                        meta.title, meta.content_type, meta.size
                    ));
                }
            }
            // The message is delivered as work for the peer; nothing about it is kept on the plaza.
            let to = agents::connected_agent(&recipient);
            let (low, high) = if me.cid < to.cid {
                (me.cid, to.cid)
            } else {
                (to.cid, me.cid)
            };
            let origin = Origin {
                from_cid: Some(me.cid),
                ..Origin::default()
            };
            let _ = app.work.enqueue(
                app,
                to.cid,
                WorkKind::Message,
                format!("dm-{low}-{high}"),
                format!("{text}{suffix}"),
                origin,
            );
            to_text(&json!({"to_cid": to_cid, "delivered": true, "attachments": files}))
        })()),
        "plaza_agents" => Some({
            let rows: Vec<Value> = app
                .agents
                .list()
                .iter()
                .map(|a| {
                    json!({
                        "cid": a.cid,
                        "role": a.role,
                        "status": a.status,
                        "project": crate::projects::project_of(app, a.cid),
                    })
                })
                .collect();
            Ok(Value::Array(rows).to_string())
        }),
        _ => None,
    }
}

/// Runs an already-gated host tool; `Err` carries the reason for the model.
fn host_call(
    app: &App,
    agent: &Agent,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    if crate::tool_library::TOOLS.contains(&name) {
        return crate::tool_library::run(app, agent, work, name, input);
    }
    let me = agents::connected_agent(agent);
    if let Some(result) = host_tasks(app, agent, name, input) {
        return result;
    }
    if let Some(result) = host_social(app, agent, &me, name, input) {
        return result;
    }
    match name {
        name if crate::foundry::breeder::TOOLS.contains(&name) => {
            crate::foundry::breeder::run(app, agent, name, input)
        }
        "submit_claim" => {
            str_arg(input, "summary")?;
            Ok("claim recorded".into())
        }
        "transcript_search" => {
            let query = str_arg(input, "query")?;
            to_text(&app.transcripts.search(agent.cid, query, 20))
        }
        "plaza_get_file" => {
            let id = str_arg(input, "id")?;
            let meta = app
                .deliverables
                .meta(id)
                .ok_or_else(|| format!("no shared file {id}"))?;
            if !app.deliverables.can_read(id, agent.cid) {
                return Err(format!("you cannot read file {id}"));
            }
            let text = if meta.size <= 256 * 1024 {
                String::from_utf8(app.deliverables.read_blob(&meta)?).ok()
            } else {
                None
            };
            Ok(text.unwrap_or_else(|| {
                format!(
                    "{}: {}, {} bytes — binary; fetch http://plaza.gateway/files/{id} with download",
                    meta.title, meta.content_type, meta.size
                )
            }))
        }
        "team_set" => crate::work_loop::team_set(app, agent, input),
        "thread_read" => crate::work_loop::thread_read(app, agent.cid, str_arg(input, "thread")?),
        "thread_answer" => crate::work_loop::thread_answer(
            app,
            agent.cid,
            str_arg(input, "thread")?,
            str_arg(input, "text")?,
            input.get("close").and_then(Value::as_bool).unwrap_or(false),
        ),
        "plaza_message_seat" => crate::work_loop::message_seat(
            app,
            agent.cid,
            str_arg(input, "to")?,
            str_arg(input, "text")?,
            input.get("thread").and_then(Value::as_str),
            input.get("subject").and_then(Value::as_str),
            input.get("close").and_then(Value::as_bool).unwrap_or(false),
        ),
        "plaza_verdict" => {
            let task = str_arg(input, "task")?;
            let pass = input
                .get("pass")
                .and_then(Value::as_bool)
                .ok_or_else(|| "missing boolean `pass`".to_string())?;
            let notes = str_arg(input, "notes")?;
            // A reviewer seat's verdict moves the task's seats on (work_loop.rs).
            if let Some(reply) = crate::work_loop::on_verdict(app, agent.cid, task, pass, notes) {
                return Ok(reply);
            }
            app.work.verdict(app, task, work, pass, notes)
        }
        other if crate::journal::TOOLS.contains(&other) => {
            crate::journal::run(app, agent, other, input)
        }
        other if crate::planning::TOOLS.contains(&other) => {
            crate::planning::run(app, agent, other, input)
        }
        other if crate::skills::agent_tools::TOOLS.contains(&other) => {
            crate::skills::agent_tools::run(app, agent, other, input)
        }
        other if crate::toolbox::TOOLS.contains(&other) => {
            crate::toolbox::run(app, agent, other, input)
        }
        other if crate::projects::TOOLS.contains(&other) => {
            crate::projects::run(app, agent, other, input)
        }
        other if crate::maintenance::TOOLS.contains(&other) => {
            crate::maintenance::run(app, agent, other, input)
        }
        other if crate::environments::TOOLS.contains(&other) => {
            crate::environments::run(app, agent, other, input)
        }
        other if crate::harness_control::TOOLS.contains(&other) => {
            crate::harness_control::run(app, agent, other, input)
        }
        other if crate::stations::tools::TOOLS.contains(&other) => {
            crate::stations::tools::run(app, agent, other, input)
        }
        "plaza_agent_propose" => crate::agent_proposals::run(app, agent, input),
        other if crate::project_memory::TOOLS.contains(&other) => {
            crate::project_memory::run(app, agent, other, input)
        }
        other if crate::curator::TOOLS.contains(&other) => crate::curator::run(app, agent, input),
        other if crate::verifier::TOOLS.contains(&other) => crate::verifier::run(app, agent, input),
        "plaza_project_draft" => crate::drafts::run(app, agent, input),
        "plaza_repository" => {
            crate::platforms::repositories::run(app, agent, input).map(|value| value.to_string())
        }
        other if crate::review_graph::tools::TOOLS.contains(&other) => {
            crate::review_graph::tools::run(app, agent, work, other, input)
        }
        other if crate::test_select::TOOLS.contains(&other) => {
            crate::test_select::run(app, agent, input)
        }
        other if crate::lab_observe::TOOLS.contains(&other) => {
            crate::lab_observe::run(app, agent, other, input)
        }
        "plaza_connector_propose" => crate::connectors::propose(app, agent, input),
        other if crate::connectors::by_tool(other).is_some() => {
            let (connector, action) =
                crate::connectors::by_tool(other).ok_or("unknown connector tool")?;
            let name = agent.label();
            crate::connectors::call(
                app,
                Some((agent.cid, name.as_str())),
                &agent.role,
                connector.id,
                action.name,
                input,
            )
        }
        other if other.starts_with(crate::mcp::PREFIX) => {
            crate::mcp::call_for_agent(app, agent, work, other, input)
        }
        other => Err(format!("{other} is not implemented")),
    }
}

/// The project a task `agent` posts goes to: `named`, else the agent's own. An agent on a project posts only to
/// it; the orchestrator, which works on every project, names one.
fn own_project(app: &App, agent: &Agent, named: Option<String>) -> Result<String, String> {
    if crate::settings::plaza_wide(&agent.role) {
        // In the project chat a task's work is in, the task is that project's unless another is named (the main chat
        // is no project's).
        let from_chat = || {
            app.work
                .current(agent.cid)
                .and_then(|item| item.thread.strip_prefix("chat:").map(str::to_string))
                .and_then(|id| app.chats.get(&id))
                .filter(|c| c.kind == crate::chats::Kind::Project)
                .and_then(|c| c.project)
        };
        return named.or_else(from_chat).ok_or_else(|| {
            "a task belongs to a project: name it with `project` (plaza_projects lists them)"
                .to_string()
        });
    }
    let own = crate::projects::project_of(app, agent.cid)
        .ok_or("you have no project: you post tasks only for your project")?;
    match named {
        Some(named) if named != own => Err(format!(
            "you work on project {own}: you post tasks only for it, not for {named}"
        )),
        _ => Ok(own),
    }
}

/// Refuses tasks for an archived project: it is over, and its agents' work has ended.
fn takes_tasks(app: &App, pid: &str) -> Result<(), String> {
    match app.projects.get(pid) {
        None => Err(format!("no project {pid}")),
        Some(p) if p.archived => Err(format!("project {pid} is archived: it takes no new tasks")),
        // In maintenance the project's work is what the owner picked from the proposals.
        Some(_) => crate::maintenance::refuse_agent_tasks(app, pid),
    }
}

/// Whether `from` may message `to`: both work now, on the same project, or one of them is the orchestrator.
fn may_message(app: &App, from: &Agent, to: &Agent) -> Result<(), String> {
    if let Some(why) = crate::projects::held(app, to) {
        return Err(format!("{} cannot be messaged now: {why}", to.label()));
    }
    if crate::settings::plaza_wide(&from.role) || crate::settings::plaza_wide(&to.role) {
        return Ok(());
    }
    let (mine, theirs) = (
        crate::projects::project_of(app, from.cid),
        crate::projects::project_of(app, to.cid),
    );
    if mine == theirs {
        Ok(())
    } else {
        Err(format!(
            "{} works on another project: you message only your project's agents and the orchestrator",
            to.label()
        ))
    }
}

/// Claims `task` for the agent `cid` and queues the work for it.
fn assign(app: &App, task: &Task, cid: u64) -> Result<(), String> {
    let target = app
        .agents
        .get(cid)
        .ok_or_else(|| format!("no connected agent {cid}"))?;
    crate::projects::may_take_task(app, &target, task.project())?;
    task.claim(&agents::connected_agent(&target))
        .map_err(|e| e.to_string())?;
    let (prompt, origin) = crate::weft_hub::task_work(app, task, cid, task_text(task));
    let _ = app.work.enqueue(
        app,
        cid,
        WorkKind::Task,
        format!("task-{}", task.id()),
        prompt,
        origin,
    );
    Ok(())
}

/// The prompt a task's assignee receives: its goal and acceptance.
fn task_text(task: &Task) -> String {
    match task.acceptance() {
        Some(acceptance) => format!("{}\n\nAcceptance: {acceptance}", task.goal()),
        None => task.goal().to_string(),
    }
}

/// A task as the model sees it.
fn task_json(task: &Task) -> Value {
    let view = task.view();
    json!({
        "id": view.id,
        "goal": view.goal,
        "state": view.state,
        "role": view.role,
        "acceptance": view.acceptance,
        "project": view.project,
        "after": view.after,
        "ready": view.ready,
        "waiting_on": view.waiting_on,
        "blocked_by": view.blocked_by
    })
}

/// A list of task ids: absent is none, and anything but an array of strings is refused.
fn ids_arg(input: &Value, key: &str) -> Result<Vec<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("`{key}` must be a list of task ids"))
            })
            .collect(),
        Some(_) => Err(format!("`{key}` must be a list of task ids")),
    }
}

/// The task named by the `task` argument.
fn task_arg(app: &App, input: &Value) -> Result<Arc<Task>, String> {
    let id = str_arg(input, "task")?;
    app.plaza
        .tasks
        .find(id)
        .ok_or_else(|| format!("no task {id}"))
}

/// A required, non-blank string argument.
fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing or empty string `{key}`"))
}

/// A required integer argument.
fn u64_arg(input: &Value, key: &str) -> Result<u64, String> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing integer `{key}`"))
}

/// The optional `attachments` argument: at most 8 shared files the caller may read.
fn attachments_arg(app: &App, input: &Value, cid: u64) -> Result<Vec<String>, String> {
    let ids: Vec<String> = match input.get("attachments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "attachments must be file ids (strings)".to_string())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("attachments must be an array of file ids".to_string()),
    };
    if ids.len() > 8 {
        return Err("at most 8 attachments".to_string());
    }
    for id in &ids {
        if app.deliverables.meta(id).is_none() {
            return Err(format!("no shared file {id}"));
        }
        if !app.deliverables.can_read(id, cid) {
            return Err(format!("you cannot read file {id}"));
        }
    }
    Ok(ids)
}

/// Makes each private file public, as a board post or comment does.
fn publish_files(app: &App, files: &[String], cid: u64) -> Result<(), String> {
    for id in files {
        if app
            .deliverables
            .meta(id)
            .is_some_and(|m| m.audience != "public")
        {
            app.deliverables
                .publish(id, cid)
                .map_err(|e| format!("file {id}: {e}"))?;
        }
    }
    Ok(())
}

/// Lets the recipient read each private file, as a direct message does.
fn grant_files(app: &App, files: &[String], cid: u64, to: u64) -> Result<(), String> {
    for id in files {
        if app
            .deliverables
            .meta(id)
            .is_some_and(|m| m.audience != "public")
        {
            app.deliverables
                .grant(id, cid, &[to])
                .map_err(|e| format!("file {id}: {e}"))?;
        }
    }
    Ok(())
}

/// Deserializes a plaza request built from tool input.
fn from_req<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Serializes a plaza or store view as the tool's output text.
fn to_text<T: Serialize>(view: &T) -> Result<String, String> {
    serde_json::to_string(view).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// Every built-in spec is complete: a name it is served under, a non-empty description and an object
    /// schema. `bash` is pinned to the spec it had as code, before the specs moved into `builtin_specs.json`.
    #[test]
    fn every_builtin_spec_is_complete_and_bash_is_unchanged() {
        assert!(!BUILTIN.is_empty());
        for name in BUILTIN.keys() {
            let built = build_spec(name).unwrap_or_else(|| panic!("no spec for {name}"));
            assert_eq!(built.name, *name);
            assert!(!built.description.is_empty(), "{name} has no description");
            assert_eq!(
                built.input_schema["type"], "object",
                "{name} schema is not an object"
            );
        }
        let run = build_spec("run").expect("the run spec");
        assert_eq!(run.name, "run");
        assert_eq!(run.description, BUILTIN["run"].description);
        assert_eq!(run.input_schema, BUILTIN["run"].input_schema);
    }

    use super::*;
    use crate::settings::ORCHESTRATOR_ROLE;

    #[test]
    fn code_engine_model_input_names_a_provider_or_takes_the_default() {
        let ask = |model: &str| json!({"engine": "claude", "prompt": "p", "model": model});
        // A provider/model id is passed through as written, slashes and effort suffix included.
        // An absent, null or blank model is the default.
        for (case, input, want) in [
            (
                "an openrouter id keeps its nested slashes",
                ask(" openrouter/anthropic/claude-x "),
                Ok(Some("openrouter/anthropic/claude-x")),
            ),
            (
                "an effort suffix is part of the shape",
                ask("opencode/claude-sonnet-5-5@high"),
                Ok(Some("opencode/claude-sonnet-5-5@high")),
            ),
            (
                "an absent model is the default",
                json!({"prompt": "p"}),
                Ok(None),
            ),
            (
                "a blank model is the default",
                json!({"model": "  "}),
                Ok(None),
            ),
            (
                "a null model is the default",
                json!({"model": null}),
                Ok(None),
            ),
        ] {
            assert_eq!(model_input(&input), want, "{case}");
        }
        // A bare model name is refused: the model must name a provider (code_engine_model_must_name_a_provider).
        {
            let bare = json!({"engine": "codex", "prompt": "p", "model": "claude-sonnet-5-5"});
            let err = model_input(&bare).unwrap_err();
            assert!(err.contains("<provider>/<model>"), "{err}");
            assert!(model_input(&json!({"model": 3})).is_err());
        }
    }

    #[test]
    fn code_engine_routes_take_each_engines_provider_models() {
        // Gemini CLI takes only gemini/<model> models; the other engines keep their own routes.
        {
            let ask = |engine: &str, model: &str| json!({"engine": engine, "prompt": "p", "model": model});
            assert!(
                engine_provider(
                    &ask("gemini", "gemini/gemini-2.5-pro@high"),
                    "gemini/gemini-2.5-pro@high"
                )
                .is_ok()
            );
            let err = engine_provider(&ask("gemini", "openrouter/x"), "openrouter/x").unwrap_err();
            assert!(
                err.contains("Gemini CLI takes gemini/<model> models"),
                "{err}"
            );
            // The other engines keep their own routes.
            assert!(engine_provider(&ask("claude", "openrouter/x"), "openrouter/x").is_ok());
        }

        // Qwen Code takes the openai, openrouter and opencode chat-completions routes.
        {
            let ask = |model: &str| json!({"engine": "qwen", "prompt": "p", "model": model});
            for model in [
                "openai/gpt-5.6-terra",
                "openrouter/x/y",
                "opencode/deepseek-v4-pro",
            ] {
                assert!(engine_provider(&ask(model), model).is_ok(), "{model}");
            }
            for model in ["anthropic/claude-x", "bare-model"] {
                let err = engine_provider(&ask(model), model).unwrap_err();
                assert!(err.contains("Qwen Code takes openai/<model>"), "{err}");
            }
        }

        // Copilot and Goose take their provider routes and refuse a missing model.
        {
            let copilot = |model: &str| json!({"engine": "copilot", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openai/gpt-x",
                "openrouter/anthropic/claude-x",
                "opencode/gpt-x",
            ] {
                assert!(engine_provider(&copilot(model), model).is_ok(), "{model}");
            }
            assert!(engine_provider(&copilot("gemini/x"), "gemini/x").is_err());
            let err =
                model_required(&json!({"engine": "copilot", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Copilot needs \"model\""), "{err}");
            let goose = |model: &str| json!({"engine": "goose", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openrouter/anthropic/claude-x",
                "openai/gpt-x",
            ] {
                assert!(engine_provider(&goose(model), model).is_ok(), "{model}");
            }
            assert!(engine_provider(&goose("qwen/x"), "qwen/x").is_err());
            let err =
                model_required(&json!({"engine": "goose", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Goose needs \"model\""), "{err}");
        }

        // Kimi Code takes its provider routes and needs a model.
        {
            let kimi = |model: &str| json!({"engine": "kimi", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openai/gpt-x",
                "openrouter/anthropic/claude-x",
                "opencode/gpt-x",
            ] {
                assert!(engine_provider(&kimi(model), model).is_ok(), "{model}");
            }
            assert!(engine_provider(&kimi("gemini/x"), "gemini/x").is_err());
            let err =
                model_required(&json!({"engine": "kimi", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Kimi Code needs \"model\""), "{err}");
        }

        // Auggie takes only augment/<model> and needs no model.
        {
            let auggie = |model: &str| json!({"engine": "auggie", "prompt": "p", "model": model});
            assert!(engine_provider(&auggie("augment/claude-x"), "augment/claude-x").is_ok());
            let err = engine_provider(&auggie("anthropic/claude-x"), "anthropic/claude-x")
                .expect_err("refused");
            assert!(err.contains("use augment/<model>"), "{err}");
            assert!(model_required(&json!({"engine": "auggie", "prompt": "p"})).is_ok());
        }

        // Vibe takes the three routes its config maps and needs a model.
        {
            let vibe = |model: &str| json!({"engine": "vibe", "prompt": "p", "model": model});
            for model in [
                "openai/gpt-x",
                "openrouter/mistralai/mistral-large",
                "opencode/gpt-x",
            ] {
                assert!(engine_provider(&vibe(model), model).is_ok(), "{model}");
            }
            let err = engine_provider(&vibe("anthropic/x"), "anthropic/x").unwrap_err();
            assert!(err.contains("Vibe takes openai/..."), "{err}");
            let err =
                model_required(&json!({"engine": "vibe", "prompt": "p"})).expect_err("refused");
            assert!(err.contains("Vibe needs \"model\""), "{err}");
        }

        // Cline takes the four routes and needs a model.
        {
            let ask = |model: &str| json!({"engine": "cline", "prompt": "p", "model": model});
            for model in [
                "anthropic/claude-x",
                "openrouter/anthropic/claude-x",
                "openai/gpt-x",
                "opencode/deepseek-v4-pro",
            ] {
                assert!(engine_provider(&ask(model), model).is_ok(), "{model}");
            }
            let err = engine_provider(&ask("gemini/x"), "gemini/x").unwrap_err();
            assert!(err.contains("Cline takes anthropic/..."), "{err}");

            let no_model = json!({"engine": "cline", "prompt": "p"});
            assert_eq!(
                model_required(&no_model).unwrap_err(),
                "invalid input: Cline needs \"model\""
            );
            assert!(model_required(&json!({"engine": "claude", "prompt": "p"})).is_ok());
        }

        // Kilo has no provider rule of its own.
        {
            let input = json!({"engine": "kilo", "prompt": "p", "model": "anthropic/x"});
            assert!(engine_provider(&input, "anthropic/x").is_ok());
        }
    }

    #[test]
    fn code_engine_offers_sessions_modes_and_only_the_enabled_engines() {
        let spec = spec("code_engine").expect("code_engine is a spec");
        assert!(spec.description.contains("streams live"));
        assert!(spec.description.contains("`session`"));
        assert!(spec.description.contains("gemini = Gemini CLI"));
        assert!(spec.description.contains("qwen = Qwen Code"));
        assert!(spec.description.contains("kilo = Kilo Code"));
        assert!(spec.description.contains("cline = Cline"));
        let props = &spec.input_schema["properties"];
        assert_eq!(
            props["engine"]["enum"],
            json!([
                "claude", "codex", "opencode", "gemini", "qwen", "kilo", "cline", "copilot",
                "goose", "kimi", "auggie", "vibe"
            ])
        );
        assert_eq!(props["session"]["type"], "string");
        assert_eq!(props["mode"]["type"], "string");
        let mut schema = spec.input_schema.clone();
        restrict_engines(&mut schema, &["codex".to_string()]);
        assert_eq!(schema["properties"]["engine"]["enum"], json!(["codex"]));
    }

    #[test]
    fn code_engine_refuses_an_engine_the_owner_has_not_enabled() {
        let enabled = vec!["claude".to_string(), "codex".to_string()];
        let ask = |engine: &str| json!({"engine": engine, "prompt": "p"});
        assert_eq!(engine_enabled(&enabled, &ask("codex")), Ok(()));
        assert_eq!(
            engine_enabled(&enabled, &ask("opencode")),
            Err("the owner has not enabled the opencode engine".to_string())
        );
        assert!(engine_enabled(&enabled, &json!({"prompt": "p"})).is_err());
    }

    #[test]
    fn role_offer_follows_tier_seat_projects_and_laws() {
        // A tier changes the offer: orchestrators post tasks, workers do not.
        {
            let worker = offered("worker", &[]);
            let orchestrator = offered(ORCHESTRATOR_ROLE, &[]);
            assert!(!worker.iter().any(|n| n == "plaza_post_task"));
            assert!(orchestrator.iter().any(|n| n == "plaza_post_task"));
            // The order of a plan's tasks is the orchestrator's too: it posts the list and relinks.
            for tool in ["plaza_tasks_post", "plaza_task_link"] {
                assert!(orchestrator.iter().any(|n| n == tool), "{tool}");
                assert!(!worker.iter().any(|n| n == tool), "{tool}");
            }
        }

        // Projects and harness tools go to their roles; everyone reads projects and critiques them.
        {
            let has = |role: &str, tool: &str| offered(role, &[]).iter().any(|n| n == tool);
            assert!(has(ORCHESTRATOR_ROLE, "plaza_project_open"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_project_advance"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_agent_control"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_loop"));
            assert!(has("planner", "plaza_project_advance"));
            assert!(has("planner", "team_set"));
            assert!(!has("builder", "team_set"));
            assert!(has(ORCHESTRATOR_ROLE, "plaza_project_approve"));
            assert!(!has("builder", "plaza_project_open"));
            assert!(!has("builder", "plaza_project_approve"));
            assert!(!has("builder", "plaza_agent_control"));
            // Everyone can read projects, attach to them and critique them.
            assert!(has("builder", "plaza_projects") && has("builder", "plaza_project_critique"));
        }

        // The laws decide the offer: a builder cannot post a task or approve a project, the orchestrator can.
        {
            assert!(
                !offered("builder", &[])
                    .iter()
                    .any(|n| n == "plaza_post_task")
            );
            assert!(
                !offered("builder", &[])
                    .iter()
                    .any(|n| n == "plaza_project_approve")
            );
            assert!(
                offered(ORCHESTRATOR_ROLE, &[])
                    .iter()
                    .any(|n| n == "plaza_project_approve")
            );
            assert!(refusal("builder", "plaza_post_task").is_some());
        }

        // A worker calling post_task is refused.
        {
            assert!(refusal("worker", "plaza_post_task").is_some());
            assert!(refusal(ORCHESTRATOR_ROLE, "plaza_post_task").is_none());
            assert!(refusal("builder", "plaza_task_link").is_some());
            assert!(refusal(ORCHESTRATOR_ROLE, "plaza_task_link").is_none());
            assert!(refusal("planner", "plaza_tasks_post").is_some());
            assert!(refusal("worker", "run").is_some());
        }
    }

    #[test]
    fn repository_provisioning_is_offered_and_admitted_only_to_the_orchestrator() {
        let name = "plaza_repository";
        assert!(
            offered(ORCHESTRATOR_ROLE, &[])
                .iter()
                .any(|tool| tool == name)
        );
        assert!(refusal(ORCHESTRATOR_ROLE, name).is_none());
        for role in [
            "worker", "planner", "builder", "reviewer", "verifier", "sceptic", "merger",
        ] {
            assert!(
                !offered(role, &[]).iter().any(|tool| tool == name),
                "{role} was offered repository provisioning"
            );
            assert!(
                refusal(role, name).is_some(),
                "{role} was admitted to repository provisioning"
            );
        }
        let tool = spec(name).expect("repository provisioning has a declared schema");
        assert_eq!(tool.input_schema["additionalProperties"], false);
        assert_eq!(
            tool.input_schema["properties"]["action"]["enum"],
            json!(["list", "create", "connect"])
        );
    }

    #[test]
    fn a_tool_spec_is_the_same_on_every_call_and_as_a_fresh_build() {
        for name in ["run", "read", "code_engine", "artifact_create"] {
            let first = spec(name).expect("a spec for each named tool");
            assert_eq!(first, spec(name).expect("the same spec again"));
            assert_eq!(
                first,
                build_spec(name).expect("a fresh build of the same spec")
            );
        }
    }

    #[test]
    fn every_role_gets_the_in_vm_tools() {
        for role in ["worker", "reviewer", "planner"] {
            let offered = offered(role, &[]);
            for name in IN_VM {
                assert!(offered.iter().any(|n| n == name), "{role} lacks {name}");
            }
        }
        assert!(runs("run") == Some(Runs::Guest));
        // A script is offered to every role with its schema, and runs in the VM as a command does.
        assert!(runs("run_script") == Some(Runs::Guest));
        assert!(CORE.contains(&"run_script"));
    }

    #[test]
    fn artifact_tools_are_offered_with_schemas_that_parse() {
        for name in ["artifact_create", "artifact_update", "artifact_attach_data"] {
            assert!(
                offered("worker", &[]).iter().any(|n| n == name),
                "{name} is offered to every role"
            );
            assert!(runs(name) == Some(Runs::Guest), "{name} runs in the VM");
            let tool = spec(name).expect("a spec for each artifact tool");
            assert_eq!(tool.name, name);
            assert!(!tool.description.is_empty(), "{name} is described");
            assert!(
                tool.input_schema["type"] == "object",
                "{name} takes an object"
            );
            assert!(
                tool.input_schema["properties"].is_object(),
                "{name} names its properties"
            );
        }
        let create = spec("artifact_create").expect("the create spec");
        assert_eq!(create.input_schema["required"], json!(["title", "panels"]));
        // The shapes the description teaches are the hub's own: a dashboard parses as a Spec and passes its
        // validation, and each other body kind parses as a Body.
        let dashboard = json!({
            "title": "Build times",
            "sub": "last three runs",
            "panels": [
                {"id": "runs", "title": "Duration", "span": 8, "body": {
                    "type": "chart", "unit": "ms",
                    "x": {"type": "labels", "labels": ["run 1", "run 2", "run 3"]},
                    "series": [{"name": "build", "tone": "chart-1", "values": [1200, 1100, null]}],
                    "height": 240}},
                {"id": "now", "title": "Latest", "span": 4, "body": {
                    "type": "tiles",
                    "tiles": [{"label": "Median", "value": "1.2 s", "spark": [1.2, 1.1], "tone": "chart-1"}]}},
                {"id": "steps", "title": "Steps", "span": 6, "body": {
                    "type": "table",
                    "columns": [{"head": "Step"}, {"head": "ms", "num": true}],
                    "rows": [{"cells": [{"type": "mono", "text": "compile"}, {"type": "num", "text": "800"}]}]}},
                {"id": "deps", "title": "Dependencies", "span": 6, "body": {
                    "type": "graph",
                    "nodes": [{"id": "core", "label": "core"}, {"id": "api", "label": "api"}],
                    "edges": [{"from": "api", "to": "core"}], "layout": "layered", "directed": true}},
                {"id": "notes", "title": "Notes", "span": 12, "body": {
                    "type": "document", "markdown": "Compile time fell."}}
            ]
        });
        let parsed: apz_view::artifact::Spec =
            serde_json::from_value(dashboard).expect("a spec the hub reads");
        assert_eq!(
            apz_view::artifact::validate(&parsed, &apz_view::artifact::Caps::default()),
            Vec::<String>::new()
        );
        let bodies = [
            json!({"type": "bars", "rows": [{"label": "api", "value": "4.2k", "frac": 0.8, "status": "ok"}]}),
            json!({"type": "dataset", "columns": [{"key": "name", "label": "Name"}, {"key": "ms", "label": "ms", "kind": "number", "unit": "ms"}], "rows": [["compile", 800]], "total": 1}),
            json!({"type": "image", "blob": {"id": "b1"}, "alt": "A chart of build times"}),
            json!({"type": "code", "language": "rust", "text": "fn main() {}", "filename": "main.rs"}),
            json!({"type": "html", "entry": "index.html", "files": [{"path": "index.html", "text": "<p>hi</p>"}], "height": 240}),
        ];
        for body in bodies {
            serde_json::from_value::<apz_view::model::Body>(body).expect("a body the hub reads");
        }
    }

    /// Agents never run anything on the host: the host-side tools are the
    /// plaza's data operations only (no processes, files or network).
    #[test]
    fn host_tools_are_plaza_data_operations_only() {
        let host: Vec<&str> = [
            "transcript_search",
            "submit_claim",
            "plaza_tasks",
            "plaza_board_read",
            "plaza_board_post",
            "plaza_board_comment",
            "plaza_send_message",
            "plaza_agents",
            "plaza_post_task",
            "plaza_assign",
            "plaza_cancel_task",
            "plaza_task_result",
            "plaza_get_file",
            "plaza_verdict",
        ]
        .into_iter()
        .filter(|name| runs(name) == Some(Runs::Host))
        .collect();
        assert_eq!(host.len(), 14);
        assert_eq!(runs("fetch_url"), Some(Runs::Guest));
        for name in ["exec", "shell", "http_get", "read_host_file"] {
            assert_eq!(runs(name), None, "{name} must not exist");
        }
    }

    /// The sources a section is built from, as the tests move them.
    struct Sources {
        gens: Generations<String>,
        laws: Laws,
    }

    impl Sources {
        fn new() -> Sources {
            Sources {
                gens: Generations::new(),
                laws: Laws::builtin(),
            }
        }
    }

    /// The section every test reads: builder on project `p1`, running on node `n1`.
    fn section_key() -> SectionKey {
        SectionKey {
            role: "builder".into(),
            project: Some("p1".into()),
            node: Some("n1".into()),
        }
    }

    fn empty_section() -> Section {
        Section {
            tools: Vec::new(),
            mcp: Vec::new(),
            skills: None,
        }
    }

    /// Reads the section of `key` through `sections` now; true when it was built rather than answered from the cache.
    fn builds_on_read(sections: &Sections, src: &Sources, key: &SectionKey) -> bool {
        let before = sections.builds();
        sections.get(
            key,
            || section_generation(&src.gens, &src.laws, key),
            empty_section,
        );
        sections.builds() > before
    }

    /// A section read once is answered from the cache until a change to one of its sources; that change rebuilds it
    /// once, and the read after it is answered from the new build.
    fn a_change_rebuilds_the_section(change: impl FnOnce(&Sources)) {
        let src = Sources::new();
        let sections = Sections::new();
        let key = section_key();
        assert!(
            builds_on_read(&sections, &src, &key),
            "the first read builds"
        );
        assert!(
            !builds_on_read(&sections, &src, &key),
            "nothing changed: no build"
        );
        change(&src);
        assert!(
            builds_on_read(&sections, &src, &key),
            "the change rebuilds it"
        );
        assert!(
            !builds_on_read(&sections, &src, &key),
            "the next read is cached"
        );
    }

    #[test]
    fn a_section_is_built_once_while_its_sources_do_not_move() {
        let src = Sources::new();
        let sections = Sections::new();
        let key = section_key();
        for _ in 0..3 {
            sections.get(
                &key,
                || section_generation(&src.gens, &src.laws, &key),
                empty_section,
            );
        }
        assert_eq!(sections.builds(), 1);
        // Another session's section is built apart.
        let other = SectionKey {
            role: "reviewer".into(),
            ..section_key()
        };
        assert!(builds_on_read(&sections, &src, &other));
    }

    #[test]
    fn a_change_to_the_role_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::role("builder"));
        });
    }

    #[test]
    fn a_change_to_the_project_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::project("p1"));
        });
    }

    #[test]
    fn a_change_to_the_connectors_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::CONNECTORS.to_string());
        });
    }

    #[test]
    fn a_change_to_the_vault_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::VAULT.to_string());
        });
    }

    #[test]
    fn a_change_to_the_mcp_servers_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::MCP.to_string());
        });
    }

    #[test]
    fn a_change_to_the_settings_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::SETTINGS.to_string());
        });
    }

    #[test]
    fn a_change_to_the_engines_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::sources::ENGINES.to_string());
        });
    }

    #[test]
    fn a_change_to_a_skill_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| {
            src.gens.bump(&crate::skills::LIBRARY.to_string());
        });
    }

    #[test]
    fn a_change_to_the_laws_rebuilds_its_section() {
        a_change_rebuilds_the_section(|src| src.laws.set_strict(true));
    }

    #[test]
    fn a_change_to_what_a_section_is_not_built_from_leaves_it_cached() {
        let src = Sources::new();
        let sections = Sections::new();
        let key = section_key();
        assert!(builds_on_read(&sections, &src, &key));
        src.gens.bump(&crate::sources::role("reviewer"));
        src.gens.bump(&crate::sources::project("p2"));
        src.gens.bump(&crate::skills::skill_key("pdf-forms"));
        assert!(
            !builds_on_read(&sections, &src, &key),
            "another role, project or skill leaves the section cached"
        );
    }
}
