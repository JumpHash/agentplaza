//! The runtime: one instance per (server, node). An instance starts on first use, stops after the server's
//! `idle_stop_min` without use, restarts with backoff (1 s, 2 s, 4 s … 60 s) and gives up after five
//! failures in ten minutes. Every output, error and log line is scrubbed of the secret values the
//! instance was started with.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_sandbox::{Network, Spec};
use serde::Serialize;
use serde_json::Value;

use super::client::{Client, Error, ServerInfo, ToolInfo};
use super::config::{McpServer, Package, Setting, TransportKind};
use super::egress::Egress;
use super::http::{HttpTransport, Policy, check_endpoint};
use super::loopback::{Machine, Need};
use super::runtimes::{Fetch, Runtimes};
use super::stdio::{self, Child, Launcher, Layout, StdioTransport, push_capped};
use super::vm::{self, VmHost};
use super::{McpSecrets, OnBehalf};
use crate::fleet::LOCAL;

/// Failures in this window count toward giving up.
pub const FAILURE_WINDOW_MS: u64 = 10 * 60_000;
/// Failures in the window that make the runtime give up.
pub const MAX_FAILURES: usize = 5;
/// The longest a restart waits.
pub const MAX_BACKOFF_MS: u64 = 60_000;
/// How long a handshake may take.
const HANDSHAKE: Duration = Duration::from_secs(30);
/// How long a stdio server's handshake may take: a PyPI package is installed by `uv tool run` on its first start.
const STDIO_HANDSHAKE: Duration = Duration::from_secs(120);
/// The marker an npm install leaves in the server's `pkg/`: the `name@version` it installed.
const MARK_INSTALLED: &str = ".apz-installed";
/// Stderr lines kept per instance.
const LOG_LINES: usize = 200;
/// The text that stands in for a secret.
pub const REDACTED: &str = "[redacted]";

/// Where an instance stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    Running,
    Error,
}

/// One stderr line, scrubbed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogLine {
    pub ts_ms: u64,
    pub stream: &'static str,
    pub line: String,
}

/// What a slot knows. Readable while a call runs.
#[derive(Default)]
struct State {
    status: Status,
    /// How the instance runs: in a microVM, or under the sandbox and why not.
    launcher: String,
    last_used_ms: u64,
    idle_ms: u64,
    last_error: Option<String>,
    failures: VecDeque<u64>,
    next_attempt_ms: u64,
    gave_up: bool,
    server_info: Option<ServerInfo>,
    secrets: Vec<String>,
    logs: VecDeque<LogLine>,
    /// The container the last start was refused for, until the owner exempts it.
    loopback: Option<Need>,
}

/// One instance: its client (held for the length of a call) and its state.
pub struct Slot {
    client: Mutex<Option<Client>>,
    state: Mutex<State>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Who asks, and when: what the secrets resolve for, and the clock.
pub struct Ctx<'a> {
    pub secrets: &'a dyn McpSecrets,
    pub behalf: OnBehalf,
    /// The node the instance runs for (`hub` for the hub's own work).
    pub node: String,
    /// The fetch blocklist, checked for http endpoints.
    pub blocklist: Vec<String>,
    pub now: u64,
}

/// The shaped reply to a tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub text: String,
    pub is_error: bool,
    /// Image and audio content, decoded.
    pub media: Vec<Media>,
    /// The fresh tool listing, when this call started the instance.
    pub tools: Option<Vec<ToolInfo>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A test of a server: what it answered.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub server_info: ServerInfo,
    pub tools: Vec<ToolInfo>,
    pub resources: Vec<Value>,
    pub prompts: Vec<Value>,
}

/// The status a server shows, over its instances.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub status: Status,
    /// How the server's instance runs ("vm", or "sandbox: <why>"); empty before it has started.
    pub launcher: String,
    pub last_used_ms: u64,
    pub last_error: Option<String>,
    pub server_info: Option<ServerInfo>,
}

