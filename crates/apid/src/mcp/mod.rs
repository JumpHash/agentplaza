//! MCP servers: outside tools the hub runs for agents (docs/design/mcp.md).
//!
//! The owner adds a server (a pinned package for stdio, or an https URL). Each tool it offers becomes a plaza
//! tool, named `mcp__<id>__<tool>` on the wire, for the agents whose role and node pass the server's `access`.
//! Every call is made by the hub: agents never hold a server's credentials. A stdio server runs under the OS
//! sandbox, with the network only through the hub's egress proxy (docs/design/mcp.md).
//!
//! Secrets resolve through the vault with binding `mcp:<id>`, on behalf of the calling agent (or the owner
//! for a test). A server whose secret is not in scope for its caller does not start.

pub mod catalog;
pub mod client;
pub mod config;
pub mod egress;
#[cfg(test)]
pub mod fixture;
pub mod http;
pub mod routes;
pub mod runtime;
pub mod runtimes;
pub mod stdio;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use apz_metrics::{EntityKind, SeriesKey};
use apz_proto::chat::ToolSpec;
use secrecy::ExposeSecret;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::agents::Agent;
use crate::deliverables::Upload;
use crate::state::App;
use client::ToolInfo;
use config::{McpServer, Risk};
use runtime::{Ctx, Runtime};

/// The prefix every MCP tool name carries on the wire.
pub const PREFIX: &str = "mcp__";
/// The longest tool name a model API accepts.
const MAX_NAME: usize = 64;

/// A secret's value. It has no Display or Serialize, and its Debug is redacted.
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(***)")
    }
}

/// Who a secret is read for: the agent that caused the read, or the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnBehalf {
    Agent { cid: u64 },
    Owner { node: Option<String> },
}

/// The vault as MCP reads it. `resolve` returns the entry bound to `mcp:<server>` that is in scope for the
/// caller's node; a secret not bound to the server is not found.
pub trait McpSecrets: Send + Sync {
    fn resolve(&self, name: &str, server: &str, behalf: &OnBehalf) -> Result<Secret, String>;
}

/// The hub's vault, read for one caller: the entries bound to `mcp:<server>`, in scope for the caller's node.
pub struct VaultSecrets<'a> {
    vault: &'a crate::vault::Store,
    nodes: Vec<crate::nodes::Node>,
}

impl<'a> VaultSecrets<'a> {
    pub fn new(app: &'a App) -> VaultSecrets<'a> {
        VaultSecrets {
            vault: &app.vault,
            nodes: app.nodes.list(),
        }
    }
}

impl McpSecrets for VaultSecrets<'_> {
    fn resolve(&self, name: &str, server: &str, behalf: &OnBehalf) -> Result<Secret, String> {
        let binding = format!("mcp:{server}");
        let consumer = match behalf {
            OnBehalf::Agent { cid } => crate::vault::Consumer::agent(&self.nodes, *cid, &binding),
            OnBehalf::Owner { node } => crate::vault::Consumer {
                binding,
                on_behalf: crate::vault::OnBehalf::Owner { node: node.clone() },
            },
        };
        self.vault
            .resolve(name, &consumer, apz_plaza::now_ms())
            .map(|value| Secret(value.expose_secret().to_string()))
            .map_err(|err| err.message())
    }
}

/// The context a call or a probe runs in: the secrets it reads, who for, and the node the instance is for.
pub fn ctx<'a>(app: &App, secrets: &'a dyn McpSecrets, behalf: OnBehalf, node: String) -> Ctx<'a> {
    Ctx {
        secrets,
        behalf,
        node,
        blocklist: app.settings.get().fetch_block.clone(),
        now: apz_plaza::now_ms(),
    }
}

/// The MCP state of the hub: the store of servers and the running instances.
pub struct Mcp {
    pub store: config::Store,
    pub runtime: Runtime,
}

impl Mcp {
    /// Counts the server store's changes in `gens` (the hub's generations).
    pub fn with_generations(
        mut self,
        gens: std::sync::Arc<apz_cache::Generations<String>>,
    ) -> Self {
        self.store = self.store.with_generations(gens);
        self
    }

    /// An empty MCP state: the store fills from the database at start. Stdio servers run under the OS sandbox, with
    /// their directories and the runtimes under `state_dir`, and reach the network through the egress proxy on
    /// loopback `port`.
    pub fn load(state_dir: &Path, port: u16) -> Mcp {
        Mcp {
            store: config::Store::default(),
            runtime: Runtime::new(
                state_dir.to_path_buf(),
                Arc::new(runtimes::HttpFetch),
                Arc::new(stdio::SandboxLauncher),
                Some(egress::Egress::new(port)),
            ),
        }
    }
}

/// A tool as the registry knows it: which server and tool it is, and its risk class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub server: String,
    pub tool: String,
    pub risk: &'static str,
}

/// Every offered MCP tool by its wire name. The laws and the call path read it.
static REGISTRY: RwLock<BTreeMap<String, Entry>> = RwLock::new(BTreeMap::new());

