#![forbid(unsafe_code)]

//! `noded` — the node daemon: one machine's microVMs over a loopback HTTP API.
//!
//! Machine and monitoring:
//!
//! * `GET /v1/machine` — the machine's facts
//! * `GET /v1/machine/stats` — live CPU, load, memory and VM counts, with history
//! * `GET /v1/ws` — a WebSocket (`apz.noded.v1`) of `run`, `vm` and `serial` events; read-only
//! * `GET /v1/host/scale` — whether this host suits scale mode, item by item
//! * `GET /v1/metrics` — the node's Prometheus metrics, as text
//! * `GET /v1/metrics/batch` — the metrics collected since the last batch (`?live=1`: 204 unless the hub streams this node)
//! * `POST /v1/metrics/live` — stream live metrics until `until_ms`
//!
//! Shared volumes and warm templates:
//!
//! * `GET /v1/volumes` — every shared data volume, with the live VMs that mount it
//! * `POST /v1/volumes` — create a shared data volume
//! * `DELETE /v1/volumes/{name}` — delete a volume (409 while a live VM mounts it)
//! * `GET /v1/templates` — the warm templates built
//! * `DELETE /v1/templates/{key}` — remove a warm template
//!
//! Runs and their VMs:
//!
//! * `GET /v1/runs` — every run, oldest first
//! * `POST /v1/runs` — boot a run (a group of VMs)
//! * `GET /v1/runs/{id}` — one run, with its serial captures
//! * `DELETE /v1/runs/{id}` — kill a run's VMs
//! * `POST /v1/runs/{id}/input` — write to one VM's serial console
//! * `POST /v1/runs/{id}/vms/{index}/memory` — resize one VM's balloon
//! * `POST /v1/runs/{id}/vms/{index}/restart` — restart one VM as it was booted, with the same CID
//! * `POST /v1/runs/{id}/vms/{index}/wipe` — reset one VM's workspace to the base image and boot it
//! * `POST /v1/runs/{id}/vms/{index}/stop` — stop one VM for good
//! * `POST /v1/runs/{id}/vms/{index}/pause` — pause one VM
//! * `POST /v1/runs/{id}/vms/{index}/resume` — resume a paused VM
//! * `POST /v1/runs/{id}/vms/{index}/doze` — pause a VM and page its guest memory out of host RAM
//! * `POST /v1/runs/{id}/vms/{index}/sleep` — sleep a VM to its snapshot and stop it
//! * `POST /v1/runs/{id}/vms/{index}/wake` — wake a sleeping VM from its snapshot
//! * `POST /v1/runs/{id}/vms/{index}/fork` — checkpoint a VM and fork it into new single-VM runs
//! * `GET /v1/runs/{id}/vms/{index}/checkpoints` — list a VM's checkpoints
//! * `POST /v1/runs/{id}/vms/{index}/checkpoints` — checkpoint a VM
//! * `POST /v1/runs/{id}/vms/{index}/checkpoints/{name}/restore` — restore a VM to a checkpoint
//! * `DELETE /v1/runs/{id}/vms/{index}/checkpoints/{name}` — delete a checkpoint
//!
//! Moving a sleeping VM between nodes (`migrate.rs`):
//!
//! * `POST /v1/runs/{id}/vms/{index}/export` — record a sleeping VM's files for a transfer
//! * `GET /v1/exports/{export}/{name}` — read one chunk of an exported file (`offset`, `len`)
//! * `DELETE /v1/exports/{export}` — forget an export
//! * `POST /v1/imports` — start an import of a VM's files
//! * `POST /v1/imports/{import}/{name}` — write one chunk of an imported file
//! * `POST /v1/imports/{import}/finish` — check the staged files and make the VM asleep here
//! * `DELETE /v1/imports/{import}` — abort an import and remove what it staged
//!
//! Workstations (desktop VMs):
//!
//! * `GET /v1/workstations` — every workstation
//! * `POST /v1/workstations` — create a workstation
//! * `GET /v1/workstations/{id}` — one workstation
//! * `DELETE /v1/workstations/{id}` — stop a workstation and remove its disk and snapshot
//! * `POST /v1/workstations/{id}/start` — start a workstation
//! * `POST /v1/workstations/{id}/sleep` — sleep a workstation, keeping its memory
//! * `POST /v1/workstations/{id}/wake` — resume a sleeping workstation
//! * `GET /v1/workstations/{id}/console` — the tail of the serial console (`?bytes=`, default 16 KiB)
//! * `POST /v1/workstations/{id}/stop` — stop a workstation
//! * `POST /v1/workstations/{id}/snapshot` — snapshot a stopped workstation's disk
//! * `POST /v1/workstations/{id}/revert` — copy a snapshot back over the workstation's disk
//!
//! MCP servers in microVMs:
//!
//! * `GET /v1/mcp-vm` — whether this node can run an MCP server in a microVM
//! * `POST /v1/mcp-runs` — boot a run for an MCP server
//! * `GET /v1/mcp-runs/{id}` — one MCP server run
//! * `DELETE /v1/mcp-runs/{id}` — stop an MCP server's VM and remove its run
//! * `DELETE /v1/mcp-storage/{server}` — remove a server's kept workspace
//!
//! Trainer jobs:
//!
//! * `GET /v1/train` — the running trainer jobs
//! * `POST /v1/train` — start a trainer job
//! * `GET /v1/train/{job_id}/events?after=N` — what the job printed since line N
//! * `POST /v1/train/{job_id}/stop` — stop a trainer job
//! * `DELETE /v1/train/{job_id}` — remove a finished job's directory
//!
//
//! With `--agents <roles manifest>` it also boots the role fleet at start:
//! one long-lived, harnessed VM per role. The plaza itself (agents, tasks,
//! models) lives in `apid`.

mod collect;
pub mod cpu_baseline;
pub mod events;
pub mod fleet;
pub mod hostcheck;
mod mcp_vm;
pub mod metrics;
pub mod migrate;
pub mod registry;
pub mod snapshots;
pub mod stats;
pub mod templates;
pub mod train;
pub mod transfer;
pub mod volumes;
pub mod workspace_image;
pub mod workstation;
pub mod ws_qemu;
pub mod ws_relay;

use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apz_http::{Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::registry::{CreateRun, Detail, Registry, VmImages};

pub const DEFAULT_PORT: u16 = 8899;

/// The guest kernel unless `--kernel` says otherwise.
pub const DEFAULT_KERNEL: &str = "/boot/vmlinuz-linux";

/// The guest initrd unless `--initrd` says otherwise, relative to the working
/// directory.
pub const DEFAULT_INITRD: &str = "target/guest/initrd.x86_64.gz";

/// The Unix socket directory of the per-VM vsock sockets (VM `i`'s muxer binds `{dir}/vm{cid}`). The
/// config default is `apz_os::dirs::vsock_sockets()`, which is this on Linux and macOS and differs on Windows.
pub const DEFAULT_UDS_DIR: &str = "/tmp/plaza-vsock";

/// The first guest CID of the initial role fleet unless `--agents-cid-base`
/// says otherwise; a role with slot `s` gets `DEFAULT_AGENTS_CID_BASE + s`
/// (in a manifest with no slots, `s` is the role's alphabetical index).
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
    /// Where VM snapshots are kept (`--snapshot-dir`; by default `snapshots` under the persist
    /// directory, else under the daemon's state directory); `None` (`--no-snapshots`) keeps none, and
    /// every snapshot operation is refused.
    pub snapshot_dir: Option<PathBuf>,
    /// Where the node's shared data volumes are kept (`--volume-dir`; by default `volumes` beside the
    /// snapshots); `None` keeps none, and a run that names a volume is refused.
    pub volume_dir: Option<PathBuf>,
    /// Start new keepalive agent VMs from warm templates kept under `<snapshot_dir>/templates`
    /// (`--no-warm-templates` turns it off). Needs a snapshot dir.
    pub warm_templates: bool,
    /// Run each VM in its own `apz-vmm` process with cgroup limits (`--vm-processes`).
    pub vm_processes: bool,
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
    /// The desktop guest workstation VMs boot (`--workstation-kernel`, `--workstation-initrd`,
    /// `--workstation-image`), and where their directories live (`--workstation-dir`).
    pub workstation: workstation::Guest,
    /// The machine's internal token; the HTTP API answers only requests that carry it
    /// (`--internal-token-file`). None: open.
    pub internal_token: apz_http::auth::InternalToken,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            kernel: PathBuf::from(DEFAULT_KERNEL),
            initrd: PathBuf::from(DEFAULT_INITRD),
            uds_dir: apz_os::dirs::vsock_sockets(),
            workspace: None,
            persist_dir: None,
            snapshot_dir: None,
            volume_dir: None,
            warm_templates: true,
            vm_processes: false,
            agents: None,
            agents_cid_base: DEFAULT_AGENTS_CID_BASE,
            agents_memory_mib: DEFAULT_AGENTS_MEMORY_MIB,
            agents_vcpus: DEFAULT_AGENTS_VCPUS,
            agents_balloon_mib: DEFAULT_AGENTS_BALLOON_MIB,
            run_cids: registry::RUN_CID_BASE..=registry::RUN_CID_END,
            workstation: workstation::Guest::default(),
            internal_token: apz_http::auth::InternalToken::none(),
        }
    }
}

/// The daemon's state: its runs, when it started, and the machine facts probed once at start.
pub struct Node {
    registry: Registry,
    started_at_ms: u64,
    stats: Arc<stats::Stats>,
    /// What this node records for the hub (metrics.rs), filled by the collectors (collect.rs).
    pub metrics: metrics::NodeMetrics,
    probed: Probed,
    /// The fabric addresses and when they were read (see `cached_fabric`).
    fabric: Mutex<Option<(Instant, Vec<String>)>>,
    /// The workstation VMs this node makes for the hub (workstation.rs).
    workstations: workstation::Store,
    /// The exports and imports of sleeping VMs in flight (migrate.rs).
    migrations: migrate::Migrations,
    /// The blob store and the server node-to-node transfers use (transfer.rs).
    transfers: transfer::Transfers,
    /// The MCP servers this node runs in microVMs (mcp_vm.rs).
    mcp: mcp_vm::McpVms,
}

/// `GET /v1/machine`.
#[derive(Debug, Serialize)]
pub struct MachineView {
    /// The port this node serves node-to-node transfers on (transfer.rs); null when it does not.
    pub transfer_port: Option<u16>,
    /// The SHA-256 pin of this node's transfer certificate (hex); null when it serves none.
    pub transfer_pin: Option<String>,
    pub name: String,
    pub arch: &'static str,
    pub os: &'static str,
    pub backend: &'static str,
    pub irqchip: &'static str,
    pub class: &'static str,
    pub kind: &'static str,
    /// The maker, lower-case (`apple`, `dell`, `raspberrypi`); null when unknown.
    pub vendor: Option<String>,
    /// The maker's model name; null when unknown.
    pub model: Option<String>,
    /// A Mac's Apple Silicon details; null elsewhere.
    pub apple: Option<Apple>,
    pub cpu: Cpu,
    pub gpus: Vec<Gpu>,
    /// The devices that run trainers (GPUs, Apple's GPU); empty on a CPU-only machine.
    pub accelerators: Vec<Accelerator>,
    /// The trainer runtime this machine can run jobs in.
    pub trainer: Trainer,
    /// The addresses other machines may reach this one at, private ranges first.
    pub fabric: Vec<String>,
    pub cpus: u32,
    pub memory_mib: u64,
    /// The size of the state directory's filesystem, in GiB; null when unknown.
    pub storage_gib: Option<u64>,
    /// The free space on that filesystem now, in GiB; null when unknown.
    pub storage_free_gib: Option<u64>,
    /// Whether the state directory's filesystem clones files (reflinks, `vmm::clonefile`).
    pub clones: bool,
    /// The CPU baseline a pool is keyed by (`cpu_baseline`); null where the platform has none to read.
    pub cpu_baseline: Option<String>,
    /// The warm templates this node holds, by key (the bases of plan 16.1 that a node keeps).
    pub bases: Vec<String>,
    pub vsock_mode: &'static str,
    /// The launch backends this node can run a workstation VM under: `plaza`, and `qemu` when QEMU and its firmware are here.
    pub workstation_backends: Vec<&'static str>,
    pub started_at_ms: u64,
}

impl Node {
    pub fn new(config: Config) -> Self {
        let mut guest = config.workstation;
        if guest.qemu.is_none() {
            guest.qemu = ws_qemu::detect();
        }
        // A workstation VM's vsock sockets live with the agents' (`--uds-dir`), under its own CID range.
        guest.vsock_dir = config.uds_dir.clone();
        let images = VmImages {
            kernel: config.kernel,
            initrd: config.initrd,
            workspace: config.workspace,
        };
        Self {
            registry: Registry::new(images, config.uds_dir, events::EventBus::new())
                .with_persist_dir(config.persist_dir)
                .with_templates(
                    config
                        .snapshot_dir
                        .as_ref()
                        .filter(|_| config.warm_templates)
                        .map(|dir| crate::templates::Templates::new(dir.join("templates"))),
                )
                .with_snapshots(config.snapshot_dir.map(snapshots::SnapshotStore::new))
                .with_volumes(config.volume_dir.and_then(|dir| {
                    volumes::VolumeStore::open(dir.clone())
                        .map_err(|err| {
                            eprintln!(
                                "noded: no data volumes: cannot open {}: {err}",
                                dir.display()
                            )
                        })
                        .ok()
                }))
                .with_run_cids(config.run_cids),
            started_at_ms: epoch_ms(),
            stats: Arc::new(stats::Stats::new()),
            metrics: metrics::NodeMetrics::new(),
            probed: probe_host(&apz_os::dirs::state()),
            fabric: Mutex::new(None),
            workstations: workstation::Store::open(guest),
            migrations: migrate::Migrations::new(),
            transfers: transfer::Transfers::open(&apz_os::dirs::state().join("transfers")),
            mcp: mcp_vm::McpVms::kvm(),
        }
    }