/// Every instance, by server id and node.
pub struct Runtime {
    launcher: Arc<dyn Launcher>,
    slots: Mutex<BTreeMap<(String, String), Arc<Slot>>>,
    /// The egress proxy stdio servers reach the network through. None runs them with no network (tests).
    egress: Option<Arc<Egress>>,
    /// The microVM host of this hub's node. None runs every server under the sandbox.
    vm: Option<Arc<dyn VmHost>>,
    /// Where the servers' directories (`<state>/mcp/<id>/`) and the runtimes (`<state>/runtimes/`) are.
    state_dir: PathBuf,
    runtimes: Runtimes,
    /// The machine that exempts a sandboxed server's container from loopback (Windows). None elsewhere.
    loopback: Option<Arc<dyn Machine>>,
}

/// The backoff before restart number `failures` (1 s, 2 s, 4 s … capped at 60 s).
pub fn backoff_ms(failures: usize) -> u64 {
    if failures == 0 {
        return 0;
    }
    let shift = (failures - 1).min(16) as u32;
    (1000u64 << shift).min(MAX_BACKOFF_MS)
}

/// Whether an instance idle since `last_used_ms` should stop now.
pub fn idle_expired(last_used_ms: u64, idle_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_used_ms) >= idle_ms
}

/// The forms a secret takes in text: the value itself, its base64, and its URL-encoded form.
pub fn forms(secret: &str) -> Vec<String> {
    let mut out = vec![secret.to_string()];
    let b64 = crate::connectors::base64(secret.as_bytes());
    if b64.len() >= 4 {
        out.push(b64);
    }
    let encoded = crate::connectors::encode(secret);
    if encoded != secret {
        out.push(encoded);
    }
    out.retain(|f| !f.is_empty());
    out
}

/// Every secret form replaced by [`REDACTED`]. Longest forms go first, so a value inside a longer one is
/// still caught.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut all: Vec<String> = secrets.iter().flat_map(|s| forms(s)).collect();
    all.sort_by_key(|f| std::cmp::Reverse(f.len()));
    all.dedup();
    let mut out = text.to_string();
    for form in all {
        if out.contains(&form) {
            out = out.replace(&form, REDACTED);
        }
    }
    out
}

/// Cuts `text` to `cap` bytes at a character boundary, saying so.
fn cap_text(text: String, cap: usize) -> String {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated at {} KiB]", &text[..end], cap / 1024)
}

/// Decodes standard base64 (padding optional, whitespace ignored). None when the input is not base64.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for byte in text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
    {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Turns a `tools/call` result into text and media, scrubbed and capped.
pub fn shape(result: &Value, secrets: &[String], cap: usize) -> (String, bool, Vec<Media>) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts = Vec::new();
    let mut media = Vec::new();
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => parts.push(
                item.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            "resource" => match item.pointer("/resource/text").and_then(Value::as_str) {
                Some(text) => parts.push(text.to_string()),
                None => parts.push(format!(
                    "[resource {}]",
                    item.pointer("/resource/uri")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                )),
            },
            kind @ ("image" | "audio") => {
                let mime = item
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream")
                    .to_string();
                match item
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(decode_base64)
                {
                    Some(bytes) => {
                        parts.push(format!(
                            "[{kind} {mime}, {} bytes, saved to the file store]",
                            bytes.len()
                        ));
                        media.push(Media { mime, bytes });
                    }
                    None => parts.push(format!("[unreadable {kind}]")),
                }
            }
            other => parts.push(format!("[{other} content]")),
        }
    }
    let text = redact(&parts.join("\n"), secrets);
    (cap_text(text, cap), is_error, media)
}

impl Runtime {
    /// A runtime for stdio servers: they run under `launcher`, reach the network only through `egress` (when it is
    /// given), and provision their runtimes and binaries through `fetch`.
    pub fn new(
        state_dir: PathBuf,
        fetch: Arc<dyn Fetch>,
        launcher: Arc<dyn Launcher>,
        egress: Option<Arc<Egress>>,
    ) -> Runtime {
        Runtime {
            launcher,
            slots: Mutex::new(BTreeMap::new()),
            egress,
            vm: None,
            runtimes: Runtimes::new(&state_dir, fetch),
            state_dir,
            loopback: None,
        }
    }

    /// Runs the servers of this hub's node in microVMs that `vm` boots, when it can (see [`Runtime::microvm`]).
    pub fn with_vm(mut self, vm: Arc<dyn VmHost>) -> Runtime {
        self.vm = Some(vm);
        self
    }

