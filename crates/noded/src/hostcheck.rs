//! Whether this host suits scale mode: thousands of mostly idle agents on one node.
//!
//! [`report`] probes the host and says, item by item, whether it is fit: zswap compresses cold
//! guest pages in RAM, swap is zram or encrypted, the snapshot directory takes reflinks, the VMs'
//! cgroups can be created, and KSM is off (it merges pages across VMs, a timing side channel).
//! Each probe is a pure function over the text it reads, so the parsing is tested without root or
//! sysfs; the file reads are a thin wrapper. Served on `GET /v1/host/scale` and printed by
//! `noded --check-scale`.

// The probes read Linux's sysfs and procfs; elsewhere they are compiled for their tests only.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use vmm::cgroup::{SWAP_ENV, SwapPolicy, swap_policy_from};

const ZSWAP_ADVICE: &str = "enable zswap (zswap.enabled=1 on the kernel command line, or echo Y > /sys/module/zswap/parameters/enabled) so cold guest pages compress in RAM";
const SWAP_ADVICE: &str = "guest memory holds agent secrets: use zram swap, or swap onto an encrypted (dm-crypt) device; no swap, or unencrypted swap, is not fit";
#[cfg(not(target_os = "macos"))]
const REFLINK_ADVICE: &str = "put the snapshot directory on XFS, Btrfs or bcachefs: without reflinks, sleeps, forks and workspaces copy whole files";
#[cfg(target_os = "macos")]
const REFLINK_ADVICE: &str = "put the snapshot directory on APFS: without clonefile, sleeps, forks and workspaces copy whole files";
const CGROUP_ADVICE: &str =
    "delegate the cpu, memory and pids controllers to the user manager (systemd user@ delegation)";
const KSM_ADVICE: &str = "KSM merges identical pages across VMs, a cross-VM timing side channel: write 0 to /sys/kernel/mm/ksm/run";
#[cfg(target_os = "macos")]
const HVF_ADVICE: &str = "sign apz-vmm with the hypervisor entitlement (codesign -s - --entitlements scripts/hvf-entitlements.plist -f apz-vmm): without it no VM boots";
const VM_SWAP_ADVICE: &str =
    "zswap is on: set APZ_VMM_SWAP=ram so a VM's cold pages can be compressed instead of held";

/// One probed item of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// The item's name, as the report and the API spell it.
    pub name: &'static str,
    /// What the probe found.
    pub value: String,
    /// Whether the host is fit in this item.
    pub ok: bool,
    /// One line on what to do: set when the item is not ok, and for an item that is ok but
    /// worth acting on.
    pub advice: Option<String>,
}

impl Check {
    /// An item that is fit, or not: the advice is kept only when it is not fit.
    fn fit(name: &'static str, value: impl Into<String>, ok: bool, advice: &str) -> Self {
        Self {
            name,
            value: value.into(),
            ok,
            advice: (!ok).then(|| advice.to_string()),
        }
    }

    /// An item this platform cannot probe: fit, by not being a concern here.
    #[cfg(not(target_os = "linux"))]
    fn not_applicable(name: &'static str) -> Self {
        Self {
            name,
            value: "not applicable".to_string(),
            ok: true,
            advice: None,
        }
    }

    /// The item as a JSON object: `{"name", "value", "ok", "advice"}`.
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "value": self.value,
            "ok": self.ok,
            "advice": self.advice,
        })
    }

    /// The item as one report line: `ok|warn <name>: <value>`, then ` — <advice>` when there is one.
    fn line(&self) -> String {
        let level = if self.ok { "ok" } else { "warn" };
        match &self.advice {
            Some(advice) => format!("{level} {}: {} — {advice}", self.name, self.value),
            None => format!("{level} {}: {}", self.name, self.value),
        }
    }
}

/// The host's report: every item, in the order they are probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The probed items.
    pub checks: Vec<Check>,
}

