//! What the daemon reports about its machine: OS, arch, form, CPUs, memory, GPUs, displays and the account.
//!
//! Collection is per OS (`/proc`, `/sys` and the `nvidia-smi` tool on Linux; `sysctl`, `pmset`, `sw_vers` and
//! `system_profiler` on macOS; one PowerShell query on Windows). Every parser is a plain function over the text
//! a command or file gave, so the tests run on any host with fixtures. [`build`] turns the collected [`Raw`]
//! into [`Facts`] and holds no I/O.

#[cfg(target_os = "linux")]
use std::fs;
use std::io::Read;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use apz_station::{AccountMode, Display, Facts, Form, Gpu, Os};
use serde_json::Value;

const MIB: u64 = 1024 * 1024;
/// How long one collection command may run before it is killed and its fact left out.
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// The chassis codes (SMBIOS, as Linux's `chassis_type` and Windows' `ChassisTypes` give them) of each form.
const LAPTOP_CHASSIS: [u32; 8] = [8, 9, 10, 11, 14, 30, 31, 32];
const SERVER_CHASSIS: [u32; 5] = [17, 23, 25, 28, 29];
const MINI_CHASSIS: [u32; 2] = [35, 36];

/// The OS this build runs on.
pub fn host_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Macos
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    }
}

/// What the daemon knows that the machine does not say about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extras {
    pub account: AccountMode,
    /// The applications the desktop backend found (0 when there is no real backend).
    pub apps: u32,
    pub virtual_displays: bool,
    pub ui_tree: bool,
}

/// What collection found, before it is shaped into [`Facts`].
#[derive(Debug, Clone, PartialEq)]
pub struct Raw {
    pub os: Os,
    pub os_version: Option<String>,
    pub arch: String,
    pub hostname: String,
    pub cpus: u32,
    pub ram_mb: Option<u64>,
    /// The product name or model identifier (`MacBookPro18,3`, `Macmini9,1`, `VMware Virtual Platform`).
    pub model: String,
    pub vendor: String,
    pub chassis: Vec<u32>,
    pub battery: bool,
    pub hypervisor: bool,
    pub gpus: Vec<Gpu>,
    pub displays: Vec<Display>,
}

impl Raw {
    /// Nothing collected yet, for this OS.
    pub fn new(os: Os) -> Raw {
        Raw {
            os,
            os_version: None,
            arch: arch_name(std::env::consts::ARCH),
            hostname: String::new(),
            cpus: thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1),
            ram_mb: None,
            model: String::new(),
            vendor: String::new(),
            chassis: Vec::new(),
            battery: false,
            hypervisor: false,
            gpus: Vec::new(),
            displays: Vec::new(),
        }
    }
}

/// The machine's facts now: collected on this machine, shaped with `extras`.
pub fn gather(extras: &Extras) -> Facts {
    let mut facts = build(&collect(), extras);
    facts.machine_id = apz_os::machine::id();
    facts
}

/// The facts for collected `raw` and `extras`. Pure.
pub fn build(raw: &Raw, extras: &Extras) -> Facts {
    Facts {
        os: raw.os,
        os_version: raw
            .os_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        arch: raw.arch.clone(),
        hostname: raw.hostname.clone(),
        form: classify_form(raw),
        cpus: raw.cpus,
        ram_mb: raw.ram_mb.unwrap_or(0),
        gpus: raw.gpus.clone(),
        displays: raw.displays.clone(),
        account: extras.account.clone(),
        apps: extras.apps,
        virtual_displays: extras.virtual_displays,
        ui_tree: extras.ui_tree,
        // Set by `gather`, which reads the machine's own identifier; `build` is pure.
        machine_id: None,
    }
}

