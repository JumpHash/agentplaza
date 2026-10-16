// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: Firecracker sets no cgroup limits (its jailer's docs
// say so), and a per-VM `apz-vmm` process is the unit a cgroup can bound.

//! cgroup v2 resource limits for a per-VM process ([`crate::spawner::SpawnMode::Process`]).
//!
//! **Why.** An in-process VM is a set of threads of the daemon that boots it,
//! so nothing can bound one VM's memory or CPU without bounding the daemon.
//! A process that holds exactly one VM can be placed in a cgroup of its own,
//! and the kernel then enforces that VM's shape: `memory.max` is the guest's
//! RAM plus the VMM's own overhead, `cpu.max` is one period's quota per vCPU,
//! and `pids.max` caps the VM's threads.
//!
//! **Where.** VM cgroups are children of `agentplaza.slice`, which sits under
//! the user's systemd manager (`user@<uid>.service`). That manager delegates
//! `cpu`, `memory` and `pids` to its user; `io` is not delegated, so no
//! block-I/O limit is set. [`CgroupRoot::detect`] derives the slice from this
//! process's own `/proc/self/cgroup`, or takes it from `APZ_VMM_CGROUP_ROOT`.
//!
//! **Failure.** Detection either yields a usable root or `None` with the
//! reason logged, so a host without a delegated hierarchy runs its VMs
//! unlimited rather than refusing to run them. Once a root exists, a limit
//! that cannot be written is an error (the VM is not started with a limit it
//! does not have), while a limit *file* the kernel does not offer is skipped
//! with a note.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// The cgroup v2 mount the process's own hierarchy is read from.
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";

/// The environment variable that names the root directly, instead of deriving
/// it from `/proc/self/cgroup`.
pub const ROOT_ENV: &str = "APZ_VMM_CGROUP_ROOT";

/// The environment variable that opts VM cgroups into swap: `ram` lets a VM
/// swap up to its guest RAM, so cold guest pages can be compressed (zswap) or
/// written out (encrypted swap) instead of the VM being killed. Unset, `0` or
/// anything else keeps swap off.
pub const SWAP_ENV: &str = "APZ_VMM_SWAP";

/// The slice the VMs' cgroups are created under, beneath the user manager.
pub const SLICE: &str = "agentplaza.slice";

/// The controllers a VM's limits need. They must be delegated to the user
/// manager, and enabled down the path to the VM's own cgroup.
const CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];

/// Memory the VMM needs beyond the guest's RAM: its own heap, its page tables
/// and its device state. `memory.max` is the guest RAM plus this.
pub const VMM_OVERHEAD_MIB: u64 = 64;

/// The most tasks (threads) a VM's process may hold: its vCPU threads and its
/// helper threads. A VM that needs more is refused by the kernel, not slowed.
pub const PIDS_MAX: u64 = 64;

/// Reclaim starts at this share of `memory.max`, so a VM at its cap is slowed
/// before it is killed.
const MEMORY_HIGH_PERCENT: u64 = 95;

/// The period `cpu.max` is written against: one vCPU is one period's full quota.
const CPU_PERIOD_US: u64 = 100_000;

const MIB: u64 = 1024 * 1024;

/// Whether a VM's cgroup may swap, and how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapPolicy {
    /// No swap: `memory.swap.max` is `0`. The default.
    Off,
    /// Swap up to the guest's RAM: `memory.swap.max` is the RAM in bytes.
    GuestRam,
}

/// The swap policy an `APZ_VMM_SWAP` value asks for. Only `ram` (ignoring
/// surrounding whitespace) turns swap on; unset, `0` and any other value mean
/// [`SwapPolicy::Off`]. Pure.
pub fn swap_policy_from(value: Option<&str>) -> SwapPolicy {
    match value.map(str::trim) {
        Some("ram") => SwapPolicy::GuestRam,
        _ => SwapPolicy::Off,
    }
}