impl Report {
    /// Whether every item is fit.
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }

    /// The report as `{"ok": all_ok, "checks": [...]}`.
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "checks": self.checks.iter().map(Check::to_json).collect::<Vec<_>>(),
        })
    }

    /// One line per item (see [`Check`]'s line format).
    pub fn lines(&self) -> Vec<String> {
        self.checks.iter().map(Check::line).collect()
    }
}

/// Probes this host. `snapshot_dir` is the directory the VMs' snapshots live in, `None` when the
/// node keeps none. Linux-only probes answer "not applicable" elsewhere.
pub fn report(snapshot_dir: Option<&Path>) -> Report {
    Report {
        checks: checks(snapshot_dir),
    }
}

#[cfg(target_os = "linux")]
fn checks(snapshot_dir: Option<&Path>) -> Vec<Check> {
    let zswap = read_optional("/sys/module/zswap/parameters/enabled");
    let zswap_on = zswap.as_deref().is_some_and(zswap_enabled);
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let proc_swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    let env = std::env::var(SWAP_ENV).ok();
    vec![
        zswap_check(zswap.as_deref()),
        swap_check(&meminfo, &proc_swaps, |dm| {
            read_optional(&format!("/sys/block/{dm}/dm/uuid"))
        }),
        reflink_check(snapshot_dir),
        cgroups_check(
            vmm::cgroup::CgroupRoot::detect()
                .as_ref()
                .map(|root| root.path()),
        ),
        ksm_check(read_optional("/sys/kernel/mm/ksm/run").as_deref()),
        vm_swap_check(env.as_deref(), zswap_on),
    ]
}

#[cfg(not(target_os = "linux"))]
fn checks(snapshot_dir: Option<&Path>) -> Vec<Check> {
    let env = std::env::var(SWAP_ENV).ok();
    let mut checks = vec![
        Check::not_applicable("zswap"),
        Check::not_applicable("swap"),
        platform_reflink(snapshot_dir),
        Check::not_applicable("cgroups"),
        Check::not_applicable("ksm"),
    ];
    checks.extend(platform_hvf());
    checks.push(vm_swap_check(env.as_deref(), false));
    checks
}

/// The reflink item where the copies are clones: APFS `clonefile` on macOS.
#[cfg(target_os = "macos")]
fn platform_reflink(snapshot_dir: Option<&Path>) -> Check {
    reflink_check(snapshot_dir)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_reflink(_snapshot_dir: Option<&Path>) -> Check {
    Check::not_applicable("reflink")
}

/// The Hypervisor.framework item on Apple Silicon; no item elsewhere.
#[cfg(target_os = "macos")]
fn platform_hvf() -> Vec<Check> {
    vec![hvf_check()]
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_hvf() -> Vec<Check> {
    Vec::new()
}

/// The Hypervisor.framework item (Apple Silicon): every VM is an `apz-vmm` process next to this
/// daemon, and each one fails its boot with `HV_DENIED` unless it carries the hypervisor entitlement.
#[cfg(target_os = "macos")]
fn hvf_check() -> Check {
    let child = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("apz-vmm")));
    let shown = child
        .as_deref()
        .map_or_else(|| "apz-vmm".to_string(), |path| path.display().to_string());
    let entitled = child.as_deref().is_some_and(vm_process_entitled);
    let value = if entitled {
        format!("{shown} carries com.apple.security.hypervisor")
    } else {
        format!("{shown} lacks com.apple.security.hypervisor")
    };
    Check::fit("hvf", value, entitled, HVF_ADVICE)
}

/// Whether the binary at `path` is signed with the hypervisor entitlement, read back with
/// `codesign`. A binary that cannot be read, or that `codesign` cannot read, is not entitled.
#[cfg(target_os = "macos")]
fn vm_process_entitled(path: &Path) -> bool {
    std::process::Command::new("codesign")
        .args(["-d", "--entitlements", ":-"])
        .arg(path)
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| hypervisor_entitled(&String::from_utf8_lossy(&output.stdout)))
}