/// The form of the machine: a VM when a hypervisor or a virtual model shows, a laptop with a battery or a
/// laptop chassis, a server by chassis, a mini by chassis or the model name, else a desktop.
pub fn classify_form(raw: &Raw) -> Form {
    if raw.hypervisor || is_virtual(&raw.model, &raw.vendor) {
        Form::Vm
    } else if raw.battery
        || raw.chassis.iter().any(|c| LAPTOP_CHASSIS.contains(c))
        || raw.model.starts_with("MacBook")
    {
        Form::Laptop
    } else if raw.chassis.iter().any(|c| SERVER_CHASSIS.contains(c)) {
        Form::Server
    } else if raw.chassis.iter().any(|c| MINI_CHASSIS.contains(c))
        || raw.model.starts_with("Macmini")
        || raw.model.starts_with("MacStudio")
    {
        Form::Mini
    } else {
        Form::Desktop
    }
}

/// Whether a model or vendor name is a virtual machine's.
pub fn is_virtual(model: &str, vendor: &str) -> bool {
    let text = format!("{model} {vendor}").to_ascii_lowercase();
    [
        "virtual",
        "vmware",
        "kvm",
        "qemu",
        "xen",
        "parallels",
        "innotek",
        "bhyve",
        "standard pc",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

/// The GPU vendor a PCI vendor id names.
pub fn vendor_name(id: &str) -> &'static str {
    match id.trim().to_ascii_lowercase().as_str() {
        "0x10de" => "nvidia",
        "0x1002" => "amd",
        "0x8086" => "intel",
        "0x106b" => "apple",
        "0x1414" => "microsoft",
        _ => "unknown",
    }
}

/// The arch name the plaza uses (`arm64`, not `aarch64`).
pub fn arch_name(rust_arch: &str) -> String {
    match rust_arch {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

/// The `PRETTY_NAME` of `/etc/os-release`.
pub fn parse_os_release(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
}

/// `MemTotal` of `/proc/meminfo`, in MiB.
pub fn parse_meminfo_mb(text: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// A byte count (as `sysctl -n hw.memsize` or a WMI size prints it), in MiB.
pub fn parse_bytes_mb(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok().map(|bytes| bytes / MIB)
}

/// Whether `/proc/cpuinfo`'s flags name the `hypervisor` bit, which a guest CPU sets.
pub fn parse_cpuinfo_hypervisor(text: &str) -> bool {
    text.lines()
        .filter(|line| line.starts_with("flags"))
        .any(|line| line.split_whitespace().any(|flag| flag == "hypervisor"))
}

/// Whether `pmset -g batt` shows an internal battery.
pub fn parse_pmset_battery(text: &str) -> bool {
    text.contains("InternalBattery")
}

/// A `WxH` pair, with or without spaces around the `x` (`3024 x 1964`).
pub fn parse_pair(text: &str) -> Option<(u32, u32)> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let (w, h) = compact.split_once(['x', 'X'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// A size such as `16 GB` or `1536 MB`, in MiB; 0 when it is not one.
pub fn parse_size_mb(text: &str) -> u64 {
    let mut parts = text.split_whitespace();
    let Some(amount) = parts.next().and_then(|n| n.parse::<u64>().ok()) else {
        return 0;
    };
    match parts
        .next()
        .map(|unit| unit.to_ascii_uppercase())
        .as_deref()
    {
        Some("GB") => amount * 1024,
        Some("MB") => amount,
        Some("KB") => amount / 1024,
        _ => 0,
    }
}

/// `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`: one (name, MiB) per GPU.
pub fn parse_nvidia_smi(text: &str) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|line| {
            let (name, mib) = line.rsplit_once(',')?;
            let mib: u64 = mib.trim().parse().ok()?;
            Some((name.trim().to_string(), mib))
        })
        .collect()
}

/// `system_profiler SPDisplaysDataType -json`: the GPUs and the displays they drive.
pub fn parse_spdisplays(text: &str) -> (Vec<Gpu>, Vec<Display>) {
    let mut gpus = Vec::new();
    let mut displays = Vec::new();
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return (gpus, displays);
    };
    for card in value["SPDisplaysDataType"].as_array().into_iter().flatten() {
        let name = str_field(card, "sppci_model").unwrap_or_else(|| "GPU".to_string());
        let vendor = str_field(card, "spdisplays_vendor")
            .map(|v| v.trim_start_matches("sppci_vendor_").to_ascii_lowercase())
            .unwrap_or_else(|| "unknown".to_string());
        let vram_mb = str_field(card, "spdisplays_vram")
            .or_else(|| str_field(card, "spdisplays_vram_shared"))
            .map(|text| parse_size_mb(&text))
            .unwrap_or(0);
        gpus.push(Gpu {
            vendor,
            name,
            vram_mb,
        });
        for screen in card["spdisplays_ndrvs"].as_array().into_iter().flatten() {
            let Some((pixels_w, pixels_h)) =
                str_field(screen, "_spdisplays_pixels").and_then(|p| parse_pair(&p))
            else {
                continue;
            };
            let retina =
                str_field(screen, "spdisplays_retina").as_deref() == Some("spdisplays_yes");
            let scale = if retina { 2.0 } else { 1.0 };
            displays.push(Display {
                id: displays.len() as u32 + 1,
                w: (pixels_w as f32 / scale) as u32,
                h: (pixels_h as f32 / scale) as u32,
                scale,
            });
        }
    }
    (gpus, displays)
}

/// The Windows query's JSON (see [`WINDOWS_QUERY`]) applied to `raw`. Returns false when it is not that JSON.
pub fn apply_windows(raw: &mut Raw, text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    if let Some(version) = str_field(&value, "os_version") {
        raw.os_version = Some(version);
    }
    if let Some(model) = str_field(&value, "model") {
        raw.model = model;
    }
    if let Some(vendor) = str_field(&value, "manufacturer") {
        raw.vendor = vendor;
    }
    if let Some(bytes) = value["memory"].as_u64() {
        raw.ram_mb = Some(bytes / MIB);
    }
    raw.chassis = match &value["chassis"] {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect(),
        Value::Number(n) => n.as_u64().map(|n| vec![n as u32]).unwrap_or_default(),
        _ => Vec::new(),
    };
    raw.battery = value["battery"].as_u64().is_some_and(|count| count > 0);
    for card in many(value.get("video")) {
        let name = str_field(card, "Name").unwrap_or_else(|| "GPU".to_string());
        let vendor =
            vendor_from_compatibility(&str_field(card, "AdapterCompatibility").unwrap_or_default());
        let vram_mb = card["AdapterRAM"].as_u64().unwrap_or(0) / MIB;
        raw.gpus.push(Gpu {
            vendor,
            name,
            vram_mb,
        });
        let w = card["CurrentHorizontalResolution"].as_u64().unwrap_or(0);
        let h = card["CurrentVerticalResolution"].as_u64().unwrap_or(0);
        if w > 0 && h > 0 {
            raw.displays.push(Display {
                id: raw.displays.len() as u32 + 1,
                w: w as u32,
                h: h as u32,
                scale: 1.0,
            });
        }
    }
    true
}

/// The Windows query: one PowerShell expression that prints the facts as one line of JSON.
pub const WINDOWS_QUERY: &str = "$cs = Get-CimInstance Win32_ComputerSystem; \
$os = Get-CimInstance Win32_OperatingSystem; \
$enc = Get-CimInstance Win32_SystemEnclosure; \
$video = @(Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,AdapterCompatibility,CurrentHorizontalResolution,CurrentVerticalResolution); \
[pscustomobject]@{ os_version = $os.Version; model = $cs.Model; manufacturer = $cs.Manufacturer; \
memory = $cs.TotalPhysicalMemory; chassis = @($enc.ChassisTypes); battery = @(Get-CimInstance Win32_Battery).Count; \
video = $video } | ConvertTo-Json -Compress -Depth 4";

fn vendor_from_compatibility(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let vendor = if lower.contains("nvidia") {
        "nvidia"
    } else if lower.contains("amd") || lower.contains("advanced micro") {
        "amd"
    } else if lower.contains("intel") {
        "intel"
    } else if lower.contains("microsoft") {
        "microsoft"
    } else if lower.contains("apple") {
        "apple"
    } else if lower.is_empty() {
        "unknown"
    } else {
        return lower;
    };
    vendor.to_string()
}

/// A JSON field as a non-empty string.
fn str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// A JSON value as a list: an array's items, one object as itself, anything else as none.
fn many(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(item @ Value::Object(_)) => vec![item],
        _ => Vec::new(),
    }
}

/// Collects the host's facts (the OS-specific part).
#[cfg(target_os = "linux")]
fn collect() -> Raw {
    let mut raw = Raw::new(Os::Linux);
    raw.os_version = read_text("/etc/os-release")
        .and_then(|text| parse_os_release(&text))
        .or_else(|| {
            read_trim("/proc/sys/kernel/osrelease").map(|release| format!("Linux {release}"))
        });
    raw.hostname = read_trim("/proc/sys/kernel/hostname").unwrap_or_default();
    raw.ram_mb = read_text("/proc/meminfo").and_then(|text| parse_meminfo_mb(&text));
    raw.model = read_trim("/sys/class/dmi/id/product_name").unwrap_or_default();
    raw.vendor = read_trim("/sys/class/dmi/id/sys_vendor").unwrap_or_default();
    raw.chassis = read_trim("/sys/class/dmi/id/chassis_type")
        .and_then(|text| text.parse().ok())
        .into_iter()
        .collect();
    raw.battery = power_supply_has_battery();
    raw.hypervisor = read_text("/proc/cpuinfo").is_some_and(|text| parse_cpuinfo_hypervisor(&text));
    let nvidia = run(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ],
    )
    .map(|text| parse_nvidia_smi(&text))
    .unwrap_or_default();
    raw.gpus = drm_gpus(&nvidia);
    raw.displays = drm_displays();
    raw
}

/// Collects the host's facts (the OS-specific part).
#[cfg(target_os = "macos")]
fn collect() -> Raw {
    let mut raw = Raw::new(Os::Macos);
    raw.os_version = run("sw_vers", &["-productVersion"])
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    raw.hostname = run("scutil", &["--get", "ComputerName"])
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    raw.ram_mb = run("sysctl", &["-n", "hw.memsize"]).and_then(|text| parse_bytes_mb(&text));
    raw.model = run("sysctl", &["-n", "hw.model"])
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    raw.vendor = "Apple".to_string();
    raw.hypervisor = run("sysctl", &["-n", "kern.hv_vmm_present"]).is_some_and(|t| t.trim() == "1");
    raw.battery = run("pmset", &["-g", "batt"]).is_some_and(|text| parse_pmset_battery(&text));
    let (gpus, displays) = run("system_profiler", &["SPDisplaysDataType", "-json"])
        .map(|text| parse_spdisplays(&text))
        .unwrap_or_default();
    raw.gpus = gpus;
    raw.displays = displays;
    raw
}

/// Collects the host's facts (the OS-specific part).
#[cfg(windows)]
fn collect() -> Raw {
    let mut raw = Raw::new(Os::Windows);
    raw.hostname = std::env::var("COMPUTERNAME").unwrap_or_default();
    if let Some(text) = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_QUERY],
    ) {
        apply_windows(&mut raw, &text);
    }
    raw
}

