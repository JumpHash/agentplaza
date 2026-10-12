#![forbid(unsafe_code)]

//! `noded` — the node daemon: one machine's microVMs over a loopback HTTP API.
//!
//! * `GET /v1/machine` — the machine's facts
//! * `GET /v1/machine/stats` — live CPU, load, memory and VM counts, with history
//! * `GET /v1/runs` · `POST /v1/runs` — runs (groups of VMs) · boot one
//! * `GET /v1/runs/{id}` · `DELETE /v1/runs/{id}` — one run with its serial captures · kill it
//! * `POST /v1/runs/{id}/input` — write to one VM's serial console
//! * `POST /v1/runs/{id}/vms/{index}/memory` — resize one VM's balloon
//! * `GET /v1/events` — SSE: `run`, `vm`, `serial`
//!
//! With `--agents <roles manifest>` it also boots the role fleet at start:
//! one long-lived, harnessed VM per role. The plaza itself (agents, tasks,
//! models) lives in `apid`.

pub mod events;
pub mod fleet;
pub mod registry;
pub mod stats;

use std::path::{Path, PathBuf};
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

/// Guest memory ceiling per agent VM unless `--agents-memory-mib` says
/// otherwise. A VM boots with only its `--agents-balloon-mib` share usable (32 MiB
/// by default); the rest sits in a balloon, and the autoscaler grows the VM into
/// it as its work asks. An idle VM therefore costs nothing for the ceiling.
pub const DEFAULT_AGENTS_MEMORY_MIB: u32 = 1024;

/// vCPUs per agent VM unless `--agents-vcpus` says otherwise.
pub const DEFAULT_AGENTS_VCPUS: u32 = 1;

/// MiB of each agent VM's memory usable at boot unless `--agents-balloon-mib`
/// says otherwise; the rest of `--agents-memory-mib` is held in a balloon the VM
/// can grow into. Zero, or no less than the VM's memory, means no balloon.
pub const DEFAULT_AGENTS_BALLOON_MIB: u32 = 32;

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
    /// Where keepalive agents' workspace images persist across restarts
    /// (`DIR/workspaces/cid-<guest_cid>.ext4`); `None` keeps per-run copies.
    pub persist_dir: Option<PathBuf>,
    /// The roles manifest whose fleet boots at start.
    pub agents: Option<PathBuf>,
    pub agents_cid_base: u64,
    pub agents_memory_mib: u32,
    pub agents_vcpus: u32,
    /// MiB usable per agent VM at boot; `0`, or no less than the memory: no balloon.
    pub agents_balloon_mib: u32,
    /// The guest CIDs runs draw from when their create request names no
    /// `vsock_cid` (inclusive; `--run-cids`).
    pub run_cids: std::ops::RangeInclusive<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            kernel: PathBuf::from(DEFAULT_KERNEL),
            initrd: PathBuf::from(DEFAULT_INITRD),
            uds_dir: PathBuf::from(DEFAULT_UDS_DIR),
            workspace: None,
            persist_dir: None,
            agents: None,
            agents_cid_base: DEFAULT_AGENTS_CID_BASE,
            agents_memory_mib: DEFAULT_AGENTS_MEMORY_MIB,
            agents_vcpus: DEFAULT_AGENTS_VCPUS,
            agents_balloon_mib: DEFAULT_AGENTS_BALLOON_MIB,
            run_cids: registry::RUN_CID_BASE..=registry::RUN_CID_END,
        }
    }
}

/// The daemon's state: its runs and when it started.
pub struct Node {
    registry: Registry,
    started_at_ms: u64,
    stats: Arc<stats::Stats>,
}

/// `GET /v1/machine`.
#[derive(Debug, Serialize)]
pub struct MachineView {
    pub name: String,
    pub arch: &'static str,
    pub backend: &'static str,
    pub irqchip: &'static str,
    pub class: &'static str,
    pub kind: &'static str,
    pub gpus: Vec<Gpu>,
    pub cpus: u32,
    pub memory_mib: u64,
    pub vsock_mode: &'static str,
    pub started_at_ms: u64,
}

impl Node {
    pub fn new(config: Config) -> Self {
        let images = VmImages { kernel: config.kernel, initrd: config.initrd, workspace: config.workspace };
        Self {
            registry: Registry::new(images, config.uds_dir, events::EventBus::new())
                .with_persist_dir(config.persist_dir)
                .with_run_cids(config.run_cids),
            started_at_ms: epoch_ms(),
            stats: Arc::new(stats::Stats::new()),
        }
    }

