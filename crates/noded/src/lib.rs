// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error.
#![forbid(unsafe_code)]

//! `noded` - the AgentPlaza node daemon v0: run lifecycle and VM control over
//! a localhost HTTP/1.1 JSON API.
//!
//! ```text
//! noded [--port 8899] [--kernel /boot/vmlinuz-linux]
//!       [--initrd target/guest/initrd.x86_64.gz]
//! ```
//!
//! The daemon binds `127.0.0.1` and serves, as JSON:
//!
//! * `GET    /api/v0/runs`             - every run, without serial captures
//! * `POST   /api/v0/runs`             - boot a run's VMs
//! * `GET    /api/v0/runs/{id}`        - one run, with every VM's serial
//! * `POST   /api/v0/runs/{id}/input`  - write to one VM's serial console
//! * `DELETE /api/v0/runs/{id}`        - kill a run's VMs
//! * `GET    /api/v0/tasks`            - every task, oldest first
//! * `POST   /api/v0/tasks`            - create a task, `open` (its budget minted into escrow)
//! * `GET    /api/v0/tasks/{id}`       - one task
//! * `POST   /api/v0/tasks/{id}/claim` - claim an open task as a live agent
//! * `POST   /api/v0/tasks/{id}/finish`- end a claimed task (`done`/`failed`), refunding its escrow
//! * `POST   /api/v0/tasks/{id}/artifact` - read a file the task's agent wrote
//! * `POST   /api/v0/tasks/{id}/bid`   - bid on a roled open task's call
//! * `POST   /api/v0/tasks/{id}/award` - award the call to a bidder (binding
//!   the task through the claim)
//! * `GET    /api/v0/agora`            - the calls, with their bids and awards
//! * `DELETE /api/v0/tasks/{id}`       - withdraw a task before it ends (refunding its escrow)
//! * `GET    /api/v0/treasury`         - the accounts and their derived balances, and totals
//! * `GET    /api/v0/treasury/journal` - the double-entry journal, oldest first (bounded)
//! * `POST   /api/v0/agents/{cid}/tell`- drive one tool call on a connected agent
//! * `GET    /api/v0/hub`              - the hub's `/status`, proxied verbatim
//! * `GET    /api/v0/hub/healthz`      - the hub's `/healthz`, proxied verbatim
//! * `GET    /` and `GET /<file>`      - the console's static files
//!
//! Run states and transitions are the schema's (`core.run_transitions`,
//! `docs/orchestrator-draft/schema.sql`), restricted to the states noded v0
//! implements: see [`registry`]. The task ledger is `core.tasks` restricted
//! the same way, and only ever accepts a claim from an agent the register
//! shows connected right now: see [`tasks`]. A task's budget is real money in
//! the treasury's journal, minted into the task's escrow on create and
//! refunded when the task ends: see [`treasury`].

pub mod agora;
pub mod events;
pub mod fleet;
pub mod http;
pub mod hub;
pub mod registry;
pub mod static_files;
pub mod tasks;
pub mod treasury;

use std::borrow::Cow;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use vmm::spawner::{MAX_GUEST_CID, MIN_GUEST_CID};

use crate::agora::Agora;
use crate::http::Response;
use crate::hub::{HubAdmin, HubStatus, TellOutcome};
use crate::registry::{ConnectedAgent, CreateRun, Detail, Registry, VmImages};
use crate::tasks::{ClaimError, ClaimRequest, CreateTask, FinishRequest, TaskLedger, TasksView};
use crate::treasury::Treasury;

/// The port the daemon listens on unless `--port` says otherwise.
pub const DEFAULT_PORT: u16 = 8899;

/// The guest kernel unless `--kernel` says otherwise.
pub const DEFAULT_KERNEL: &str = "/boot/vmlinuz-linux";

/// The guest initrd unless `--initrd` says otherwise, relative to the working
/// directory.
pub const DEFAULT_INITRD: &str = "target/guest/initrd.x86_64.gz";

/// The web app's BUILT files (the static build output under apps/web — what a
/// static host like S3 would serve), relative to the working directory.
pub const DEFAULT_CONSOLE_DIR: &str = "apps/web/dist";

/// Where the per-VM vsock Unix sockets live unless `--uds-dir` says otherwise;
/// VM `i`'s muxer binds `{DEFAULT_UDS_DIR}/vm{cid}`.
pub const DEFAULT_UDS_DIR: &str = "/tmp/plaza-vsock";

/// The first guest CID of the initial role fleet unless `--agents-cid-base`
/// says otherwise; role `i` (alphabetical) gets `DEFAULT_AGENTS_CID_BASE + i`.
pub const DEFAULT_AGENTS_CID_BASE: u64 = 100;

/// Guest memory per agent VM unless `--agents-memory-mib` says otherwise: the
/// validated per-VM scale figure the plaza runs its agents at.
pub const DEFAULT_AGENTS_MEMORY_MIB: u32 = 128;

/// vCPUs per agent VM unless `--agents-vcpus` says otherwise.
pub const DEFAULT_AGENTS_VCPUS: u32 = 1;