/// Other platforms have no collector yet: the facts are the defaults.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn collect() -> Raw {
    Raw::new(host_os())
}

#[cfg(target_os = "linux")]
fn drm_cards() -> Vec<PathBuf> {
    sorted_entries("/sys/class/drm", |name| {
        name.strip_prefix("card")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    })
}

#[cfg(target_os = "linux")]
fn drm_gpus(nvidia: &[(String, u64)]) -> Vec<Gpu> {
    let mut names = nvidia.iter();
    let mut gpus = Vec::new();
    for card in drm_cards() {
        let device = card.join("device");
        let Some(vendor_id) = read_trim(device.join("vendor")) else {
            continue;
        };
        let vendor = vendor_name(&vendor_id).to_string();
        let nvidia_row = if vendor == "nvidia" {
            names.next()
        } else {
            None
        };
        if let Some((name, vram_mb)) = nvidia_row {
            gpus.push(Gpu {
                vendor,
                name: name.clone(),
                vram_mb: *vram_mb,
            });
            continue;
        }
        let vram_mb = read_trim(device.join("mem_info_vram_total"))
            .and_then(|text| text.parse::<u64>().ok())
            .map(|bytes| bytes / MIB)
            .unwrap_or(0);
        let name = match read_trim(device.join("device")) {
            Some(id) => format!("{vendor} GPU {id}"),
            None => format!("{vendor} GPU"),
        };
        gpus.push(Gpu {
            vendor,
            name,
            vram_mb,
        });
    }
    gpus
}