/// How long [`VmCgroup::remove`] waits for a killed cgroup's processes to
/// leave it before `rmdir` (which refuses a populated cgroup) is tried anyway.
const EMPTY_WAIT: Duration = Duration::from_secs(2);

/// The shape a VM's cgroup limits are derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmLimits {
    /// The VM's vCPUs.
    pub vcpus: u8,
    /// The guest's RAM, in MiB.
    pub memory_mib: usize,
}

/// Errors of the cgroup layer.
#[derive(Debug, thiserror::Error)]
pub enum CgroupError {
    /// VM cgroup limits are unavailable here: {reason}
    #[error("VM cgroup limits are unavailable: {reason}")]
    Unavailable {
        /// Why the hierarchy cannot host VM limits.
        reason: String,
    },
    /// {path}: {source}
    #[error("{}: {source}", .path.display())]
    Io {
        /// The file or directory the operation touched.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A VM cgroup name must be one path component, not {name:?}
    #[error("a VM cgroup name must be one path component, not {name:?}")]
    BadName {
        /// The refused name.
        name: String,
    },
}

fn io_error(path: &Path, source: io::Error) -> CgroupError {
    CgroupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The directory VM cgroups are created in: `agentplaza.slice`, delegated to
/// this user. Cheap to clone; the cgroups themselves are [`VmCgroup`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupRoot(PathBuf);

impl CgroupRoot {
    /// Finds (creating it if missing) the slice VM cgroups go under, or
    /// returns `None` with the reason logged when the host cannot give VMs
    /// limits: no cgroup v2, not under a user manager, or the controllers not
    /// delegated to it.
    pub fn detect() -> Option<Self> {
        match detect_root() {
            Ok(root) => Some(root),
            Err(err) => {
                eprintln!("vmm: VM cgroup limits are off: {err}");
                None
            }
        }
    }

    /// The directory this root names.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Creates the cgroup `name` under this root with `limits` applied.
    ///
    /// A limit file the kernel does not offer is skipped with a note; one that
    /// is offered but refuses the value is an error, and the half-made cgroup
    /// is removed on the way out.
    pub fn create(&self, name: &str, limits: VmLimits) -> Result<VmCgroup, CgroupError> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(CgroupError::BadName {
                name: name.to_string(),
            });
        }
        let path = self.0.join(name);
        fs::create_dir(&path).map_err(|source| io_error(&path, source))?;
        // From here on, an error drops `cgroup` and removes the directory.
        let cgroup = VmCgroup {
            path,
            removed: false,
        };
        let swap = swap_policy_from(std::env::var(SWAP_ENV).ok().as_deref());
        for (file, value) in limit_files(limits, swap) {
            let target = cgroup.path.join(file);
            if !target.exists() {
                eprintln!(
                    "vmm: cgroup {}: {file} is not offered by the kernel here, not set",
                    cgroup.path.display()
                );
                continue;
            }
            fs::write(&target, &value).map_err(|source| io_error(&target, source))?;
        }
        Ok(cgroup)
    }
}

/// One VM's cgroup. Dropping it kills what is left in it and removes it.
#[derive(Debug)]
pub struct VmCgroup {
    path: PathBuf,
    removed: bool,
}

impl VmCgroup {
    /// The cgroup's directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Moves `pid` into this cgroup (`cgroup.procs`). The process's limits
    /// apply from its next allocation or scheduling decision.
    pub fn add_process(&self, pid: u32) -> Result<(), CgroupError> {
        let procs = self.path.join("cgroup.procs");
        fs::write(&procs, pid.to_string()).map_err(|source| io_error(&procs, source))
    }

    /// The processes in this cgroup (`cgroup.procs`, including its subtree's
    /// leaves only where the kernel lists them).
    pub fn pids(&self) -> Result<Vec<i32>, CgroupError> {
        let procs = self.path.join("cgroup.procs");
        let text = fs::read_to_string(&procs).map_err(|source| io_error(&procs, source))?;
        Ok(text
            .lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .collect())
    }