/// How long a client has to send its request and to take its response.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the daemon listens and what it boots.
#[derive(Debug, Clone)]
pub struct Config {
    /// The port to bind on `127.0.0.1`; `0` takes an ephemeral port.
    pub port: u16,
    /// The guest kernel image every run boots.
    pub kernel: PathBuf,
    /// The guest initrd every run boots.
    pub initrd: PathBuf,
    /// Where the hub's status surface is, when `--hub-status` named one. `Some`
    /// serves `GET /api/v0/hub` and `/api/v0/hub/healthz` as a proxy of it;
    /// `None` (`--hub-status off`) answers those endpoints with a 503, because
    /// a daemon not told where the hub is cannot know its state and must not
    /// pretend the registry is empty.
    pub hub_status: Option<HubStatus>,
    /// Where the hub's admin surface is, when `--hub-admin` named one. `Some`
    /// serves `POST /api/v0/tasks/{id}/artifact`, which drives the task's
    /// agent's `read_file` through it; `None` (`--hub-admin off`) answers that
    /// endpoint with a 503, because a daemon not told where the admin surface
    /// is cannot ask any agent to read anything.
    pub hub_admin: Option<HubAdmin>,
    /// The directory `GET /` and `GET <file>` serve.
    pub console_dir: PathBuf,
    /// The directory holding the per-VM vsock Unix sockets; VM `i`'s muxer
    /// binds `{uds_dir}/vm{cid}`. With `workspace`, it also holds each run's
    /// directory of per-VM image copies.
    pub uds_dir: PathBuf,
    /// The base image of every run's writable workspace volume. `None` boots
    /// the guests with no block device; `Some` gives every run one copy of the
    /// image per VM, in the run's own directory under `uds_dir` — a writable
    /// volume is never shared between VMs.
    pub workspace: Option<PathBuf>,
    /// The roles manifest of the initial agent fleet, when the daemon was asked
    /// to boot one (`--agents`). `None` boots no fleet: the daemon serves the
    /// API exactly as it did before agents existed.
    pub agents: Option<PathBuf>,
    /// The first guest CID of the fleet; role `i` (alphabetical) gets
    /// `agents_cid_base + i`.
    pub agents_cid_base: u64,
    /// Guest memory per agent VM, in MiB.
    pub agents_memory_mib: u32,
    /// vCPUs per agent VM.
    pub agents_vcpus: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            kernel: PathBuf::from(DEFAULT_KERNEL),
            initrd: PathBuf::from(DEFAULT_INITRD),
            hub_status: Some(HubStatus::default()),
            hub_admin: Some(HubAdmin::default()),
            console_dir: PathBuf::from(DEFAULT_CONSOLE_DIR),
            uds_dir: PathBuf::from(DEFAULT_UDS_DIR),
            workspace: None,
            agents: None,
            agents_cid_base: DEFAULT_AGENTS_CID_BASE,
            agents_memory_mib: DEFAULT_AGENTS_MEMORY_MIB,
            agents_vcpus: DEFAULT_AGENTS_VCPUS,
        }
    }
}

/// The daemon's state: its runs, its task ledger, the treasury its tasks'
/// budgets are escrowed in, the agora its calls' bids and awards live in, the
/// console directory it serves, the hub's status surface it proxies, and when
/// it started (the machine view's uptime).
pub struct Node {
    registry: Registry,
    tasks: TaskLedger,
    treasury: Arc<Treasury>,
    agora: Arc<Agora>,
    console_dir: PathBuf,
    hub_status: Option<HubStatus>,
    hub_admin: Option<HubAdmin>,
    started_at_ms: u64,
}

/// The node's machine, self-reported (`core.machines`'s fields, plus the
/// vsock transport mode this daemon runs).
#[derive(Debug, Serialize)]
pub struct MachineView {
    /// The host's hostname.
    pub name: String,
    /// The host architecture (`x86_64`/`aarch64`).
    pub arch: &'static str,
    /// The VMM backend (`kvm`/`hvf`).
    pub backend: &'static str,
    /// The interrupt controller in use.
    pub irqchip: &'static str,
    /// The machine class per the schema (`server`/`apple`).
    pub class: &'static str,
    /// The host's CPU count.
    pub cpus: u32,
    /// The host's total memory, in MiB.
    pub memory_mib: u64,
    /// The vsock transport: per-VM UDS paths behind a userspace muxer.
    pub vsock_mode: &'static str,
    /// When the daemon started, in milliseconds since the Unix epoch.
    pub started_at_ms: u64,
}

impl Node {
    /// A node with no runs and no tasks that serves `config.console_dir`,
    /// boots every run from `config`'s images, and publishes every state
    /// transition, serial delta and task transition to its event bus.
    pub fn new(config: Config) -> Self {
        let events = events::EventBus::new();
        let tasks = TaskLedger::new(Arc::clone(&events));
        let treasury = Arc::clone(tasks.treasury());
        // The agora publishes its bids and awards to the same bus the ledger
        // and the registry do, so one events stream carries the whole plaza.
        let agora = Arc::new(Agora::new(Arc::clone(&events)));
        Self {
            registry: Registry::new(
                VmImages {
                    kernel: config.kernel,
                    initrd: config.initrd,
                    workspace: config.workspace,
                },
                config.uds_dir,
                events,
            ),
            tasks,
            treasury,
            agora,
            console_dir: config.console_dir,
            hub_status: config.hub_status,
            hub_admin: config.hub_admin,
            started_at_ms: epoch_ms(),
        }
    }

    /// The node as a machine (`GET /api/v0/machine`).
    pub fn machine(&self) -> MachineView {
        MachineView {
            name: hostname(),
            arch: machine_arch(),
            backend: machine_backend(),
            irqchip: machine_irqchip(),
            class: machine_class(),
            cpus: host_cpus(),
            memory_mib: host_memory_mib(),
            vsock_mode: "uds-muxer",
            started_at_ms: self.started_at_ms,
        }
    }

    /// The node's runs.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The node's task ledger.
    pub fn tasks(&self) -> &TaskLedger {
        &self.tasks
    }

    /// The node's treasury: the accounts and the journal its tasks' budgets
    /// are escrowed in.
    pub fn treasury(&self) -> &Treasury {
        &self.treasury
    }

    /// The node's agora: the calls its roled tasks offer, the bids on them,
    /// and the awards that closed them.
    pub fn agora(&self) -> &Agora {
        &self.agora
    }

    /// The node's event bus (`GET /api/v0/events` streams it).
    pub fn events(&self) -> &events::EventBus {
        self.registry.events_bus()
    }
}

impl Node {
    /// The directory the console's files are served from.
    pub fn console_dir(&self) -> &Path {
        &self.console_dir
    }

    /// Where the hub's status surface is, or `None` when the daemon was not
    /// told (`--hub-status off`).
    pub fn hub_status(&self) -> Option<&HubStatus> {
        self.hub_status.as_ref()
    }