    /// Checks the containers of sandboxed servers for the loopback exemption with `machine` (Windows only).
    pub fn with_loopback(mut self, machine: Option<Arc<dyn Machine>>) -> Runtime {
        self.loopback = machine;
        self
    }

    /// The machine that checks and grants the loopback exemption, if this hub has one.
    pub fn loopback(&self) -> Option<&dyn Machine> {
        self.loopback.as_deref()
    }

    /// The directory a server runs in, under its data directory.
    pub fn workdir(&self, server: &McpServer) -> PathBuf {
        workdir_of(&Layout::new(&self.state_dir, &server.id), server)
    }

    /// The container a sandboxed server was refused for, when its last start was refused for want of the loopback
    /// exemption. None once a start gets past that check, or when it was refused for any other reason.
    pub fn loopback_need(&self, server_id: &str) -> Option<Need> {
        self.slots_of(server_id)
            .iter()
            .find_map(|slot| lock(&slot.state).loopback.clone())
    }

    /// Sets the reason `server_id` last failed, on each of its instances.
    pub fn set_error(&self, server_id: &str, why: &str) {
        for slot in self.slots_of(server_id) {
            lock(&slot.state).last_error = Some(why.to_string());
        }
    }

    fn slots_of(&self, server_id: &str) -> Vec<Arc<Slot>> {
        lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
            .collect()
    }