    /// Kills every process in the cgroup, its whole tree included: `cgroup.kill`
    /// where the kernel has it (5.14 and later), else `SIGKILL` to each pid
    /// it lists. A process that exits meanwhile is not an error.
    pub fn kill(&self) -> Result<(), CgroupError> {
        let kill = self.path.join("cgroup.kill");
        if kill.exists() {
            return fs::write(&kill, "1").map_err(|source| io_error(&kill, source));
        }
        for pid in self.pids()? {
            // Best effort per pid: a process that is already gone is the goal,
            // and anything that survives is reported by `remove`'s wait.
            let _ = kill_pid(pid);
        }
        Ok(())
    }

    /// Kills the cgroup, waits briefly for it to empty, and removes it. A
    /// second call after success does nothing.
    pub fn remove(&mut self) -> Result<(), CgroupError> {
        if self.removed {
            return Ok(());
        }
        if !self.path.exists() {
            self.removed = true;
            return Ok(());
        }
        self.kill()?;
        let deadline = Instant::now() + EMPTY_WAIT;
        while !self.pids()?.is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        fs::remove_dir(&self.path).map_err(|source| io_error(&self.path, source))?;
        self.removed = true;
        Ok(())
    }
}

impl Drop for VmCgroup {
    fn drop(&mut self) {
        // Nothing to report to: a failure here leaves an empty or killed
        // directory, and the next `create` under the same name fails loudly.
        let _ = self.remove();
    }
}

#[cfg(target_os = "linux")]
fn kill_pid(pid: i32) -> io::Result<()> {
    crate::sys::send_signal(pid, libc::SIGKILL)
}

#[cfg(not(target_os = "linux"))]
fn kill_pid(_pid: i32) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// The limit files and their values for a VM of `limits`, in the order they
/// are written. Pure, so the arithmetic is tested without a kernel.
///
/// `memory.max` is the guest RAM plus [`VMM_OVERHEAD_MIB`]; `memory.high` is
/// [`MEMORY_HIGH_PERCENT`] of it; `memory.swap.max` is `0` under
/// [`SwapPolicy::Off`] and the guest RAM in bytes under
/// [`SwapPolicy::GuestRam`]; `cpu.max` is one period's quota per vCPU;
/// `pids.max` is [`PIDS_MAX`].
pub fn limit_files(limits: VmLimits, swap: SwapPolicy) -> Vec<(&'static str, String)> {
    let ram_mib = u64::try_from(limits.memory_mib).unwrap_or(u64::MAX);
    let max = ram_mib.saturating_add(VMM_OVERHEAD_MIB).saturating_mul(MIB);
    let high = max / 100 * MEMORY_HIGH_PERCENT;
    let swap_max = match swap {
        SwapPolicy::Off => 0,
        SwapPolicy::GuestRam => ram_mib.saturating_mul(MIB),
    };
    let quota = u64::from(limits.vcpus) * CPU_PERIOD_US;
    vec![
        ("memory.max", max.to_string()),
        ("memory.high", high.to_string()),
        ("memory.swap.max", swap_max.to_string()),
        ("cpu.max", format!("{quota} {CPU_PERIOD_US}")),
        ("pids.max", PIDS_MAX.to_string()),
    ]
}

/// The slice a process's own cgroup implies, from the contents of
/// `/proc/self/cgroup` and the mount it is read against. The slice is
/// `agentplaza.slice` under the first `user@<uid>.service` on the process's
/// path: the user manager is what delegates the controllers. Pure.
pub fn root_from_proc_cgroup(mount: &Path, proc_cgroup: &str) -> Result<PathBuf, String> {
    let unified = proc_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| "this process has no cgroup v2 entry in /proc/self/cgroup".to_string())?;
    let mut path = mount.to_path_buf();
    for component in unified.split('/').filter(|component| !component.is_empty()) {
        path.push(component);
        if component.starts_with("user@") && component.ends_with(".service") {
            return Ok(path.join(SLICE));
        }
    }
    Err("this process is not under a systemd user manager (user@<uid>.service)".to_string())
}