    /// Where the hub's admin surface is, or `None` when the daemon was not
    /// told (`--hub-admin off`).
    pub fn hub_admin(&self) -> Option<&HubAdmin> {
        self.hub_admin.as_ref()
    }
}

/// The current time in milliseconds since the Unix epoch.
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// The host's hostname (`/proc/sys/kernel/hostname`; `unknown` off Linux or
/// on a read error).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// The host's architecture, per the schema's machine model.
fn machine_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "aarch64"
    }
}

/// The VMM backend this daemon boots VMs with.
fn machine_backend() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "hvf"
    } else {
        "kvm"
    }
}

/// The interrupt controller the VMM sets up (`vgicv2` on Pi 4/5 hosts is the
/// schema's example; the aarch64 port auto-selects GICv3 with a GICv2
/// fallback).
fn machine_irqchip() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "ioapic"
    } else {
        "vgic (v3, v2 fallback)"
    }
}

/// The machine class per the schema (`server`/`apple`).
fn machine_class() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "apple"
    } else {
        "server"
    }
}

/// The host's CPU count.
fn host_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|cpus| u32::from(u16::try_from(cpus.get()).unwrap_or(u16::MAX)))
        .unwrap_or(1)
}

/// The host's total memory, in MiB (`MemTotal` from `/proc/meminfo`).
fn host_memory_mib() -> u64 {
    let Ok(info) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    for line in info.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .unwrap_or(0);
            return kb / 1024;
        }
    }
    0
}


/// A bound, serving node daemon: one accept thread, one thread per connection.
pub struct Server {
    port: u16,
    shutdown: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    /// The daemon's state, also held by the accept thread. The daemon's
    /// startup (the initial role fleet) reaches the registry through this.
    node: Arc<Node>,
}

impl Server {
    /// Binds `config.port` on localhost and starts serving.
    pub fn start(config: Config) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", config.port))?;
        let port = listener.local_addr()?.port();
        let node = Arc::new(Node::new(config));
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let serving = Arc::clone(&node);
        let accept = thread::spawn(move || accept_loop(&listener, &serving, &flag));
        Ok(Self {
            port,
            shutdown,
            accept: Some(accept),
            node,
        })
    }

    /// The daemon's state (`registry`, event bus), for the startup that has to
    /// reach it before the daemon is left to serve.
    pub fn node(&self) -> &Arc<Node> {
        &self.node
    }

    /// The port the daemon is listening on: `config.port`, or the ephemeral
    /// port the kernel chose when that was `0`.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Serves until the listener fails.
    pub fn wait(mut self) -> std::io::Result<()> {
        match self.accept.take() {
            Some(accept) => accept
                .join()
                .map_err(|_| std::io::Error::other("the accept thread panicked")),
            None => Ok(()),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if self.accept.is_some() {
            // Wake the accept loop out of `accept(2)`: it checks `shutdown`
            // after every connection it accepts.
            let _ = TcpStream::connect(("127.0.0.1", self.port));
            if let Some(accept) = self.accept.take() {
                let _ = accept.join();
            }
        }
    }
}

/// Binds `config.port` and serves until the listener fails.
pub fn serve(config: Config) -> std::io::Result<()> {
    Server::start(config)?.wait()
}

/// Accepts connections until `shutdown` is set or the listener fails, and
/// hands each one to its own thread.
fn accept_loop(listener: &TcpListener, node: &Arc<Node>, shutdown: &AtomicBool) {
    for stream in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        match stream {
            Ok(stream) => {
                let node = Arc::clone(node);
                thread::spawn(move || handle_connection(stream, &node));
            }
            Err(err) => {
                eprintln!("noded: accept failed: {err}");
                return;
            }
        }
    }
}

/// Whether a `Host` header names loopback: `127.0.0.1` or `localhost`
/// (case-insensitively), with any `:port` suffix. Anything else — a rebinding
/// hostname, a LAN address, a bracketed IPv6 form — is not this daemon's name.
fn host_names_loopback(host: &str) -> bool {
    let name = host.split(':').next().unwrap_or(host);
    name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost")
}

/// Serves one connection: read one request, answer it, close. The events
/// endpoint is the one exception — its answer is a stream that ends when the
/// connection does, not a response.
fn handle_connection(stream: TcpStream, node: &Node) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(CONNECTION_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECTION_TIMEOUT));
    let mut stream = BufReader::new(stream);
    let response = match http::read_request(&mut stream) {
        Ok(request) => {
            // The loopback guard: a page served from any hostname that
            // resolves to 127.0.0.1 (DNS rebinding) can post a simple request
            // here and the browser executes it, even though the answer is
            // unreadable. A Host that names anything but loopback is refused;
            // a request with no Host at all (a raw probe, not a browser) is
            // served as before.
            if let Some(host) = request.host()
                && !host_names_loopback(host)
            {
                Response::error(403, "the Host header must name loopback")
            } else if request.method() == "GET" && request.path() == "/api/v0/events" {
                let receiver = node.events().subscribe();
                if let Err(err) = events::stream(&receiver, stream.get_mut()) {
                    eprintln!("noded: an events stream ended: {err}");
                }
                return;
            } else {
                route(&request, node)
            }
        }
        Err(err) => Response::error(400, err.message()),
    };
    if let Err(err) = response.write_to(stream.get_mut()) {
        eprintln!("noded: could not answer a request: {err}");
    }
}

/// The `POST /api/v0/runs/{id}/input` body.
#[derive(Debug, Deserialize)]
struct InputRequest {
    /// The line (or bytes) to write to the VM's console.
    data: String,
    /// The index of the VM to write to.
    vm: u32,
}