    pub fn machine(&self) -> MachineView {
        let class = machine_class();
        MachineView {
            transfer_port: self.transfers.port(),
            transfer_pin: self.transfers.pin(),
            name: hostname(),
            arch: machine_arch(),
            os: machine_os(),
            backend: machine_backend(),
            irqchip: machine_irqchip(),
            class,
            kind: self.probed.kind,
            vendor: self.probed.vendor.clone(),
            model: self.probed.model.clone(),
            apple: self.probed.apple.clone(),
            cpu: self.probed.cpu.clone(),
            gpus: host_gpus(),
            accelerators: accelerators_now(&self.probed.facts.accelerators, &train::busy_devices()),
            trainer: self.probed.facts.trainer.clone(),
            fabric: cached_fabric(&self.fabric, Instant::now(), probe_fabric),
            cpus: host_cpus(),
            memory_mib: self.probed.memory_mib,
            storage_gib: self.probed.storage_gib,
            storage_free_gib: storage_free_gib(&apz_os::dirs::state()),
            clones: self.probed.clones,
            cpu_baseline: self.probed.cpu_baseline.clone(),
            bases: self
                .registry
                .templates()
                .map(|templates| templates.list().into_iter().map(|view| view.key).collect())
                .unwrap_or_default(),
            vsock_mode: "uds-muxer",
            workstation_backends: self.workstations.backends(),
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
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
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
    } else if cfg!(windows) {
        "whp"
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

/// The host's total memory, in MiB (`MemTotal` from `/proc/meminfo`; `hw.memsize` on macOS).
fn host_memory_mib() -> u64 {
    if cfg!(target_os = "macos") {
        return sysctl("hw.memsize")
            .and_then(|n| parse_bytes(&n))
            .map_or(0, |bytes| bytes / MIB);
    }
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
    gpus_in(
        Path::new("/sys/class/drm"),
        Path::new("/proc/driver/nvidia/gpus"),
    )
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
                n.strip_prefix("card").is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                })
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
            .and_then(|uevent| {
                uevent
                    .lines()
                    .find_map(|line| line.strip_prefix("PCI_ID="))
                    .map(str::to_string)
            })
        else {
            continue;
        };
        let name = if vendor == "NVIDIA" {
            nvidia_seen += 1;
            models
                .get(nvidia_seen - 1)
                .cloned()
                .unwrap_or_else(|| format!("{vendor} GPU {pci_id}"))
        } else {
            format!("{vendor} GPU {pci_id}")
        };
        let memory_mib = (vendor == "AMD")
            .then(|| std::fs::read_to_string(device.join("mem_info_vram_total")).ok())
            .flatten()
            .and_then(|bytes| bytes.trim().parse::<u64>().ok())
            .map(|bytes| bytes / (1024 * 1024));
        gpus.push(Gpu {
            vendor: vendor.to_string(),
            name,
            memory_mib,
        });
    }
    gpus
}

/// The `Model:` line of each NVIDIA driver GPU directory, in directory order.
fn nvidia_models(dir: &Path) -> Vec<String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|gpu| {
            let info = std::fs::read_to_string(gpu.join("information")).ok()?;
            info.lines()
                .find_map(|line| line.strip_prefix("Model:"))
                .map(|model| model.trim().to_string())
        })
        .collect()
}

/// The host's operating system, for the machine model.
fn machine_os() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// The CPU: the model name, the physical cores and the logical CPUs (threads).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cpu {
    pub model: String,
    pub cores: u32,
    pub threads: u32,
}

/// The machine facts that do not change while the daemon runs, probed once at start.
#[derive(Debug, Clone)]
struct Probed {
    kind: &'static str,
    vendor: Option<String>,
    model: Option<String>,
    apple: Option<Apple>,
    cpu: Cpu,
    memory_mib: u64,
    storage_gib: Option<u64>,
    /// Whether the state directory's filesystem clones files.
    clones: bool,
    /// The CPU baseline (`cpu_baseline`), read once at start.
    cpu_baseline: Option<String>,
    facts: MachineFacts,
}

/// A Mac with Apple Silicon: its chip and core counts, GPU cores, Neural Engine cores and unified
/// memory. Counts that the host does not report are null.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Apple {
    pub chip: String,
    pub p_cores: Option<u32>,
    pub e_cores: Option<u32>,
    pub gpu_cores: Option<u32>,
    pub neural_cores: u32,
    pub unified_memory_gib: Option<u64>,
}

const DMI_DIR: &str = "/sys/class/dmi/id";
const DMI_CHASSIS: &str = "/sys/class/dmi/id/chassis_type";
const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";
const DEVICE_TREE_MODEL: &str = "/proc/device-tree/model";
const WINDOWS_CPU_KEY: &str = r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0";
/// `Win32_Processor`: the physical cores and the logical processors, summed over the sockets.
const WINDOWS_CORES: &str = "$p = Get-CimInstance Win32_Processor; '{0}|{1}' -f ($p | Measure-Object -Property NumberOfCores -Sum).Sum, ($p | Measure-Object -Property NumberOfLogicalProcessors -Sum).Sum";
/// `Win32_ComputerSystem`, `Win32_SystemEnclosure` and `Win32_Battery` in one call: the memory in
/// bytes, the first chassis type, the battery count, then the manufacturer and the model.
const WINDOWS_SYSTEM: &str = "$c = Get-CimInstance Win32_ComputerSystem; $e = Get-CimInstance Win32_SystemEnclosure; $b = @(Get-CimInstance Win32_Battery); '{0}|{1}|{2}|{3}|{4}' -f $c.TotalPhysicalMemory, @($e.ChassisTypes)[0], $b.Count, $c.Manufacturer, $c.Model";
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// Probes the machine once: its kind, maker and model, CPU, memory and the size of the state
/// directory's filesystem.
fn probe_host(state_dir: &Path) -> Probed {
    let tree = device_tree_model();
    if cfg!(target_os = "macos") {
        let (kind, vendor, model, apple) = probe_mac();
        let facts = probe_machine_facts(apple.as_ref());
        return Probed {
            kind,
            vendor,
            model,
            apple,
            cpu: probe_cpu(None),
            memory_mib: host_memory_mib(),
            storage_gib: storage_gib(state_dir),
            clones: vmm::clonefile::reflink_supported(state_dir),
            cpu_baseline: cpu_baseline::host(),
            facts,
        };
    }
    // Windows reads its system, enclosure and battery facts in one PowerShell call.
    let windows = cfg!(target_os = "windows")
        .then(|| parse_windows_system(&powershell(WINDOWS_SYSTEM).unwrap_or_default()));
    let sbc = matches!(machine_class(), "pi4" | "pi5")
        || tree
            .as_deref()
            .is_some_and(|model| model.contains("Raspberry Pi"));
    let chassis = match &windows {
        Some(system) => system.chassis,
        None => chassis_type(),
    };
    let battery = windows.as_ref().is_some_and(|system| system.battery)
        || has_battery(Path::new(POWER_SUPPLY_DIR));
    let kind = kind_of(cfg!(target_os = "android"), sbc, chassis, battery);
    let (vendor, model, memory_mib) = match windows {
        Some(system) => (system.vendor, system.model, system.memory_mib),
        None => {
            let (vendor, model) = host_maker(tree.as_deref());
            (vendor, model, host_memory_mib())
        }
    };
    Probed {
        kind,
        vendor,
        model,
        apple: None,
        cpu: probe_cpu(tree.as_deref()),
        memory_mib,
        storage_gib: storage_gib(state_dir),
        clones: vmm::clonefile::reflink_supported(state_dir),
        cpu_baseline: cpu_baseline::host(),
        facts: probe_machine_facts(None),
    }
}

/// A Mac's kind, maker, model and Apple Silicon details. The model name is `system_profiler`'s
/// ("MacBook Pro", "Mac Studio") with the chip, falling back to `hw.model`.
fn probe_mac() -> (&'static str, Option<String>, Option<String>, Option<Apple>) {
    let hardware = command_stdout("system_profiler", &["SPHardwareDataType"]).unwrap_or_default();
    let hw_model = sysctl("hw.model").unwrap_or_default();
    let name = parse_model_name(&hardware);
    let chip = sysctl("machdep.cpu.brand_string").filter(|brand| brand.starts_with("Apple "));
    let kind = apple_kind(name.as_deref().unwrap_or(&hw_model));
    let model = match name {
        Some(name) => Some(apple_model(&name, chip.as_deref())),
        None => clean_model(&hw_model),
    };
    let apple = chip.map(|chip| Apple {
        p_cores: sysctl("hw.perflevel0.physicalcpu").and_then(|n| parse_count(&n)),
        e_cores: sysctl("hw.perflevel1.physicalcpu").and_then(|n| parse_count(&n)),
        gpu_cores: command_stdout("system_profiler", &["SPDisplaysDataType"])
            .and_then(|text| parse_gpu_cores(&text)),
        neural_cores: neural_cores(&chip),
        unified_memory_gib: sysctl("hw.memsize")
            .and_then(|n| parse_bytes(&n))
            .map(|bytes| bytes / GIB),
        chip,
    });
    (kind, Some("apple".to_string()), model, apple)
}

/// The maker and model of a machine that is neither a Mac nor a Windows computer: an Android
/// device's properties, an arm board's device-tree model, or a PC's DMI strings.
fn host_maker(tree: Option<&str>) -> (Option<String>, Option<String>) {
    if cfg!(target_os = "android") {
        let manufacturer =
            command_stdout("getprop", &["ro.product.manufacturer"]).unwrap_or_default();
        let model = command_stdout("getprop", &["ro.product.model"]).unwrap_or_default();
        return maker_pair(&manufacturer, &model);
    }
    if let Some(model) = tree {
        return (normalize_vendor(model), clean_model(model));
    }
    let read =
        |file: &str| std::fs::read_to_string(format!("{DMI_DIR}/{file}")).unwrap_or_default();
    let vendor = normalize_vendor(&read("sys_vendor"));
    let model = dmi_model(
        vendor.as_deref(),
        &read("product_name"),
        &read("product_version"),
    );
    (vendor, model)
}

/// The maker and model from a raw vendor and model string.
fn maker_pair(vendor: &str, model: &str) -> (Option<String>, Option<String>) {
    (normalize_vendor(vendor), clean_model(model))
}

/// What the Windows probe reports about the computer.
#[derive(Debug, PartialEq)]
struct WindowsSystem {
    vendor: Option<String>,
    model: Option<String>,
    memory_mib: u64,
    chassis: Option<u32>,
    battery: bool,
}

/// The Windows probe's `memory|chassis|batteries|manufacturer|model` line. The numbers come first
/// and the model last, so a model with a `|` of its own stays whole. Missing fields read as absent.
fn parse_windows_system(text: &str) -> WindowsSystem {
    let mut fields = text.trim().splitn(5, '|');
    let mut next = || fields.next().unwrap_or("");
    let memory = parse_bytes(next()).map_or(0, |bytes| bytes / MIB);
    let chassis = parse_chassis(next());
    let battery = parse_count(next()).is_some_and(|batteries| batteries > 0);
    let (vendor, model) = maker_pair(next(), next());
    WindowsSystem {
        vendor,
        model,
        memory_mib: memory,
        chassis,
        battery,
    }
}

/// The model of a PC from its DMI strings. Lenovo's `product_name` is a machine code and its
/// `product_version` the marketing name (`ThinkPad X1 Carbon Gen 9`), so that is preferred there.
fn dmi_model(vendor: Option<&str>, product_name: &str, product_version: &str) -> Option<String> {
    let name = clean_model(product_name);
    if vendor == Some("lenovo") {
        clean_model(product_version).or(name)
    } else {
        name
    }
}

/// Placeholders that firmware and drivers write instead of a real maker or model.
const PLACEHOLDERS: &[&str] = &[
    "",
    "default string",
    "none",
    "not applicable",
    "not defined",
    "not specified",
    "o.e.m.",
    "system manufacturer",
    "system product name",
    "system version",
    "to be filled by o.e.m.",
    "undefined",
    "unknown",
];

fn is_placeholder(text: &str) -> bool {
    PLACEHOLDERS.contains(&text.trim().to_ascii_lowercase().as_str())
}

/// A maker's name as the lower-case short name every node reports (`dell`, `lenovo`, `apple`):
/// the first word of the vendor string, letters and digits only, with the few makers whose first
/// word differs from their short name mapped (`ASUSTeK COMPUTER INC.` is `asus`). None for a
/// placeholder or an empty string.
fn normalize_vendor(raw: &str) -> Option<String> {
    if is_placeholder(raw) {
        return None;
    }
    let first = raw.split_whitespace().next()?;
    let key: String = first
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    let name = match key.as_str() {
        "asustek" => "asus",
        "microstar" => "msi",
        "hewlett" | "hewlettpackard" => "hp",
        "raspberry" => "raspberrypi",
        other => other,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// A model name as the host reports it, trimmed; None for a placeholder or an empty string.
fn clean_model(raw: &str) -> Option<String> {
    let text = raw.trim();
    (!is_placeholder(text)).then(|| text.to_string())
}

/// The model name of a Mac from `system_profiler SPHardwareDataType` (`Model Name: MacBook Pro`).
fn parse_model_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| clean_model(line.trim().strip_prefix("Model Name:")?))
}

/// A Mac's marketing model with its chip: `MacBook Pro (M3 Max)`, `Mac Studio (M2 Ultra)`.
fn apple_model(name: &str, chip: Option<&str>) -> String {
    match chip {
        Some(chip) => format!("{name} ({})", chip.trim_start_matches("Apple ")),
        None => name.to_string(),
    }
}

