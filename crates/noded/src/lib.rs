#![forbid(unsafe_code)]

//! `noded` — the node daemon: one machine's microVMs over a loopback HTTP API.
//!
//! * `GET /v1/machine` — the machine's facts
//! * `GET /v1/runs` · `POST /v1/runs` — runs (groups of VMs) · boot one
//! * `GET /v1/runs/{id}` · `DELETE /v1/runs/{id}` — one run with its serial captures · kill it
//! * `POST /v1/runs/{id}/input` — write to one VM's serial console
//! * `GET /v1/events` — SSE: `run`, `vm`, `serial`
//!
//! With `--agents <roles manifest>` it also boots the role fleet at start:
//! one long-lived, harnessed VM per role. The plaza itself (agents, tasks,
//! models) lives in `apid`.

pub mod events;
pub mod fleet;
pub mod registry;

use std::path::PathBuf;
use std::sync::Arc;

use apz_http::{Response, Router};
use serde::{Deserialize, Serialize};

use crate::registry::{CreateRun, Detail, Registry, VmImages};

pub const DEFAULT_PORT: u16 = 8899;

/// The guest kernel unless `--kernel` says otherwise.
pub const DEFAULT_KERNEL: &str = "/boot/vmlinuz-linux";

/// The guest initrd unless `--initrd` says otherwise, relative to the working
/// directory.
pub const DEFAULT_INITRD: &str = "target/guest/initrd.x86_64.gz";


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

/// The daemon's configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    /// Directory for the per-VM vsock Unix sockets.
    pub uds_dir: PathBuf,
    /// The writable workspace image each VM gets a copy of.
    pub workspace: Option<PathBuf>,
    /// The roles manifest whose fleet boots at start.
    pub agents: Option<PathBuf>,
    pub agents_cid_base: u64,
    pub agents_memory_mib: u32,
    pub agents_vcpus: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            kernel: PathBuf::from(DEFAULT_KERNEL),
            initrd: PathBuf::from(DEFAULT_INITRD),
            uds_dir: PathBuf::from(DEFAULT_UDS_DIR),
            workspace: None,
            agents: None,
            agents_cid_base: DEFAULT_AGENTS_CID_BASE,
            agents_memory_mib: DEFAULT_AGENTS_MEMORY_MIB,
            agents_vcpus: DEFAULT_AGENTS_VCPUS,
        }
    }
}

/// The daemon's state: its runs and when it started.
pub struct Node {
    registry: Registry,
    started_at_ms: u64,
}

/// `GET /v1/machine`.
#[derive(Debug, Serialize)]
pub struct MachineView {
    pub name: String,
    pub arch: &'static str,
    pub backend: &'static str,
    pub irqchip: &'static str,
    pub class: &'static str,
    pub cpus: u32,
    pub memory_mib: u64,
    pub vsock_mode: &'static str,
    pub started_at_ms: u64,
}

impl Node {
    pub fn new(config: Config) -> Self {
        let images = VmImages { kernel: config.kernel, initrd: config.initrd, workspace: config.workspace };
        Self {
            registry: Registry::new(images, config.uds_dir, events::EventBus::new()),
            started_at_ms: epoch_ms(),
        }
    }

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

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn events(&self) -> &events::EventBus {
        self.registry.events_bus()
    }
}

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


/// A bound, serving node daemon.

pub struct Server {
    http: apz_http::Handle,
    node: Arc<Node>,
}

impl Server {
    /// Binds `config.port` on 127.0.0.1 and starts serving.
    pub fn start(config: Config) -> std::io::Result<Self> {
        let server = apz_http::Server::bind(config.port)?;
        let node = Arc::new(Node::new(config));
        let (router, serving) = (router(), Arc::clone(&node));
        let http = server.spawn(move |request| router.handle(&serving, request))?;
        Ok(Self { http, node })
    }

    /// The daemon's state, for the startup that runs before it serves.
    pub fn node(&self) -> &Arc<Node> {
        &self.node
    }

    /// The bound port (the kernel's choice when `config.port` was 0).
    pub fn port(&self) -> u16 {
        self.http.port()
    }

    /// Serves until the server is stopped.
    pub fn wait(self) -> std::io::Result<()> {
        self.http.join()
    }
}

/// Binds `config.port` and serves.
pub fn serve(config: Config) -> std::io::Result<()> {
    Server::start(config)?.wait()
}

/// The `POST /v1/runs/{id}/input` body.
#[derive(Debug, Deserialize)]
struct InputRequest {
    /// The line (or bytes) to write to the VM's console.
    data: String,
    /// The index of the VM to write to.
    vm: u32,
}

/// The API under `/v1`, and the console's files.
///
/// Everything that changes state or drives an agent is a POST (or DELETE), so

/// The API under `/v1`. Everything that changes state is a POST or DELETE.
fn router() -> Router<Node> {
    Router::<Node>::new()
        .get("/v1/machine", |node, _, _| Response::json(200, &node.machine()))
        .get("/v1/events", |node, _, _| {
            let receiver = node.events().subscribe();
            Response::sse(move |sse| events::stream(&receiver, sse))
        })
        .get("/v1/runs", |node, _, _| list_runs(node))
        .post("/v1/runs", |node, request, _| create_run(node, &request.body))
        .get("/v1/runs/{id}", |node, _, params| get_run(node, params.get("id")))
        .delete("/v1/runs/{id}", |node, _, params| delete_run(node, params.get("id")))
        .post("/v1/runs/{id}/input", |node, request, params| send_input(node, params.get("id"), &request.body))
}

/// `GET /v1/runs`: every run, without serial captures.
fn list_runs(node: &Node) -> Response {
    let runs = node.registry().list();
    let view = registry::RunsView {
        runs: runs.iter().map(|run| run.view(Detail::Summary)).collect(),
    };
    Response::json(200, &view)
}

/// `POST /v1/runs`: boots a run's VMs.
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
        Ok(run) => Response::json(201, &run.view(Detail::Summary)),
        Err(reason) => Response::error(400, &reason),
    }
}

/// `GET /v1/runs/{id}`: one run, with every VM's serial capture.
fn get_run(node: &Node, id: &str) -> Response {
    match node.registry().find(id) {
        Some(run) => Response::json(200, &run.view(Detail::Full)),
        None => Response::error(404, &format!("no run with id {id}")),
    }
}

/// `POST /v1/runs/{id}/input`: writes to one VM's serial console.
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
        Ok(()) => Response::json(200, &run.view(Detail::Full)),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `DELETE /v1/runs/{id}`: kills a run's VMs.
fn delete_run(node: &Node, id: &str) -> Response {
    match node.registry().find(id) {
        Some(run) => {
            run.kill();
            Response::json(200, &run.view(Detail::Full))
        }
        None => Response::error(404, &format!("no run with id {id}")),
    }
}