fn sanitize(original: &str) -> String {
    original
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `name` cut to leave room for `_` and six hex digits of the sha256 of `original`.
fn suffixed(name: &str, original: &str) -> String {
    let digest = Sha256::digest(original.as_bytes());
    let hex: String = digest.iter().take(3).map(|b| format!("{b:02x}")).collect();
    let keep = name.len().min(MAX_NAME - 7);
    format!("{}_{hex}", &name[..keep])
}

/// The wire names of a server's tools, in the order of `tools`: `mcp__<id>__<tool>` with every character
/// outside `[A-Za-z0-9_-]` turned into `_`. A name over 64 characters is cut and given the suffix; so is a
/// name an earlier tool of the same server already has.
pub fn wire_names(server: &str, tools: &[ToolInfo]) -> Vec<String> {
    let mut taken = BTreeSet::new();
    tools
        .iter()
        .map(|tool| {
            let original = format!("{PREFIX}{server}__{}", tool.name);
            let mut name = sanitize(&original);
            if name.len() > MAX_NAME || taken.contains(&name) {
                name = suffixed(&name, &original);
            }
            taken.insert(name.clone());
            name
        })
        .collect()
}

/// The registry entry for a wire name, if it is an offered MCP tool.
pub fn lookup(name: &str) -> Option<Entry> {
    REGISTRY
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(name)
        .cloned()
}

/// The risk class of an MCP tool's wire name. The laws read it (`laws::risk_class`, C-004a).
pub fn risk_of(name: &str) -> Option<&'static str> {
    lookup(name).map(|e| e.risk)
}

/// Rebuilds the registry entries of the servers given, from their tool listings. Other servers keep theirs.
pub fn publish_risks(
    servers: &BTreeMap<String, McpServer>,
    tools: &BTreeMap<String, Vec<ToolInfo>>,
) {
    let mut registry = REGISTRY.write().unwrap_or_else(|p| p.into_inner());
    registry.retain(|_, entry| !servers.contains_key(&entry.server));
    for (id, server) in servers {
        let listed = tools.get(id).cloned().unwrap_or_default();
        for (tool, name) in listed.iter().zip(wire_names(id, &listed)) {
            registry.insert(
                name,
                Entry {
                    server: id.clone(),
                    tool: tool.name.clone(),
                    risk: risk_for(server, tool).as_str(),
                },
            );
        }
    }
}

/// Drops the registry entries of a removed server.
pub fn forget_risks(server: &str) {
    REGISTRY
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|_, entry| entry.server != server);
}

/// The risk a tool's annotations give, before any owner override (destructive is irreversible, read-only
/// is read, anything else is write).
pub fn auto_risk(tool: &ToolInfo) -> Risk {
    if tool.annotations.destructive_hint == Some(true) {
        Risk::Irreversible
    } else if tool.annotations.read_only_hint == Some(true) {
        Risk::Read
    } else {
        Risk::Write
    }
}

/// A tool's effective risk: the owner's override, else the annotations' risk.
pub fn risk_for(server: &McpServer, tool: &ToolInfo) -> Risk {
    server
        .tools
        .risk
        .get(&tool.name)
        .copied()
        .unwrap_or_else(|| auto_risk(tool))
}

/// Whether a server is offered to `role` on `node`, working on `project`: enabled, and its access lists admit them.
/// A caller with no known node is refused, and one on no project only by a server open to every project.
pub fn allowed(server: &McpServer, role: &str, node: Option<&str>, project: Option<&str>) -> bool {
    server.enabled
        && server.access.roles.admits(role)
        && node.is_some_and(|n| server.access.nodes.admits(n))
        && match project {
            Some(project) => server.access.projects.admits(project),
            None => server.access.projects.all,
        }
}

/// The tool specs a session of `role` on `node`, working on `project`, is offered: each allowed tool of each server it may
/// use, that the laws permit. Irreversible tools are withheld by C-004a until an owner approval is threaded through calls.
pub fn specs_for(
    app: &App,
    role: &str,
    node: Option<&str>,
    project: Option<&str>,
) -> Vec<ToolSpec> {
    let laws = crate::laws::global();
    let mut specs = Vec::new();
    for server in app.mcp.store.list() {
        if !allowed(&server, role, node, project) {
            continue;
        }
        let tools = app.mcp.store.tools_of(&server.id);
        for (tool, name) in tools.iter().zip(wire_names(&server.id, &tools)) {
            if !server.tools.allow.admits(&tool.name) || !laws.permits(role, &name) {
                continue;
            }
            let note = match risk_for(&server, tool) {
                Risk::Irreversible => " [irreversible: needs the owner's approval]",
                _ => "",
            };
            specs.push(ToolSpec {
                name,
                description: format!("[MCP {}] {}{note}", server.name, tool.description),
                input_schema: tool.input_schema.clone(),
            });
        }
    }
    specs
}