    pub fn machine(&self) -> MachineView {
        let class = machine_class();
        let gpus = host_gpus();
        MachineView {
            name: hostname(),
            arch: machine_arch(),
            backend: machine_backend(),
            irqchip: machine_irqchip(),
            class,
            kind: machine_kind(class, &gpus),
            gpus,
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


/// A GPU the host has, read from sysfs and the NVIDIA driver's proc files
/// (no driver process needed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gpu {
    pub vendor: String,
    pub name: String,
    pub memory_mib: Option<u64>,
}

/// The vendor name for a sysfs PCI vendor id (`0x10de`), for the GPU vendors
/// the node knows.
fn gpu_vendor(id: &str) -> Option<&'static str> {
    match id.trim() {
        "0x10de" => Some("NVIDIA"),
        "0x1002" => Some("AMD"),
        "0x8086" => Some("Intel"),
        "0x106b" => Some("Apple"),
        _ => None,
    }
}

/// The host's GPUs: `/sys/class/drm` cards and the NVIDIA driver's GPU
/// directories.
pub fn host_gpus() -> Vec<Gpu> {
    gpus_in(Path::new("/sys/class/drm"), Path::new("/proc/driver/nvidia/gpus"))
}

/// The GPUs under a DRM class directory. Connector entries (`card0-HDMI-A-1`)
/// and devices whose files cannot be read are skipped; the Nth NVIDIA card
/// takes the Nth NVIDIA driver `Model:`.
fn gpus_in(drm: &Path, nvidia: &Path) -> Vec<Gpu> {
    let mut cards: Vec<PathBuf> = std::fs::read_dir(drm)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.strip_prefix("card").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
            })
        })
        .collect();
    cards.sort();
    let models = nvidia_models(nvidia);
    let mut nvidia_seen = 0;
    let mut gpus = Vec::new();
    for card in cards {
        let device = card.join("device");
        let Ok(vendor_id) = std::fs::read_to_string(device.join("vendor")) else {
            continue;
        };
        let Some(vendor) = gpu_vendor(&vendor_id) else {
            continue;
        };
        let Some(pci_id) = std::fs::read_to_string(device.join("uevent"))
            .ok()
            .and_then(|uevent| uevent.lines().find_map(|line| line.strip_prefix("PCI_ID=")).map(str::to_string))
        else {
            continue;
        };
        let name = if vendor == "NVIDIA" {
            nvidia_seen += 1;
            models.get(nvidia_seen - 1).cloned().unwrap_or_else(|| format!("{vendor} GPU {pci_id}"))
        } else {
            format!("{vendor} GPU {pci_id}")
        };
        let memory_mib = (vendor == "AMD")
            .then(|| std::fs::read_to_string(device.join("mem_info_vram_total")).ok())
            .flatten()
            .and_then(|bytes| bytes.trim().parse::<u64>().ok())
            .map(|bytes| bytes / (1024 * 1024));
        gpus.push(Gpu { vendor: vendor.to_string(), name, memory_mib });
    }
    gpus
}

/// The `Model:` line of each NVIDIA driver GPU directory, in directory order.
fn nvidia_models(dir: &Path) -> Vec<String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|gpu| {
            let info = std::fs::read_to_string(gpu.join("information")).ok()?;
            info.lines().find_map(|line| line.strip_prefix("Model:")).map(|model| model.trim().to_string())
        })
        .collect()
}

/// The machine kind the console groups by: `edge` (Pi), `apple`,
/// `workstation` (a server-class machine with a GPU), or `server`.
fn machine_kind(class: &str, gpus: &[Gpu]) -> &'static str {
    match class {
        "pi4" | "pi5" => "edge",
        "apple" => "apple",
        "server" if !gpus.is_empty() => "workstation",
        _ => "server",
    }
}

#[cfg(test)]
mod tests {
    use super::gpu_vendor;

    #[test]
    fn pci_vendor_ids_map_to_gpu_vendors() {
        assert_eq!(gpu_vendor("0x10de\n"), Some("NVIDIA"));
        assert_eq!(gpu_vendor("0x1002"), Some("AMD"));
        assert_eq!(gpu_vendor("0x8086"), Some("Intel"));
        assert_eq!(gpu_vendor("0x106b"), Some("Apple"));
        assert_eq!(gpu_vendor("0x1af4"), None);
    }
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
        stats::spawn(Arc::clone(&node.stats))?;
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
        .get("/v1/machine/stats", |node, _, _| machine_stats(node))
        .get("/v1/runs", |node, _, _| list_runs(node))
        .post("/v1/runs", |node, request, _| create_run(node, &request.body))
        .get("/v1/runs/{id}", |node, _, params| get_run(node, params.get("id")))
        .delete("/v1/runs/{id}", |node, _, params| delete_run(node, params.get("id")))
        .post("/v1/runs/{id}/input", |node, request, params| send_input(node, params.get("id"), &request.body))
        .post("/v1/runs/{id}/vms/{index}/memory", |node, request, params| {
            set_vm_memory(node, params.get("id"), params.get("index"), &request.body)
        })
        // One VM of a run: restart it as it was booted (same CID, same workspace image), reset
        // its workspace to the base image and restart it, or stop it for good.
        .post("/v1/runs/{id}/vms/{index}/restart", |node, _, params| restart_vm(node, params.get("id"), params.get("index")))
        .post("/v1/runs/{id}/vms/{index}/wipe", |node, _, params| wipe_vm(node, params.get("id"), params.get("index")))
        .post("/v1/runs/{id}/vms/{index}/stop", |node, _, params| stop_vm(node, params.get("id"), params.get("index")))
}

