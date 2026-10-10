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
//! * `POST   /api/v0/tasks`            - create a task, `open`
//! * `GET    /api/v0/tasks/{id}`       - one task
//! * `POST   /api/v0/tasks/{id}/claim` - claim an open task as a live agent
//! * `POST   /api/v0/tasks/{id}/finish`- end a claimed task (`done`/`failed`)
//! * `DELETE /api/v0/tasks/{id}`       - withdraw a task before it ends
//! * `GET    /api/v0/hub`              - the hub's `/status`, proxied verbatim
//! * `GET    /api/v0/hub/healthz`      - the hub's `/healthz`, proxied verbatim
//! * `GET    /` and `GET /<file>`      - the console's static files
//!
//! Run states and transitions are the schema's (`core.run_transitions`,
//! `docs/orchestrator-draft/schema.sql`), restricted to the states noded v0
//! implements: see [`registry`]. The task ledger is `core.tasks` restricted
//! the same way, and only ever accepts a claim from an agent the register
//! shows connected right now: see [`tasks`].

pub mod events;
pub mod fleet;
pub mod http;
pub mod hub;
pub mod registry;
pub mod static_files;
pub mod tasks;

use std::borrow::Cow;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::http::Response;
use crate::hub::HubStatus;
use crate::registry::{CreateRun, Detail, Registry, VmImages};
use crate::tasks::{ClaimError, ClaimRequest, CreateTask, FinishRequest, TaskLedger, TasksView};

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

/// The daemon's state: its runs, its task ledger, the console directory it
/// serves, the hub's status surface it proxies, and when it started (the
/// machine view's uptime).
pub struct Node {
    registry: Registry,
    tasks: TaskLedger,
    console_dir: PathBuf,
    hub_status: Option<HubStatus>,
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
        Self {
            registry: Registry::new(
                VmImages {
                    kernel: config.kernel,
                    initrd: config.initrd,
                    workspace: config.workspace,
                },
                config.uds_dir,
                Arc::clone(&events),
            ),
            tasks: TaskLedger::new(events),
            console_dir: config.console_dir,
            hub_status: config.hub_status,
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
            if request.method() == "GET" && request.path() == "/api/v0/events" {
                let receiver = node.events().subscribe();
                if let Err(err) = events::stream(&receiver, stream.get_mut()) {
                    eprintln!("noded: an events stream ended: {err}");
                }
                return;
            }
            route(&request, node)
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
        "/" => match request.method() {
            "GET" => static_files::serve(node.console_dir(), static_files::INDEX),
            _ => method_not_allowed("GET"),
        },
        _ => match path.strip_prefix("/api/v0/runs/") {
            Some(rest) => run_endpoint(rest, request, node),
            None => match path.strip_prefix("/api/v0/tasks/") {
                Some(rest) => task_endpoint(rest, request, node),
                None => match request.method() {
                    "GET" => {
                        static_files::serve(node.console_dir(), path.strip_prefix('/').unwrap_or(path))
                    }
                    _ => method_not_allowed("GET"),
                },
            },
        },
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
    let Some(agent) = node.registry().connected_agent(request.agent_cid) else {
        let refused = ClaimError::AgentNotConnected {
            cid: request.agent_cid,
        };
        return Response::error(refused.status(), &refused.to_string());
    };
    match task.claim(&agent) {
        Ok(()) => json_response(200, &task.view()),
        Err(refused) => {
            let refused = ClaimError::Refused(refused);
            Response::error(refused.status(), &refused.to_string())
        }
    }
}

/// `POST /api/v0/tasks/{id}/finish`: ends a claimed task as `done` or
/// `failed`.
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
        Err(refused) => Response::error(409, &refused.to_string()),
    }
}

/// `DELETE /api/v0/tasks/{id}`: withdraws a task that has not ended.
fn withdraw_task(node: &Node, id: &str) -> Response {
    match node.tasks().find(id) {
        Some(task) => match task.withdraw() {
            Ok(()) => json_response(200, &task.view()),
            Err(refused) => Response::error(409, &refused.to_string()),
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