fn detect_root() -> Result<CgroupRoot, CgroupError> {
    let path = match std::env::var_os(ROOT_ENV) {
        Some(path) => PathBuf::from(path),
        None => {
            let mount = Path::new(CGROUP_MOUNT);
            if !mount.join("cgroup.controllers").exists() {
                return Err(CgroupError::Unavailable {
                    reason: format!("cgroup v2 is not mounted at {CGROUP_MOUNT}"),
                });
            }
            let proc_cgroup = "/proc/self/cgroup";
            let text = fs::read_to_string(proc_cgroup)
                .map_err(|source| io_error(Path::new(proc_cgroup), source))?;
            root_from_proc_cgroup(mount, &text)
                .map_err(|reason| CgroupError::Unavailable { reason })?
        }
    };
    if !path.exists() {
        fs::create_dir(&path).map_err(|source| io_error(&path, source))?;
    }
    if !path.join("cgroup.controllers").exists() {
        return Err(CgroupError::Unavailable {
            reason: format!("{} is not a cgroup v2 directory", path.display()),
        });
    }
    // The controllers must be enabled in the parent to reach this directory,
    // and in this directory to reach the VMs' cgroups below it.
    let parent = path.parent().ok_or_else(|| CgroupError::Unavailable {
        reason: format!("{} has no parent cgroup", path.display()),
    })?;
    enable_controllers(parent)?;
    let available = fs::read_to_string(path.join("cgroup.controllers"))
        .map_err(|source| io_error(&path, source))?;
    let missing: Vec<&str> = CONTROLLERS
        .into_iter()
        .filter(|controller| !available.split_whitespace().any(|a| a == *controller))
        .collect();
    if !missing.is_empty() {
        return Err(CgroupError::Unavailable {
            reason: format!(
                "the controllers {missing:?} are not delegated to {}",
                path.display()
            ),
        });
    }
    enable_controllers(&path)?;
    Ok(CgroupRoot(path))
}

