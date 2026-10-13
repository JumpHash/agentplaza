#![forbid(unsafe_code)]

//! `noded` — the node daemon: one machine's microVMs over a loopback HTTP API.
//!
//! * `GET /v1/machine` — the machine's facts
//! * `GET /v1/machine/stats` — live CPU, load, memory and VM counts, with history
//! * `GET /v1/runs` · `POST /v1/runs` — runs (groups of VMs) · boot one
//! * `GET /v1/runs/{id}` · `DELETE /v1/runs/{id}` — one run with its serial captures · kill it
//! * `POST /v1/runs/{id}/input` — write to one VM's serial console
//! * `POST /v1/runs/{id}/vms/{index}/memory` — resize one VM's balloon
//! * `GET /v1/ws` — a WebSocket (`apz.noded.v1`): one text message per event,
//!   `{"event":"run"|"vm"|"serial","data":{…}}`; read-only, no browsers
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


/// The Unix socket directory of the per-VM vsock sockets (VM `i`'s muxer binds `{dir}/vm{cid}`). The
/// config default is `apz_os::dirs::vsock_sockets()`, which is this on Linux and macOS and differs on Windows.
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
            uds_dir: apz_os::dirs::vsock_sockets(),
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

/// The daemon's state: its runs, when it started, and the machine facts probed once at start.
pub struct Node {
    registry: Registry,
    started_at_ms: u64,
    stats: Arc<stats::Stats>,
    probed: Probed,
}

/// `GET /v1/machine`.
#[derive(Debug, Serialize)]
pub struct MachineView {
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
    pub cpus: u32,
    pub memory_mib: u64,
    /// The size of the state directory's filesystem, in GiB; null when unknown.
    pub storage_gib: Option<u64>,
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
            probed: probe_host(&apz_os::dirs::state()),
        }
    }

    pub fn machine(&self) -> MachineView {
        let class = machine_class();
        MachineView {
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
            cpus: host_cpus(),
            memory_mib: self.probed.memory_mib,
            storage_gib: self.probed.storage_gib,
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

/// The host's total memory, in MiB (`MemTotal` from `/proc/meminfo`; `hw.memsize` on macOS).
fn host_memory_mib() -> u64 {
    if cfg!(target_os = "macos") {
        return sysctl("hw.memsize").and_then(|n| parse_bytes(&n)).map_or(0, |bytes| bytes / MIB);
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
        return Probed {
            kind,
            vendor,
            model,
            apple,
            cpu: probe_cpu(None),
            memory_mib: host_memory_mib(),
            storage_gib: storage_gib(state_dir),
        };
    }
    // Windows reads its system, enclosure and battery facts in one PowerShell call.
    let windows = cfg!(target_os = "windows").then(|| parse_windows_system(&powershell(WINDOWS_SYSTEM).unwrap_or_default()));
    let sbc = matches!(machine_class(), "pi4" | "pi5") || tree.as_deref().is_some_and(|model| model.contains("Raspberry Pi"));
    let chassis = match &windows {
        Some(system) => system.chassis,
        None => chassis_type(),
    };
    let battery = windows.as_ref().is_some_and(|system| system.battery) || has_battery(Path::new(POWER_SUPPLY_DIR));
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
        gpu_cores: command_stdout("system_profiler", &["SPDisplaysDataType"]).and_then(|text| parse_gpu_cores(&text)),
        neural_cores: neural_cores(&chip),
        unified_memory_gib: sysctl("hw.memsize").and_then(|n| parse_bytes(&n)).map(|bytes| bytes / GIB),
        chip,
    });
    (kind, Some("apple".to_string()), model, apple)
}

/// The maker and model of a machine that is neither a Mac nor a Windows computer: an Android
/// device's properties, an arm board's device-tree model, or a PC's DMI strings.
fn host_maker(tree: Option<&str>) -> (Option<String>, Option<String>) {
    if cfg!(target_os = "android") {
        let manufacturer = command_stdout("getprop", &["ro.product.manufacturer"]).unwrap_or_default();
        let model = command_stdout("getprop", &["ro.product.model"]).unwrap_or_default();
        return maker_pair(&manufacturer, &model);
    }
    if let Some(model) = tree {
        return (normalize_vendor(model), clean_model(model));
    }
    let read = |file: &str| std::fs::read_to_string(format!("{DMI_DIR}/{file}")).unwrap_or_default();
    let vendor = normalize_vendor(&read("sys_vendor"));
    let model = dmi_model(vendor.as_deref(), &read("product_name"), &read("product_version"));
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
    WindowsSystem { vendor, model, memory_mib: memory, chassis, battery }
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
    let key: String = first.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
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
    text.lines().find_map(|line| clean_model(line.trim().strip_prefix("Model Name:")?))
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
    if chip.contains("Ultra") {
        32
    } else {
        16
    }
}