    /// Removes a stdio server's data directory (its storage) and the storage its microVM keeps. Its installed packages
    /// and binaries stay.
    pub fn purge_data(&self, server_id: &str) -> Result<(), String> {
        if let Some(vm) = &self.vm {
            vm.purge(server_id)?;
        }
        let data = Layout::new(&self.state_dir, server_id).data;
        match std::fs::remove_dir_all(&data) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove {}: {e}", data.display())),
        }
    }

    /// The microVM host that runs a server for `node`, or why the sandbox runs it instead. Only this hub's own node
    /// has microVMs the hub can reach.
    fn microvm(&self, node: &str) -> Result<&Arc<dyn VmHost>, String> {
        if node != LOCAL {
            return Err(format!("node {node} is not this hub's node"));
        }
        let vm = self.vm.as_ref().ok_or("this hub has no microVM host")?;
        vm.available()?;
        Ok(vm)
    }

    fn slot(&self, server: &str, node: &str) -> Arc<Slot> {
        let mut slots = lock(&self.slots);
        Arc::clone(
            slots
                .entry((server.to_string(), node.to_string()))
                .or_insert_with(|| {
                    Arc::new(Slot {
                        client: Mutex::new(None),
                        state: Mutex::new(State::default()),
                    })
                }),
        )
    }

    /// Calls `tool` on `server` for `ctx.node`, starting the instance if needed.
    pub fn call(
        &self,
        server: &McpServer,
        ctx: &Ctx,
        tool: &str,
        args: Value,
    ) -> Result<Output, String> {
        let slot = self.slot(&server.id, &ctx.node);
        let timeout = Duration::from_secs(server.limits.timeout_s);
        let mut fresh = None;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut guard = lock(&slot.client);
            let started = self.ensure(server, &slot, &mut guard, ctx)?;
            if started.is_some() {
                fresh = started;
            }
            let secrets = lock(&slot.state).secrets.clone();
            lock(&slot.state).idle_ms = server.limits.idle_stop_min * 60_000;
            lock(&slot.state).last_used_ms = ctx.now;
            let client = guard.as_mut().ok_or("the server is not running")?;
            match client.call_tool(tool, args.clone(), timeout) {
                Ok(result) => {
                    let (text, is_error, media) = shape(
                        &result,
                        &secrets,
                        server.limits.max_output_kb as usize * 1024,
                    );
                    return Ok(Output {
                        text,
                        is_error,
                        media,
                        tools: fresh,
                    });
                }
                Err(Error::SessionLost) if attempt == 1 => {
                    // The HTTP session expired: start a new one and send the call again, once.
                    take_client(&mut guard);
                    continue;
                }
                Err(Error::Rpc { code, message }) => {
                    return Err(redact(&format!("server error {code}: {message}"), &secrets));
                }
                Err(Error::Timeout) => {
                    take_client(&mut guard);
                    let why = format!("timed out after {} s", server.limits.timeout_s);
                    let mut st = lock(&slot.state);
                    st.status = Status::Stopped;
                    st.last_error = Some(why.clone());
                    return Err(why);
                }
                Err(other) => {
                    take_client(&mut guard);
                    let why = redact(&other.to_string(), &secrets);
                    record_failure(&mut lock(&slot.state), ctx.now, &why);
                    return Err(format!("{} failed: {why}", server.id));
                }
            }
        }
    }

    /// Starts the instance (or finds it running) and lists its tools, resources and prompts. The probe
    /// does not keep the instance if the caller stops it afterwards.
    pub fn probe(&self, server: &McpServer, ctx: &Ctx) -> Result<Probe, String> {
        let slot = self.slot(&server.id, &ctx.node);
        let timeout = Duration::from_secs(server.limits.timeout_s);
        let mut guard = lock(&slot.client);
        let started = self.ensure(server, &slot, &mut guard, ctx)?;
        let secrets = lock(&slot.state).secrets.clone();
        let info = lock(&slot.state).server_info.clone().unwrap_or(ServerInfo {
            name: String::new(),
            version: String::new(),
            protocol: String::new(),
        });
        let client = guard.as_mut().ok_or("the server is not running")?;
        let tools = match started {
            Some(tools) => tools,
            None => client
                .list_tools(timeout)
                .map_err(|e| redact(&e.to_string(), &secrets))?,
        };
        let resources = client.list_resources(timeout).unwrap_or_default();
        let prompts = client.list_prompts(timeout).unwrap_or_default();
        Ok(Probe {
            server_info: info,
            tools,
            resources,
            prompts,
        })
    }

    /// Starts the instance if it is not running. Returns the tool listing when it did start.
    fn ensure(
        &self,
        server: &McpServer,
        slot: &Arc<Slot>,
        guard: &mut Option<Client>,
        ctx: &Ctx,
    ) -> Result<Option<Vec<ToolInfo>>, String> {
        // Only a refusal of this start asks the owner for the loopback exemption.
        lock(&slot.state).loopback = None;
        if let Some(client) = guard.as_mut() {
            if client.alive() {
                return Ok(None);
            }
            take_client(guard);
            record_failure(&mut lock(&slot.state), ctx.now, "the server exited");
        }
        {
            let st = lock(&slot.state);
            if st.gave_up {
                return Err(format!(
                    "{} gave up after {MAX_FAILURES} failures in ten minutes: {}. Start it again from the console",
                    server.id,
                    st.last_error.clone().unwrap_or_default()
                ));
            }
            if ctx.now < st.next_attempt_ms {
                let wait = st.next_attempt_ms.saturating_sub(ctx.now).div_ceil(1000);
                return Err(format!(
                    "{} restarts after a failure, in {wait} s: {}",
                    server.id,
                    st.last_error.clone().unwrap_or_default()
                ));
            }
        }
        // Secrets resolve before anything starts; a refused resolution is not a failure of the server.
        let data_dir = Layout::new(&self.state_dir, &server.id)
            .data
            .to_string_lossy()
            .into_owned();
        let resolved = match resolve(server, ctx, &data_dir) {
            Ok(resolved) => resolved,
            Err(why) => {
                record_failure(&mut lock(&slot.state), ctx.now, &why);
                return Err(format!("{} did not start: {why}", server.id));
            }
        };
        lock(&slot.state).secrets = resolved.secrets.clone();
        let transport: Box<dyn super::client::Transport> = match server.transport.kind {
            TransportKind::Stdio => match self.start_stdio(server, slot, ctx, &resolved) {
                Ok(transport) => Box::new(transport),
                Err(why) => {
                    record_failure(
                        &mut lock(&slot.state),
                        ctx.now,
                        &redact(&why, &resolved.secrets),
                    );
                    return Err(format!("{} did not start: {why}", server.id));
                }
            },
            TransportKind::Http => {
                let url = server.transport.url.clone().unwrap_or_default();
                if let Err(why) = check_endpoint(&url, server.allow_private, &ctx.blocklist) {
                    record_failure(&mut lock(&slot.state), ctx.now, &why);
                    return Err(format!("{}: {why}", server.id));
                }
                let policy = Policy {
                    allow_private: server.allow_private,
                    blocklist: ctx.blocklist.clone(),
                };
                Box::new(HttpTransport::new(
                    url,
                    resolved.headers,
                    resolved.secrets.clone(),
                    policy,
                ))
            }
        };
        let timeout = match server.transport.kind {
            TransportKind::Stdio => STDIO_HANDSHAKE,
            TransportKind::Http => HANDSHAKE,
        };
        let mut client = Client::new(transport);
        let handshake = client
            .initialize("agentplaza", env!("CARGO_PKG_VERSION"), timeout)
            .and_then(|info| {
                let tools = client.list_tools(HANDSHAKE)?;
                Ok((info, tools))
            });
        match handshake {
            Ok((info, tools)) => {
                let mut st = lock(&slot.state);
                st.status = Status::Running;
                st.server_info = Some(info);
                st.last_error = None;
                st.next_attempt_ms = 0;
                *guard = Some(client);
                Ok(Some(tools))
            }
            Err(err) => {
                client.close();
                let why = redact(&err.to_string(), &resolved.secrets);
                let last = lock(&slot.state).logs.back().map(|l| l.line.clone());
                let detail = match last {
                    Some(line) if !line.is_empty() => format!("{why} (stderr: {line})"),
                    _ => why,
                };
                record_failure(&mut lock(&slot.state), ctx.now, &detail);
                Err(format!("{} did not start: {detail}", server.id))
            }
        }
    }

    /// Provisions the server's runtime and package, attaches its proxy credential, and starts its process under the
    /// sandbox. An npm package is installed first, sandboxed the same way, unless its install marker is there.
    fn start_stdio(
        &self,
        server: &McpServer,
        slot: &Arc<Slot>,
        ctx: &Ctx,
        resolved: &Resolved,
    ) -> Result<StdioTransport, String> {
        let package = server
            .transport
            .package
            .clone()
            .ok_or("the server names no package")?;
        let layout = Layout::new(&self.state_dir, &server.id);
        layout.prepare(server.storage.volume)?;
        let workdir = workdir_of(&layout, server);
        std::fs::create_dir_all(&workdir)
            .map_err(|e| format!("cannot make {}: {e}", workdir.display()))?;
        let lease = match &self.egress {
            Some(egress) => {
                Some(egress.attach(&server.id, server.allow_private, &ctx.blocklist)?)
            }
            None => None,
        };
        let proxy = lease.as_ref().map(|l| l.proxy().to_string());
        let network = match &lease {
            Some(lease) => Network::LoopbackPort(lease.port()),
            None => Network::None,
        };
        if let (Network::LoopbackPort(port), Some(machine)) = (network, &self.loopback) {
            check_loopback(machine.as_ref(), slot, &server.id, &workdir, port)?;
        }
        let runtimes_root = self.state_dir.join("runtimes");
        let mut read_only = vec![layout.bin.clone(), layout.pkg.clone()];
        let (program, args, path_dirs, extra_env) = match &package {
            Package::Npm { name, version } => {
                let node = self.runtimes.node()?;
                read_only.push(runtimes_root.clone());
                if !npm_installed(&layout, name, version) {
                    let mut install_env = stdio::base_env(
                        &layout,
                        std::slice::from_ref(&node.bin_dir),
                        proxy.as_deref(),
                    );
                    install_env.extend(npm_env(&layout));
                    let mut writable = layout.writable();
                    writable.push(layout.pkg.clone());
                    let spec = Spec {
                        program: node.node.clone(),
                        args: stdio::npm_install_args(&node.npm_cli, &layout.pkg, name, version),
                        env: install_env,
                        workdir: layout.tmp.clone(),
                        read_only: vec![runtimes_root.clone()],
                        read_write: writable,
                        network,
                        limits: apz_sandbox::Limits::default(),
                    };
                    stdio::run_to_end(self.launcher.as_ref(), &spec, stdio::INSTALL_TIMEOUT)
                        .map_err(|why| format!("cannot install {name}@{version}: {why}"))?;
                    std::fs::write(layout.pkg.join(MARK_INSTALLED), format!("{name}@{version}"))
                        .map_err(|e| format!("cannot finish the install of {name}: {e}"))?;
                }
                let bin = stdio::npm_bin(&layout.pkg.join("node_modules").join(name), name)?;
                let mut args = vec![bin.to_string_lossy().into_owned()];
                args.extend(server.transport.args.iter().cloned());
                (
                    node.node.clone(),
                    args,
                    vec![node.bin_dir.clone()],
                    Vec::new(),
                )
            }
            Package::Pypi {
                name,
                version,
                entry,
            } => {
                let uv = self.runtimes.uv()?;
                read_only.push(runtimes_root.clone());
                let args =
                    stdio::uv_run_args(name, version, entry.as_deref(), &server.transport.args);
                (uv, args, Vec::new(), uv_env(&layout))
            }
            Package::Binary { url, sha256, entry } => {
                let file = self.runtimes.binary(&layout.bin, url, sha256, entry)?;
                (file, server.transport.args.clone(), Vec::new(), Vec::new())
            }
        };
        let mut env = stdio::base_env(&layout, &path_dirs, proxy.as_deref());
        env.extend(extra_env);
        env.extend(resolved.env.iter().cloned());
        let spec = Spec {
            program,
            args,
            env,
            workdir,
            read_only,
            read_write: layout.writable(),
            network,
            limits: apz_sandbox::Limits::default(),
        };
        let child: Box<dyn Child> = match (self.microvm(&ctx.node), &lease) {
            (Ok(vm), Some(lease)) => {
                let run = vm::run_id(&server.id);
                let request = vm::request(
                    &spec,
                    &layout.root,
                    &server.id,
                    &run,
                    server.storage.volume,
                    lease.port(),
                )?;
                lock(&slot.state).launcher = "vm".into();
                vm.launch(&request)?
            }
            (Ok(_), None) => {
                lock(&slot.state).launcher = "sandbox: the server has no egress proxy".into();
                Box::new(self.launcher.launch(&spec)?)
            }
            (Err(why), _) => {
                lock(&slot.state).launcher = format!("sandbox: {why}");
                Box::new(self.launcher.launch(&spec)?)
            }
        };
        let sink_slot = Arc::clone(slot);
        let on_stderr: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line: String| {
            let mut st = lock(&sink_slot.state);
            let line = redact(&line, &st.secrets.clone());
            push_capped(
                &mut st.logs,
                LogLine {
                    ts_ms: apz_plaza::now_ms(),
                    stream: "stderr",
                    line,
                },
                LOG_LINES,
            );
        });
        StdioTransport::new(child, on_stderr, lease)
    }

    /// Stops every instance of `server_id` (its child is killed; an http session is forgotten).
    pub fn stop(&self, server_id: &str) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
            .collect();
        for slot in slots {
            take_client(&mut lock(&slot.client));
            let mut st = lock(&slot.state);
            if st.status == Status::Running {
                st.status = Status::Stopped;
            }
        }
    }

    /// Clears the failures of `server_id`, so an owner's start can try again after giving up.
    pub fn reset(&self, server_id: &str) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
            .collect();
        for slot in slots {
            let mut st = lock(&slot.state);
            st.failures.clear();
            st.gave_up = false;
            st.next_attempt_ms = 0;
        }
    }

    /// Stops the instances idle longer than their server allows. An instance in use is left alone.
    pub fn reap_idle(&self, now_ms: u64) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        for slot in slots {
            let Ok(mut guard) = slot.client.try_lock() else {
                continue;
            };
            if guard.is_none() {
                continue;
            }
            let mut st = lock(&slot.state);
            if idle_expired(st.last_used_ms, st.idle_ms, now_ms) {
                *guard = None;
                st.status = Status::Stopped;
            }
        }
    }

    /// The status of `server_id` over its instances.
    pub fn summary(&self, server_id: &str) -> Summary {
        let mut out = Summary::default();
        let (mut running, mut failed) = (false, false);
        for slot in lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
        {
            let st = lock(&slot.state);
            running |= st.status == Status::Running;
            failed |= st.status == Status::Error || st.gave_up;
            out.last_used_ms = out.last_used_ms.max(st.last_used_ms);
            if out.last_error.is_none() {
                out.last_error = st.last_error.clone();
            }
            if out.launcher.is_empty() {
                out.launcher = st.launcher.clone();
            }
            if out.server_info.is_none() {
                out.server_info = st.server_info.clone();
            }
        }
        out.status = if running {
            Status::Running
        } else if failed {
            Status::Error
        } else {
            Status::Stopped
        };
        out
    }

    /// The last stderr lines of `server_id`, oldest first, at most `limit`.
    pub fn logs(&self, server_id: &str, limit: usize) -> Vec<LogLine> {
        let mut lines: Vec<LogLine> = Vec::new();
        for slot in lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
        {
            lines.extend(lock(&slot.state).logs.iter().cloned());
        }
        lines.sort_by_key(|l| l.ts_ms);
        let skip = lines.len().saturating_sub(limit);
        lines.split_off(skip)
    }
}