#[cfg(target_os = "linux")]
fn drm_displays() -> Vec<Display> {
    let outputs = sorted_entries("/sys/class/drm", |name| {
        name.starts_with("card") && name.contains('-')
    });
    let mut displays = Vec::new();
    for output in outputs {
        if read_trim(output.join("status")).as_deref() != Some("connected") {
            continue;
        }
        let mode = read_text(output.join("modes"))
            .and_then(|text| text.lines().next().and_then(parse_pair));
        if let Some((w, h)) = mode {
            displays.push(Display {
                id: displays.len() as u32 + 1,
                w,
                h,
                scale: 1.0,
            });
        }
    }
    displays
}

#[cfg(target_os = "linux")]
fn power_supply_has_battery() -> bool {
    sorted_entries("/sys/class/power_supply", |_| true)
        .iter()
        .any(|supply| read_trim(supply.join("type")).as_deref() == Some("Battery"))
}

#[cfg(target_os = "linux")]
fn sorted_entries(dir: &str, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_name().to_str().is_some_and(&keep))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

#[cfg(target_os = "linux")]
fn read_text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

#[cfg(target_os = "linux")]
fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Runs `program` with `args` and returns its stdout when it exits 0 within [`RUN_TIMEOUT`]. A command that
/// runs long is killed, and its fact is left out.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn run(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + RUN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader.join().ok()?;
                return status.success().then_some(text);
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> AccountMode {
        AccountMode::AgentUser {
            name: "agentws".into(),
        }
    }

    fn raw(os: Os) -> Raw {
        Raw {
            os,
            os_version: None,
            arch: "x86_64".into(),
            hostname: "box".into(),
            cpus: 8,
            ram_mb: Some(32768),
            model: String::new(),
            vendor: String::new(),
            chassis: Vec::new(),
            battery: false,
            hypervisor: false,
            gpus: Vec::new(),
            displays: Vec::new(),
        }
    }

    fn extras() -> Extras {
        Extras {
            account: account(),
            apps: 12,
            virtual_displays: true,
            ui_tree: false,
        }
    }

    #[test]
    fn os_release_gives_the_pretty_name() {
        let text = "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n";
        assert_eq!(parse_os_release(text).as_deref(), Some("Arch Linux"));
        assert_eq!(parse_os_release("ID=arch\n"), None);
    }

    #[test]
    fn meminfo_and_hw_memsize_give_mib() {
        assert_eq!(
            parse_meminfo_mb("MemTotal:       65536000 kB\nMemFree: 1 kB\n"),
            Some(64000)
        );
        assert_eq!(parse_meminfo_mb("MemFree: 1 kB\n"), None);
        assert_eq!(parse_bytes_mb("17179869184\n"), Some(16384));
        assert_eq!(parse_bytes_mb("lots"), None);
    }

    #[test]
    fn a_guest_cpu_shows_the_hypervisor_flag() {
        let guest = "processor\t: 0\nflags\t\t: fpu vme sse4_2 hypervisor avx\n";
        let host = "processor\t: 0\nflags\t\t: fpu vme sse4_2 avx vmx\n";
        assert!(parse_cpuinfo_hypervisor(guest));
        assert!(!parse_cpuinfo_hypervisor(host));
    }

    #[test]
    fn nvidia_smi_lists_name_and_mib_per_gpu() {
        assert_eq!(
            parse_nvidia_smi("NVIDIA GeForce RTX 4090, 24564\nNVIDIA RTX A4000, 16376\n"),
            vec![
                ("NVIDIA GeForce RTX 4090".to_string(), 24564),
                ("NVIDIA RTX A4000".to_string(), 16376),
            ]
        );
        assert!(parse_nvidia_smi("No devices were found").is_empty());
    }

    #[test]
    fn pci_vendors_and_virtual_names_are_recognised() {
        assert_eq!(vendor_name("0x10de"), "nvidia");
        assert_eq!(vendor_name("0x8086\n"), "intel");
        assert_eq!(vendor_name("0xdead"), "unknown");
        assert!(is_virtual("VMware Virtual Platform", "VMware, Inc."));
        assert!(is_virtual("Standard PC (Q35 + ICH9, 2009)", "QEMU"));
        assert!(is_virtual("VirtualBox", "innotek GmbH"));
        assert!(!is_virtual("MacBookPro18,3", "Apple"));
    }

    #[test]
    fn pairs_and_sizes_parse_with_or_without_spaces() {
        assert_eq!(parse_pair("3024 x 1964"), Some((3024, 1964)));
        assert_eq!(parse_pair("2560x1440"), Some((2560, 1440)));
        assert_eq!(parse_pair("big"), None);
        assert_eq!(parse_size_mb("16 GB"), 16384);
        assert_eq!(parse_size_mb("1536 MB"), 1536);
        assert_eq!(parse_size_mb("shared"), 0);
    }

    #[test]
    fn a_mac_reports_its_gpu_and_its_retina_display() {
        let json = r#"{"SPDisplaysDataType":[{"sppci_model":"Apple M4 Pro","spdisplays_vendor":"sppci_vendor_Apple","spdisplays_vram_shared":"24 GB","spdisplays_ndrvs":[{"_name":"Color LCD","_spdisplays_pixels":"3024 x 1964","spdisplays_retina":"spdisplays_yes"}]}]}"#;
        let (gpus, displays) = parse_spdisplays(json);
        assert_eq!(
            gpus,
            vec![Gpu {
                vendor: "apple".into(),
                name: "Apple M4 Pro".into(),
                vram_mb: 24576,
            }]
        );
        assert_eq!(
            displays,
            vec![Display {
                id: 1,
                w: 1512,
                h: 982,
                scale: 2.0,
            }]
        );
        assert_eq!(parse_spdisplays("not json"), (Vec::new(), Vec::new()));
    }

    #[test]
    fn windows_facts_are_read_from_one_json_line() {
        // windows_facts_come_from_one_json_line: a full report with two video cards.
        {
            let json = r#"{"os_version":"10.0.22631","model":"Precision 5690","manufacturer":"Dell Inc.","memory":34359738368,"chassis":[10],"battery":1,"video":[{"Name":"NVIDIA RTX 2000 Ada","AdapterRAM":4293918720,"AdapterCompatibility":"NVIDIA","CurrentHorizontalResolution":2560,"CurrentVerticalResolution":1600},{"Name":"Intel(R) Graphics","AdapterRAM":1073741824,"AdapterCompatibility":"Intel Corporation","CurrentHorizontalResolution":0,"CurrentVerticalResolution":0}]}"#;
            let mut machine = raw(Os::Windows);
            assert!(apply_windows(&mut machine, json));
            assert_eq!(machine.os_version.as_deref(), Some("10.0.22631"));
            assert_eq!(machine.model, "Precision 5690");
            assert_eq!(machine.vendor, "Dell Inc.");
            assert_eq!(machine.ram_mb, Some(32768));
            assert_eq!(machine.chassis, vec![10]);
            assert!(machine.battery);
            assert_eq!(machine.gpus.len(), 2);
            assert_eq!(machine.gpus[0].vendor, "nvidia");
            assert_eq!(machine.gpus[0].vram_mb, 4095);
            assert_eq!(machine.gpus[1].vendor, "intel");
            assert_eq!(
                machine.displays,
                vec![Display {
                    id: 1,
                    w: 2560,
                    h: 1600,
                    scale: 1.0,
                }]
            );
            assert!(!apply_windows(&mut machine, "not json"));
        }
        // a_single_windows_video_card_is_an_object_not_a_list: one card given as an object, not a list.
        {
            let json = r#"{"video":{"Name":"Microsoft Basic Display","AdapterRAM":0,"AdapterCompatibility":"Microsoft"}}"#;
            let mut machine = raw(Os::Windows);
            apply_windows(&mut machine, json);
            assert_eq!(machine.gpus.len(), 1);
            assert_eq!(machine.gpus[0].vendor, "microsoft");
            assert!(machine.displays.is_empty());
        }
    }

    #[test]
    fn the_form_follows_battery_chassis_and_model() {
        let mut laptop = raw(Os::Linux);
        laptop.battery = true;
        assert_eq!(classify_form(&laptop), Form::Laptop);

        let mut mac_book = raw(Os::Macos);
        mac_book.model = "MacBookPro18,3".into();
        assert_eq!(classify_form(&mac_book), Form::Laptop);

        let mut mini = raw(Os::Linux);
        mini.chassis = vec![35];
        assert_eq!(classify_form(&mini), Form::Mini);
        let mut mac_mini = raw(Os::Macos);
        mac_mini.model = "Macmini9,1".into();
        assert_eq!(classify_form(&mac_mini), Form::Mini);

        let mut server = raw(Os::Linux);
        server.chassis = vec![17];
        assert_eq!(classify_form(&server), Form::Server);

        let mut vm = raw(Os::Linux);
        vm.battery = true;
        vm.hypervisor = true;
        assert_eq!(classify_form(&vm), Form::Vm, "a VM beats the battery");

        let mut tower = raw(Os::Linux);
        tower.chassis = vec![3];
        assert_eq!(classify_form(&tower), Form::Desktop);
    }

    #[test]
    fn build_passes_the_collected_facts_and_the_extras_through() {
        let mut machine = raw(Os::Linux);
        machine.os_version = Some("Arch Linux".into());
        machine.gpus = vec![Gpu {
            vendor: "nvidia".into(),
            name: "RTX 4090".into(),
            vram_mb: 24564,
        }];
        let facts = build(&machine, &extras());
        assert_eq!(facts.os, Os::Linux);
        assert_eq!(facts.os_version, "Arch Linux");
        assert_eq!(facts.hostname, "box");
        assert_eq!(facts.form, Form::Desktop);
        assert_eq!(facts.cpus, 8);
        assert_eq!(facts.ram_mb, 32768);
        assert_eq!(facts.gpus.len(), 1);
        assert_eq!(facts.account, account());
        assert_eq!(facts.apps, 12);
        assert!(facts.virtual_displays);
        assert!(!facts.ui_tree);

        let unknown = build(&raw(Os::Linux), &extras());
        assert_eq!(unknown.os_version, "unknown");
    }

    #[test]
    fn arch_names_follow_the_plaza() {
        assert_eq!(arch_name("aarch64"), "arm64");
        assert_eq!(arch_name("x86_64"), "x86_64");
    }
}