/// The Neural Engine's core count of an Apple Silicon chip: 16 on every chip so far, 32 on the
/// "Ultra" chips, which fuse two dies (an assumption about the chips, not a probed value).
fn neural_cores(chip: &str) -> u32 {
    if chip.contains("Ultra") { 32 } else { 16 }
}

/// The GPU core count from `system_profiler SPDisplaysDataType` (`Total Number of Cores: 40`).
fn parse_gpu_cores(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| parse_count(line.trim().strip_prefix("Total Number of Cores:")?))
}

/// A count from a command's output or a sysctl value (`12`).
fn parse_count(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

/// A byte count from a sysctl value (`hw.memsize`).
fn parse_bytes(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// The physical cores and logical processors from the Windows probe's `cores|threads` line; None
/// when either is zero or the line is malformed.
fn parse_core_pair(text: &str) -> Option<(u32, u32)> {
    let (cores, threads) = text.trim().split_once('|')?;
    let (cores, threads) = (parse_count(cores)?, parse_count(threads)?);
    (cores > 0 && threads > 0).then_some((cores, threads))
}

/// Runs a PowerShell command and returns its trimmed output, when it succeeds.
fn powershell(script: &str) -> Option<String> {
    command_stdout("powershell", &["-NoProfile", "-Command", script])
}

/// The machine kind from what the host reports: `phone` (Android), `edge` (a single-board
/// computer), `laptop` (a laptop chassis or a battery), `server` (a server chassis) or
/// `workstation`. Chassis types are the SMBIOS values of `chassis_type`.
fn kind_of(android: bool, sbc: bool, chassis: Option<u32>, battery: bool) -> &'static str {
    const LAPTOP: [u32; 6] = [8, 9, 10, 14, 31, 32];
    const SERVER: [u32; 4] = [17, 23, 25, 28];
    // The chassis decides when it says something (types 1 and 2 are "Other" and "Unknown"); a
    // battery only when it does not, since a desktop on a UPS reports one too.
    let known = chassis.filter(|c| *c > 2);
    if android {
        "phone"
    } else if sbc {
        "edge"
    } else if known.is_some_and(|c| LAPTOP.contains(&c)) || (known.is_none() && battery) {
        "laptop"
    } else if known.is_some_and(|c| SERVER.contains(&c)) {
        "server"
    } else {
        "workstation"
    }
}

/// A Mac's kind from its `hw.model` (`MacBookPro18,3`, `Macmini9,1`): a laptop when the model is a "Book".
fn apple_kind(model: &str) -> &'static str {
    if model.contains("Book") {
        "laptop"
    } else {
        "workstation"
    }
}

/// The DMI chassis type of the machine, when the firmware reports one.
fn chassis_type() -> Option<u32> {
    parse_chassis(&std::fs::read_to_string(DMI_CHASSIS).ok()?)
}

fn parse_chassis(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

/// Whether a `BAT*` power supply is listed under `dir` (a laptop's battery).
fn has_battery(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("BAT"))
}

/// The device-tree model of an arm board (`/proc/device-tree/model`), when there is one.
fn device_tree_model() -> Option<String> {
    parse_device_tree_model(&std::fs::read(DEVICE_TREE_MODEL).ok()?)
}

/// The device-tree `model` property: text ending in a NUL byte.
fn parse_device_tree_model(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// The CPU of the host. Linux reads `/proc/cpuinfo`; macOS asks `sysctl`; Windows reads the
/// registry through `reg query` and the physical cores and logical processors through PowerShell
/// (both counts fall back to the threads when that fails). Probed once, at start.
fn probe_cpu(tree: Option<&str>) -> Cpu {
    let logical = host_cpus();
    if cfg!(target_os = "macos") {
        return Cpu {
            model: sysctl("machdep.cpu.brand_string").unwrap_or_else(|| "unknown".to_string()),
            cores: sysctl("hw.physicalcpu")
                .and_then(|n| parse_count(&n))
                .unwrap_or(logical),
            threads: sysctl("hw.logicalcpu")
                .and_then(|n| parse_count(&n))
                .unwrap_or(logical),
        };
    }
    if cfg!(target_os = "windows") {
        let model = command_stdout(
            "reg",
            &["query", WINDOWS_CPU_KEY, "/v", "ProcessorNameString"],
        )
        .and_then(|out| parse_reg_string(&out, "ProcessorNameString"));
        let (cores, threads) = powershell(WINDOWS_CORES)
            .and_then(|out| parse_core_pair(&out))
            .unwrap_or((logical, logical));
        return Cpu {
            model: model.unwrap_or_else(|| "unknown".to_string()),
            cores,
            threads,
        };
    }
    parse_cpuinfo(
        &std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(),
        tree,
        logical,
    )
}

/// `/proc/cpuinfo` as a CPU. The model is the first "model name", else the device-tree model
/// (arm boards), else the "Hardware" line. Threads are the "processor" blocks; cores are the
/// distinct (physical id, core id) pairs, a block without a core id counting as its own core.
/// `logical` stands for both when the text lists no processor.
fn parse_cpuinfo(text: &str, tree: Option<&str>, logical: u32) -> Cpu {
    let (mut model, mut hardware) = (None, None);
    // (physical id, core id) of each "processor" block, in order.
    let mut blocks: Vec<(Option<&str>, Option<&str>)> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "processor" => blocks.push((None, None)),
            "physical id" => {
                if let Some(block) = blocks.last_mut() {
                    block.0 = Some(value);
                }
            }
            "core id" => {
                if let Some(block) = blocks.last_mut() {
                    block.1 = Some(value);
                }
            }
            "model name" => model = model.or(Some(value)),
            "Hardware" => hardware = hardware.or(Some(value)),
            _ => {}
        }
    }
    let cores: std::collections::BTreeSet<(String, String)> = blocks
        .iter()
        .enumerate()
        .map(|(index, &(physical, core))| {
            (
                physical.unwrap_or("0").to_string(),
                core.map_or_else(|| index.to_string(), str::to_string),
            )
        })
        .collect();
    let (threads, cores) = match u32::try_from(blocks.len()) {
        Ok(threads) if threads > 0 => (threads, u32::try_from(cores.len()).unwrap_or(u32::MAX)),
        _ => (logical, logical),
    };
    let model = model
        .or(tree)
        .or(hardware)
        .map_or_else(|| "unknown".to_string(), str::to_string);
    Cpu {
        model,
        cores,
        threads,
    }
}

/// The data of value `name` in `reg query` output (`    ProcessorNameString    REG_SZ    Intel ...`).
fn parse_reg_string(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, data) = line.trim().strip_prefix(name)?.split_once("REG_SZ")?;
        let data = data.trim();
        (!data.is_empty()).then(|| data.to_string())
    })
}

/// The total size in GiB of the filesystem holding `dir` (`/` when `dir` does not exist), from
/// `df -Pk`. None on Windows or when `df` fails.
fn storage_gib(dir: &Path) -> Option<u64> {
    df_gib(dir, 1)
}

/// The free space in GiB of the filesystem holding `dir` now (see [`storage_gib`]).
fn storage_free_gib(dir: &Path) -> Option<u64> {
    df_gib(dir, 3)
}

/// One column of `df -Pk` for the filesystem holding `dir`, in GiB.
fn df_gib(dir: &Path, column: usize) -> Option<u64> {
    if cfg!(target_os = "windows") {
        return None;
    }
    let target = if dir.exists() { dir } else { Path::new("/") };
    parse_df_gib(&command_stdout("df", &["-Pk", target.to_str()?])?, column)
}

/// The GiB in `column` of a `df -Pk` table's first data line (1024-byte blocks: column 1 is the total,
/// column 3 the available space).
fn parse_df_gib(output: &str, column: usize) -> Option<u64> {
    let kib: u64 = output
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(column)?
        .parse()
        .ok()?;
    Some(kib / (1024 * 1024))
}

fn sysctl(name: &str) -> Option<String> {
    command_stdout("sysctl", &["-n", name])
}

/// The trimmed stdout of a command that succeeds, or None. On Windows the probe starts without a
/// console window.
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = hidden_command(program, args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A command with no console window on Windows.
fn hidden_command(program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// What a command printed (untrimmed) and whether it exited successfully.
#[derive(Debug)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a command, reading both pipes while it runs, and kills it when it has not exited within
/// `timeout`. None when it cannot start or is killed.
fn command_within(program: &str, args: &[&str], timeout: Duration) -> Option<Output> {
    let mut command = hidden_command(program, args);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some(Output {
        ok: status.success(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Reads a pipe to its end on a thread, so a command that writes a lot cannot block on a full pipe.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// A device that runs trainers: a GPU, or Apple Silicon's GPU. CPUs are not listed (the planner
/// handles them). `index` is the vendor's own device number; `busy` is set while a training job
/// on this node holds the device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Accelerator {
    pub index: u32,
    /// `nvidia`, `amd`, `intel` or `apple`.
    pub vendor: &'static str,
    pub name: String,
    pub memory_mib: Option<u64>,
    /// `cuda`, `rocm`, `xpu` or `mps`.
    pub backend: &'static str,
    /// The CUDA compute capability (`8.6`); null elsewhere.
    pub capability: Option<String>,
    /// `pcie`, `nvlink`, or `soc` (unified memory).
    pub link: &'static str,
    pub busy: bool,
    /// Refreshed on demand for NVIDIA devices; null elsewhere.
    pub util_pct: Option<u32>,
    pub mem_used_mib: Option<u64>,
}

/// The trainer runtime this node can run jobs in (see `train`). `runtime` is null when none
/// applies; `ok` is false when the runtime is missing something, and `detail` says what.
/// `trainer_env` is the torch build the trainer's environment holds (`ok` does not depend on it).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trainer {
    pub runtime: Option<&'static str>,
    pub ok: bool,
    pub detail: String,
    pub trainer_env: TrainerEnv,
}

/// The torch build in the trainer's environment, and whether it serves this machine's accelerators. `torch` is
/// `cuda`, `rocm`, `xpu`, `cpu` or `default` (the PyPI wheel: macOS's, with MPS), null when there is no torch.
/// `detail` says what matches, or the `uv sync` that installs the right build.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrainerEnv {
    pub torch: Option<&'static str>,
    pub matches: bool,
    pub detail: String,
}

/// The machine facts probed once at start, next to the CPU and memory.
#[derive(Debug, Clone)]
struct MachineFacts {
    accelerators: Vec<Accelerator>,
    trainer: Trainer,
}

/// How long the fabric addresses are kept before they are read again.
const FABRIC_TTL: Duration = Duration::from_secs(30);

/// The fabric addresses: the cached read while it is younger than `FABRIC_TTL`, else `probe` again.
/// The lock is held while probing, so concurrent calls read once.
fn cached_fabric(
    cache: &Mutex<Option<(Instant, Vec<String>)>>,
    now: Instant,
    probe: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((read_at, fabric)) = cache.as_ref()
        && now.saturating_duration_since(*read_at) < FABRIC_TTL
    {
        return fabric.clone();
    }
    let fabric = probe();
    *cache = Some((now, fabric.clone()));
    fabric
}

const NVIDIA_QUERY: &str =
    "--query-gpu=index,name,memory.total,memory.used,utilization.gpu,compute_cap";
const NVIDIA_CSV: &str = "--format=csv,noheader,nounits";
const ROCM_QUERY: [&str; 4] = ["--showproductname", "--showmeminfo", "vram", "--json"];
const NVIDIA_REFRESH: Duration = Duration::from_secs(2);
const WINDOWS_ADDRESSES: &str = "Get-NetIPAddress | ForEach-Object { $_.IPAddress }";

/// Probes the accelerators and the trainer runtime.
fn probe_machine_facts(apple: Option<&Apple>) -> MachineFacts {
    let accelerators = probe_accelerators(apple);
    let vendors: Vec<&str> = accelerators.iter().map(|device| device.vendor).collect();
    let trainer = train::probe_trainer(&vendors);
    MachineFacts {
        accelerators,
        trainer,
    }
}

fn probe_accelerators(apple: Option<&Apple>) -> Vec<Accelerator> {
    let mut found = nvidia_probe();
    found.extend(amd_probe());
    found.extend(intel_probe(Path::new("/sys/class/drm")));
    found.extend(apple.map(apple_accelerator));
    found
}

fn nvidia_probe() -> Vec<Accelerator> {
    let Some(query) = command_stdout("nvidia-smi", &[NVIDIA_QUERY, NVIDIA_CSV]) else {
        return Vec::new();
    };
    let topo = command_stdout("nvidia-smi", &["topo", "-m"]).unwrap_or_default();
    nvidia_accelerators(&query, &topo)
}

/// The NVIDIA devices from `nvidia-smi --query-gpu` (`0, NVIDIA GeForce RTX 3090, 24576, 5803,
/// 36, 8.6`) and the topology matrix: NVLink when the matrix has an `NV<n>` link.
fn nvidia_accelerators(query: &str, topo: &str) -> Vec<Accelerator> {
    let link = if topo_has_nvlink(topo) {
        "nvlink"
    } else {
        "pcie"
    };
    query
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let n = fields.len();
            if n < 6 {
                return None;
            }
            // The name may hold commas; the index is first and the four numbers last.
            let number = |text: &str| text.parse::<u64>().ok();
            Some(Accelerator {
                index: fields[0].parse().ok()?,
                vendor: "nvidia",
                name: fields[1..n - 4].join(", "),
                memory_mib: number(fields[n - 4]),
                backend: "cuda",
                capability: Some(fields[n - 1].to_string())
                    .filter(|cc| cc != "[N/A]" && !cc.is_empty()),
                link,
                busy: false,
                util_pct: number(fields[n - 2]).map(|pct| pct as u32),
                mem_used_mib: number(fields[n - 3]),
            })
        })
        .collect()
}

/// Whether `nvidia-smi topo -m` shows an NVLink between GPUs (`NV4`, `NV12`). The legend's `NV#` is
/// not one, and escape codes around a link do not hide it.
fn topo_has_nvlink(topo: &str) -> bool {
    topo.match_indices("NV")
        .any(|(at, _)| topo[at + 2..].starts_with(|c: char| c.is_ascii_digit()))
}

fn amd_probe() -> Vec<Accelerator> {
    command_stdout("rocm-smi", &ROCM_QUERY)
        .map(|text| amd_accelerators(&text))
        .unwrap_or_default()
}

/// The AMD devices from `rocm-smi --showproductname --showmeminfo vram --json`: one object per
/// `cardN`. Parsed defensively, since the keys have shifted between ROCm releases.
fn amd_accelerators(text: &str) -> Vec<Accelerator> {
    let Ok(Value::Object(cards)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut found: Vec<Accelerator> = cards
        .iter()
        .filter_map(|(key, card)| {
            let index = key.strip_prefix("card")?.parse::<u32>().ok()?;
            let card = card.as_object()?;
            let text_of = |field: &str| card.get(field).and_then(Value::as_str).map(str::to_string);
            let name = text_of("Card series")
                .or_else(|| text_of("Card model"))
                .unwrap_or_else(|| format!("AMD GPU {index}"));
            let memory_mib = card
                .iter()
                .find(|(field, _)| field.starts_with("VRAM Total Memory"))
                .and_then(|(_, value)| {
                    value
                        .as_u64()
                        .or_else(|| value.as_str()?.trim().parse().ok())
                })
                .map(|bytes| bytes / MIB);
            Some(Accelerator {
                index,
                vendor: "amd",
                name,
                memory_mib,
                backend: "rocm",
                capability: None,
                link: "pcie",
                busy: false,
                util_pct: None,
                mem_used_mib: None,
            })
        })
        .collect();
    found.sort_by_key(|device| device.index);
    found
}

/// The discrete Intel GPUs under a DRM class directory: `card<N>` with the vendor `0x8086` and a
/// discrete device id. Memory comes from the driver's `lmem_total_bytes` when it has one.
fn intel_probe(drm: &Path) -> Vec<Accelerator> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut found: Vec<Accelerator> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let index = card_index(&entry.file_name().to_string_lossy())?;
            let device = entry.path().join("device");
            if read_trim(&device.join("vendor"))? != "0x8086" {
                return None;
            }
            let device_id = read_trim(&device.join("device"))?;
            if !intel_discrete(&device_id) {
                return None;
            }
            let slot = std::fs::read_link(&device)
                .ok()?
                .file_name()?
                .to_string_lossy()
                .into_owned();
            let name = lspci_name(&slot).unwrap_or_else(|| format!("Intel GPU {device_id}"));
            let memory_mib = read_trim(&device.join("lmem_total_bytes"))
                .and_then(|text| text.parse::<u64>().ok())
                .map(|bytes| bytes / MIB);
            Some(Accelerator {
                index,
                vendor: "intel",
                name,
                memory_mib,
                backend: "xpu",
                capability: None,
                link: "pcie",
                busy: false,
                util_pct: None,
                mem_used_mib: None,
            })
        })
        .collect();
    found.sort_by_key(|device| device.index);
    found
}