/// A sandboxed server reaches the proxy only through its container's loopback exemption, which Windows grants once
/// and only an administrator can grant. Until it is there, the start is refused and the container is kept as the
/// need, for the owner's approval.
fn check_loopback(
    machine: &dyn Machine,
    slot: &Slot,
    server: &str,
    workdir: &Path,
    port: u16,
) -> Result<(), String> {
    let sid = machine.container_sid(workdir)?;
    let exempt = machine.is_exempt(&sid)?;
    lock(&slot.state).loopback = (!exempt).then(|| Need {
        sid: sid.clone(),
        port,
    });
    if exempt {
        return Ok(());
    }
    Err(format!(
        "{server} cannot reach the egress proxy: its container {sid} is not exempt from loopback, and a Windows administrator must exempt it once"
    ))
}

/// The directory a server runs in, under its layout.
fn workdir_of(layout: &Layout, server: &McpServer) -> PathBuf {
    match server.transport.workdir.as_deref() {
        Some(dir) if !dir.is_empty() => layout.data.join(dir),
        _ => layout.data.clone(),
    }
}

/// Drops the client: its child is killed when its transport drops.
fn take_client(guard: &mut Option<Client>) {
    if let Some(mut client) = guard.take() {
        client.close();
    }
}

/// Counts a failure at `now`, and sets the backoff, or gives up at the limit.
fn record_failure(st: &mut State, now: u64, why: &str) {
    st.failures
        .retain(|t| now.saturating_sub(*t) < FAILURE_WINDOW_MS);
    st.failures.push_back(now);
    st.status = Status::Error;
    st.last_error = Some(why.to_string());
    if st.failures.len() >= MAX_FAILURES {
        st.gave_up = true;
    }
    st.next_attempt_ms = now + backoff_ms(st.failures.len());
}