/// Routes one parsed request to its endpoint.
fn route(request: &http::Request, node: &Node) -> Response {
    let path = request.path();
    match path {
        "/api/v0/machine" => match request.method() {
            "GET" => json_response(200, &node.machine()),
            _ => method_not_allowed("GET"),
        },
        "/api/v0/hub" => match request.method() {
            "GET" => proxy_hub(node, "/status"),
            _ => method_not_allowed("GET"),
        },
        "/api/v0/hub/healthz" => match request.method() {
            "GET" => proxy_hub(node, "/healthz"),
            _ => method_not_allowed("GET"),
        },
        "/api/v0/runs" => match request.method() {
            "GET" => list_runs(node),
            "POST" => create_run(node, request.body()),
            _ => method_not_allowed("GET, POST"),
        },
        "/api/v0/tasks" => match request.method() {
            "GET" => list_tasks(node),
            "POST" => create_task(node, request.body()),
            _ => method_not_allowed("GET, POST"),
        },
        "/api/v0/treasury" => match request.method() {
            "GET" => treasury_view(node),
            _ => method_not_allowed("GET"),
        },
        "/api/v0/agora" => match request.method() {
            "GET" => agora_view(node),
            _ => method_not_allowed("GET"),
        },
        "/" => match request.method() {
            "GET" => static_files::serve(node.console_dir(), static_files::INDEX),
            _ => method_not_allowed("GET"),
        },
        _ => match path.strip_prefix("/api/v0/runs/") {
            Some(rest) => run_endpoint(rest, request, node),
            None => match path.strip_prefix("/api/v0/tasks/") {
                Some(rest) => task_endpoint(rest, request, node),
                None => match path.strip_prefix("/api/v0/agents/") {
                    Some(rest) => agent_endpoint(rest, request, node),
                    None => match path.strip_prefix("/api/v0/treasury/") {
                        Some(rest) => treasury_endpoint(rest, request, node),
                        None => match request.method() {
                            "GET" => static_files::serve(
                                node.console_dir(),
                                path.strip_prefix('/').unwrap_or(path),
                            ),
                            _ => method_not_allowed("GET"),
                        },
                    },
                },
            },
        },
    }
}

/// Routes a request under `/api/v0/treasury/`.
fn treasury_endpoint(rest: &str, request: &http::Request, node: &Node) -> Response {
    match rest {
        "journal" => match request.method() {
            "GET" => treasury_journal(node),
            _ => method_not_allowed("GET"),
        },
        _ => no_such_endpoint(),
    }
}

/// Routes a request under `/api/v0/tasks/`.
fn task_endpoint(rest: &str, request: &http::Request, node: &Node) -> Response {
    if let Some(id) = rest.strip_suffix("/claim") {
        if request.method() != "POST" {
            return method_not_allowed("POST");
        }
        return claim_task(node, id, request.body());
    }
    if let Some(id) = rest.strip_suffix("/finish") {
        if request.method() != "POST" {
            return method_not_allowed("POST");
        }
        return finish_task(node, id, request.body());
    }
    if let Some(id) = rest.strip_suffix("/bid") {
        if request.method() != "POST" {
            return method_not_allowed("POST");
        }
        return bid_task(node, id, request.body());
    }
    if let Some(id) = rest.strip_suffix("/award") {
        if request.method() != "POST" {
            return method_not_allowed("POST");
        }
        return award_task(node, id, request.body());
    }
    if let Some(id) = rest.strip_suffix("/artifact") {
        // The read-through is a POST, not a GET, and deliberately so: it
        // drives the task's agent - a tool call with a real cost, a real
        // refusal mode and a real effect on the agent's turn - so a request
        // that can be replayed by a prefetch, a retry or a crawler must not be
        // advertised as safe to repeat. The console posts it.
        if request.method() != "POST" {
            return method_not_allowed("POST");
        }
        return read_artifact(node, id, request.body());
    }
    if rest.is_empty() || rest.contains('/') {
        return no_such_endpoint();
    }
    match request.method() {
        "GET" => get_task(node, rest),
        "DELETE" => withdraw_task(node, rest),
        _ => method_not_allowed("GET, DELETE"),
    }
}

/// Routes a request under `/api/v0/runs/`.
fn run_endpoint(rest: &str, request: &http::Request, node: &Node) -> Response {
    if let Some(id) = rest.strip_suffix("/input") {
        if request.method() != "POST" {
            return method_not_allowed("POST");
        }
        return send_input(node, id, request.body());
    }
    if rest.is_empty() || rest.contains('/') {
        return no_such_endpoint();
    }
    match request.method() {
        "GET" => get_run(node, rest),
        "DELETE" => delete_run(node, rest),
        _ => method_not_allowed("GET, DELETE"),
    }
}

/// Routes a request under `/api/v0/agents/`.
fn agent_endpoint(rest: &str, request: &http::Request, node: &Node) -> Response {
    let Some(cid) = rest.strip_suffix("/tell") else {
        return no_such_endpoint();
    };
    if cid.is_empty() || cid.contains('/') {
        return no_such_endpoint();
    }
    // Driving an agent is a POST, not a GET, and deliberately so - for the same
    // reason `POST /api/v0/tasks/{id}/artifact` is: it runs a real tool in the
    // agent's session (a call with a cost, a refusal mode and an effect on the
    // agent's turn), so a request that can be replayed by a prefetch, a retry
    // or a crawler must not be advertised as safe to repeat. The console posts
    // it.
    if request.method() != "POST" {
        return method_not_allowed("POST");
    }
    tell_agent(node, cid, request.body())
}

/// `GET /api/v0/hub` and `GET /api/v0/hub/healthz`: the hub's status surface,
/// proxied so the console can read it from this daemon's own origin (its CSP's
/// `connect-src 'self'` blocks the cross-origin fetch the hub would need).
///
/// The hub's answer is passed through as it is - the body byte for byte and
/// the hub's own status - so the console parses one shape. The failure modes
/// stay apart: a hub that is unreachable or unreadable is a 502 naming why,
/// and a daemon that was never told where the hub is a 503. Both are JSON
/// errors, never an empty registry dressed up as an answer.
fn proxy_hub(node: &Node, path: &str) -> Response {
    let Some(hub) = node.hub_status() else {
        return Response::error(503, "hub status is not configured");
    };
    match hub::get(hub, path) {
        Ok(answer) => {
            let (status, content_type, body) = answer.into_parts();
            Response::bytes(status, Cow::Owned(content_type), body)
        }
        Err(reason) => Response::error(502, &format!("hub unreachable: {reason}")),
    }
}