/// Runs MCP tool `name` for `agent` in work `work`. The law check on the call (`mcp.call`, with the server,
/// tool and risk) already ran in the caller. Errors are tool errors: a refused call, a server error, or a
/// result the server marked `isError`. Each call that reaches the server is recorded in the transcript as
/// an `mcp_call` with its server, tool, time, outcome and output size.
pub fn call_for_agent(
    app: &App,
    agent: &Agent,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    let entry = lookup(name).ok_or_else(|| format!("{name} is not an MCP tool"))?;
    let server = app
        .mcp
        .store
        .get(&entry.server)
        .ok_or_else(|| format!("no MCP server `{}`", entry.server))?;
    let node = app
        .nodes
        .owner_of_cid(agent.cid)
        .map(|n| n.id)
        .ok_or("your node is not known")?;
    let project = crate::projects::project_of(app, agent.cid);
    if !allowed(&server, &agent.role, Some(&node), project.as_deref()) {
        return Err(format!(
            "{name} is not available to the {} role on this node and project",
            agent.role
        ));
    }
    if !server.tools.allow.admits(&entry.tool) {
        return Err(format!("{} does not allow {}", entry.server, entry.tool));
    }
    if !app
        .mcp
        .store
        .tools_of(&entry.server)
        .iter()
        .any(|t| t.name == entry.tool)
    {
        return Err(format!("{} does not offer {}", entry.server, entry.tool));
    }
    if !crate::laws::global().permits(&agent.role, name) {
        return Err(format!(
            "{name} is not available to the {} role",
            agent.role
        ));
    }
    let secrets = VaultSecrets::new(app);
    let ctx = ctx(app, &secrets, OnBehalf::Agent { cid: agent.cid }, node);
    let started = Instant::now();
    let result = app
        .mcp
        .runtime
        .call(&server, &ctx, &entry.tool, input.clone());
    let ms = started.elapsed().as_millis() as u64;
    let record = |ok: bool, output_bytes: usize| {
        app.transcripts.record(
            &app.bus,
            agent.cid,
            work,
            crate::transcript::Entry::McpCall {
                server: entry.server.clone(),
                tool: entry.tool.clone(),
                ms,
                ok,
                output_bytes,
            },
        );
    };
    let output = match result {
        Ok(output) => output,
        Err(why) => {
            record(false, why.len());
            observe(app, agent.cid, &entry.server, &entry.tool, ms as f64, false);
            return Err(why);
        }
    };
    if let Some(tools) = &output.tools {
        app.mcp.store.set_tools(&entry.server, tools.clone());
    }
    record(!output.is_error, output.text.len());
    observe(
        app,
        agent.cid,
        &entry.server,
        &entry.tool,
        ms as f64,
        !output.is_error,
    );
    let author = agent.label();
    let mut text = output.text;
    for media in &output.media {
        let upload = Upload {
            name: format!("{}-{}.{}", entry.server, entry.tool, extension(&media.mime)),
            title: format!("{} {}", entry.server, entry.tool),
            description: None,
            task: None,
            content_type: media.mime.clone(),
            author_cid: agent.cid,
            author: author.clone(),
            kind: "shared".into(),
            audience: "private".into(),
            allowed: vec![agent.cid],
        };
        let stored = app.deliverables.add(upload, &media.bytes)?;
        text.push_str(&format!("\nSaved as file {}. Fetch it into your VM with download from http://plaza.gateway/files/{}", stored.id, stored.id));
    }
    if output.is_error { Err(text) } else { Ok(text) }
}

/// Records one MCP call: the tool's span and `tool.*` series (as `mcp:<server>.<tool>`), and the
/// server's `ext.latency_ms`, with `ext.errors` when the call failed.
fn observe(app: &App, cid: u64, server: &str, tool: &str, ms: f64, ok: bool) {
    let registry = &app.metrics.registry;
    let entity = format!("MCP: {server}");
    registry
        .hist(SeriesKey::new(
            "ext.latency_ms",
            EntityKind::Service,
            &entity,
        ))
        .record(ms);
    if !ok {
        registry
            .counter(SeriesKey::new("ext.errors", EntityKind::Service, &entity))
            .inc();
    }
    crate::metrics::collect::tool_call(app, cid, &format!("mcp:{server}.{tool}"), ms, ok);
}

/// A file extension for the image and audio types MCP servers return.
fn extension(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => "bin",
    }
}

/// The law operation an MCP call is, for `plaza_operations`: `mcp.call` on the server, with the tool and its
/// risk class. None when `name` is not an offered MCP tool.
pub fn law_operation(name: &str) -> Option<(String, String, &'static str)> {
    lookup(name).map(|e| (e.server, e.tool, e.risk))
}

/// The background work of the MCP runtime: stops the instances that sat idle. Run every three seconds from
/// `state::App::start` (the store itself is saved by the saver, with the other components).
pub fn tick(app: &App) {
    app.mcp.runtime.reap_idle(apz_plaza::now_ms());
}