/// The GPU core count from `system_profiler SPDisplaysDataType` (`Total Number of Cores: 40`).
fn parse_gpu_cores(text: &str) -> Option<u32> {
    text.lines().find_map(|line| parse_count(line.trim().strip_prefix("Total Number of Cores:")?))
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
    let text = String::from_utf8_lossy(bytes).trim_end_matches('\0').trim().to_string();
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
            cores: sysctl("hw.physicalcpu").and_then(|n| parse_count(&n)).unwrap_or(logical),
            threads: sysctl("hw.logicalcpu").and_then(|n| parse_count(&n)).unwrap_or(logical),
        };
    }
    if cfg!(target_os = "windows") {
        let model = command_stdout("reg", &["query", WINDOWS_CPU_KEY, "/v", "ProcessorNameString"])
            .and_then(|out| parse_reg_string(&out, "ProcessorNameString"));
        let (cores, threads) = powershell(WINDOWS_CORES).and_then(|out| parse_core_pair(&out)).unwrap_or((logical, logical));
        return Cpu { model: model.unwrap_or_else(|| "unknown".to_string()), cores, threads };
    }
    parse_cpuinfo(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(), tree, logical)
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
            (physical.unwrap_or("0").to_string(), core.map_or_else(|| index.to_string(), str::to_string))
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
    Cpu { model, cores, threads }
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
    if cfg!(target_os = "windows") {
        return None;
    }
    let target = if dir.exists() { dir } else { Path::new("/") };
    parse_df_gib(&command_stdout("df", &["-Pk", target.to_str()?])?)
}

/// The total of a `df -Pk` table: the second column (1024-byte blocks) of its first data line.
fn parse_df_gib(output: &str) -> Option<u64> {
    let kib: u64 = output.lines().nth(1)?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / (1024 * 1024))
}

fn sysctl(name: &str) -> Option<String> {
    command_stdout("sysctl", &["-n", name])
}