/// Whether the server's npm package is installed at the version named (its marker is there).
fn npm_installed(layout: &Layout, name: &str, version: &str) -> bool {
    std::fs::read_to_string(layout.pkg.join(MARK_INSTALLED))
        .map(|text| text == format!("{name}@{version}"))
        .unwrap_or(false)
}

/// npm's environment for an install: its cache and user config inside the server, and no update checks.
fn npm_env(layout: &Layout) -> Vec<(String, String)> {
    vec![
        (
            "npm_config_cache".into(),
            layout.cache.join("npm").to_string_lossy().into_owned(),
        ),
        (
            "npm_config_userconfig".into(),
            layout.home.join(".npmrc").to_string_lossy().into_owned(),
        ),
        ("npm_config_update_notifier".into(), "false".into()),
        ("npm_config_fund".into(), "false".into()),
    ]
}

/// uv's environment: its cache, its tool environments and its managed Pythons, all inside the server.
fn uv_env(layout: &Layout) -> Vec<(String, String)> {
    vec![
        (
            "UV_CACHE_DIR".into(),
            layout.cache.join("uv").to_string_lossy().into_owned(),
        ),
        (
            "UV_TOOL_DIR".into(),
            layout.tools.to_string_lossy().into_owned(),
        ),
        (
            "UV_PYTHON_INSTALL_DIR".into(),
            layout.python.to_string_lossy().into_owned(),
        ),
        ("UV_NO_PROGRESS".into(), "1".into()),
    ]
}