/// Whether an entitlements plist (what `codesign -d --entitlements :-` prints) grants the hypervisor
/// entitlement: the key is present and its value is `true`.
pub fn hypervisor_entitled(plist: &str) -> bool {
    plist
        .split("<key>com.apple.security.hypervisor</key>")
        .nth(1)
        .is_some_and(|rest| rest.trim_start().starts_with("<true/>"))
}

/// A file's text, or `None` when it cannot be read (absent, or not for this user).
#[cfg(target_os = "linux")]
fn read_optional(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Whether zswap is enabled, from the text of `/sys/module/zswap/parameters/enabled` (`Y` or `N`).
pub fn zswap_enabled(text: &str) -> bool {
    text.trim() == "Y"
}

/// The zswap item. `text` is the parameter's contents, `None` when the module is not there.
pub fn zswap_check(text: Option<&str>) -> Check {
    match text {
        Some(text) => Check::fit("zswap", text.trim(), zswap_enabled(text), ZSWAP_ADVICE),
        None => Check::fit(
            "zswap",
            "unavailable (no zswap module)",
            false,
            ZSWAP_ADVICE,
        ),
    }
}

/// The `SwapTotal` of `/proc/meminfo`, in KiB.
pub fn swap_total_kib(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("SwapTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
}

/// The swap devices of `/proc/swaps`: the first field of every line after the header.
pub fn swap_devices(proc_swaps: &str) -> Vec<&str> {
    proc_swaps
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

/// What a swap device is, for the encryption rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapKind {
    /// A zram device: compressed RAM, never written to disk.
    Zram,
    /// A device-mapper device whose uuid is `CRYPT-` (dm-crypt, LUKS): encrypted on disk.
    Crypt,
    /// Anything else: a disk partition, a file, an unencrypted device-mapper device.
    Plain,
}

/// The name `dm-N` of a `/dev/dm-N` device, `None` for any other path.
fn dm_name(device: &str) -> Option<&str> {
    let name = device.strip_prefix("/dev/")?;
    let digits = name.strip_prefix("dm-")?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(name)
}

/// The kind of swap `device` is. `dm_uuid` is the uuid of its device-mapper device, read by the
/// caller from `/sys/block/dm-N/dm/uuid`, when the device is one.
pub fn swap_kind(device: &str, dm_uuid: Option<&str>) -> SwapKind {
    if device.starts_with("/dev/zram") {
        SwapKind::Zram
    } else if dm_name(device).is_some()
        && dm_uuid.is_some_and(|uuid| uuid.trim().starts_with("CRYPT-"))
    {
        SwapKind::Crypt
    } else {
        SwapKind::Plain
    }
}

/// The swap item: fit when swap is on (`SwapTotal` above zero) and every device is zram or
/// dm-crypt. `meminfo` and `proc_swaps` are the two files' text; `dm_uuid` maps a `dm-N` name to
/// its uuid, `None` when it has none.
pub fn swap_check(
    meminfo: &str,
    proc_swaps: &str,
    dm_uuid: impl Fn(&str) -> Option<String>,
) -> Check {
    let total = swap_total_kib(meminfo).unwrap_or(0);
    let kinds: Vec<(&str, SwapKind)> = swap_devices(proc_swaps)
        .into_iter()
        .map(|device| {
            let uuid = dm_name(device).and_then(&dm_uuid);
            (device, swap_kind(device, uuid.as_deref()))
        })
        .collect();
    let listed: Vec<String> = kinds
        .iter()
        .map(|(device, kind)| {
            let label = match kind {
                SwapKind::Zram => "zram",
                SwapKind::Crypt => "dm-crypt",
                SwapKind::Plain => "unencrypted",
            };
            format!("{device} ({label})")
        })
        .collect();
    let all_protected = !kinds.is_empty() && kinds.iter().all(|(_, kind)| *kind != SwapKind::Plain);
    let value = if listed.is_empty() {
        format!("SwapTotal {total} KiB, no swap devices")
    } else {
        format!("SwapTotal {total} KiB: {}", listed.join(", "))
    };
    Check::fit("swap", value, total > 0 && all_protected, SWAP_ADVICE)
}

/// The reflink item for `dir`: fit when `dir` exists and takes reflinks. `is_dir` and `supported`
/// are the probe's answers for it (see [`reflink_check`]).
pub fn reflink_outcome(dir: &Path, is_dir: bool, supported: bool) -> Check {
    let shown = dir.display();
    if !is_dir {
        return Check::fit(
            "reflink",
            format!("{shown} does not exist"),
            false,
            REFLINK_ADVICE,
        );
    }
    let value = if supported {
        format!("reflink in {shown}")
    } else {
        format!("sparse copy in {shown}")
    };
    Check::fit("reflink", value, supported, REFLINK_ADVICE)
}

/// The reflink item: probes `dir` with two scratch files, as the disk copies do. No snapshot
/// directory is not fit.
pub fn reflink_check(dir: Option<&Path>) -> Check {
    match dir {
        None => Check::fit("reflink", "no snapshot directory", false, REFLINK_ADVICE),
        Some(dir) => {
            let is_dir = dir.is_dir();
            let supported = is_dir && vmm::clonefile::reflink_supported(dir);
            reflink_outcome(dir, is_dir, supported)
        }
    }
}

/// The cgroups item: fit when a delegated cgroup v2 tree was found. `detected` is its path.
pub fn cgroups_check(detected: Option<&Path>) -> Check {
    match detected {
        Some(root) => Check::fit(
            "cgroups",
            format!("delegated under {}", root.display()),
            true,
            CGROUP_ADVICE,
        ),
        None => Check::fit(
            "cgroups",
            "no delegated cgroup v2 tree",
            false,
            CGROUP_ADVICE,
        ),
    }
}

/// The KSM item, from the text of `/sys/kernel/mm/ksm/run`: fit when it is `0`, or the file is
/// missing (KSM is not in the kernel). `1` merges pages across VMs.
pub fn ksm_check(run: Option<&str>) -> Check {
    match run.map(str::trim) {
        None => Check::fit("ksm", "absent", true, KSM_ADVICE),
        Some(text) => Check::fit("ksm", text, text == "0", KSM_ADVICE),
    }
}

/// The `vm_swap` item: information, fit either way. Its advice asks for `APZ_VMM_SWAP=ram` when
/// zswap is on and the VMs' cgroups do not swap yet. `env` is the variable's value.
pub fn vm_swap_check(env: Option<&str>, zswap_on: bool) -> Check {
    let policy = swap_policy_from(env);
    let value = env.map_or("unset", str::trim).to_string();
    let advice = (zswap_on && policy == SwapPolicy::Off).then(|| VM_SWAP_ADVICE.to_string());
    Check {
        name: "vm_swap",
        value,
        ok: true,
        advice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SWAPS_HEADER: &str = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";

    fn no_uuid(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn zswap_is_on_only_for_y() {
        assert!(zswap_enabled("Y\n"));
        assert!(!zswap_enabled("N\n"));
        assert!(!zswap_enabled(""));
        let check = zswap_check(Some("Y\n"));
        assert!(check.ok);
        assert_eq!(check.advice, None);
        let check = zswap_check(Some("N\n"));
        assert!(!check.ok);
        assert_eq!(check.value, "N");
        assert_eq!(check.advice.as_deref(), Some(ZSWAP_ADVICE));
        // No zswap module at all is not fit either.
        assert!(!zswap_check(None).ok);
    }

    #[test]
    fn swap_total_is_the_meminfo_line() {
        let meminfo = "MemTotal:       16000000 kB\nSwapTotal:       8388604 kB\nSwapFree: 1 kB\n";
        assert_eq!(swap_total_kib(meminfo), Some(8_388_604));
        assert_eq!(swap_total_kib("SwapTotal:       0 kB\n"), Some(0));
        assert_eq!(swap_total_kib("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn swap_devices_skip_the_header() {
        let swaps = format!(
            "{SWAPS_HEADER}/dev/zram0 \t\t\t\tpartition\t8388604\t\t0\t\t100\n/dev/dm-1 \t\t\t\tpartition\t4194300\t\t0\t\t-2\n"
        );
        assert_eq!(swap_devices(&swaps), vec!["/dev/zram0", "/dev/dm-1"]);
        assert!(swap_devices(SWAPS_HEADER).is_empty());
    }

    #[test]
    fn swap_kind_is_zram_crypt_or_plain() {
        assert_eq!(swap_kind("/dev/zram0", None), SwapKind::Zram);
        assert_eq!(
            swap_kind("/dev/dm-1", Some("CRYPT-LUKS2-0f3a\n")),
            SwapKind::Crypt
        );
        // A device-mapper device that is not dm-crypt (LVM, say) is not encrypted swap.
        assert_eq!(swap_kind("/dev/dm-2", Some("LVM-abc")), SwapKind::Plain);
        assert_eq!(swap_kind("/dev/dm-2", None), SwapKind::Plain);
        // A plain partition or swap file is never encrypted by the kernel.
        assert_eq!(swap_kind("/dev/sda2", None), SwapKind::Plain);
        assert_eq!(swap_kind("/swapfile", None), SwapKind::Plain);
        // A uuid on a non-dm device does not make it crypt.
        assert_eq!(
            swap_kind("/dev/sda2", Some("CRYPT-LUKS2-x")),
            SwapKind::Plain
        );
    }

    #[test]
    fn swap_is_fit_only_when_on_and_protected() {
        let meminfo = "SwapTotal:       8388604 kB\n";
        let zram = format!("{SWAPS_HEADER}/dev/zram0 partition 8388604 0 100\n");
        let crypt = format!("{SWAPS_HEADER}/dev/dm-1 partition 8388604 0 -2\n");
        let plain = format!("{SWAPS_HEADER}/dev/sda2 partition 8388604 0 -2\n");
        let crypt_uuid = |name: &str| (name == "dm-1").then(|| "CRYPT-LUKS2-x".to_string());

        // zram alone, and dm-crypt alone, are fit.
        let check = swap_check(meminfo, &zram, no_uuid);
        assert!(check.ok, "{}", check.value);
        assert_eq!(check.advice, None);
        assert!(swap_check(meminfo, &crypt, crypt_uuid).ok);
        // A mix of zram and dm-crypt is fit too.
        let mixed = format!("{zram}/dev/dm-1 partition 8388604 0 -2\n");
        assert!(swap_check(meminfo, &mixed, crypt_uuid).ok);
        // An unencrypted disk partition is not fit, whatever else is listed.
        let check = swap_check(meminfo, &plain, no_uuid);
        assert!(!check.ok);
        assert!(check.value.contains("/dev/sda2 (unencrypted)"));
        assert_eq!(check.advice.as_deref(), Some(SWAP_ADVICE));
        assert!(!swap_check(meminfo, &format!("{zram}{plain}"), no_uuid).ok);
        // Swap off is not fit, even with devices listed.
        assert!(!swap_check("SwapTotal: 0 kB\n", &zram, no_uuid).ok);
        // No devices is not fit.
        let none = swap_check(meminfo, SWAPS_HEADER, no_uuid);
        assert!(!none.ok);
        assert!(none.value.contains("no swap devices"));
    }

    #[test]
    fn ksm_is_fit_when_off_or_missing() {
        assert!(ksm_check(Some("0\n")).ok);
        assert!(ksm_check(None).ok);
        let merging = ksm_check(Some("1\n"));
        assert!(!merging.ok);
        assert_eq!(merging.value, "1");
        assert_eq!(merging.advice.as_deref(), Some(KSM_ADVICE));
        // Any other value is not known to be off, so not fit.
        assert!(!ksm_check(Some("2")).ok);
    }

    #[test]
    fn reflink_needs_a_directory_that_takes_reflinks() {
        let dir = PathBuf::from("/snapshots");
        let ok = reflink_outcome(&dir, true, true);
        assert!(ok.ok);
        assert_eq!(ok.value, "reflink in /snapshots");
        let sparse = reflink_outcome(&dir, true, false);
        assert!(!sparse.ok);
        assert_eq!(sparse.value, "sparse copy in /snapshots");
        let missing = reflink_outcome(&dir, false, false);
        assert!(!missing.ok);
        assert_eq!(missing.value, "/snapshots does not exist");
        // No snapshot directory is named as such.
        let none = reflink_check(None);
        assert!(!none.ok);
        assert_eq!(none.value, "no snapshot directory");
    }

    #[test]
    fn cgroups_are_fit_when_detected() {
        let detected = cgroups_check(Some(Path::new("/sys/fs/cgroup/agentplaza.slice")));
        assert!(detected.ok);
        assert_eq!(detected.advice, None);
        let undetected = cgroups_check(None);
        assert!(!undetected.ok);
        assert_eq!(undetected.advice.as_deref(), Some(CGROUP_ADVICE));
    }

    #[test]
    fn vm_swap_is_information_and_advised_under_zswap() {
        // Off, with zswap off: nothing to say.
        let quiet = vm_swap_check(None, false);
        assert!(quiet.ok);
        assert_eq!(quiet.value, "unset");
        assert_eq!(quiet.advice, None);
        // Off, with zswap on: the advice names the variable.
        let advised = vm_swap_check(None, true);
        assert!(advised.ok);
        assert_eq!(advised.advice.as_deref(), Some(VM_SWAP_ADVICE));
        // On: nothing to advise, zswap or not.
        let on = vm_swap_check(Some("ram"), true);
        assert!(on.ok);
        assert_eq!(on.value, "ram");
        assert_eq!(on.advice, None);
    }

    #[test]
    fn a_report_is_ok_when_every_item_is_and_lines_follow_the_format() {
        let fit = Check::fit("ksm", "0", true, KSM_ADVICE);
        let unfit = Check::fit("zswap", "N", false, ZSWAP_ADVICE);
        let report = Report {
            checks: vec![fit.clone()],
        };
        assert!(report.ok());
        assert_eq!(report.lines(), vec!["ok ksm: 0".to_string()]);
        assert_eq!(report.to_json()["ok"], json!(true));

        let report = Report {
            checks: vec![fit, unfit],
        };
        assert!(!report.ok());
        assert_eq!(report.to_json()["ok"], json!(false));
        assert_eq!(report.lines()[1], format!("warn zswap: N — {ZSWAP_ADVICE}"));
        assert_eq!(report.to_json()["checks"][1]["name"], json!("zswap"));
        assert_eq!(report.to_json()["checks"][1]["ok"], json!(false));
    }
}

#[cfg(test)]
mod hypervisor_tests {
    use super::hypervisor_entitled;

    #[test]
    fn the_hypervisor_entitlement_is_read_from_an_entitlements_plist() {
        let granted = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
	<key>com.apple.security.hypervisor</key>
	<true/>
</dict></plist>"#;
        assert!(hypervisor_entitled(granted));
        let refused = granted.replace("<true/>", "<false/>");
        assert!(
            !hypervisor_entitled(&refused),
            "a false entitlement is not granted"
        );
        assert!(!hypervisor_entitled(""), "no plist, no entitlement");
        assert!(
            !hypervisor_entitled("<key>com.apple.security.app-sandbox</key><true/>"),
            "another entitlement does not grant the hypervisor"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_report_names_the_hypervisor_and_the_clone_probe() {
        let names: Vec<&str> = super::report(None)
            .checks
            .iter()
            .map(|check| check.name)
            .collect();
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert!(names.contains(&"hvf"), "{names:?}");
        }
        assert!(names.contains(&"reflink"), "{names:?}");
    }
}