/// The trimmed stdout of a command that succeeds, or None. On Windows the probe starts without a
/// console window.
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::{apple_kind, kind_of, parse_chassis, parse_cpuinfo, parse_df_gib, parse_device_tree_model, parse_reg_string};
    use super::{gpu_vendor, has_battery, Cpu};

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
    fn cpuinfo_counts_threads_and_distinct_cores() {
        let cpu = parse_cpuinfo(X86_CPUINFO, None, 1);
        assert_eq!(cpu, Cpu { model: "AMD Ryzen 9 5900X 12-Core Processor".into(), cores: 2, threads: 4 });
    }

    #[test]
    fn arm_cpuinfo_falls_back_to_the_device_tree_model_then_hardware() {
        let arm = "processor\t: 0\nBogoMIPS\t: 108.00\nCPU implementer\t: 0x41\n\nprocessor\t: 1\nBogoMIPS\t: 108.00\n\
            Hardware\t: BCM2835\nModel\t\t: Raspberry Pi 4 Model B\n";
        let tree = parse_cpuinfo(arm, Some("Raspberry Pi 4 Model B Rev 1.4"), 4);
        assert_eq!(tree, Cpu { model: "Raspberry Pi 4 Model B Rev 1.4".into(), cores: 2, threads: 2 });
        let hardware = parse_cpuinfo(arm, None, 4);
        assert_eq!(hardware.model, "BCM2835");
    }

    #[test]
    fn cpuinfo_without_processors_reports_the_logical_count() {
        let cpu = parse_cpuinfo("", None, 6);
        assert_eq!(cpu, Cpu { model: "unknown".into(), cores: 6, threads: 6 });
    }

    #[test]
    fn device_tree_model_drops_the_nul_terminator() {
        assert_eq!(parse_device_tree_model(b"Raspberry Pi 5 Model B Rev 1.0\0").as_deref(), Some("Raspberry Pi 5 Model B Rev 1.0"));
        assert_eq!(parse_device_tree_model(b"\0"), None);
    }

    #[test]
    fn chassis_types_parse_as_numbers() {
        assert_eq!(parse_chassis("10\n"), Some(10));
        assert_eq!(parse_chassis(""), None);
    }

    #[test]
    fn kind_follows_the_chassis_table() {
        for chassis in [8, 9, 10, 14, 31, 32] {
            assert_eq!(kind_of(false, false, Some(chassis), false), "laptop", "chassis {chassis}");
        }
        for chassis in [17, 23, 25, 28] {
            assert_eq!(kind_of(false, false, Some(chassis), false), "server", "chassis {chassis}");
        }
        for chassis in [3, 4, 6, 7] {
            assert_eq!(kind_of(false, false, Some(chassis), false), "workstation", "chassis {chassis}");
        }
        assert_eq!(kind_of(false, false, None, false), "workstation");
        assert_eq!(kind_of(false, false, Some(17), true), "server", "the chassis wins over a battery (a UPS)");
        assert_eq!(kind_of(false, false, Some(3), true), "workstation", "a desktop on a UPS");
        assert_eq!(kind_of(false, false, None, true), "laptop", "a battery when the chassis says nothing");
        assert_eq!(kind_of(false, false, Some(2), true), "laptop", "chassis 2 is Unknown");
    }

    #[test]
    fn kind_puts_phones_and_single_board_computers_first() {
        assert_eq!(kind_of(true, false, Some(10), true), "phone");
        assert_eq!(kind_of(false, true, Some(3), false), "edge");
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
        assert_eq!(parse_reg_string(out, "ProcessorNameString").as_deref(), Some("Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz"));
        assert_eq!(parse_reg_string(out, "Identifier"), None);
    }

    #[test]
    fn df_table_gives_the_total_in_gib() {
        let out = "Filesystem     1024-blocks       Used Available Capacity Mounted on\n/dev/sda2      1953514584 1000000000 900000000      53% /\n";
        assert_eq!(parse_df_gib(out), Some(1863));
        assert_eq!(parse_df_gib("Filesystem\n"), None);
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


#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn vendors_are_lower_case_short_names() {
        assert_eq!(normalize_vendor("Dell Inc.").as_deref(), Some("dell"));
        assert_eq!(normalize_vendor("LENOVO").as_deref(), Some("lenovo"));
        assert_eq!(normalize_vendor("ASUSTeK COMPUTER INC.").as_deref(), Some("asus"));
        assert_eq!(normalize_vendor("Micro-Star International Co., Ltd.").as_deref(), Some("msi"));
        assert_eq!(normalize_vendor("Gigabyte Technology Co., Ltd.").as_deref(), Some("gigabyte"));
        assert_eq!(normalize_vendor("HP").as_deref(), Some("hp"));
        assert_eq!(normalize_vendor("Hewlett-Packard").as_deref(), Some("hp"));
        assert_eq!(normalize_vendor("Apple Inc.").as_deref(), Some("apple"));
        assert_eq!(normalize_vendor("Raspberry Pi").as_deref(), Some("raspberrypi"));
        assert_eq!(normalize_vendor("Raspberry Pi 5 Model B Rev 1.0").as_deref(), Some("raspberrypi"));
        assert_eq!(normalize_vendor("samsung").as_deref(), Some("samsung"));
    }

    #[test]
    fn placeholder_vendors_are_unknown() {
        for raw in ["System manufacturer", "To Be Filled By O.E.M.", "Default string", "  ", "Unknown", ""] {
            assert_eq!(normalize_vendor(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn dmi_model_prefers_the_version_on_lenovo() {
        assert_eq!(dmi_model(Some("lenovo"), "20XW0000US\n", "ThinkPad X1 Carbon Gen 9\n").as_deref(), Some("ThinkPad X1 Carbon Gen 9"));
        assert_eq!(dmi_model(Some("lenovo"), "20XW0000US", "Not Defined").as_deref(), Some("20XW0000US"));
        assert_eq!(dmi_model(Some("dell"), "XPS 13 9310", "1.0").as_deref(), Some("XPS 13 9310"));
        assert_eq!(dmi_model(Some("asus"), "System Product Name", "System Version"), None);
    }

    #[test]
    fn windows_desktop_gives_memory_chassis_and_maker() {
        // 16 GiB, a desktop chassis (3), no battery.
        let desktop = parse_windows_system("17179869184|3|0|Dell Inc.|XPS 8950\r\n");
        assert_eq!(
            desktop,
            WindowsSystem { vendor: Some("dell".into()), model: Some("XPS 8950".into()), memory_mib: 16384, chassis: Some(3), battery: false }
        );
        assert_eq!(kind_of(false, false, desktop.chassis, desktop.battery), "workstation");
    }

    #[test]
    fn windows_laptop_with_a_battery_is_a_laptop() {
        // 32 GiB, a notebook chassis (10), one battery.
        let laptop = parse_windows_system("34359738368|10|1|Dell Inc.|XPS 13 9310\r\n");
        assert_eq!(
            laptop,
            WindowsSystem { vendor: Some("dell".into()), model: Some("XPS 13 9310".into()), memory_mib: 32768, chassis: Some(10), battery: true }
        );
        assert_eq!(kind_of(false, false, laptop.chassis, laptop.battery), "laptop");
        // A convertible (31) is a laptop too.
        let convertible = parse_windows_system("34359738368|31|1|LENOVO|21HM001QUS");
        assert_eq!(kind_of(false, false, convertible.chassis, convertible.battery), "laptop");
    }

    #[test]
    fn windows_server_chassis_is_a_server() {
        // 128 GiB, a rack mount chassis (23), no battery.
        let server = parse_windows_system("137438953472|23|0|Dell Inc.|PowerEdge R740\r\n");
        assert_eq!(server.memory_mib, 131072);
        assert_eq!(server.chassis, Some(23));
        assert_eq!(kind_of(false, false, server.chassis, server.battery), "server");
    }

    #[test]
    fn windows_placeholders_and_missing_fields_read_as_absent() {
        let placeholders = parse_windows_system("17179869184|3|0|System manufacturer|System Product Name");
        assert_eq!(placeholders, WindowsSystem { vendor: None, model: None, memory_mib: 16384, chassis: Some(3), battery: false });
        // An enclosure that reports no chassis, no memory figure and no probe output at all.
        assert_eq!(parse_windows_system("||0|Dell Inc.|XPS 13 9310").chassis, None);
        assert_eq!(parse_windows_system("|||Dell Inc.|XPS 13 9310").memory_mib, 0);
        assert_eq!(
            parse_windows_system(""),
            WindowsSystem { vendor: None, model: None, memory_mib: 0, chassis: None, battery: false }
        );
    }

    #[test]
    fn a_windows_model_keeps_its_own_pipes() {
        let system = parse_windows_system("17179869184|3|0|Acme|Model A|B");
        assert_eq!(system.model.as_deref(), Some("Model A|B"));
    }

    #[test]
    fn android_properties_give_maker_and_model() {
        // getprop prints the value and a newline; the manufacturer is lower-case on most devices.
        assert_eq!(maker_pair("samsung\n", "SM-S918B\n"), (Some("samsung".into()), Some("SM-S918B".into())));
        assert_eq!(maker_pair("Google\n", "Pixel 8\n"), (Some("google".into()), Some("Pixel 8".into())));
    }

    #[test]
    fn a_device_tree_model_is_the_board_and_its_maker() {
        let tree = parse_device_tree_model(b"Raspberry Pi 5 Model B Rev 1.0\0").unwrap();
        assert_eq!(maker_pair(&tree, &tree), (Some("raspberrypi".into()), Some("Raspberry Pi 5 Model B Rev 1.0".into())));
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
        assert_eq!(apple_model("MacBook Pro", Some("Apple M3 Max")), "MacBook Pro (M3 Max)");
        assert_eq!(apple_model("Mac Studio", Some("Apple M2 Ultra")), "Mac Studio (M2 Ultra)");
        assert_eq!(apple_model("Mac mini", None), "Mac mini");
    }

    #[test]
    fn gpu_cores_come_from_the_displays_report() {
        let text = "Graphics/Displays:\n\n    Apple M3 Max:\n\n      Chipset Model: Apple M3 Max\n      Type: GPU\n      Bus: Built-In\n      Total Number of Cores: 40\n      Vendor: Apple (0x106b)\n";
        assert_eq!(parse_gpu_cores(text), Some(40));
        assert_eq!(parse_gpu_cores("      Chipset Model: Intel UHD\n"), None);
    }

    #[test]
    fn neural_engine_has_32_cores_on_ultra_chips_and_16_elsewhere() {
        assert_eq!(neural_cores("Apple M3 Max"), 16);
        assert_eq!(neural_cores("Apple M2 Ultra"), 32);
    }

    #[test]
    fn sysctl_counts_and_bytes_parse() {
        assert_eq!(parse_count("12\n"), Some(12));
        assert_eq!(parse_count("n/a"), None);
        assert_eq!(parse_bytes("137438953472\n"), Some(128 * GIB));
        assert_eq!(parse_bytes(""), None);
        assert_eq!(parse_bytes("137438953472").map(|bytes| bytes / MIB), Some(131072));
    }

    #[test]
    fn windows_core_counts_are_physical_then_logical() {
        assert_eq!(parse_core_pair("12|24\r\n"), Some((12, 24)));
        assert_eq!(parse_core_pair("0|0"), None);
        assert_eq!(parse_core_pair("12"), None);
        assert_eq!(parse_core_pair(""), None);
    }
}