/// The env and headers a server gets, with secrets resolved for `ctx`.
struct Resolved {
    env: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    /// The secret values (for scrubbing), never literals.
    secrets: Vec<String>,
}

fn resolve(server: &McpServer, ctx: &Ctx, data_dir: &str) -> Result<Resolved, String> {
    let mut out = Resolved {
        env: Vec::new(),
        headers: Vec::new(),
        secrets: Vec::new(),
    };
    for (name, setting) in &server.env {
        let value = value_of(setting, server, ctx, &mut out.secrets, data_dir)?;
        out.env.push((name.clone(), value));
    }
    for (name, setting) in &server.headers {
        let value = value_of(setting, server, ctx, &mut out.secrets, data_dir)?;
        out.headers.push((name.clone(), value));
    }
    Ok(out)
}

/// One setting's value: the literal (with `{data}` made the server's data directory), or the vault's value for the
/// server (with a header's `format` applied).
fn value_of(
    setting: &Setting,
    server: &McpServer,
    ctx: &Ctx,
    secrets: &mut Vec<String>,
    data_dir: &str,
) -> Result<String, String> {
    match setting {
        Setting::Value { value } => Ok(value.replace("{data}", data_dir)),
        Setting::Secret { secret, format } => {
            let raw = ctx.secrets.resolve(secret, &server.id, &ctx.behalf)?;
            secrets.push(raw.expose().to_string());
            Ok(match format {
                Some(format) => format.replace("{}", raw.expose()),
                None => raw.expose().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_one_second_and_stops_at_a_minute() {
        let steps: Vec<u64> = (1..=8).map(backoff_ms).collect();
        assert_eq!(
            steps,
            vec![1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000]
        );
    }

    #[test]
    fn an_instance_stops_after_its_idle_minutes() {
        assert!(!idle_expired(1_000, 600_000, 600_999));
        assert!(idle_expired(1_000, 600_000, 601_000));
    }

    #[test]
    fn redaction_catches_a_secret_in_every_form_and_inside_a_longer_one() {
        // The value and its base64 and url forms are all redacted.
        {
            let secret = "sk-SENTINEL/+&=value".to_string();
            let b64 = crate::connectors::base64(secret.as_bytes());
            let text = format!(
                "raw {secret}; b64 {b64}; url {}",
                crate::connectors::encode(&secret)
            );
            let clean = redact(&text, std::slice::from_ref(&secret));
            assert!(!clean.contains("SENTINEL"), "{clean}");
            assert_eq!(clean.matches(REDACTED).count(), 3);
        }
        // A secret inside a longer secret is still caught.
        {
            let secrets = vec!["abcdef".to_string(), "bcd".to_string()];
            assert!(!redact("xxabcdefyy", &secrets).contains("abc"));
        }
    }

    #[test]
    fn output_is_redacted_before_it_is_capped() {
        let secret = "SENTINEL-SECRET-1234".to_string();
        let result = serde_json::json!({"content": [{"type": "text", "text": format!("prefix {secret} tail")}], "isError": false});
        let (text, is_error, media) = shape(&result, std::slice::from_ref(&secret), 1024);
        assert!(!text.contains("SENTINEL") && text.contains(REDACTED));
        assert!(!is_error && media.is_empty());
    }

    #[test]
    fn text_is_joined_capped_and_marked() {
        let big = "x".repeat(3000);
        let result = serde_json::json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": big}], "isError": true});
        let (text, is_error, _) = shape(&result, &[], 1024);
        assert!(is_error);
        assert!(text.starts_with("a\nxxx"));
        assert!(text.ends_with("[truncated at 1 KiB]"));
    }

    #[test]
    fn images_are_decoded_for_the_file_store() {
        let result = serde_json::json!({"content": [{"type": "image", "mimeType": "image/png", "data": "iVBORw0K"}]});
        let (text, _, media) = shape(&result, &[], 1024);
        assert_eq!(media.len(), 1);
        assert_eq!(&media[0].bytes[..4], &[0x89, b'P', b'N', b'G']);
        assert!(text.contains("image/png"));
    }

    #[test]
    fn the_failure_window_gives_up_at_five_and_forgets_old_failures() {
        // Five failures inside the window give up; the fifth is the one that trips it.
        {
            let mut st = State::default();
            for i in 0..4 {
                record_failure(&mut st, i * 1000, "boom");
            }
            assert!(!st.gave_up);
            record_failure(&mut st, 5000, "boom");
            assert!(st.gave_up);
        }
        // Failures older than the window fall out of it.
        {
            let mut st = State::default();
            for i in 0..4 {
                record_failure(&mut st, i, "boom");
            }
            record_failure(&mut st, FAILURE_WINDOW_MS + 10, "boom");
            assert!(!st.gave_up);
            assert_eq!(st.failures.len(), 1);
        }
    }
}