/// `POST /v1/runs/{id}/vms/{index}/restart` → 200 with the run's summary view.
fn restart_vm(node: &Node, id: &str, index: &str) -> Response {
    vm_op(node, id, index, Registry::restart_vm)
}

/// `POST /v1/runs/{id}/vms/{index}/wipe`: stop the VM, replace its workspace image with a fresh
/// copy of the base image (the persistent image too, for a keepalive run), boot it again.
fn wipe_vm(node: &Node, id: &str, index: &str) -> Response {
    vm_op(node, id, index, Registry::wipe_vm)
}

/// `POST /v1/runs/{id}/vms/{index}/stop`: stop the VM and do not restart it (the run keeps its
/// other VMs; a keepalive VM stopped this way is not brought back).
fn stop_vm(node: &Node, id: &str, index: &str) -> Response {
    vm_op(node, id, index, Registry::stop_vm)
}

/// Runs one per-VM operation on VM `index` of run `id`. An index that is not a
/// number is a 404, like an unknown run or VM; the answer is the run's summary.
fn vm_op(
    node: &Node,
    id: &str,
    index: &str,
    op: fn(&Registry, &str, u32) -> Result<Arc<crate::registry::Run>, crate::registry::VmOpError>,
) -> Response {
    let Ok(vm) = index.parse::<u32>() else {
        return Response::error(404, &format!("no VM {index} in run {id}"));
    };
    match op(node.registry(), id, vm) {
        Ok(run) => Response::json(200, &run.view(Detail::Summary)),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `GET /v1/machine/stats`: the latest host sample, the last minute of samples,
/// and the running VMs with the guest memory they were booted with.
fn machine_stats(node: &Node) -> Response {
    let (mut vms_running, mut guest_mem_mib) = (0_u64, 0_u64);
    let mut vms = Vec::new();
    for run in node.registry().list() {
        let view = run.view(Detail::Summary);
        let pids = run.process_ids();
        for vm in view.vms.iter().filter(|vm| vm.state == "running") {
            vms_running += 1;
            guest_mem_mib += u64::from(view.memory_mib);
            let Some(&(_, pid)) = pids.iter().find(|(index, _)| *index == vm.id) else {
                continue;
            };
            vms.push(stats::VmProcess {
                pid,
                run: view.id.to_string(),
                vm: vm.id,
                cid: vm.guest_cid,
                role: vm.role.clone(),
                usable_mib: vm.memory.as_ref().map(|memory| memory.usable_mib),
                guest_used_mib: vm.memory.as_ref().and_then(|memory| {
                    let available = memory.available_mib?;
                    Some(u64::from(memory.max_mib).saturating_sub(available))
                }),
            });
        }
    }
    match node.stats.view(vms_running, guest_mem_mib, &vms) {
        Some(view) => Response::json(200, &view),
        None => Response::error(503, "no host sample yet"),
    }
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

/// The `POST /v1/runs/{id}/vms/{index}/memory` body.
#[derive(Debug, Deserialize)]
struct MemoryRequest {
    /// The guest's usable memory, in MiB.
    usable_mib: u32,
}

/// `POST /v1/runs/{id}/vms/{index}/memory`: resizes one VM's balloon and
/// answers with the VM's memory as the balloon now reads it. An unknown run or
/// VM is a 404; a VM with no balloon, or one that has stopped, is a 409 (see
/// `registry::MemoryError::status`).
fn set_vm_memory(node: &Node, id: &str, index: &str, body: &[u8]) -> Response {
    let Ok(vm) = index.parse::<u32>() else {
        return Response::error(404, &format!("no VM {index} in run {id}"));
    };
    let request: MemoryRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid memory request: {err}")),
    };
    match node.registry().set_vm_memory(id, vm, request.usable_mib) {
        Ok(memory) => Response::json(200, &memory),
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