/// Enables [`CONTROLLERS`] in `dir`'s `cgroup.subtree_control`, if they are not
/// already there. Nothing is written when they are.
fn enable_controllers(dir: &Path) -> Result<(), CgroupError> {
    let file = dir.join("cgroup.subtree_control");
    let enabled = fs::read_to_string(&file).map_err(|source| io_error(&file, source))?;
    let all_enabled = CONTROLLERS
        .iter()
        .all(|controller| enabled.split_whitespace().any(|e| e == *controller));
    if all_enabled {
        return Ok(());
    }
    let request = CONTROLLERS
        .iter()
        .map(|controller| format!("+{controller}"))
        .collect::<Vec<_>>()
        .join(" ");
    fs::write(&file, request).map_err(|source| io_error(&file, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(files: &'a [(&'static str, String)], name: &str) -> &'a str {
        files
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("no {name} in the limit files"))
    }

    #[test]
    fn cgroup_limits_follow_the_vm_shape() {
        // The limits a 2 vCPU, 512 MiB VM gets, and those of a single vCPU VM.
        {
            // 512 MiB of RAM plus 64 MiB of overhead is 576 MiB.
            let files = limit_files(
                VmLimits {
                    vcpus: 2,
                    memory_mib: 512,
                },
                SwapPolicy::Off,
            );
            assert_eq!(value(&files, "memory.max"), "603979776");
            // 95% of 603979776, rounded down to a whole 1/100 of the cap first.
            assert_eq!(value(&files, "memory.high"), "573780715");
            assert_eq!(value(&files, "memory.swap.max"), "0");
            assert_eq!(value(&files, "cpu.max"), "200000 100000");
            assert_eq!(value(&files, "pids.max"), "64");
        }
        // A single vCPU is one full period.
        {
            let files = limit_files(
                VmLimits {
                    vcpus: 1,
                    memory_mib: 0,
                },
                SwapPolicy::Off,
            );
            assert_eq!(value(&files, "cpu.max"), "100000 100000");
            assert_eq!(value(&files, "memory.max"), "67108864");
        }
    }

    #[test]
    fn swap_is_the_guest_ram_only_when_asked_for() {
        let limits = VmLimits {
            vcpus: 1,
            memory_mib: 512,
        };
        // Off: no swap at all, as before the knob existed.
        assert_eq!(
            value(&limit_files(limits, SwapPolicy::Off), "memory.swap.max"),
            "0"
        );
        // GuestRam: swap up to the guest's RAM in bytes; memory.max is unchanged.
        let files = limit_files(limits, SwapPolicy::GuestRam);
        assert_eq!(value(&files, "memory.swap.max"), "536870912");
        assert_eq!(value(&files, "memory.max"), "603979776");
    }

    #[test]
    fn only_ram_turns_swap_on() {
        assert_eq!(swap_policy_from(None), SwapPolicy::Off);
        assert_eq!(swap_policy_from(Some("0")), SwapPolicy::Off);
        assert_eq!(swap_policy_from(Some("")), SwapPolicy::Off);
        assert_eq!(swap_policy_from(Some("disk")), SwapPolicy::Off);
        assert_eq!(swap_policy_from(Some("ram")), SwapPolicy::GuestRam);
        assert_eq!(swap_policy_from(Some(" ram\n")), SwapPolicy::GuestRam);
    }

    #[test]
    fn cgroup_root_comes_from_the_user_manager_slice() {
        let mount = Path::new("/sys/fs/cgroup");
        // The root is the user manager's slice of the caller's own cgroup.
        {
            let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope\n";
            assert_eq!(
                root_from_proc_cgroup(mount, line),
                Ok(PathBuf::from(
                    "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/agentplaza.slice"
                ))
            );
        }
        // A process outside a user manager has no root.
        {
            assert!(root_from_proc_cgroup(mount, "0::/system.slice/sshd.service\n").is_err());
            // A cgroup v1 entry is not a v2 path at all.
            assert!(root_from_proc_cgroup(mount, "12:cpu,cpuacct:/user.slice\n").is_err());
        }
    }

    #[test]
    fn a_name_must_be_one_path_component() {
        let root = CgroupRoot(PathBuf::from("/nonexistent-root"));
        for name in ["", "a/b", ".", ".."] {
            assert!(matches!(
                root.create(
                    name,
                    VmLimits {
                        vcpus: 1,
                        memory_mib: 1
                    }
                ),
                Err(CgroupError::BadName { .. })
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_cgroup_takes_limits_a_process_and_its_kill() {
        let Some(root) = CgroupRoot::detect() else {
            eprintln!("skipped: no delegated cgroup v2 hierarchy on this host");
            return;
        };
        let name = format!("test-{}", std::process::id());
        let mut cgroup = root
            .create(
                &name,
                VmLimits {
                    vcpus: 2,
                    memory_mib: 64,
                },
            )
            .expect("the test cgroup is created");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep starts");
        cgroup
            .add_process(child.id())
            .expect("the child joins the cgroup");

        let read = |file: &str| {
            fs::read_to_string(cgroup.path().join(file))
                .unwrap_or_else(|err| panic!("{file}: {err}"))
                .trim()
                .to_string()
        };
        // (64 + 64) MiB.
        assert_eq!(read("memory.max"), "134217728");
        assert_eq!(read("cpu.max"), "200000 100000");
        assert_eq!(read("pids.max"), "64");
        assert!(
            read("cgroup.procs")
                .lines()
                .any(|line| line == child.id().to_string()),
            "the child is listed in cgroup.procs"
        );

        cgroup.kill().expect("the cgroup is killed");
        let status = child.wait().expect("the child is reaped");
        assert!(!status.success(), "the child was killed, not finished");
        cgroup.remove().expect("the cgroup is removed");
        assert!(!root.path().join(&name).exists());
    }
}