/// The N of a DRM card name (`card1`); None for connectors (`card1-HDMI-A-1`) and render nodes.
fn card_index(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("card")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether an Intel PCI device id is a discrete GPU: Ponte Vecchio (`0x0bd0`-`0x0bdb`), Arc
/// Alchemist (`0x5690`-`0x56cf`) and Battlemage (`0xe200`-`0xe2ff`). Integrated graphics are not
/// listed; a new discrete part needs a line here.
fn intel_discrete(device_id: &str) -> bool {
    let Some(id) = device_id
        .strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
    else {
        return false;
    };
    matches!(id, 0x0bd0..=0x0bdb | 0x5690..=0x56cf | 0xe200..=0xe2ff)
}

/// The device's name from `lspci -s <slot>` (`03:00.0 VGA compatible controller: Intel Corporation
/// DG2 [Arc A770] (rev 08)`), or None when lspci is missing.
fn lspci_name(slot: &str) -> Option<String> {
    let line = command_stdout("lspci", &["-s", slot])?;
    name_from_lspci(&line)
}

fn name_from_lspci(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(": ")?;
    let name = rest.rfind(" (rev ").map_or(rest, |at| &rest[..at]);
    Some(name.trim().to_string()).filter(|name| !name.is_empty())
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

/// Apple Silicon's GPU: one `mps` device named by the chip, with the unified memory.
fn apple_accelerator(apple: &Apple) -> Accelerator {
    Accelerator {
        index: 0,
        vendor: "apple",
        name: apple.chip.clone(),
        memory_mib: apple.unified_memory_gib.map(|gib| gib * 1024),
        backend: "mps",
        capability: None,
        link: "soc",
        busy: false,
        util_pct: None,
        mem_used_mib: None,
    }
}

/// The accelerators as they are now: the probed list with `busy` set from the jobs on this node and
/// the NVIDIA utilization and memory read again, with a short timeout so a hung driver cannot stall
/// `GET /v1/machine`.
fn accelerators_now(cached: &[Accelerator], busy: &[(&'static str, u32)]) -> Vec<Accelerator> {
    let fresh = if cached.iter().any(|device| device.vendor == "nvidia") {
        command_within("nvidia-smi", &[NVIDIA_QUERY, NVIDIA_CSV], NVIDIA_REFRESH)
            .filter(|output| output.ok)
            .map(|output| nvidia_accelerators(&output.stdout, ""))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    cached
        .iter()
        .map(|device| {
            let mut device = device.clone();
            if let Some(now) = fresh
                .iter()
                .find(|now| device.vendor == "nvidia" && now.index == device.index)
            {
                device.util_pct = now.util_pct;
                device.mem_used_mib = now.mem_used_mib;
            }
            device.busy = busy.contains(&(device.vendor, device.index));
            device
        })
        .collect()
}

/// The addresses other machines may reach this one at: Windows' `Get-NetIPAddress`, macOS'
/// `ifconfig`, Linux' `ip -j addr`.
fn probe_fabric() -> Vec<String> {
    let addresses = if cfg!(windows) {
        powershell(WINDOWS_ADDRESSES)
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    } else if cfg!(target_os = "macos") {
        command_stdout("ifconfig", &[])
            .map(|text| parse_ifconfig(&text))
            .unwrap_or_default()
    } else {
        command_stdout("ip", &["-j", "addr"])
            .map(|text| parse_ip_json(&text))
            .unwrap_or_default()
    };
    fabric_order(addresses)
}

/// The `local` addresses of `ip -j addr` (`[{"ifname":…,"addr_info":[{"family":"inet","local":…}]}]`),
/// without the virtual interfaces that no other machine reaches.
fn parse_ip_json(text: &str) -> Vec<String> {
    let Ok(Value::Array(links)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    links
        .iter()
        .filter(|link| !link["ifname"].as_str().is_some_and(virtual_interface))
        .flat_map(|link| link["addr_info"].as_array().into_iter().flatten())
        .filter_map(|info| info["local"].as_str().map(str::to_string))
        .collect()
}

/// The `inet` and `inet6` addresses of `ifconfig`, without their `%zone` suffixes and the virtual
/// interfaces' addresses.
fn parse_ifconfig(text: &str) -> Vec<String> {
    let mut virtual_link = false;
    let mut addresses = Vec::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            virtual_link = line.split(':').next().is_some_and(virtual_interface);
            continue;
        }
        let mut words = line.split_whitespace();
        if let Some("inet" | "inet6") = words.next()
            && let Some(address) = words.next().and_then(|address| address.split('%').next())
            && !virtual_link
        {
            addresses.push(address.to_string());
        }
    }
    addresses
}

/// Bridges, container and VM networks, and peer-to-peer links: their addresses are local to the
/// machine (`docker0` is `172.17.0.1` on every host), so they are not a fabric.
fn virtual_interface(name: &str) -> bool {
    const PREFIXES: [&str; 8] = [
        "docker", "br-", "veth", "virbr", "cni", "flannel", "lxcbr", "bridge",
    ];
    const MAC_PEER: [&str; 2] = ["awdl", "llw"];
    PREFIXES
        .iter()
        .chain(MAC_PEER.iter())
        .any(|prefix| name.starts_with(prefix))
}

/// The usable addresses, each once: private IPv4 and unique-local IPv6 first, then other IPv4,
/// then global IPv6. Loopback, unspecified, multicast and link-local addresses are dropped.
fn fabric_order(addresses: Vec<String>) -> Vec<String> {
    let mut usable: Vec<(u8, String)> = addresses
        .into_iter()
        .filter_map(|text| {
            let address: IpAddr = text.split('%').next()?.trim().parse().ok()?;
            Some((fabric_rank(address)?, address.to_string()))
        })
        .collect();
    usable.sort();
    usable.dedup();
    usable.into_iter().map(|(_, address)| address).collect()
}

fn fabric_rank(address: IpAddr) -> Option<u8> {
    match address {
        IpAddr::V4(v4) => {
            if v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
            {
                None
            } else if v4.is_private() {
                Some(0)
            } else {
                Some(2)
            }
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || first & 0xffc0 == 0xfe80
            {
                None
            } else if first & 0xfe00 == 0xfc00 {
                Some(1)
            } else {
                Some(3)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cpu, gpu_vendor, has_battery};
    use super::{
        apple_kind, kind_of, parse_chassis, parse_cpuinfo, parse_device_tree_model, parse_df_gib,
        parse_reg_string,
    };

    #[test]
    fn pci_vendor_ids_map_to_gpu_vendors() {
        assert_eq!(gpu_vendor("0x10de\n"), Some("NVIDIA"));
        assert_eq!(gpu_vendor("0x1002"), Some("AMD"));
        assert_eq!(gpu_vendor("0x8086"), Some("Intel"));
        assert_eq!(gpu_vendor("0x106b"), Some("Apple"));
        assert_eq!(gpu_vendor("0x1af4"), None);
    }

    const X86_CPUINFO: &str = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 9 5900X 12-Core Processor\n\
        physical id\t: 0\ncore id\t\t: 0\n\nprocessor\t: 1\nmodel name\t: AMD Ryzen 9 5900X 12-Core Processor\n\
        physical id\t: 0\ncore id\t\t: 1\n\nprocessor\t: 2\nmodel name\t: AMD Ryzen 9 5900X 12-Core Processor\n\
        physical id\t: 0\ncore id\t\t: 0\n\nprocessor\t: 3\nmodel name\t: AMD Ryzen 9 5900X 12-Core Processor\n\
        physical id\t: 0\ncore id\t\t: 1\n";

    #[test]
    fn cpuinfo_is_parsed_into_model_cores_and_threads() {
        // cpuinfo_counts_threads_and_distinct_cores: an x86 listing counts threads and distinct cores.
        {
            let cpu = parse_cpuinfo(X86_CPUINFO, None, 1);
            assert_eq!(
                cpu,
                Cpu {
                    model: "AMD Ryzen 9 5900X 12-Core Processor".into(),
                    cores: 2,
                    threads: 4
                }
            );
        }

        // arm_cpuinfo_falls_back_to_the_device_tree_model_then_hardware: the device tree model, then the Hardware line.
        {
            let arm = "processor\t: 0\nBogoMIPS\t: 108.00\nCPU implementer\t: 0x41\n\nprocessor\t: 1\nBogoMIPS\t: 108.00\n\
                Hardware\t: BCM2835\nModel\t\t: Raspberry Pi 4 Model B\n";
            let tree = parse_cpuinfo(arm, Some("Raspberry Pi 4 Model B Rev 1.4"), 4);
            assert_eq!(
                tree,
                Cpu {
                    model: "Raspberry Pi 4 Model B Rev 1.4".into(),
                    cores: 2,
                    threads: 2
                }
            );
            let hardware = parse_cpuinfo(arm, None, 4);
            assert_eq!(hardware.model, "BCM2835");
        }

        // cpuinfo_without_processors_reports_the_logical_count: no processors reports the logical count.
        {
            let cpu = parse_cpuinfo("", None, 6);
            assert_eq!(
                cpu,
                Cpu {
                    model: "unknown".into(),
                    cores: 6,
                    threads: 6
                }
            );
        }
    }

    #[test]
    fn device_tree_model_drops_the_nul_terminator() {
        assert_eq!(
            parse_device_tree_model(b"Raspberry Pi 5 Model B Rev 1.0\0").as_deref(),
            Some("Raspberry Pi 5 Model B Rev 1.0")
        );
        assert_eq!(parse_device_tree_model(b"\0"), None);
    }

    #[test]
    fn chassis_types_parse_as_numbers() {
        assert_eq!(parse_chassis("10\n"), Some(10));
        assert_eq!(parse_chassis(""), None);
    }

    #[test]
    fn kind_follows_the_chassis_table_then_form_factor() {
        // kind_follows_the_chassis_table: the chassis codes decide; a battery counts only when the chassis says nothing.
        {
            for chassis in [8, 9, 10, 14, 31, 32] {
                assert_eq!(
                    kind_of(false, false, Some(chassis), false),
                    "laptop",
                    "chassis {chassis}"
                );
            }
            for chassis in [17, 23, 25, 28] {
                assert_eq!(
                    kind_of(false, false, Some(chassis), false),
                    "server",
                    "chassis {chassis}"
                );
            }
            for chassis in [3, 4, 6, 7] {
                assert_eq!(
                    kind_of(false, false, Some(chassis), false),
                    "workstation",
                    "chassis {chassis}"
                );
            }
            assert_eq!(kind_of(false, false, None, false), "workstation");
            assert_eq!(
                kind_of(false, false, Some(17), true),
                "server",
                "the chassis wins over a battery (a UPS)"
            );
            assert_eq!(
                kind_of(false, false, Some(3), true),
                "workstation",
                "a desktop on a UPS"
            );
            assert_eq!(
                kind_of(false, false, None, true),
                "laptop",
                "a battery when the chassis says nothing"
            );
            assert_eq!(
                kind_of(false, false, Some(2), true),
                "laptop",
                "chassis 2 is Unknown"
            );
        }

        // kind_puts_phones_and_single_board_computers_first: a phone or an edge board wins over the chassis.
        {
            assert_eq!(kind_of(true, false, Some(10), true), "phone");
            assert_eq!(kind_of(false, true, Some(3), false), "edge");
        }
    }

    #[test]
    fn a_battery_marks_a_laptop() {
        let dir = std::env::temp_dir().join(format!("noded-power-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("AC")).unwrap();
        assert!(!has_battery(&dir));
        std::fs::create_dir_all(dir.join("BAT0")).unwrap();
        assert!(has_battery(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apple_models_with_book_are_laptops() {
        assert_eq!(apple_kind("MacBookPro18,3"), "laptop");
        assert_eq!(apple_kind("Macmini9,1"), "workstation");
        assert_eq!(apple_kind(""), "workstation");
    }

    #[test]
    fn reg_query_output_yields_the_named_string() {
        let out = "\nHKEY_LOCAL_MACHINE\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\n    ProcessorNameString    REG_SZ    Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz\n";
        assert_eq!(
            parse_reg_string(out, "ProcessorNameString").as_deref(),
            Some("Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz")
        );
        assert_eq!(parse_reg_string(out, "Identifier"), None);
    }

    #[test]
    fn df_table_gives_the_total_in_gib() {
        let out = "Filesystem     1024-blocks       Used Available Capacity Mounted on\n/dev/sda2      1953514584 1000000000 900000000      53% /\n";
        assert_eq!(parse_df_gib(out, 1), Some(1863), "the total");
        assert_eq!(parse_df_gib(out, 3), Some(858), "the available space");
        assert_eq!(parse_df_gib("Filesystem\n", 1), None);
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
        let transfer_port = std::env::var("APZ_TRANSFER_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(transfer::DEFAULT_PORT);
        let internal_token = config.internal_token.clone();
        if !internal_token.enabled() {
            println!("noded: no --internal-token-file: the HTTP API takes no token");
        }
        let vsockd_port = std::env::var("APZ_VSOCKD_HTTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(19080);
        collect::point_at_vsockd(vsockd_port, internal_token.header());
        let node = Arc::new(Node::new(config));
        if let Err(err) = node
            .transfers
            .listen(std::net::SocketAddr::from(([0, 0, 0, 0], transfer_port)))
        {
            eprintln!("noded: transfers are off: cannot listen on {transfer_port}: {err}");
        }
        stats::spawn(Arc::clone(&node.stats))?;
        spawn_metrics(Arc::clone(&node));
        // VM lifecycle events (boot, wake, sleep, snapshot …) go into the node's metrics for the hub.
        let events = Arc::clone(&node);
        registry::set_events(Box::new(move |e| events.metrics.event(e)));
        let (router, serving) = (router(), Arc::clone(&node));
        let http =
            server.spawn(internal_token.guard(move |request| router.handle(&serving, request)))?;
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

/// The `/v1` API: every route the node serves. Everything that changes state is a POST or DELETE.
fn router() -> Router<Node> {
    Router::<Node>::new()
        .get("/v1/machine", |node, _, _| {
            Response::json(200, &node.machine())
        })
        .get("/v1/ws", |node, request, _| {
            // Subscribed before the handshake answers, so no event between the
            // 101 and the socket's first read is lost.
            let subscription = node.events().subscribe();
            apz_http::ws::accept(
                request,
                apz_http::ws::Origins::None,
                Some("apz.noded.v1"),
                apz_http::ws::Limits::default(),
                move |mut socket| {
                    let _ = events::ws_stream(&subscription, &mut socket);
                },
            )
        })
        .get("/v1/machine/stats", |node, _, _| machine_stats(node))
        // Metrics for the hub (crates/noded/src/metrics.rs): the batch the node link takes, and live requests.
        .get("/v1/metrics/batch", |node, request, _| {
            metrics_batch(node, request)
        })
        .post("/v1/metrics/live", |node, request, _| {
            metrics_live(node, request)
        })
        // The node's Prometheus metrics (fixed statics for scraping and the lifecycle benchmark).
        .get("/v1/metrics", |_, _, _| {
            Response::new(200, "text/plain; version=0.0.4", crate::metrics::render())
        })
        // Whether this host suits scale mode (see `hostcheck`); the same report as `--check-scale`.
        .get("/v1/host/scale", |node, _, _| {
            let snapshots = node.registry().snapshot_root().ok();
            Response::json(200, &crate::hostcheck::report(snapshots).to_json())
        })
        // Shared data volumes: read-only images agent VMs mount at /data/<name>.
        .get("/v1/volumes", |node, _, _| list_volumes(node))
        .post("/v1/volumes", |node, request, _| {
            create_volume(node, &request.body)
        })
        .delete("/v1/volumes/{name}", |node, _, params| {
            delete_volume(node, params.get("name"))
        })
        // Warm templates: the ones built, and removing one (a VM restored from it keeps running).
        .get("/v1/templates", |node, _, _| {
            match node.registry().templates() {
                Some(templates) => Response::json(200, &templates.list()),
                None => Response::json(200, &Vec::<crate::templates::TemplateView>::new()),
            }
        })
        .delete("/v1/templates/{key}", |node, _, params| {
            match node.registry().templates() {
                Some(templates) => match templates.remove(params.get("key")) {
                    Ok(()) => Response::json(200, &serde_json::json!({ "removed": true })),
                    Err(err) => Response::error(400, &err.to_string()),
                },
                None => Response::error(409, "this node keeps no warm templates"),
            }
        })
        .get("/v1/runs", |node, _, _| list_runs(node))
        .post("/v1/runs", |node, request, _| {
            create_run(node, &request.body)
        })
        .get("/v1/workstations", |node, _, _| {
            Response::json(200, &node.workstations.list())
        })
        .post("/v1/workstations", |node, request, _| {
            workstation_create(node, &request.body)
        })
        .get("/v1/workstations/{id}", |node, _, params| {
            workstation_answer(node.workstations.view(params.get("id")))
        })
        .delete("/v1/workstations/{id}", |node, _, params| {
            workstation_answer(node.workstations.delete(params.get("id")))
        })
        .post("/v1/workstations/{id}/start", |node, _, params| {
            workstation_answer(node.workstations.start(params.get("id")))
        })
        // A plaza VM sleeps (its memory kept) and a start, or a wake, resumes it from there.
        .post("/v1/workstations/{id}/sleep", |node, _, params| {
            workstation_answer(node.workstations.sleep(params.get("id")))
        })
        .post("/v1/workstations/{id}/wake", |node, _, params| {
            workstation_answer(node.workstations.start(params.get("id")))
        })
        // The serial console's tail (`?bytes=`, default 16 KiB), as text.
        .get("/v1/workstations/{id}/console", |node, request, params| {
            let bytes = request
                .query_param("bytes")
                .and_then(|b| b.parse::<usize>().ok())
                .unwrap_or(16 * 1024)
                .min(1 << 20);
            match node.workstations.console(params.get("id"), bytes) {
                Ok(text) => Response::text(200, &text),
                Err((status, message)) => Response::error(status, &message),
            }
        })
        .post("/v1/workstations/{id}/stop", |node, _, params| {
            workstation_answer(node.workstations.stop(params.get("id")))
        })
        .post("/v1/workstations/{id}/snapshot", |node, _, params| {
            workstation_answer(node.workstations.snapshot(params.get("id")))
        })
        .post("/v1/workstations/{id}/revert", |node, _, params| {
            workstation_answer(node.workstations.revert(params.get("id")))
        })
        .get("/v1/runs/{id}", |node, _, params| {
            get_run(node, params.get("id"))
        })
        .delete("/v1/runs/{id}", |node, _, params| {
            delete_run(node, params.get("id"))
        })
        // The ports a run's forks serve: an inbound stream (vsockd, for the hub's terminal and model streams) is
        // allowed to a port only while a live run registered it.
        .post("/v1/runs/{id}/ports", |node, request, params| {
            register_port(node, params.get("id"), &request.body)
        })
        .delete("/v1/runs/{id}/ports/{cid}/{port}", |node, _, params| {
            unregister_port(
                node,
                params.get("id"),
                params.get("cid"),
                params.get("port"),
            )
        })
        .get("/v1/forks/{cid}/ports/{port}", |node, _, params| {
            served_port(node, params.get("cid"), params.get("port"))
        })
        .post("/v1/runs/{id}/input", |node, request, params| {
            send_input(node, params.get("id"), &request.body)
        })
        .post(
            "/v1/runs/{id}/vms/{index}/memory",
            |node, request, params| {
                set_vm_memory(node, params.get("id"), params.get("index"), &request.body)
            },
        )
        // One VM of a run: restart it as it was booted (same CID, same workspace image), reset
        // its workspace to the base image and restart it, or stop it for good.
        .post("/v1/runs/{id}/vms/{index}/restart", |node, _, params| {
            restart_vm(node, params.get("id"), params.get("index"))
        })
        .post("/v1/runs/{id}/vms/{index}/wipe", |node, _, params| {
            wipe_vm(node, params.get("id"), params.get("index"))
        })
        .post("/v1/runs/{id}/vms/{index}/stop", |node, _, params| {
            stop_vm(node, params.get("id"), params.get("index"))
        })
        // One VM's snapshots: pause and resume it; sleep it to a snapshot and wake it from there;
        // take checkpoints, restore one, delete one; fork the VM into new single-VM runs.
        .post("/v1/runs/{id}/vms/{index}/pause", |node, _, params| {
            vm_op(
                node,
                params.get("id"),
                params.get("index"),
                Registry::pause_vm,
            )
        })
        .post("/v1/runs/{id}/vms/{index}/resume", |node, _, params| {
            vm_op(
                node,
                params.get("id"),
                params.get("index"),
                Registry::resume_vm,
            )
        })
        // A dozing VM is a paused VM whose guest memory is paged out of host RAM; resume wakes it.
        .post("/v1/runs/{id}/vms/{index}/doze", |node, _, params| {
            doze_vm(node, params.get("id"), params.get("index"))
        })
        .post(
            "/v1/runs/{id}/vms/{index}/sleep",
            |node, request, params| {
                sleep_vm(node, params.get("id"), params.get("index"), &request.body)
            },
        )
        .post("/v1/runs/{id}/vms/{index}/wake", |node, request, params| {
            wake_vm(node, params.get("id"), params.get("index"), &request.body)
        })
        // A sleeping VM moves between nodes (migrate.rs): export it here and read its files in
        // chunks; import them on the target, which makes the VM asleep there. A finish is registered
        // before the chunk route below, which would match it too: the first match wins.
        .post("/v1/runs/{id}/vms/{index}/export", |node, _, params| {
            export_vm(node, params.get("id"), params.get("index"))
        })
        .get("/v1/exports/{export}/{name}", |node, request, params| {
            read_export(
                node,
                params.get("export"),
                params.get("name"),
                &request.query,
            )
        })
        .delete("/v1/exports/{export}", |node, _, params| {
            delete_export(node, params.get("export"))
        })
        .post(
            "/v1/runs/{id}/vms/{index}/checkpoints/{name}/manifest",
            |node, request, params| {
                checkpoint_manifest(
                    node,
                    params.get("id"),
                    params.get("index"),
                    params.get("name"),
                    &request.body,
                )
            },
        )
        .post("/v1/exports/{export}/manifest", |node, request, params| {
            migrate_answer(
                201,
                transfer::publish_export(
                    &node.transfers,
                    &node.migrations,
                    params.get("export"),
                    &request.body,
                ),
            )
        })
        .post("/v1/transfers/manifests", |node, request, _| {
            migrate_answer(200, transfer::publish_files(&node.transfers, &request.body))
        })
        .post("/v1/transfers/fetch", |node, request, _| {
            migrate_answer(
                200,
                transfer::fetch_manifest(&node.transfers, &request.body),
            )
        })
        .post("/v1/imports", |node, request, _| {
            create_import(node, &request.body)
        })
        .post("/v1/imports/from-transfer", |node, request, _| {
            migrate_answer(
                201,
                transfer::import_from_transfer(
                    &node.transfers,
                    &node.migrations,
                    node.registry(),
                    &request.body,
                ),
            )
        })
        .post("/v1/imports/{import}/finish", |node, _, params| {
            finish_import(node, params.get("import"))
        })
        .post("/v1/imports/{import}/{name}", |node, request, params| {
            write_import(
                node,
                params.get("import"),
                params.get("name"),
                &request.body,
            )
        })
        .delete("/v1/imports/{import}", |node, _, params| {
            abort_import(node, params.get("import"))
        })
        .get(
            "/v1/runs/{id}/vms/{index}/checkpoints",
            |node, _, params| checkpoint_list(node, params.get("id"), params.get("index")),
        )
        .post(
            "/v1/runs/{id}/vms/{index}/checkpoints",
            |node, request, params| {
                checkpoint_create(node, params.get("id"), params.get("index"), &request.body)
            },
        )
        .post(
            "/v1/runs/{id}/vms/{index}/checkpoints/{name}/restore",
            |node, _, params| {
                checkpoint_restore(
                    node,
                    params.get("id"),
                    params.get("index"),
                    params.get("name"),
                )
            },
        )
        .delete(
            "/v1/runs/{id}/vms/{index}/checkpoints/{name}",
            |node, _, params| {
                checkpoint_delete(
                    node,
                    params.get("id"),
                    params.get("index"),
                    params.get("name"),
                )
            },
        )
        .post("/v1/runs/{id}/vms/{index}/fork", |node, request, params| {
            fork_vm(node, params.get("id"), params.get("index"), &request.body)
        })
        // MCP servers in microVMs (mcp_vm.rs): whether this node can run one, a run per server, and a
        // server's kept workspace.
        .get("/v1/mcp-vm", |node, _, _| {
            mcp_vm::availability_answer(node.registry(), &node.mcp)
        })
        .post("/v1/mcp-runs", |node, request, _| {
            mcp_vm::create_run(node.registry(), &node.mcp, &request.body)
        })
        .get("/v1/mcp-runs/{id}", |node, _, params| {
            mcp_vm::get_run(&node.mcp, params.get("id"))
        })
        .delete("/v1/mcp-runs/{id}", |node, _, params| {
            mcp_vm::delete_run(&node.mcp, params.get("id"))
        })
        .delete("/v1/mcp-storage/{server}", |node, _, params| {
            mcp_vm::delete_storage(node.registry(), &node.mcp, params.get("server"))
        })
        // Trainer jobs: a native run per job, under the OS sandbox; see `train`.
        .get("/v1/train", |_, _, _| train::list())
        .post("/v1/train", |node, request, _| {
            train::start(node, &request.body)
        })
        .get("/v1/train/{job_id}/events", |_, request, params| {
            let after = request
                .query_param("after")
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            train::events(params.get("job_id"), after)
        })
        .post("/v1/train/{job_id}/stop", |_, _, params| {
            train::stop(params.get("job_id"))
        })
        .delete("/v1/train/{job_id}", |_, _, params| {
            train::delete(params.get("job_id"))
        })
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

/// A VM index from a route, or the 404 an index that is not a number gets, like an unknown VM.
fn vm_index(id: &str, index: &str) -> Result<u32, Response> {
    index
        .parse::<u32>()
        .map_err(|_| Response::error(404, &format!("no VM {index} in run {id}")))
}

/// `POST /v1/runs/{id}/vms/{index}/sleep` → 200 with `{"run": <summary>, "snapshot": <view>}`: the
/// VM is snapshotted to its sleep snapshot and stopped, and waits there until it is woken. The
/// optional body `{"memory": false}` stops it without a memory image instead: `snapshot` is `null`
/// and the VM wakes cold.
fn sleep_vm(node: &Node, id: &str, index: &str, body: &[u8]) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    let keep_memory = match memory_flag(body) {
        Ok(keep) => keep,
        Err(err) => return Response::error(400, &format!("invalid sleep request: {err}")),
    };
    match node.registry().sleep_vm(id, vm, keep_memory) {
        Ok((run, snapshot)) => Response::json(
            200,
            &serde_json::json!({"run": run.view(Detail::Summary), "snapshot": snapshot}),
        ),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// The optional `{"memory": bool}` body of a sleep or wake; a missing field means true.
#[derive(Debug, Deserialize)]
struct SleepWakeRequest {
    #[serde(default = "keeps_memory")]
    memory: bool,
}

fn keeps_memory() -> bool {
    true
}

/// Reads the optional memory flag of a sleep or wake body: true for an empty body.
fn memory_flag(body: &[u8]) -> Result<bool, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(true);
    }
    serde_json::from_slice::<SleepWakeRequest>(body)
        .map(|request| request.memory)
        .map_err(|err| err.to_string())
}

/// `POST /v1/runs/{id}/vms/{index}/wake` → 200 with the run view. The optional body
/// `{"memory": false}` boots the VM cold from its spec instead of restoring its sleep snapshot.
fn wake_vm(node: &Node, id: &str, index: &str, body: &[u8]) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    let cold = match memory_flag(body) {
        Ok(keep) => !keep,
        Err(err) => return Response::error(400, &format!("invalid wake request: {err}")),
    };
    match node.registry().wake_vm(id, vm, cold) {
        Ok((run, fallback)) => {
            let mut body = serde_json::to_value(run.view(Detail::Summary)).unwrap_or_default();
            if let Some(map) = body.as_object_mut() {
                map.insert("cold_fallback".into(), serde_json::Value::Bool(fallback));
            }
            Response::json(200, &body)
        }
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// Serializes a transfer step's answer: `status` with its value, or the error's status and message.
fn migrate_answer<T: Serialize>(status: u16, answer: Result<T, migrate::MigrateError>) -> Response {
    match answer {
        Ok(value) => Response::json(status, &value),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `POST /v1/runs/{id}/vms/{index}/export` → 201 with `{"export", "files", "meta"}`: the files of the
/// asleep VM, kept for an hour for chunked reads.
fn export_vm(node: &Node, id: &str, index: &str) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    migrate_answer(
        201,
        migrate::export_vm(&node.migrations, node.registry(), id, vm),
    )
}

/// `GET /v1/exports/{export}/{name}?offset=O&len=L` → 200 with `{"data": <base64>, "eof": bool}`.
fn read_export(node: &Node, export: &str, name: &str, query: &str) -> Response {
    migrate_answer(
        200,
        migrate::read_export(&node.migrations, export, name, query),
    )
}

/// `DELETE /v1/exports/{export}` → 200 with `{"deleted": export}`.
fn delete_export(node: &Node, export: &str) -> Response {
    migrate_answer(
        200,
        migrate::delete_export(&node.migrations, export)
            .map(|()| serde_json::json!({"deleted": export})),
    )
}

/// `POST /v1/imports` → 201 with `{"import": id}`: the VM's shape and file list, refused for a CID
/// a run of this node holds.
fn create_import(node: &Node, body: &[u8]) -> Response {
    migrate_answer(
        201,
        migrate::create_import(&node.migrations, node.registry(), body)
            .map(|import| serde_json::json!({"import": import})),
    )
}

/// `POST /v1/imports/{import}/{name}` → 200 with `{"import", "name", "written"}`.
fn write_import(node: &Node, import: &str, name: &str, body: &[u8]) -> Response {
    migrate_answer(
        200,
        migrate::write_import(&node.migrations, import, name, body),
    )
}

/// `POST /v1/imports/{import}/finish` → 201 with `{"run", "cid"}`: the VM is asleep on this node.
fn finish_import(node: &Node, import: &str) -> Response {
    migrate_answer(
        201,
        migrate::finish_import(&node.migrations, node.registry(), import),
    )
}

/// `DELETE /v1/imports/{import}` → 200 with `{"aborted": import}`.
fn abort_import(node: &Node, import: &str) -> Response {
    migrate_answer(
        200,
        migrate::abort_import(&node.migrations, import)
            .map(|()| serde_json::json!({"aborted": import})),
    )
}

/// `GET /v1/runs/{id}/vms/{index}/checkpoints` → 200 with `{"checkpoints": [<view>, ...]}`, newest first.
fn checkpoint_list(node: &Node, id: &str, index: &str) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    match node.registry().checkpoints(id, vm) {
        Ok(checkpoints) => Response::json(200, &serde_json::json!({"checkpoints": checkpoints})),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `POST /v1/runs/{id}/vms/{index}/checkpoints/{name}/manifest`: the checkpoint's files as blobs, offered to the receiver.
fn checkpoint_manifest(node: &Node, id: &str, index: &str, name: &str, body: &[u8]) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    match node.registry().checkpoint_dir(id, vm, name) {
        Ok(dir) => migrate_answer(
            200,
            transfer::publish_checkpoint(&node.transfers, &dir, body),
        ),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// The `POST .../checkpoints` body: a label for the checkpoint's name (optional).
#[derive(Debug, Default, Deserialize)]
struct CheckpointRequest {
    #[serde(default)]
    label: String,
    /// `false` keeps the VM's disk only: a restore or a fork of it boots cold from that disk.
    #[serde(default = "keep_memory")]
    memory: bool,
}

fn keep_memory() -> bool {
    true
}

/// `POST /v1/runs/{id}/vms/{index}/checkpoints` → 201 with the checkpoint's view. The body is
/// optional; `{"label": "..."}` names the checkpoint, and `"memory": false` keeps the disk only.
fn checkpoint_create(node: &Node, id: &str, index: &str, body: &[u8]) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    let (label, memory) = if body.iter().all(u8::is_ascii_whitespace) {
        (String::new(), true)
    } else {
        match serde_json::from_slice::<CheckpointRequest>(body) {
            Ok(request) => (request.label, request.memory),
            Err(err) => {
                return Response::error(400, &format!("invalid checkpoint request: {err}"));
            }
        }
    };
    match node.registry().checkpoint_vm(id, vm, &label, memory) {
        Ok(view) => Response::json(201, &view),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `POST /v1/runs/{id}/vms/{index}/checkpoints/{name}/restore` → 200 with
/// `{"run": <summary>, "snapshot": <view>}`, where `snapshot` is the `before-restore` checkpoint
/// taken first.
fn checkpoint_restore(node: &Node, id: &str, index: &str, name: &str) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    match node.registry().restore_checkpoint(id, vm, name) {
        Ok((run, snapshot)) => Response::json(
            200,
            &serde_json::json!({"run": run.view(Detail::Summary), "snapshot": snapshot}),
        ),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// `DELETE /v1/runs/{id}/vms/{index}/checkpoints/{name}` → 200 with `{"deleted": name}`.
fn checkpoint_delete(node: &Node, id: &str, index: &str, name: &str) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    match node.registry().delete_checkpoint(id, vm, name) {
        Ok(()) => Response::json(200, &serde_json::json!({"deleted": name})),
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
}

/// The `POST .../fork` body: how many forks to make (1 to 8), and optionally the name of an existing
/// checkpoint to fork from instead of taking a new one.
#[derive(Debug, Deserialize)]
struct ForkRequest {
    n: u32,
    checkpoint: Option<String>,
}

/// `GET /v1/volumes` → 200 with every data volume and the guest CIDs of the live VMs that mount it.
fn list_volumes(node: &Node) -> Response {
    let Some(store) = node.registry().volume_store() else {
        return Response::json(200, &Vec::<serde_json::Value>::new());
    };
    let used = node.registry().volumes_in_use();
    let listed: Vec<serde_json::Value> = store
        .list()
        .into_iter()
        .map(|volume| {
            let in_use = used.get(&volume.name).cloned().unwrap_or_default();
            serde_json::json!({
                "name": volume.name,
                "size_bytes": volume.size_bytes,
                "created_ms": volume.created_ms,
                "source": volume.source,
                "in_use": in_use,
            })
        })
        .collect();
    Response::json(200, &listed)
}

/// The `POST /v1/volumes` body: the volume's name and the absolute path, on this node, of an ext4
/// image (cloned) or a directory (packed into a new image).
#[derive(Debug, serde::Deserialize)]
struct CreateVolume {
    name: String,
    source: PathBuf,
}

/// The HTTP status of a refused volume operation.
fn volume_status(err: &volumes::VolumeError) -> u16 {
    match err {
        volumes::VolumeError::Invalid(_) => 400,
        volumes::VolumeError::NotFound(_) => 404,
        volumes::VolumeError::Exists(_) | volumes::VolumeError::InUse(_) => 409,
        volumes::VolumeError::Io(_) => 500,
    }
}

/// `POST /v1/volumes` → 201 with the new volume.
fn create_volume(node: &Node, body: &[u8]) -> Response {
    let Some(store) = node.registry().volume_store() else {
        return Response::error(409, "this node keeps no volumes");
    };
    let request: CreateVolume = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid volume request: {err}")),
    };
    match store.create(&request.name, &request.source) {
        Ok(view) => Response::json(201, &view),
        Err(err) => Response::error(volume_status(&err), &err.to_string()),
    }
}

/// `DELETE /v1/volumes/{name}` → 200 `{"removed": true}`; 409 while a live VM mounts it.
fn delete_volume(node: &Node, name: &str) -> Response {
    let Some(store) = node.registry().volume_store() else {
        return Response::error(409, "this node keeps no volumes");
    };
    let in_use = node.registry().volumes_in_use().contains_key(name);
    match store.remove(name, in_use) {
        Ok(()) => Response::json(200, &serde_json::json!({ "removed": true })),
        Err(err) => Response::error(volume_status(&err), &err.to_string()),
    }
}

/// `POST /v1/runs/{id}/vms/{index}/fork` → 201 with `{"forks": [{"run": id, "cid": n}, ...],
/// "checkpoint": name}`: a checkpoint of the VM, and one new single-VM run per fork.
fn fork_vm(node: &Node, id: &str, index: &str, body: &[u8]) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    let request: ForkRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid fork request: {err}")),
    };
    let from = request.checkpoint.as_deref();
    match node.registry().fork_vm(id, vm, request.n, from) {
        Ok((checkpoint, forks)) => {
            let forks: Vec<serde_json::Value> = forks
                .iter()
                .map(|(run, cid)| serde_json::json!({"run": run.id(), "cid": cid}))
                .collect();
            Response::json(
                201,
                &serde_json::json!({"forks": forks, "checkpoint": checkpoint.name}),
            )
        }
        Err(err) => Response::error(err.status(), &err.to_string()),
    }
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

/// `POST /v1/runs/{id}/vms/{index}/doze`: pauses the VM and pages its guest memory out of host RAM.
/// The answer is the run, as pause answers it, with `reclaimed_bytes` added.
fn doze_vm(node: &Node, id: &str, index: &str) -> Response {
    let vm = match vm_index(id, index) {
        Ok(vm) => vm,
        Err(response) => return response,
    };
    match node.registry().doze_vm(id, vm) {
        Ok((run, reclaimed)) => {
            let mut body = serde_json::to_value(run.view(Detail::Summary)).unwrap_or_default();
            if let Some(map) = body.as_object_mut() {
                map.insert("reclaimed_bytes".into(), serde_json::json!(reclaimed));
            }
            Response::json(200, &body)
        }
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

/// `POST /v1/workstations`: makes a workstation VM and boots it with the join code the hub issued (201 with its view).
fn workstation_create(node: &Node, body: &[u8]) -> Response {
    let request: workstation::CreateWorkstation = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid workstation request: {err}")),
    };
    match node.workstations.create(&request, epoch_ms()) {
        Ok(view) => Response::json(201, &view),
        Err((status, message)) => Response::error(status, &message),
    }
}

/// A workstation VM's answer: its view, or the refusal's status and message.
fn workstation_answer(result: Result<workstation::View, workstation::Refusal>) -> Response {
    match result {
        Ok(view) => Response::json(200, &view),
        Err((status, message)) => Response::error(status, &message),
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
/// `POST /v1/runs/{id}/ports` with `{"cid", "port"}`: a fork of the run serves that port on that guest.
fn register_port(node: &Node, id: &str, body: &[u8]) -> Response {
    let Ok(request) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Response::error(400, "a port request is JSON {cid, port}");
    };
    let (Some(cid), Some(port)) = (
        request["cid"].as_u64(),
        request["port"].as_u64().and_then(|p| u16::try_from(p).ok()),
    ) else {
        return Response::error(400, "a port request needs a cid and a port");
    };
    if node.registry().find(id).is_none() {
        return Response::error(404, &format!("no run with id {id}"));
    }
    match node.registry().register_port(id, cid, port) {
        Ok(()) => Response::json(
            200,
            &serde_json::json!({"cid": cid, "port": port, "served": true}),
        ),
        Err(err) => Response::error(409, &err),
    }
}

/// `DELETE /v1/runs/{id}/ports/{cid}/{port}`: the run's fork stops serving that port.
fn unregister_port(node: &Node, id: &str, cid: &str, port: &str) -> Response {
    let (Ok(cid), Ok(port)) = (cid.parse::<u64>(), port.parse::<u16>()) else {
        return Response::error(400, "a CID and a port");
    };
    let Some(run) = node.registry().find(id) else {
        return Response::error(404, &format!("no run with id {id}"));
    };
    Response::json(
        200,
        &serde_json::json!({"removed": run.unregister_port(cid, port)}),
    )
}

/// `GET /v1/forks/{cid}/ports/{port}`: 200 when a live run serves the port on that guest, 404 otherwise. vsockd asks
/// this before it dials a guest for an inbound stream.
fn served_port(node: &Node, cid: &str, port: &str) -> Response {
    let (Ok(cid), Ok(port)) = (cid.parse::<u64>(), port.parse::<u16>()) else {
        return Response::error(400, "a CID and a port");
    };
    if node.registry().port_served(cid, port) {
        Response::json(200, &serde_json::json!({"served": true}))
    } else {
        Response::json(404, &serde_json::json!({"served": false}))
    }
}

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
            // A deleted run's VMs take their snapshots with them.
            node.registry().drop_snapshots(&run);
            Response::json(200, &run.view(Detail::Full))
        }
        None => Response::error(404, &format!("no run with id {id}")),
    }
}

#[cfg(test)]
mod internal_token_tests {
    use super::*;

    #[test]
    fn a_token_file_admits_only_requests_that_carry_its_token() {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("apz-noded-token-{pid}"));
        std::fs::write(&path, "s3cret\n").expect("a token file");
        let root = std::env::temp_dir().join(format!("apz-noded-token-state-{pid}"));
        let server = Server::start(Config {
            port: 0,
            internal_token: apz_http::auth::InternalToken::file(&path),
            uds_dir: root.join("vsock"),
            workstation: workstation::Guest {
                dir: root.join("workstations"),
                ..workstation::Guest::default()
            },
            ..Config::default()
        })
        .expect("a server on a free port");
        let (port, timeout) = (server.port(), Duration::from_secs(5));
        let anonymous =
            apz_http::client::send(port, "GET", "/v1/runs", None, timeout).expect("an answer");
        assert_eq!(anonymous.status, 401);
        let carried = [(apz_http::auth::HEADER, "s3cret")];
        let admitted =
            apz_http::client::send_with(port, "GET", "/v1/runs", &carried, None, timeout)
                .expect("an answer");
        assert_eq!(admitted.status, 200);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod snapshot_route_tests {
    use super::*;

    /// A node that keeps its snapshots under `snapshot_dir` (none when `None`).
    fn node(snapshot_dir: Option<PathBuf>) -> Node {
        Node::new(Config {
            snapshot_dir,
            ..Config::default()
        })
    }

    #[test]
    fn snapshot_routes_refuse_bad_ids_bodies_and_unknown_runs() {
        // a_node_with_no_snapshot_dir_refuses_every_snapshot_route_with_409: no snapshot dir means 409 on every route.
        {
            let node = node(None);
            assert_eq!(sleep_vm(&node, "r1", "0", b"").status, 409);
            assert_eq!(checkpoint_list(&node, "r1", "0").status, 409);
            assert_eq!(checkpoint_create(&node, "r1", "0", b"").status, 409);
            assert_eq!(checkpoint_restore(&node, "r1", "0", "cp-1-x").status, 409);
            assert_eq!(checkpoint_delete(&node, "r1", "0", "cp-1-x").status, 409);
            assert_eq!(fork_vm(&node, "r1", "0", br#"{"n": 2}"#).status, 409);
        }

        // a_vm_index_that_is_not_a_number_is_a_404.
        {
            let node = node(None);
            assert_eq!(sleep_vm(&node, "r1", "x", b"").status, 404);
            assert_eq!(checkpoint_list(&node, "r1", "x").status, 404);
            assert_eq!(checkpoint_create(&node, "r1", "x", b"").status, 404);
            assert_eq!(fork_vm(&node, "r1", "x", br#"{"n": 1}"#).status, 404);
        }

        // a_malformed_snapshot_body_is_a_400.
        {
            let dir = std::env::temp_dir().join(format!("apz-noded-routes-{}", std::process::id()));
            let node = node(Some(dir));
            assert_eq!(checkpoint_create(&node, "r1", "0", b"{").status, 400);
            assert_eq!(fork_vm(&node, "r1", "0", b"not json").status, 400);
            assert_eq!(fork_vm(&node, "r1", "0", br#"{"n": 0}"#).status, 400);
            assert_eq!(fork_vm(&node, "r1", "0", br#"{"n": 9}"#).status, 400);
        }

        // a_snapshot_route_on_an_unknown_run_is_a_404.
        {
            let dir =
                std::env::temp_dir().join(format!("apz-noded-unknown-{}", std::process::id()));
            let node = node(Some(dir));
            assert_eq!(sleep_vm(&node, "r9", "0", b"").status, 404);
            assert_eq!(checkpoint_list(&node, "r9", "0").status, 404);
            assert_eq!(checkpoint_restore(&node, "r9", "0", "cp-1-x").status, 404);
            assert_eq!(checkpoint_delete(&node, "r9", "0", "cp-1-x").status, 404);
        }
    }

    /// A JSON response's body.
    fn body_json(response: &Response) -> serde_json::Value {
        match &response.body {
            apz_http::Body::Bytes(bytes) => serde_json::from_slice(bytes).expect("a JSON body"),
            other => panic!("not a byte body: {other:?}"),
        }
    }

    #[test]
    fn data_volumes_are_made_listed_and_removed_through_the_routes() {
        let root = std::env::temp_dir().join(format!("apz-noded-volumes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        let image = root.join("source.ext4");
        std::fs::write(&image, vec![0u8; 1 << 20]).expect("a 1 MiB image");
        let node = Node::new(Config {
            volume_dir: Some(root.join("volumes")),
            ..Config::default()
        });
        assert_eq!(body_json(&list_volumes(&node)), serde_json::json!([]));
        let body = serde_json::json!({"name": "datasets", "source": image}).to_string();
        assert_eq!(create_volume(&node, body.as_bytes()).status, 201);
        assert_eq!(
            create_volume(&node, body.as_bytes()).status,
            409,
            "the name is taken"
        );
        let bad = serde_json::json!({"name": "Bad Name", "source": image}).to_string();
        assert_eq!(create_volume(&node, bad.as_bytes()).status, 400);
        let listed = body_json(&list_volumes(&node));
        assert_eq!(listed[0]["name"], "datasets");
        assert_eq!(listed[0]["size_bytes"], 1 << 20);
        assert_eq!(listed[0]["in_use"], serde_json::json!([]));
        assert_eq!(delete_volume(&node, "datasets").status, 200);
        assert_eq!(delete_volume(&node, "datasets").status, 404);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_node_with_no_volume_dir_lists_none_and_refuses_to_make_one() {
        let node = node(None);
        assert_eq!(body_json(&list_volumes(&node)), serde_json::json!([]));
        assert_eq!(
            create_volume(&node, br#"{"name":"a","source":"/x"}"#).status,
            409
        );
    }

    #[test]
    fn the_sleep_wake_and_fork_bodies_default_to_memory_and_no_checkpoint() {
        assert!(memory_flag(b"").unwrap());
        assert!(memory_flag(b" \n").unwrap());
        assert!(memory_flag(b"{}").unwrap());
        assert!(!memory_flag(br#"{"memory": false}"#).unwrap());
        assert!(memory_flag(b"not json").is_err());
        let plain: ForkRequest = serde_json::from_slice(br#"{"n": 2}"#).unwrap();
        assert_eq!(plain.n, 2);
        assert!(plain.checkpoint.is_none());
        let named: ForkRequest =
            serde_json::from_slice(br#"{"n": 1, "checkpoint": "cp-1-x"}"#).unwrap();
        assert_eq!(named.checkpoint.as_deref(), Some("cp-1-x"));
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn normalize_vendor_gives_lower_case_short_names_and_hides_placeholders() {
        for (raw, want) in [
            ("Dell Inc.", Some("dell")),
            ("LENOVO", Some("lenovo")),
            ("ASUSTeK COMPUTER INC.", Some("asus")),
            ("Micro-Star International Co., Ltd.", Some("msi")),
            ("Gigabyte Technology Co., Ltd.", Some("gigabyte")),
            ("HP", Some("hp")),
            ("Hewlett-Packard", Some("hp")),
            ("Apple Inc.", Some("apple")),
            ("Raspberry Pi", Some("raspberrypi")),
            ("Raspberry Pi 5 Model B Rev 1.0", Some("raspberrypi")),
            ("samsung", Some("samsung")),
        ] {
            assert_eq!(normalize_vendor(raw).as_deref(), want, "{raw:?}");
        }
        for raw in [
            "System manufacturer",
            "To Be Filled By O.E.M.",
            "Default string",
            "  ",
            "Unknown",
            "",
        ] {
            assert_eq!(normalize_vendor(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn dmi_model_prefers_the_version_on_lenovo() {
        assert_eq!(
            dmi_model(Some("lenovo"), "20XW0000US\n", "ThinkPad X1 Carbon Gen 9\n").as_deref(),
            Some("ThinkPad X1 Carbon Gen 9")
        );
        assert_eq!(
            dmi_model(Some("lenovo"), "20XW0000US", "Not Defined").as_deref(),
            Some("20XW0000US")
        );
        assert_eq!(
            dmi_model(Some("dell"), "XPS 13 9310", "1.0").as_deref(),
            Some("XPS 13 9310")
        );
        assert_eq!(
            dmi_model(Some("asus"), "System Product Name", "System Version"),
            None
        );
    }

    #[test]
    fn windows_system_reads_memory_chassis_battery_and_maker() {
        // windows_desktop_gives_memory_chassis_and_maker: a desktop chassis with no battery.
        {
            // 16 GiB, a desktop chassis (3), no battery.
            let desktop = parse_windows_system("17179869184|3|0|Dell Inc.|XPS 8950\r\n");
            assert_eq!(
                desktop,
                WindowsSystem {
                    vendor: Some("dell".into()),
                    model: Some("XPS 8950".into()),
                    memory_mib: 16384,
                    chassis: Some(3),
                    battery: false
                }
            );
            assert_eq!(
                kind_of(false, false, desktop.chassis, desktop.battery),
                "workstation"
            );
        }

        // windows_laptop_with_a_battery_is_a_laptop: a notebook or convertible chassis with a battery.
        {
            // 32 GiB, a notebook chassis (10), one battery.
            let laptop = parse_windows_system("34359738368|10|1|Dell Inc.|XPS 13 9310\r\n");
            assert_eq!(
                laptop,
                WindowsSystem {
                    vendor: Some("dell".into()),
                    model: Some("XPS 13 9310".into()),
                    memory_mib: 32768,
                    chassis: Some(10),
                    battery: true
                }
            );
            assert_eq!(
                kind_of(false, false, laptop.chassis, laptop.battery),
                "laptop"
            );
            // A convertible (31) is a laptop too.
            let convertible = parse_windows_system("34359738368|31|1|LENOVO|21HM001QUS");
            assert_eq!(
                kind_of(false, false, convertible.chassis, convertible.battery),
                "laptop"
            );
        }

        // windows_server_chassis_is_a_server: a rack mount chassis is a server.
        {
            // 128 GiB, a rack mount chassis (23), no battery.
            let server = parse_windows_system("137438953472|23|0|Dell Inc.|PowerEdge R740\r\n");
            assert_eq!(server.memory_mib, 131072);
            assert_eq!(server.chassis, Some(23));
            assert_eq!(
                kind_of(false, false, server.chassis, server.battery),
                "server"
            );
        }

        // windows_placeholders_and_missing_fields_read_as_absent: placeholders and missing fields read as absent.
        {
            let placeholders =
                parse_windows_system("17179869184|3|0|System manufacturer|System Product Name");
            assert_eq!(
                placeholders,
                WindowsSystem {
                    vendor: None,
                    model: None,
                    memory_mib: 16384,
                    chassis: Some(3),
                    battery: false
                }
            );
            // An enclosure that reports no chassis, no memory figure and no probe output at all.
            assert_eq!(
                parse_windows_system("||0|Dell Inc.|XPS 13 9310").chassis,
                None
            );
            assert_eq!(
                parse_windows_system("|||Dell Inc.|XPS 13 9310").memory_mib,
                0
            );
            assert_eq!(
                parse_windows_system(""),
                WindowsSystem {
                    vendor: None,
                    model: None,
                    memory_mib: 0,
                    chassis: None,
                    battery: false
                }
            );
        }

        // a_windows_model_keeps_its_own_pipes: a model name with a pipe is kept whole.
        {
            let system = parse_windows_system("17179869184|3|0|Acme|Model A|B");
            assert_eq!(system.model.as_deref(), Some("Model A|B"));
        }
    }

    #[test]
    fn maker_pair_reads_android_properties_and_device_tree_models() {
        // android_properties_give_maker_and_model: getprop output gives the maker and the model.
        {
            // getprop prints the value and a newline; the manufacturer is lower-case on most devices.
            assert_eq!(
                maker_pair("samsung\n", "SM-S918B\n"),
                (Some("samsung".into()), Some("SM-S918B".into()))
            );
            assert_eq!(
                maker_pair("Google\n", "Pixel 8\n"),
                (Some("google".into()), Some("Pixel 8".into()))
            );
        }

        // a_device_tree_model_is_the_board_and_its_maker: the device tree model names the board and its maker.
        {
            let tree = parse_device_tree_model(b"Raspberry Pi 5 Model B Rev 1.0\0").unwrap();
            assert_eq!(
                maker_pair(&tree, &tree),
                (
                    Some("raspberrypi".into()),
                    Some("Raspberry Pi 5 Model B Rev 1.0".into())
                )
            );
        }
    }

    #[test]
    fn system_profiler_gives_the_marketing_model_name() {
        let text = "Hardware:\n\n    Hardware Overview:\n\n      Model Name: MacBook Pro\n      Model Identifier: Mac15,9\n      Chip: Apple M3 Max\n";
        assert_eq!(parse_model_name(text).as_deref(), Some("MacBook Pro"));
        assert_eq!(parse_model_name("      Model Identifier: Mac15,9\n"), None);
        assert_eq!(apple_kind(&parse_model_name(text).unwrap()), "laptop");
        assert_eq!(apple_kind("Mac Studio"), "workstation");
    }

    #[test]
    fn a_mac_model_carries_its_chip() {
        assert_eq!(
            apple_model("MacBook Pro", Some("Apple M3 Max")),
            "MacBook Pro (M3 Max)"
        );
        assert_eq!(
            apple_model("Mac Studio", Some("Apple M2 Ultra")),
            "Mac Studio (M2 Ultra)"
        );
        assert_eq!(apple_model("Mac mini", None), "Mac mini");
    }

    #[test]
    fn gpu_cores_come_from_the_displays_report() {
        let text = "Graphics/Displays:\n\n    Apple M3 Max:\n\n      Chipset Model: Apple M3 Max\n      Type: GPU\n      Bus: Built-In\n      Total Number of Cores: 40\n      Vendor: Apple (0x106b)\n";
        assert_eq!(parse_gpu_cores(text), Some(40));
        assert_eq!(parse_gpu_cores("      Chipset Model: Intel UHD\n"), None);
    }

    #[test]
    fn sysctl_counts_and_bytes_parse() {
        assert_eq!(parse_count("12\n"), Some(12));
        assert_eq!(parse_count("n/a"), None);
        assert_eq!(parse_bytes("137438953472\n"), Some(128 * GIB));
        assert_eq!(parse_bytes(""), None);
        assert_eq!(
            parse_bytes("137438953472").map(|bytes| bytes / MIB),
            Some(131072)
        );
    }

    #[test]
    fn windows_core_counts_are_physical_then_logical() {
        assert_eq!(parse_core_pair("12|24\r\n"), Some((12, 24)));
        assert_eq!(parse_core_pair("0|0"), None);
        assert_eq!(parse_core_pair("12"), None);
        assert_eq!(parse_core_pair(""), None);
    }
}

#[cfg(test)]
mod accelerator_tests {
    use super::*;

    const RTX_QUERY: &str = "0, NVIDIA GeForce RTX 3090, 24576, 5803, 36, 8.6\n";
    const TWO_QUERY: &str = "0, NVIDIA A100 80GB PCIe, 81920, 0, 0, 8.0\n1, NVIDIA GeForce RTX 4090, 24564, 12, 97, 8.9\n";
    const NV_TOPO: &str = "\tGPU0\tGPU1\tCPU Affinity\nGPU0\t X \tNV12\t0-23\nGPU1\tNV12\t X \t0-23\n\nLegend:\n  X    = Self\n  NV#  = Connection traversing bonded NVLinks\n";
    const PCIE_TOPO: &str = "\tGPU0\tCPU Affinity\nGPU0\t X \t0-23\n\nLegend:\n  NV#  = Connection traversing bonded NVLinks\n";

    #[test]
    fn nvidia_accelerators_parse_rows_links_and_names() {
        // nvidia_rows_parse_with_utilization_and_capability: a row gives name, memory, utilization and capability.
        {
            let devices = nvidia_accelerators(RTX_QUERY, PCIE_TOPO);
            assert_eq!(devices.len(), 1);
            let gpu = &devices[0];
            assert_eq!(gpu.name, "NVIDIA GeForce RTX 3090");
            assert_eq!(
                (gpu.index, gpu.memory_mib, gpu.mem_used_mib, gpu.util_pct),
                (0, Some(24576), Some(5803), Some(36))
            );
            assert_eq!(gpu.capability.as_deref(), Some("8.6"));
            assert_eq!(
                (gpu.vendor, gpu.backend, gpu.link, gpu.busy),
                ("nvidia", "cuda", "pcie", false)
            );
        }

        // nvlink_is_an_nv_number_in_the_matrix_not_the_legend: NV# in the matrix means NVLink, the legend does not.
        {
            let links: Vec<&str> = nvidia_accelerators(TWO_QUERY, NV_TOPO)
                .iter()
                .map(|device| device.link)
                .collect();
            assert_eq!(links, ["nvlink", "nvlink"]);
            assert_eq!(nvidia_accelerators(TWO_QUERY, PCIE_TOPO)[0].link, "pcie");
            assert!(topo_has_nvlink("GPU0 X \u{1b}[32mNV4\u{1b}[0m"));
            assert!(!topo_has_nvlink(
                "NV#  = Connection traversing bonded NVLinks"
            ));
        }

        // nvidia_names_keep_their_commas_and_unavailable_fields_are_null: names keep commas, [N/A] reads as null.
        {
            let devices = nvidia_accelerators("0, Weird, Name, 24576, 0, 0, [N/A]\n", "");
            assert_eq!(devices[0].name, "Weird, Name");
            assert_eq!(devices[0].capability, None);
            assert_eq!(devices[0].util_pct, Some(0));
            assert!(nvidia_accelerators("garbage\n", "").is_empty());
        }
    }

    #[test]
    fn amd_json_is_read_by_card_and_skips_other_keys() {
        const ROCM: &str = r#"{"card0": {"Card series": "Instinct MI250X", "Card model": "0x0c34", "VRAM Total Memory (B)": "137438953472", "VRAM Total Used Memory (B)": "1024"}, "card1": {"Card model": "0x7480", "VRAM Total Memory (B)": 25753026560}, "system": {"Driver version": "6.1"}}"#;
        let devices = amd_accelerators(ROCM);
        let seen: Vec<(u32, &str, Option<u64>)> = devices
            .iter()
            .map(|d| (d.index, d.name.as_str(), d.memory_mib))
            .collect();
        assert_eq!(
            seen,
            [
                (0, "Instinct MI250X", Some(131072)),
                (1, "0x7480", Some(24560))
            ]
        );
        assert_eq!((devices[0].vendor, devices[0].backend), ("amd", "rocm"));
        assert!(amd_accelerators("not json").is_empty());
        assert!(amd_accelerators("{}").is_empty());
    }

    #[test]
    fn intel_discrete_ids_are_the_arc_and_data_center_parts() {
        assert!(intel_discrete("0x56a0"));
        assert!(intel_discrete("0xe20b"));
        assert!(intel_discrete("0x0bd5"));
        assert!(!intel_discrete("0x9a49"), "integrated graphics");
        assert!(!intel_discrete("nonsense"));
    }

    #[test]
    fn lspci_names_drop_the_revision_and_drm_cards_are_numbered() {
        let named = name_from_lspci(
            "03:00.0 VGA compatible controller: Intel Corporation DG2 [Arc A770] (rev 08)",
        );
        assert_eq!(named.as_deref(), Some("Intel Corporation DG2 [Arc A770]"));
        assert_eq!(name_from_lspci("no colon here"), None);
        assert_eq!(card_index("card1"), Some(1));
        assert_eq!(card_index("card1-HDMI-A-1"), None);
        assert_eq!(card_index("renderD128"), None);
        assert_eq!(card_index("card"), None);
    }

    #[test]
    fn apple_gpu_is_one_mps_device_named_by_its_chip() {
        let apple = Apple {
            chip: "Apple M3 Max".into(),
            p_cores: Some(12),
            e_cores: Some(4),
            gpu_cores: Some(40),
            neural_cores: 16,
            unified_memory_gib: Some(64),
        };
        let gpu = apple_accelerator(&apple);
        assert_eq!(
            (gpu.index, gpu.vendor, gpu.backend, gpu.link),
            (0, "apple", "mps", "soc")
        );
        assert_eq!(
            (gpu.name.as_str(), gpu.memory_mib),
            ("Apple M3 Max", Some(65536))
        );
    }

    #[test]
    fn fabric_order_reads_ip_and_ifconfig_addresses() {
        // ip_json_addresses_drop_loopback_and_link_local_and_put_private_first: ip -j output.
        {
            let text = r#"[{"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1"},{"family":"inet6","local":"::1"}]},{"ifname":"docker0","addr_info":[{"family":"inet","local":"172.17.0.1"}]},{"ifname":"br-3f1e","addr_info":[{"family":"inet","local":"172.18.0.1"}]},{"ifname":"enp6s0","addr_info":[{"family":"inet","local":"8.8.8.8"},{"family":"inet","local":"192.168.1.20"},{"family":"inet6","local":"fe80::1"},{"family":"inet6","local":"2001:db8::5"},{"family":"inet6","local":"fd00::7"}]},{"ifname":"wg0","addr_info":[{"family":"inet","local":"10.8.0.3"},{"family":"inet","local":"169.254.9.9"}]}]"#;
            assert_eq!(
                fabric_order(parse_ip_json(text)),
                [
                    "10.8.0.3",
                    "192.168.1.20",
                    "fd00::7",
                    "8.8.8.8",
                    "2001:db8::5"
                ]
            );
        }

        // ifconfig_addresses_lose_their_zone_and_loopback: ifconfig output.
        {
            let text = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\n\tinet6 ::1 prefixlen 128\nen0: flags=8863<UP>\n\tinet6 fe80::1%en0 prefixlen 64 scopeid 0x4\n\tinet 192.168.0.12 netmask 0xffffff00 broadcast 192.168.0.255\n";
            let shared = "bridge100: flags=8863<UP>\n\tinet 192.168.2.1 netmask 0xffffff00\n";
            assert_eq!(
                fabric_order(parse_ifconfig(&format!("{text}{shared}"))),
                ["192.168.0.12"]
            );
        }
    }

    #[test]
    fn command_within_returns_the_output_and_kills_a_command_that_runs_too_long() {
        let done = command_within(
            "sh",
            &["-c", "printf hello; printf err >&2"],
            Duration::from_secs(5),
        )
        .expect("sh runs");
        assert!(done.ok);
        assert_eq!(
            (done.stdout.as_str(), done.stderr.as_str()),
            ("hello", "err")
        );
        assert!(command_within("sh", &["-c", "sleep 5"], Duration::from_millis(100)).is_none());
    }
}

/// Runs the collectors and scrapes the node's metrics every 10 s (2 s while the hub streams this node live).
fn spawn_metrics(node: Arc<Node>) {
    let _ = std::thread::Builder::new()
        .name("noded-metrics".into())
        .spawn(move || {
            loop {
                let now = metrics_now_ms();
                collect::sample(&node, &node.metrics, now);
                node.metrics.tick(now);
                std::thread::sleep(std::time::Duration::from_millis(
                    node.metrics.interval_ms(now),
                ));
            }
        });
}

fn metrics_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `GET /v1/metrics/batch[?live=1]`: what was collected since the last batch; with `live=1`, 204 unless the hub is
/// streaming this node.
fn metrics_batch(node: &Node, request: &apz_http::Request) -> Response {
    let now = metrics_now_ms();
    if request.query_param("live") == Some("1") && !node.metrics.is_live(now) {
        return Response::new(204, "text/plain", String::new());
    }
    Response::json(200, &node.metrics.batch(now))
}

/// `POST /v1/metrics/live {"until_ms": …}`: stream until then.
fn metrics_live(node: &Node, request: &apz_http::Request) -> Response {
    match request
        .json::<serde_json::Value>()
        .ok()
        .and_then(|v| v["until_ms"].as_u64())
    {
        Some(until) => {
            node.metrics.live(until);
            Response::json(200, &serde_json::json!({ "ok": true }))
        }
        None => Response::error(400, "until_ms is required"),
    }
}