/// `GET /api/v0/runs`: every run, without serial captures.
fn list_runs(node: &Node) -> Response {
    let runs = node.registry().list();
    let view = registry::RunsView {
        runs: runs.iter().map(|run| run.view(Detail::Summary)).collect(),
    };
    json_response(200, &view)
}

/// `POST /api/v0/runs`: boots a run's VMs.
///
/// The run is created before its VMs are booted and is `running` by the time
/// this answers (or `failed`, when a VM could not be booted - the run exists
/// either way, which is why this is a 201 and not an error).
fn create_run(node: &Node, body: &[u8]) -> Response {
    let request: CreateRun = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid create request: {err}")),
    };
    match node.registry().create_run(&request) {
        Ok(run) => json_response(201, &run.view(Detail::Summary)),
        Err(reason) => Response::error(400, &reason),
    }
}

/// `GET /api/v0/runs/{id}`: one run, with every VM's serial capture.
fn get_run(node: &Node, id: &str) -> Response {
    match node.registry().find(id) {
        Some(run) => json_response(200, &run.view(Detail::Full)),
        None => Response::error(404, &format!("no run with id {id}")),
    }
}

/// `POST /api/v0/runs/{id}/input`: writes to one VM's serial console.
///
/// The data is newline-terminated when the caller did not terminate it, so a
/// bare command line reaches the guest as a line.
fn send_input(node: &Node, id: &str, body: &[u8]) -> Response {
    let Some(run) = node.registry().find(id) else {
        return Response::error(404, &format!("no run with id {id}"));
    };
    let request: InputRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid input request: {err}")),
    };
    let mut data = request.data.into_bytes();
    if !data.ends_with(b"\n") {
        data.push(b'\n');
    }
    match run.send_input(request.vm, &data) {
        Ok(()) => json_response(200, &run.view(Detail::Full)),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `DELETE /api/v0/runs/{id}`: kills a run's VMs.
fn delete_run(node: &Node, id: &str) -> Response {
    match node.registry().find(id) {
        Some(run) => {
            run.kill();
            json_response(200, &run.view(Detail::Full))
        }
        None => Response::error(404, &format!("no run with id {id}")),
    }
}

/// `GET /api/v0/tasks`: every task, oldest first.
fn list_tasks(node: &Node) -> Response {
    let tasks = node.tasks().list();
    let view = TasksView {
        tasks: tasks.iter().map(|task| task.view()).collect(),
    };
    json_response(200, &view)
}

/// `GET /api/v0/treasury`: the accounts with their balances, a totals line,
/// and v0's honest note on real spend.
///
/// Balances are derived from the journal's postings, never stored (see
/// [`treasury`]), and the view says in its own words that v0 meters no real
/// spend (`spent: 0`, `note`).
fn treasury_view(node: &Node) -> Response {
    json_response(200, &node.treasury().view())
}

/// `GET /api/v0/treasury/journal`: the double-entry journal, oldest first and
/// newest last, bounded to the newest [`treasury::JOURNAL_LIMIT`] entries.
fn treasury_journal(node: &Node) -> Response {
    json_response(200, &node.treasury().journal(treasury::JOURNAL_LIMIT))
}

/// `POST /api/v0/tasks`: creates a task in `open`.
///
/// The ledger checks the request and its cap before an id is handed out, so a
/// refused create is a 400 (outside the contract) or a 409 (the ledger is
/// full) and leaves the ledger as it was.
fn create_task(node: &Node, body: &[u8]) -> Response {
    let request: CreateTask = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid create request: {err}")),
    };
    match node.tasks().create(&request) {
        Ok(task) => json_response(201, &task.view()),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `GET /api/v0/tasks/{id}`: one task.
fn get_task(node: &Node, id: &str) -> Response {
    match node.tasks().find(id) {
        Some(task) => json_response(200, &task.view()),
        None => Response::error(404, &format!("no task with id {id}")),
    }
}

/// `POST /api/v0/tasks/{id}/claim`: binds an open task to a live agent.
///
/// The claim is only accepted from an agent the node's register shows
/// connected right now - a VM that is running, reached by its guest CID - so
/// an unknown or stopped CID is refused before the task is touched. That
/// check is made here, against the register, and the ledger then refuses the
/// claim itself when the task is not open: one enforcement point per rule.
fn claim_task(node: &Node, id: &str, body: &[u8]) -> Response {
    let Some(task) = node.tasks().find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: ClaimRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid claim request: {err}")),
    };
    let agent = match connected_agent(node, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match task.claim(&agent) {
        Ok(()) => json_response(200, &task.view()),
        Err(refused) => {
            let refused = ClaimError::Refused(refused);
            Response::error(refused.status(), &refused.to_string())
        }
    }
}

/// The register's answer for a request that names an agent by guest CID: the
/// connected agent, or the refusal every such endpoint answers with — `no
/// live agent has guest CID n`, a 409.
///
/// The claim, the bid and the award all bind a task to an agent they have not
/// seen before, and all three ask the same register the same question, so they
/// ask it here: one check, one message, one status. *That* the agent is
/// connected is the register's to say; **what** it may do (claim a task, bid
/// on a call of its role) is the endpoint's own rule, enforced where the rule
/// lives.
fn connected_agent(node: &Node, cid: u64) -> Result<ConnectedAgent, Response> {
    node.registry().connected_agent(cid).ok_or_else(|| {
        let refused = ClaimError::AgentNotConnected { cid };
        Response::error(refused.status(), &refused.to_string())
    })
}

/// `POST /api/v0/tasks/{id}/finish`: ends a claimed task as `done` or
/// `failed`, refunding the escrow it still holds.
fn finish_task(node: &Node, id: &str, body: &[u8]) -> Response {
    let Some(task) = node.tasks().find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: FinishRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid finish request: {err}")),
    };
    if let Err(reason) = request.checked() {
        return Response::error(400, &reason);
    }
    match task.finish(request.outcome, request.note) {
        Ok(()) => json_response(200, &task.view()),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// The `POST /api/v0/tasks/{id}/bid` body: who bids, and what they ask.
#[derive(Debug, Deserialize)]
struct BidRequest {
    /// The guest CID of the agent making the bid.
    agent_cid: u64,
    /// The price the agent asks, in credits. A `u64`, so the schema's floor
    /// (`price` at or above zero) is the type: a negative price never leaves
    /// the body.
    price: u64,
}

/// The `POST /api/v0/tasks/{id}/award` body: who the call is awarded to. **No
/// price**: `agora.check_award` awards a bidder at the price they bid, so a
/// price here could only disagree with the bid.
#[derive(Debug, Deserialize)]
struct AwardRequest {
    /// The guest CID of the bidder the call is awarded to.
    agent_cid: u64,
}

/// `POST /api/v0/tasks/{id}/bid`: a connected agent bids on a roled open
/// task's call.
///
/// The rules, each at its own enforcement point:
///
/// * the request is well-formed (the body parses, a `price` is present and is
///   a non-negative integer) — this endpoint's, a 400;
/// * the task exists — the ledger's, a 404;
/// * the bidding agent is connected right now — the register's, the same check
///   and the same words the claim uses ([`connected_agent`]), a 409;
/// * everything that is the call's own — the agent's role is the call's
///   capability, the call is open, the price is at or below the call's ceiling,
///   the call has not reached its bid cap — the agora's ([`agora::Agora::bid`]),
///   a 409 with its own honest reason.
///
/// | status | when |
/// |---|---|
/// | 200 | the bid is recorded: `{"task","agent_cid","bidder","price","created_at"}` (a re-bid replaces the agent's own row and answers with the new one) |
/// | 400 | the body is outside this endpoint's contract |
/// | 404 | no task with that id |
/// | 405 | any method but `POST` |
/// | 409 | the caller is not a connected agent, or not an agent of the task's role, or the call is not open, or the price is above the task's budget, or the call holds its cap of bids |
fn bid_task(node: &Node, id: &str, body: &[u8]) -> Response {
    let Some(task) = node.tasks().find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: BidRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid bid request: {err}")),
    };
    let agent = match connected_agent(node, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match node.agora().bid(&task, &agent, request.price) {
        Ok(bid) => json_response(200, &bid),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `POST /api/v0/tasks/{id}/award`: awards a roled task's call to one of its
/// bidders — and **binds the task through the ledger's own claim**, exactly
/// the transition `POST /api/v0/tasks/{id}/claim` performs.
///
/// That integration is the agora's choice and it is deliberate: an award has
/// one way to bind a task, the same state machine and the same event every
/// other claim takes, so an awarded task is indistinguishable from a claimed
/// one afterwards. It is why the award needs a **connected** agent (the
/// claim's own precondition) and why the answer is the task, exactly as the
/// claim's is: the record of the award itself is on the board
/// ([`agora_view`]).
///
/// | status | when |
/// |---|---|
/// | 200 | the call was awarded and the task is bound: the task view, `claimed`, with `assignee_id` and `agent_cid` set |
/// | 400 | the body is outside this endpoint's contract |
/// | 404 | no task with that id |
/// | 405 | any method but `POST` |
/// | 409 | the caller is not a connected agent, or the task is not a call, or the call already has an award, or the call is not open, or the agent has no bid on it |
fn award_task(node: &Node, id: &str, body: &[u8]) -> Response {
    let Some(task) = node.tasks().find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: AwardRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid award request: {err}")),
    };
    let agent = match connected_agent(node, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match node.agora().award(&task, &agent) {
        Ok(()) => json_response(200, &task.view()),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `GET /api/v0/agora`: the board — the calls the node's roled tasks offer,
/// each with the bids it holds and the award that closed it.
///
/// The calls are **derived from the ledger, oldest first** (see [`agora`]),
/// and the response is bounded exactly as the journal's is: at most
/// [`agora::AGORA_LIMIT`] calls, the newest window of them, oldest first
/// within it, with `total`, `limit` and `truncated` saying what the window
/// left out. Polling this endpoint is the source of truth for the board; the
/// `agora` frames on `GET /api/v0/events` are the deltas.
fn agora_view(node: &Node) -> Response {
    let tasks = node.tasks().list();
    json_response(200, &node.agora().calls(&tasks, agora::AGORA_LIMIT))
}

/// The `POST /api/v0/tasks/{id}/artifact` body.
#[derive(Debug, Deserialize)]
struct ArtifactRequest {
    /// The path of the file to read, as the task's agent wrote it.
    path: String,
}

/// The most an artifact path may occupy, in bytes.
const MAX_ARTIFACT_PATH: usize = 1024;

/// The harness's `read_file` output, as the agent's own tool writes it:
/// `{"path","size","bytes","truncated","content"}`. The view keeps the size
/// and the truncation the tool reported beside the content, so a caller can
/// tell a whole file from a window the cap cut short.
#[derive(Debug, Deserialize)]
struct ReadFileOutput {
    /// The file's size on disk, in bytes.
    size: u64,
    /// Whether the cap, rather than the end of the file, ended the content.
    truncated: bool,
    /// The text returned.
    content: String,
}

/// `POST /api/v0/tasks/{id}/artifact`: reads one file the task's agent wrote
/// into its own workspace, by driving that agent's `read_file` through the
/// hub's admin surface.
///
/// The read-through exists because the dispatcher finishes a task with a note
/// naming the artifacts its agent wrote (`artifacts: index.txt (3 tool
/// call(s))`) and there is otherwise no way to read them without shelling into
/// the guest: this daemon resolves the task's assignee to its guest CID, asks
/// the hub to drive the agent's `read_file`, and returns what the agent read -
/// the console's one-origin path, like the hub proxy above.
///
/// It is a POST, not a GET, and deliberately so: it drives the agent's session
/// (a tool call with a cost and a refusal mode, and an effect on the agent's
/// turn), so it must not be advertised as safe for a prefetch, a retry or a
/// crawler to replay. The console posts it.
///
/// The status mapping, in one place:
///
/// | status | when |
/// |---|---|
/// | 200 | the agent read the file: `{"task","agent_cid","path","content","size","truncated"}` |
/// | 400 | the body or the path is outside this endpoint's contract |
/// | 403 | the hub's own refusal (a role the policy forbids the tool) |
/// | 404 | no task with that id, or the hub does not serve the agent's CID |
/// | 409 | the task has no assignee yet, or the hub's agent is not connected |
/// | 422 | the agent answered and refused the read (its own words are the reason) |
/// | 502 | the hub could not be reached, or answered something unreadable, or the agent's `read_file` output is not the tool's JSON |
/// | 503 | `--hub-admin off`: the daemon was never told where the admin surface is |
///
/// The ledger names the agent by the CID it recorded at claim time; whether
/// that CID is connected *now* is the hub's to say, and it says it in its own
/// words (a 409), which this endpoint surfaces unchanged rather than checking a
/// second time (`GET /status`) and racing its own answer.
///
/// The path is validated only for what this transport cannot carry - non-empty,
/// at most [`MAX_ARTIFACT_PATH`] bytes, no NUL - and not for where it points:
/// **the agent's harness enforces the real path discipline**, refusing a path
/// that escapes the workspace, a directory, a non-UTF-8 file and every other
/// case, with its own reason. Duplicating that here would be a second, weaker
/// copy of a rule the agent already owns; a refusal comes back as the agent's
/// own words with a 422.
fn read_artifact(node: &Node, id: &str, body: &[u8]) -> Response {
    let Some(admin) = node.hub_admin() else {
        return Response::error(503, "hub admin is not configured");
    };
    let request: ArtifactRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid artifact request: {err}")),
    };
    let path = request.path.as_str();
    if let Err(reason) = checked_artifact_path(path) {
        return Response::error(400, &reason);
    }
    let Some(task) = node.tasks().find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let Some(agent_cid) = task.view().agent_cid else {
        return Response::error(409, &format!("task {id} has no assignee yet"));
    };
    let args = serde_json::json!({ "path": path });
    match hub::tell(admin, agent_cid, hub::READ_FILE, args, hub::TELL_TIMEOUT_MS) {
        TellOutcome::Answered { ok: true, output } => {
            match serde_json::from_str::<ReadFileOutput>(&output) {
                Ok(read) => json_response(
                    200,
                    &ArtifactView {
                        task: id,
                        agent_cid,
                        path,
                        content: read.content,
                        size: read.size,
                        truncated: read.truncated,
                    },
                ),
                Err(err) => Response::error(
                    502,
                    &format!("the agent's read_file answer is not the tool's JSON: {err}"),
                ),
            }
        }
        // The agent answered and refused - a path it will not read, a
        // directory, a file that is not text - so the request was understood
        // and denied: its own words are the reason.
        TellOutcome::Answered { ok: false, output } => Response::error(422, &output),
        // The hub's own words and its own status: a role policy (403), an
        // unserved CID (404), an unconnected CID (409) or a timeout (504).
        TellOutcome::Refused { status, message } => Response::error(status, &message),
        TellOutcome::Unreachable(reason) => {
            Response::error(502, &format!("hub admin unreachable: {reason}"))
        }
    }
}

/// The path check this endpoint owns: non-empty, at most
/// [`MAX_ARTIFACT_PATH`] bytes, and with no NUL byte. Not a path *policy* -
/// the agent's harness decides what a path means and what it reaches (see
/// [`read_artifact`]).
fn checked_artifact_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("path must not be empty".to_string());
    }
    if path.len() > MAX_ARTIFACT_PATH {
        return Err(format!("path must be at most {MAX_ARTIFACT_PATH} bytes"));
    }
    if path.contains('\0') {
        return Err("path must not contain a NUL byte".to_string());
    }
    Ok(())
}

/// A task's artifact as the API reports it: which task's agent read which
/// path, and what the agent's own `read_file` returned.
#[derive(Debug, Serialize)]
struct ArtifactView<'a> {
    /// The task whose agent read the file.
    task: &'a str,
    /// The guest CID of the agent that read it (the task's assignee).
    agent_cid: u64,
    /// The path the caller asked for, as it sent it.
    path: &'a str,
    /// The text the agent read.
    content: String,
    /// The file's size on disk, in bytes, as the agent reported it.
    size: u64,
    /// Whether the agent's read cap, rather than the end of the file, ended
    /// the content.
    truncated: bool,
}

/// The `POST /api/v0/agents/{cid}/tell` body: the tool to drive and its
/// arguments, read the way the hub's own `/tell` body reads them (minus the
/// CID, which the path names).
#[derive(Debug, Deserialize)]
struct TellRequest {
    /// The tool to drive on the agent.
    name: String,
    /// The tool's arguments, a JSON object; an empty one when absent.
    #[serde(default = "empty_args")]
    args: serde_json::Value,
    /// How long to wait for the agent's answer, in ms; the hub's own bound
    /// when absent.
    timeout_ms: Option<u64>,
}

/// The default `args` an omitted field gets: an empty object, which is what a
/// tool that takes no arguments expects (the hub's own default).
fn empty_args() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// The most a tool name may occupy, in bytes - the hub's own bound
/// (`vsockd::admin::MAX_TOOL_NAME`), kept here so the two surfaces refuse the
/// same names.
const MAX_TOOL_NAME: usize = 256;

/// The longest, and the default, a `/tell` may wait for its agent: the hub's
/// own maximum (`vsockd`'s `MAX_TELL_TIMEOUT_MS`), so a caller is never cut
/// off before the hub itself would have given up.
const MAX_TELL_TIMEOUT_MS: u64 = hub::TELL_TIMEOUT_MS;

/// A `/tell` as the API reports it: the agent it was driven on, and that
/// agent's own answer.
#[derive(Debug, Serialize)]
struct TellView<'a> {
    /// The guest CID of the agent the call was driven on.
    cid: u64,
    /// The agent's own `ok`: `false` is the tool refusing, which is still an
    /// answer (its reason is `output`), never a transport failure.
    ok: bool,
    /// The agent's own `output`: the tool's result, or its refusal's reason.
    output: &'a str,
}

/// `POST /api/v0/agents/{cid}/tell`: drives one tool call on the agent
/// connected as `cid`, through the hub's admin surface - the operator's own
/// hand on the fleet, and the console's one-origin path to it.
///
/// This is the operator's console *driving a real agent*: the named tool runs
/// in that agent's session, with a real cost and a real effect on its turn. It
/// is a POST, not a GET, and deliberately so - for the same reason
/// [`read_artifact`] is: a request that can be replayed by a prefetch, a retry
/// or a crawler must not be advertised as safe to repeat. The console posts it.
///
/// The status mapping, in one place:
///
/// | status | when |
/// |---|---|
/// | 200 | the agent answered: `{"cid","ok","output"}` - the tool's result, or its own refusal, in its own words |
/// | 400 | the body or the CID path segment is outside this endpoint's contract |
/// | 403 | the hub's own role policy refuses the tool (its own status and body, verbatim) |
/// | 404 | the hub does not serve the agent's CID |
/// | 409 | the hub's agent is not connected |
/// | 504 | the agent did not answer within `timeout_ms` |
/// | 502 | the hub could not be reached, or answered something unreadable |
/// | 503 | `--hub-admin off`: the daemon was never told where the admin surface is |
///
/// The request is validated exactly as the hub's own `/tell` validates it - a
/// non-empty tool name of at most [`MAX_TOOL_NAME`] bytes, an `args` object,
/// and a `timeout_ms` of 1..=[`MAX_TELL_TIMEOUT_MS`] - so this daemon refuses
/// nothing the hub would accept and invents no rule of its own.
/// **What an agent may run is the hub's policy, not this daemon's**: the tool
/// name and its arguments are forwarded as written, and a role the policy
/// forbids the tool comes back as the hub's own 403, verbatim - this daemon
/// keeps no allow-list. Which CID is served, and connected, is the hub's to say
/// too, in its own 404 and 409, rather than a check made here and raced against
/// its own answer.
fn tell_agent(node: &Node, cid_segment: &str, body: &[u8]) -> Response {
    let Some(admin) = node.hub_admin() else {
        return Response::error(503, "hub admin is not configured");
    };
    let cid = match checked_guest_cid(cid_segment) {
        Ok(cid) => cid,
        Err(reason) => return Response::error(400, &reason),
    };
    let request: TellRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid tell request: {err}")),
    };
    if request.name.is_empty() {
        return Response::error(400, "a tell needs a tool name");
    }
    if request.name.len() > MAX_TOOL_NAME {
        return Response::error(
            400,
            &format!("the tool name is longer than {MAX_TOOL_NAME} bytes"),
        );
    }
    if !request.args.is_object() {
        return Response::error(400, "args must be a JSON object");
    }
    let timeout_ms = request.timeout_ms.unwrap_or(MAX_TELL_TIMEOUT_MS);
    if timeout_ms == 0 || timeout_ms > MAX_TELL_TIMEOUT_MS {
        return Response::error(
            400,
            &format!("timeout_ms must be 1..={MAX_TELL_TIMEOUT_MS}"),
        );
    }
    match hub::tell(admin, cid, &request.name, request.args, timeout_ms) {
        TellOutcome::Answered { ok, output } => json_response(
            200,
            &TellView {
                cid,
                ok,
                output: &output,
            },
        ),
        // The hub's own words and its own status: a role policy (403), an
        // unserved CID (404), an unconnected CID (409) or a timeout (504).
        TellOutcome::Refused { status, message } => Response::error(status, &message),
        TellOutcome::Unreachable(reason) => {
            Response::error(502, &format!("hub admin unreachable: {reason}"))
        }
    }
}

/// The guest CID a `/tell` addresses, read from the path segment: a decimal
/// number inside the transport's guest CID range ([`MIN_GUEST_CID`]..=
/// [`MAX_GUEST_CID`]), so a segment that could never name an agent of this node
/// is refused before the hub is contacted. Whether that CID is one the hub
/// serves, and whether it is connected, is the hub's to say - and it says it in
/// its own 404 and 409 (see [`tell_agent`]).
fn checked_guest_cid(segment: &str) -> Result<u64, String> {
    let cid: u64 = segment
        .parse()
        .map_err(|_| format!("{segment:?} is not a guest CID"))?;
    if !(MIN_GUEST_CID..=MAX_GUEST_CID).contains(&cid) {
        return Err(format!(
            "guest CID {cid} is outside {MIN_GUEST_CID}..={MAX_GUEST_CID}"
        ));
    }
    Ok(cid)
}

/// `DELETE /api/v0/tasks/{id}`: withdraws a task that has not ended, refunding
/// the escrow it still holds.
fn withdraw_task(node: &Node, id: &str) -> Response {
    match node.tasks().find(id) {
        Some(task) => match task.withdraw() {
            Ok(()) => json_response(200, &task.view()),
            Err(refused) => Response::error(refused.status(), &refused.to_string()),
        },
        None => Response::error(404, &format!("no task with id {id}")),
    }
}

/// A JSON response with the given status, or a 500 when the view cannot be
/// serialized (which would be a bug here, not a client error).
fn json_response<T: Serialize>(status: u16, value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(body) => Response::json(status, body),
        Err(err) => Response::error(500, &format!("cannot serialize the response: {err}")),
    }
}

/// The 405 for a method the matched endpoint does not accept.
fn method_not_allowed(allowed: &str) -> Response {
    Response::error(405, &format!("this endpoint accepts {allowed}"))
}

/// The 404 for a request that matches no endpoint at all.
fn no_such_endpoint() -> Response {
    Response::error(404, "no such endpoint")
}
