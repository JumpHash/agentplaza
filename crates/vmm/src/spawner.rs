//! The public multi-VM API: [`spawn`] boots one microVM, [`spawn_n`] boots
//! `n` of them (ids `0..n`), and each returned [`VmHandle`] exposes the VM's
//! id, its guest serial capture (output) and its serial input
//! ([`VmHandle::serial_input`]), plus blocking `wait`/`kill` control.
//!
//! Every VM is built and booted by the backend of the current platform
//! (`crate::builder` on Linux, `crate::hvf` on Apple Silicon); the state the
//! spawner and the backend share — the serial sink and its input hook, the
//! stop control and the backend's thread joins — lives in [`VmRuntime`], which
//! is backend-neutral. A backend's `boot` fills the runtime in and returns
//! once the VM is running; the spawner then owns the runtime and joins the
//! backend's threads when the VM stops.
//!
//! **Two boot modes, one API.** [`SpawnMode::InProcess`] (the default) is the
//! v0 shape above: the VMM is a set of threads of the calling process, which
//! is what `noded`, `vmctl` and `spawn_n` have always had.
//! [`SpawnMode::Process`] boots the same VM in a child `apz-vmm` process —
//! one VM per process, which is the only shape in which
//! [`crate::jail::jailer`] (a single-threaded boot, a per-VM root,
//! `pivot_root`) actually applies, since a user namespace cannot be split off
//! a multi-threaded process. The mode is selected with
//! [`set_spawn_mode`]/[`spawn_mode`] or the [`SPAWN_MODE_ENV`] switch; see
//! [`crate::spawner::process`] for what the child is handed and what the
//! parent proxies. Everything the caller sees — [`VmHandle::serial`],
//! [`VmHandle::serial_input`], [`VmHandle::wait`], [`VmHandle::kill`] — is the
//! same in both modes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The per-VM process mode ([`SpawnMode::Process`]): the parent half of the
/// `apz-vmm` child protocol. Linux only (the child boots through
/// [`crate::builder`]). Read the module documentation for what is proxied
/// and why.
#[cfg(target_os = "linux")]
pub mod process;

/// How long the per-VM process is given to report that its VM is up or to
/// fail. A boot that is still silently working after this is treated as a
/// boot failure, so a stuck child cannot hold a [`spawn`] call forever.
#[cfg(target_os = "linux")]
const CHILD_BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// How often [`VmHandle::wait_timeout`] re-checks whether the VM's backend
/// threads have all finished.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long [`VmHandle::kill`] waits for the VM's threads to finish after each
/// invocation of the stop control.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// How many times [`VmHandle::kill`] invokes the stop control: a request that
/// did not reach a vCPU asleep on a condition variable is repeated once,
/// rather than giving up on a VM that is still running.
const KILL_ATTEMPTS: usize = 2;

/// Interrupt controller (GIC) version for an `aarch64` guest.
///
/// Ignored on `x86_64`, where the in-kernel irqchip is the only interrupt
/// controller. `None` in a [`VmSpec`] means "use the backend's default",
/// which on KVM `aarch64` is GICv3 with a fallback to GICv2 (the Raspberry
/// Pi 4/5 path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicVersion {
    /// GICv2 (the Raspberry Pi 4/5 in-kernel controller).
    V2,
    /// GICv3.
    V3,
}

/// The smallest CID a guest may be given.
///
/// CID 0 is the hypervisor, 1 is loopback and 2 is the host itself, so guests
/// start at 3. The spawner refuses a guest CID below it (or above
/// [`MAX_GUEST_CID`]) before the VM is built, which is why a wrong CID is a
/// boot failure rather than a VM that silently cannot be reached.
pub const MIN_GUEST_CID: u64 = 3;

/// The largest CID a guest may be given.
///
/// The kernel's vsock stack refuses `VMADDR_CID_ANY` (`u32::MAX`) and anything
/// above it, so a valid guest CID is [`MIN_GUEST_CID`]..=`u32::MAX - 1`.
/// Callers that derive several CIDs from a base (VM `i` gets `base + i`) must
/// keep every one inside this range.
pub const MAX_GUEST_CID: u64 = u32::MAX as u64 - 1;

/// Configuration of a VM's vsock device.
///
/// A VM with a vsock device talks to the host over the **userspace** vsock
/// device: the guest is given [`guest_cid`](VsockConfig::guest_cid), and the
/// VMM's own muxer mediates every connection to a host Unix-domain socket
/// derived from [`uds_path`](VsockConfig::uds_path). A guest `connect(2, port)`
/// becomes a host `UnixStream::connect("{uds_path}_{port}")`, and the host
/// side connects to `{uds_path}` itself to initiate a connection back into the
/// guest (`connect <port>\n`). There is no kernel vsock routing anywhere: no
/// `/dev/vhost-vsock`, no `AF_VSOCK` listener, and no cross-guest path.
///
/// **The caller assigns CIDs and UDS paths.** The spawner never invents either
/// (not even in [`spawn_n`]), because the host side has to know which CID and
/// which socket path to expect for each VM; a caller that boots several VMs
/// derives each itself (see [`spawn_each`]). A guest CID is at least
/// [`MIN_GUEST_CID`], and two running VMs must not share one: the two muxers
/// would both try to bind `{uds_path}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsockConfig {
    /// The CID the guest's `AF_VSOCK` sockets carry: at least 3, and unique
    /// among the running VMs on the host.
    pub guest_cid: u64,
    /// The base path of this VM's host-side Unix socket: the muxer binds it,
    /// and a guest `connect(2, port)` reaches the host through
    /// `"{uds_path}_{port}"`. One path per VM, so the caller derives it (for
    /// example `{dir}/vm{cid}`).
    pub uds_path: PathBuf,
}

/// Specification of the microVMs to spawn.
///
/// `kernel` and `cmdline` are required; everything else has a default (see
/// [`VmSpec::default`]).
#[derive(Debug, Clone)]
pub struct VmSpec {
    /// Guest kernel image: an uncompressed ELF (`vmlinux`) or a `bzImage`.
    pub kernel: PathBuf,
    /// Kernel command line (for example `console=ttyS0 panic=-1`).
    pub cmdline: String,
    /// Number of vCPUs per VM.
    pub vcpus: u8,
    /// Guest memory per VM, in MiB.
    pub memory_mib: usize,
    /// Optional initrd image loaded into guest memory.
    pub initrd: Option<PathBuf>,
    /// `aarch64` interrupt controller to use; `None` selects the backend's
    /// default. Ignored on `x86_64`.
    pub gic: Option<GicVersion>,
    /// The VM's vsock device (see [`VsockConfig`]), `None` for a VM with no
    /// vsock device at all.
    ///
    /// Unsupported on Apple Silicon: a spec that sets it makes the boot fail
    /// with an explicit error rather than boot a VM the host can never reach
    /// (Hypervisor.framework has no vsock device, and this port has no
    /// network device to fall back to).
    pub vsock: Option<VsockConfig>,
    /// The VM's writable workspace volume: the image its block device exposes
    /// to the guest, `None` for a VM with no block device at all.
    ///
    /// **The caller gives each VM its own image.** A volume is writable, so
    /// two VMs sharing one image would corrupt each other's filesystem; the
    /// spawner neither copies nor invents the path — a caller that has one base
    /// image lays the run's private copies out with [`materialize_workspaces`]
    /// and hands each VM its own.
    ///
    /// Unsupported on Apple Silicon: a spec that sets it makes the boot fail
    /// with an explicit error rather than boot a VM whose device never appears.
    pub workspace: Option<PathBuf>,
    /// The memory the guest may use at boot, in MiB, when the VM carries a
    /// virtio-balloon: the VM is sized at [`VmSpec::memory_mib`] and the
    /// balloon holds the rest, so its memory can later grow (and shrink) with
    /// [`VmHandle::set_usable_memory_mib`] without a reboot. `None` = no
    /// balloon; the guest has all of `memory_mib`. `x86_64` only for now: on
    /// other backends the VM boots without one, with a warning.
    pub balloon: Option<u32>,
    /// Restore the VM from this snapshot directory (see [`crate::vstate::snapshot`]) instead of
    /// booting `kernel`: the guest continues where the snapshot left it. The spec must describe the
    /// same machine (vCPUs, memory, devices); `vsock` may name another CID and socket (the guest is
    /// told through a transport reset and re-reads its CID), and `workspace` is the caller's copy of
    /// the volume as it was when the snapshot was taken. `kernel`, `initrd` and `cmdline` are not used.
    /// Linux/KVM only: other backends refuse it.
    pub restore: Option<PathBuf>,
}

impl Default for VmSpec {
    fn default() -> Self {
        Self {
            kernel: PathBuf::new(),
            cmdline: String::new(),
            vcpus: 1,
            memory_mib: 512,
            initrd: None,
            gic: None,
            vsock: None,
            workspace: None,
            balloon: None,
            restore: None,
        }
    }
}

/// What a snapshot holds of the guest memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    /// Every page the guest has touched.
    Full,
    /// Only the pages the guest changed since it was restored, written over a reflinked copy of the
    /// memory it was restored from (the caller places that copy; see
    /// [`crate::vstate::snapshot_memory`]). For a VM that was booted, not restored, the same as `Full`.
    Diff,
}

/// A snapshot [`VmHandle::snapshot`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// The snapshot directory.
    pub dir: PathBuf,
    pub kind: SnapshotKind,
    /// Guest memory bytes written.
    pub memory_written: u64,
    /// How long the VM was paused for the snapshot, in milliseconds.
    pub paused_ms: u64,
}

/// How the host pauses, resumes and snapshots a running VM: in-process, the VM's own vCPUs and
/// devices; for a VM in its own process, a request over that process's control socket.
pub trait VmControl: Send + Sync {
    /// Stops the vCPUs (out of `KVM_RUN`, parked) and the devices' own threads. Idempotent.
    fn pause(&self) -> Result<(), SpawnError>;
    /// Lets a paused VM run again. Idempotent.
    fn resume(&self) -> Result<(), SpawnError>;
    /// Pauses the VM (if it is running), writes a snapshot into `dir`, and leaves it **paused**: the
    /// caller resumes it, or stops it (to sleep or move it).
    fn snapshot(&self, dir: &Path, kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError>;
}

/// How a VM stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmExit {
    /// The guest stopped the VM itself (a shutdown, a reset, or a triple
    /// fault).
    GuestExited {
        /// `true` when the guest asked for a reset rather than a shutdown
        /// (PSCI `SYSTEM_RESET` on the Hypervisor.framework path,
        /// `KVM_SYSTEM_EVENT_RESET` on the KVM paths). Every backend stops the
        /// VM either way - neither has a re-boot path - but a caller that
        /// reports or accounts for exits can tell the two apart. A machine
        /// reset taken through the x86_64 i8042 port is not distinguished from
        /// a shutdown: the port signals the same shared exit event.
        reset: bool,
    },
    /// The VM was stopped by [`VmHandle::kill`].
    Killed,
}

/// Errors returned by the spawner API.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// Failed to build or boot the VM: {0}
    #[error("failed to build or boot the VM: {0}")]
    Boot(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Failed to create a VM thread: {0}
    #[error("failed to create a VM thread: {0}")]
    Thread(#[from] std::io::Error),
    /// VM {id} did not stop within {secs} seconds of being killed
    #[error("VM {id} did not stop within {secs} seconds of being killed")]
    KillTimeout {
        /// Id of the VM that did not stop.
        id: u32,
        /// How long the VM was given to stop.
        secs: u64,
    },
    /// A backend thread of VM {0} panicked
    #[error("a backend thread of VM {0} panicked")]
    ThreadPanic(u32),
    /// More VMs were requested than the 32-bit VM ids can name
    #[error("more VMs were requested than the 32-bit VM ids can name")]
    TooManyVms,
    /// A VM failed to boot, and VMs {vms:?} could not be stopped afterwards
    #[error("{boot}; the VMs {vms:?} had already started and could not be stopped")]
    PartialCleanup {
        /// The boot failure that started the cleanup.
        boot: Box<SpawnError>,
        /// Ids of the VMs that could not be stopped and are still running.
        vms: Vec<u32>,
    },
    /// No backend is compiled in for this platform
    #[error("no VM backend on this platform yet (Intel macOS and Windows on arm64 run no VMs)")]
    NoBackend,
    /// The VM has no balloon, so its memory cannot be resized
    #[error("this VM has no balloon")]
    NoBalloon,
    /// The VM cannot be paused or snapshotted (its backend has no snapshot support)
    #[error("this VM cannot be paused or snapshotted on this backend")]
    NoSnapshots,
    /// Pausing, resuming, snapshotting or restoring the VM failed: {reason}
    #[error("VM snapshot control failed: {reason}")]
    Snapshot {
        /// What failed.
        reason: String,
    },
    /// The VM's memory control refused a resize or could not be reached: {reason}
    #[error("the VM's memory control failed: {reason}")]
    MemoryControl {
        /// Why the request failed.
        reason: String,
    },
    /// Guest CID {guest_cid} is outside the supported range {min}..={max}
    #[error("guest CID {guest_cid} is outside the supported range {min}..={max}")]
    GuestCidOutOfRange {
        /// The rejected guest CID.
        guest_cid: u64,
        /// The smallest CID a guest may be given ([`MIN_GUEST_CID`]).
        min: u64,
        /// The largest CID a guest may be given ([`MAX_GUEST_CID`]).
        max: u64,
    },
    /// VM {id} could not take the console input: {reason}
    #[error("VM {id} could not take the console input: {reason}")]
    SerialInput {
        /// Id of the VM whose console refused the input.
        id: u32,
        /// Why the console could not take the bytes. Nothing was queued when
        /// the console rejected the call, so the caller can retry the same
        /// bytes once the guest has drained the console's input queue.
        reason: String,
    },
    /// The boot mode is misconfigured: {reason}
    #[error("the VMM's boot mode is misconfigured: {reason}")]
    SpawnMode {
        /// Why the mode could not be decided.
        reason: String,
    },
    /// The per-VM process binary could not be found (tried {tried:?})
    #[error("cannot find the per-VM process binary `apz-vmm` (tried {tried:?})")]
    ChildBinary {
        /// Every path that was tried, in the order they were tried.
        tried: Vec<PathBuf>,
    },
    /// Failed to start the per-VM process {path}: {source}
    #[error("cannot start the per-VM process {path}: {source}")]
    ChildSpawn {
        /// The binary that could not be started.
        path: PathBuf,
        /// The error the spawn failed with.
        source: std::io::Error,
    },
    /// The per-VM process for VM {id} exited before its VM was up ({status}): {log}
    #[error("the per-VM process for VM {id} exited before its VM was up ({status}): {log}")]
    ChildBoot {
        /// Id of the VM whose per-VM process failed.
        id: u32,
        /// How the child exited (its status, or `still running` for a
        /// readiness timeout).
        status: String,
        /// The tail of the child's own log (its stderr).
        log: String,
    },
}

impl SpawnError {
    /// Wraps a backend's error into a [`SpawnError`].
    pub fn backend(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Boot(Box::new(err))
    }
}

/// A thread the backend must join before the VM is considered stopped.
type StopControl = Box<dyn Fn() + Send + Sync>;

// ---------------------------------------------------------------------------
// The boot mode: in this process, or in a per-VM `apz-vmm` process.
// ---------------------------------------------------------------------------

/// The environment variable that selects the boot mode: exactly `in-process`
/// or `process` (ASCII case-insensitive, trimmed). Anything else is refused,
/// so a typo stops the VM rather than silently picking a mode.
pub const SPAWN_MODE_ENV: &str = "APZ_VMM_SPAWN";

/// The environment variable that names the `apz-vmm` binary to run in
/// [`SpawnMode::Process`], overriding the sibling-of-this-binary default.
pub const CHILD_BINARY_ENV: &str = "APZ_VMM_CHILD";

/// The name of the per-VM process binary, as built from this crate
/// (`src/bin/apz-vmm.rs`) and looked for beside the running binary.
pub const CHILD_BINARY_NAME: &str = "apz-vmm";

/// The one line the per-VM process writes on **stderr** once its VM is up.
/// It is the parent's readiness gate: until it arrives there is no VM to hand
/// out, and anything else on the child's stderr is its log.
pub const CHILD_READY: &str = "apz-vmm: ready";

/// The one line the per-VM process writes on **stderr** once it has armed its
/// parent-death signal (`PR_SET_PDEATHSIG(SIGKILL)`) — before it parses its
/// command line, let alone boots a VM. It is the parent's (and a test's)
/// observable that the `prctl(2)` ran and the kernel accepted it: `/proc`
/// exposes no field for the setting itself, and `PR_GET_PDEATHSIG` can only be
/// asked of one's own thread.
pub const CHILD_PDEATHSIG: &str = "apz-vmm: parent-death signal armed (SIGKILL)";

/// The per-VM process's exit status when the guest shut the VM down
/// (`VmExit::GuestExited { reset: false }`).
pub const CHILD_EXIT_SHUTDOWN: i32 = 0;

/// The per-VM process's exit status when the guest reset the VM
/// (`VmExit::GuestExited { reset: true }`).
pub const CHILD_EXIT_RESET: i32 = 3;

/// The per-VM process's exit status when it failed: a boot error, an internal
/// error, or a VM stopped by anything other than the guest (a signal). The
/// parent reports such an exit as [`VmExit::Killed`], never as a guest exit.
pub const CHILD_EXIT_FAILED: i32 = 1;

/// The per-VM process's exit status when its command line was refused.
pub const CHILD_EXIT_USAGE: i32 = 2;

/// How a VM is booted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnMode {
    /// In the caller's own process, as a set of VMM threads (the v0 shape
    /// `noded`, `vmctl` and `spawn_n` have always used).
    InProcess,
    /// In a child `apz-vmm` process that boots exactly one VM. This is the
    /// only shape the jailer
    /// ([`crate::jail::jailer`]) can confine: `unshare(CLONE_NEWUSER)` — the
    /// unprivileged road to a mount namespace — is refused by a multi-threaded
    /// process, and the child is single-threaded at its boot by construction,
    /// because one process holds one VM. Linux only (the child boots through
    /// [`crate::builder`]).
    Process,
}

/// The mode this process is set to. `0` = read the environment; then
/// `MODE_IN_PROCESS`/`MODE_PROCESS`; `MODE_INVALID` = the environment named a
/// mode that does not exist, refused until something sets it.
static MODE: AtomicU8 = AtomicU8::new(0);

const MODE_FROM_ENV: u8 = 0;
const MODE_IN_PROCESS: u8 = 1;
const MODE_PROCESS: u8 = 2;
const MODE_INVALID: u8 = 3;

/// The `apz-vmm` binary [`SpawnMode::Process`] runs, when a caller named one
/// programmatically.
static CHILD_BINARY: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets the boot mode, overriding the environment for the rest of the process.
/// The library half of a CLI flag or of a deployment's configuration: a
/// daemon that wants every VM in its own jailed process calls this once, and
/// one that wants the v0 thread shape leaves it alone (the default).
pub fn set_spawn_mode(mode: SpawnMode) {
    MODE.store(
        match mode {
            SpawnMode::InProcess => MODE_IN_PROCESS,
            SpawnMode::Process => MODE_PROCESS,
        },
        Ordering::SeqCst,
    );
}

/// Names the `apz-vmm` binary [`SpawnMode::Process`] runs, overriding both
/// [`CHILD_BINARY_ENV`] and the sibling-of-this-binary default. A path that
/// does not exist is not refused here — it fails the boot, with the paths that
/// were tried, so the operator learns what is missing.
pub fn set_child_binary(path: impl Into<PathBuf>) {
    *CHILD_BINARY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.into());
}

/// The mode this process is set to: the programmatic one if [`set_spawn_mode`]
/// was called, otherwise the environment's, otherwise
/// [`SpawnMode::InProcess`]. An [`SPAWN_MODE_ENV`] value that names no mode is
/// refused — on every call, until something sets the mode.
pub fn spawn_mode() -> Result<SpawnMode, SpawnError> {
    match MODE.load(Ordering::SeqCst) {
        MODE_FROM_ENV => mode_from_env(),
        MODE_IN_PROCESS => Ok(SpawnMode::InProcess),
        MODE_PROCESS => Ok(SpawnMode::Process),
        // Only `MODE_INVALID` is left: `set_spawn_mode` and `mode_from_env`
        // store these four values and nothing else.
        _ => Err(SpawnError::SpawnMode {
            reason: format!(
                "{SPAWN_MODE_ENV}={:?} is not a mode: expected \"in-process\" or \"process\"",
                std::env::var(SPAWN_MODE_ENV).unwrap_or_default()
            ),
        }),
    }
}

/// The mode an [`SPAWN_MODE_ENV`] value names: `in-process` or `process`,
/// ASCII case-insensitive and trimmed. Anything else — an empty string
/// included — is refused.
fn parse_spawn_mode_env(value: &str) -> Result<SpawnMode, SpawnError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "in-process" => Ok(SpawnMode::InProcess),
        "process" => Ok(SpawnMode::Process),
        _ => Err(SpawnError::SpawnMode {
            reason: format!(
                "{SPAWN_MODE_ENV}={value:?} is not a mode: expected \"in-process\" or \"process\""
            ),
        }),
    }
}

/// Reads [`SPAWN_MODE_ENV`] and caches the decision in [`MODE`] — an invalid
/// value included, so it is never silently re-read as a default.
fn mode_from_env() -> Result<SpawnMode, SpawnError> {
    match std::env::var(SPAWN_MODE_ENV) {
        Ok(value) => {
            let mode = parse_spawn_mode_env(&value);
            if mode.is_err() {
                MODE.store(MODE_INVALID, Ordering::SeqCst);
            }
            mode
        }
        Err(_) => Ok(SpawnMode::InProcess),
    }
}

/// The `apz-vmm` binary [`SpawnMode::Process`] runs: [`set_child_binary`]'s
/// path, else [`CHILD_BINARY_ENV`], else a [`CHILD_BINARY_NAME`] sibling of
/// the running binary (and of its parent directory, which is where cargo puts
/// it relative to a `deps/` test or example binary). Every candidate is
/// returned in order when none exists, so the error names what was tried.
#[cfg(target_os = "linux")]
fn child_binary() -> Result<PathBuf, SpawnError> {
    if let Some(path) = CHILD_BINARY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return Ok(path);
    }
    if let Ok(path) = std::env::var(CHILD_BINARY_ENV)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    let mut tried = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        tried.push(dir.join(CHILD_BINARY_NAME));
        if let Some(parent) = dir.parent() {
            tried.push(parent.join(CHILD_BINARY_NAME));
        }
    }
    match tried.iter().find(|candidate| candidate.is_file()) {
        Some(found) => Ok(found.clone()),
        None => Err(SpawnError::ChildBinary { tried }),
    }
}

/// The backend's console input hook: host bytes for the guest to read from its
/// serial console (see [`VmRuntime::set_input_hook`]).
///
/// The reason is the caller-facing text of a [`SpawnError::SerialInput`]; it is
/// a plain string because the hook is dispatched dynamically across backends,
/// each of which has its own console error type.
type InputHook = Box<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;

/// A VM's memory as its balloon reports it, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStats {
    /// The VM's whole memory ([`VmSpec::memory_mib`]).
    pub max_mib: u32,
    /// What the guest may use now (the host's target).
    pub usable_mib: u32,
    /// What the balloon actually holds (the guest's acknowledged size).
    pub held_mib: u64,
    /// Free memory inside the guest, once it has reported it.
    pub free_mib: Option<u64>,
    /// Memory available without swapping inside the guest, once reported.
    pub available_mib: Option<u64>,
}

/// How the host resizes a running VM's memory: in-process, the balloon device
/// itself; for a VM in its own process, a request over that process's control
/// socket.
pub trait MemoryControl: Send + Sync {
    /// Sets the memory the guest may use, in MiB.
    fn set_usable_mib(&self, mib: u32) -> Result<(), SpawnError>;
    /// The latest counters.
    fn stats(&self) -> MemoryStats;
}

/// State shared between the spawner, the VM handle and the active backend.
///
/// The spawner creates one per VM and hands it to the backend's `boot`, which
/// installs the stop control and the console input hook, registers the threads
/// it spawned and, when the guest stops the VM by itself, marks the exit as
/// guest-initiated. The spawner then owns the VM: [`VmHandle::wait`] joins the
/// registered threads, [`VmHandle::kill`] first invokes the stop control and
/// [`VmHandle::serial_input`] calls the input hook.
pub struct VmRuntime {
    /// 0-based id of the VM this runtime belongs to.
    pub id: u32,
    /// Guest serial (console) capture. The backend's UART appends to it; the
    /// VM handle reads it (see [`VmHandle::serial`]). In
    /// [`SpawnMode::Process`] the child's console stream is appended here by
    /// the mode's reader thread, so callers see the same growing capture.
    pub serial: Arc<Mutex<Vec<u8>>>,
    /// The pid of the VM's per-VM process, when the VM runs in one (`0` = the
    /// VM is this process's threads, the v0 shape).
    child_pid: AtomicU32,
    /// The per-VM process's own log (its stderr), when the VM runs in one: the
    /// last lines it wrote, bounded by the mode's reader. Empty in
    /// [`SpawnMode::InProcess`], where there is no child to log. This is what
    /// an operator reads when a VM's confinement is in question — the jailer
    /// reports where it jailed the VM (or why it could not) on this stream.
    pub process_log: Arc<Mutex<Vec<String>>>,
    /// Stop control installed by the backend in `boot`: asks the VM's vCPU
    /// loop to finish. Left installed after it is invoked, so that
    /// [`VmHandle::kill`] can invoke it again when the first request did not
    /// stop the VM.
    stop: Mutex<Option<StopControl>>,
    /// Console input hook installed by the backend in `boot`: hands host bytes
    /// to the VM's UART, which queues them and raises the console's interrupt
    /// (see [`VmHandle::serial_input`]).
    input: Mutex<Option<InputHook>>,
    /// Memory control installed by the backend in `boot` when the VM has a
    /// balloon (see [`VmHandle::set_usable_memory_mib`]).
    memory: Mutex<Option<Arc<dyn MemoryControl>>>,
    /// Pause/resume/snapshot control installed by a backend that supports snapshots (see
    /// [`VmHandle::snapshot`]).
    vm_control: Mutex<Option<Arc<dyn VmControl>>>,
    /// Whether the stop control has been invoked.
    stop_requested: AtomicBool,
    /// Whether the guest ended the VM itself.
    guest_exited: AtomicBool,
    /// Whether the guest exit was a reset rather than a shutdown.
    guest_reset: AtomicBool,
    /// The backend's threads; every one of them must finish for the VM to be
    /// considered stopped.
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for VmRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmRuntime")
            .field("id", &self.id)
            .field(
                "serial_len",
                &self
                    .serial
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len(),
            )
            .field("stop_requested", &self.stop_requested())
            .field("guest_exited", &self.guest_exited.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl VmRuntime {
    /// Creates the runtime of VM `id`: an empty serial capture, no stop
    /// control and no threads (the backend fills those in).
    pub fn new(id: u32) -> Self {
        Self {
            id,
            serial: Arc::new(Mutex::new(Vec::new())),
            child_pid: AtomicU32::new(0),
            process_log: Arc::new(Mutex::new(Vec::new())),
            stop: Mutex::new(None),
            input: Mutex::new(None),
            memory: Mutex::new(None),
            vm_control: Mutex::new(None),
            stop_requested: AtomicBool::new(false),
            guest_exited: AtomicBool::new(false),
            guest_reset: AtomicBool::new(false),
            threads: Mutex::new(Vec::new()),
        }
    }

    /// Installs the stop control. Called by the backend from `boot`, before
    /// it returns.
    pub fn set_stop_control(&self, stop: impl Fn() + Send + Sync + 'static) {
        *self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(stop));
    }

    /// Installs the memory control of a VM that has a balloon. Called by the
    /// backend from `boot`.
    pub fn set_memory_control(&self, control: Arc<dyn MemoryControl>) {
        *self
            .memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(control);
    }

    /// The VM's memory control, `None` when it has no balloon.
    pub fn memory_control(&self) -> Option<Arc<dyn MemoryControl>> {
        self.memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Installs the VM's pause/resume/snapshot control (a backend with snapshot support, in `boot`).
    pub fn set_vm_control(&self, control: Arc<dyn VmControl>) {
        *self
            .vm_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(control);
    }

    /// The VM's pause/resume/snapshot control, when its backend installed one.
    pub fn vm_control(&self) -> Option<Arc<dyn VmControl>> {
        self.vm_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Installs the console input hook. Called by the backend from `boot`,
    /// before it returns, whether or not the boot succeeds.
    ///
    /// The hook hands `data` to the VM's UART, which queues the bytes for the
    /// guest to read through its data register and raises the console's
    /// received-data interrupt (that is what makes a guest reading `ttyS0` —
    /// a `console=ttyS0` kernel, or a `PID 1` whose stdin is the serial line —
    /// notice input without polling). It returns `Err(reason)` when the
    /// console could not take the bytes *at all*; nothing is queued in that
    /// case, so a caller can retry the same bytes.
    pub fn set_input_hook(
        &self,
        input: impl Fn(&[u8]) -> Result<(), String> + Send + Sync + 'static,
    ) {
        *self
            .input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(input));
    }

    /// Forwards `data` to the guest's serial console: this is the host side of
    /// the guest's console input (see [`VmHandle::serial_input`]).
    pub fn serial_input(&self, data: &[u8]) -> Result<(), SpawnError> {
        let input = self
            .input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(input) = input.as_ref() else {
            return Err(SpawnError::SerialInput {
                id: self.id,
                reason: "this VM's backend has no console input".to_string(),
            });
        };
        input(data).map_err(|reason| SpawnError::SerialInput {
            id: self.id,
            reason,
        })
    }

    /// Registers a thread the spawner must join before the VM counts as
    /// stopped. Called by the backend for every thread it spawns, before
    /// `boot` returns.
    pub fn add_thread(&self, thread: JoinHandle<()>) {
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(thread);
    }

    /// Marks the VM as having been stopped by the guest itself. Called by the
    /// backend when its vCPU loop ends on a guest-initiated exit (shutdown,
    /// reset, triple fault) rather than on a stop request.
    pub fn mark_guest_exited(&self) {
        self.guest_exited.store(true, Ordering::SeqCst);
    }

    /// Marks the guest exit as a reset (a PSCI `SYSTEM_RESET`, or a KVM
    /// `KVM_SYSTEM_EVENT_RESET`) rather than a shutdown. Called by the
    /// backend's exit watcher alongside [`VmRuntime::mark_guest_exited`].
    pub fn mark_guest_reset(&self) {
        self.guest_reset.store(true, Ordering::SeqCst);
        self.mark_guest_exited();
    }

    /// Whether the stop control has been invoked for this VM.
    pub fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    /// Records the pid of the VM's per-VM process. Called by
    /// [`SpawnMode::Process`]'s boot, before it returns.
    pub fn set_process_id(&self, pid: u32) {
        self.child_pid.store(pid, Ordering::SeqCst);
    }

    /// The pid of the VM's per-VM process, or `None` when the VM runs in this
    /// process ([`SpawnMode::InProcess`]).
    pub fn process_id(&self) -> Option<u32> {
        match self.child_pid.load(Ordering::SeqCst) {
            0 => None,
            pid => Some(pid),
        }
    }

    /// Invokes the stop control and records the request.
    ///
    /// The control stays installed, so that a stop that did not take (a wake
    /// lost to a vCPU that was just parking, say) can be retried instead of
    /// leaving a live VM behind; invoking it more than once is harmless, since
    /// it only sets a flag and wakes the vCPUs.
    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        let stop = self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(stop) = stop.as_ref() {
            stop();
        }
    }

    /// Whether every thread the backend registered has finished.
    pub(crate) fn all_threads_finished(&self) -> bool {
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .all(JoinHandle::is_finished)
    }

    /// Joins the backend's threads (draining the registry) and turns a
    /// panicked thread into an error.
    ///
    /// The backends call this from their own error paths too, to make sure a
    /// VM that failed to come up has no thread left behind: a vCPU thread
    /// owns the VM and its guest memory.
    pub(crate) fn join_threads(&self) -> Result<(), SpawnError> {
        let threads = std::mem::take(
            &mut *self
                .threads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for handle in threads {
            if handle.join().is_err() {
                return Err(SpawnError::ThreadPanic(self.id));
            }
        }
        Ok(())
    }

    /// How the VM stopped: guest-initiated unless it was killed.
    fn exit_reason(&self) -> VmExit {
        if self.guest_exited.load(Ordering::SeqCst) {
            VmExit::GuestExited {
                reset: self.guest_reset.load(Ordering::SeqCst),
            }
        } else {
            VmExit::Killed
        }
    }
}

/// A running microVM. Dropping the handle does not stop the VM.
#[derive(Debug)]
pub struct VmHandle {
    id: u32,
    runtime: Arc<VmRuntime>,
}

impl VmHandle {
    /// The VM's id: its 0-based index among the VMs [`spawn_n`] created.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The per-VM process's own log (its stderr) so far, line by line; empty
    /// when the VM runs as threads of this process ([`SpawnMode::InProcess`]).
    ///
    /// This carries the child's boot diagnostics — including the jailer's own
    /// line, which names the root the VM was jailed in or the reason this
    /// process could not jail it — so a caller can tell a confined VM from a
    /// VM that only kept the seccomp half without guessing.
    pub fn process_log(&self) -> Vec<String> {
        self.runtime
            .process_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The pid of the VM's per-VM process, or `None` when the VM runs as
    /// threads of this process ([`SpawnMode::InProcess`]).
    ///
    /// This is the handle's window onto the jail: the VM's confinement is the
    /// child process's root, mount namespace and seccomp state, which an
    /// operator reads from `/proc/{pid}` — not from anything the daemon says
    /// about itself.
    pub fn process_id(&self) -> Option<u32> {
        self.runtime.process_id()
    }

    /// Blocks until the VM has stopped and reports how it stopped.
    pub fn wait(&mut self) -> Result<VmExit, SpawnError> {
        self.runtime.join_threads()?;
        Ok(self.runtime.exit_reason())
    }

    /// Blocks until the VM has stopped, or until `timeout` elapses.
    ///
    /// Returns `None` if the VM is still running when the timeout elapses
    /// (the VM keeps running: use [`VmHandle::kill`] to stop it).
    pub fn wait_timeout(&mut self, timeout: Duration) -> Result<Option<VmExit>, SpawnError> {
        // `None` (a timeout so large it overflows) is an unbounded wait.
        let deadline = Instant::now().checked_add(timeout);
        while !self.runtime.all_threads_finished() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok(None);
            }
            thread::sleep(WAIT_POLL_INTERVAL);
        }
        self.runtime.join_threads()?;
        Ok(Some(self.runtime.exit_reason()))
    }

    /// Asks the VM to stop and waits for it to stop.
    ///
    /// Idempotent: killing a VM that already stopped returns `Ok(())`. A stop
    /// request that does not take within [`KILL_TIMEOUT`] is repeated once
    /// (the stop control stays installed for exactly that) before the kill is
    /// reported as a timeout.
    pub fn kill(&mut self) -> Result<(), SpawnError> {
        for _ in 0..KILL_ATTEMPTS {
            self.runtime.request_stop();
            if self.wait_timeout(KILL_TIMEOUT)?.is_some() {
                return Ok(());
            }
        }
        Err(SpawnError::KillTimeout {
            id: self.id,
            secs: KILL_TIMEOUT.as_secs() * KILL_ATTEMPTS as u64,
        })
    }

    /// The guest serial output captured so far (lossy UTF-8).
    pub fn serial(&self) -> String {
        let buffer = self
            .runtime
            .serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// The live serial capture, shared with the VM's UART: bytes appended
    /// between two calls are visible here immediately. Used by `vmctl` to
    /// stream guest output while the VM runs.
    pub fn serial_sink(&self) -> Arc<Mutex<Vec<u8>>> {
        Arc::clone(&self.runtime.serial)
    }

    /// Forwards `data` to the guest's serial console: the host's console
    /// input, the counterpart of [`VmHandle::serial`]'s captured output.
    ///
    /// The bytes go into the console's input queue and the console's interrupt
    /// is raised, so a guest reading its serial line — a `console=ttyS0`
    /// kernel console, a shell on `ttyS0`, or a `PID 1` whose stdin is that
    /// line — receives them as typed input. A guest terminal echoes what it
    /// receives, so the bytes a caller sends usually show up in
    /// [`VmHandle::serial`] as well.
    ///
    /// This is deliberately *not* a byte stream: the call takes all of `data`
    /// or none of it. It fails, without queueing anything, when the console's
    /// input queue cannot take the whole slice (the guest has not drained it
    /// yet) or when the VM has stopped. A caller that must not lose input
    /// retries the same slice once the guest has drained the queue, which is
    /// what `vmctl --console` does.
    pub fn serial_input(&self, data: &[u8]) -> Result<(), SpawnError> {
        self.runtime.serial_input(data)
    }

    /// The VM's shared runtime: everything [`VmHandle`] exposes except the
    /// `&mut self` control — [`VmRuntime::serial_input`], the serial capture,
    /// the recorded exit reason — for code that has to keep using the VM from
    /// another thread while the handle itself is being waited on or killed
    /// (`vmctl`'s stdin reader is the first such caller).
    pub fn runtime(&self) -> Arc<VmRuntime> {
        Arc::clone(&self.runtime)
    }

    /// Lets the guest use `mib` MiB of its memory: the balloon deflates to
    /// grow it or inflates to take memory back (the guest frees pages first,
    /// and the host discards their backing). Clamped to [`VmSpec::memory_mib`].
    pub fn set_usable_memory_mib(&self, mib: u32) -> Result<(), SpawnError> {
        self.runtime
            .memory_control()
            .ok_or(SpawnError::NoBalloon)?
            .set_usable_mib(mib)
    }

    /// The guest's memory as its balloon last reported it, `None` when the VM
    /// has no balloon.
    pub fn memory_stats(&self) -> Option<MemoryStats> {
        self.runtime.memory_control().map(|control| control.stats())
    }

    /// Pauses the VM (see [`VmControl::pause`]).
    pub fn pause(&self) -> Result<(), SpawnError> {
        self.runtime
            .vm_control()
            .ok_or(SpawnError::NoSnapshots)?
            .pause()
    }

    /// Resumes a paused VM (see [`VmControl::resume`]).
    pub fn resume(&self) -> Result<(), SpawnError> {
        self.runtime
            .vm_control()
            .ok_or(SpawnError::NoSnapshots)?
            .resume()
    }

    /// Writes a snapshot of the VM into `dir` and leaves it paused (see [`VmControl::snapshot`]).
    pub fn snapshot(&self, dir: &Path, kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        self.runtime
            .vm_control()
            .ok_or(SpawnError::NoSnapshots)?
            .snapshot(dir, kind)
    }
}

/// Boots one microVM.
pub fn spawn(spec: VmSpec) -> Result<VmHandle, SpawnError> {
    spawn_with_id(spec, 0)
}

/// Boots `n` microVMs, with ids `0..n`, each built from the spec `spec`
/// returns for that id.
///
/// This is the entry point for a caller whose VMs do not share one
/// [`VmSpec`] — the fleet's VMs each have their own vsock CID, and the caller
/// assigns it (see [`VsockConfig`]), so it has to build a spec per id. Every
/// other property may differ too, of course: nothing here assumes the specs are
/// alike beyond their being valid.
///
/// If one VM fails to boot, the ones that did boot are killed before the error
/// is returned; when one of those cannot be stopped either, the returned error
/// is a [`SpawnError::PartialCleanup`] that names it.
pub fn spawn_each(n: usize, spec: impl Fn(u32) -> VmSpec) -> Result<Vec<VmHandle>, SpawnError> {
    let mut handles = Vec::with_capacity(n.min(1024));
    for id in 0..n {
        let id = u32::try_from(id).map_err(|_| SpawnError::TooManyVms)?;
        match spawn_with_id(spec(id), id) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                let err = cleanup_after_boot_failure(err, &mut handles);
                return Err(err);
            }
        }
    }
    Ok(handles)
}

/// Boots `n` microVMs, with ids `0..n`, all from the same [`VmSpec`].
///
/// If one VM fails to boot, the ones that did boot are killed before the error
/// is returned; when one of those cannot be stopped either, the returned error
/// is a [`SpawnError::PartialCleanup`] that names it.
pub fn spawn_n(spec: VmSpec, n: usize) -> Result<Vec<VmHandle>, SpawnError> {
    spawn_each(n, |_| spec.clone())
}

/// Stops the VMs that did start after one of a batch failed to boot.
///
/// Returns the boot failure, or - when a VM could not be stopped either - an
/// error that carries both it and the ids of the VMs left running, because a
/// caller told only "the boot failed" would not know there are live VMs to
/// reap.
fn cleanup_after_boot_failure(boot: SpawnError, started: &mut [VmHandle]) -> SpawnError {
    let mut unstoppable = Vec::new();
    for handle in started.iter_mut() {
        if let Err(err) = handle.kill() {
            eprintln!(
                "warning: VM {} could not be stopped while cleaning up after a boot failure: {err}",
                handle.id()
            );
            unstoppable.push(handle.id());
        }
    }
    if unstoppable.is_empty() {
        boot
    } else {
        SpawnError::PartialCleanup {
            boot: Box::new(boot),
            vms: unstoppable,
        }
    }
}

fn spawn_with_id(spec: VmSpec, id: u32) -> Result<VmHandle, SpawnError> {
    validate_spec(&spec)?;
    let runtime = Arc::new(VmRuntime::new(id));
    boot(&spec, Arc::clone(&runtime))?;
    Ok(VmHandle { id, runtime })
}

/// Rejects a [`VmSpec`] the spawner must not build.
///
/// Today that is only the guest CID range: the vsock device no longer checks
/// it (the userspace device's CID is not a host resource), so the check lives
/// here, once, on the path [`spawn`]/[`spawn_n`]/[`spawn_each`] share. A CID
/// outside [`MIN_GUEST_CID`]..=[`MAX_GUEST_CID`] would boot a guest whose
/// config space claims a reserved (or unconvertible) CID and that no host
/// listener path can match, so it is refused before any VM is built.
fn validate_spec(spec: &VmSpec) -> Result<(), SpawnError> {
    if let Some(vsock) = &spec.vsock {
        if !(MIN_GUEST_CID..=MAX_GUEST_CID).contains(&vsock.guest_cid) {
            return Err(SpawnError::GuestCidOutOfRange {
                guest_cid: vsock.guest_cid,
                min: MIN_GUEST_CID,
                max: MAX_GUEST_CID,
            });
        }
    }
    Ok(())
}

/// Creates the directory that holds a VM's (or the orchestrator's) host-side
/// vsock Unix sockets, with mode `0700`.
///
/// The socket names under the directory are deterministic - `{dir}/vm{cid}`
/// for a VM's muxer, `{dir}/vm{cid}_{port}` for the orchestrator - and the
/// default directory is under world-writable `/tmp`. Without this, any local
/// user could squat a path before it is bound (failing a boot or a daemon
/// start with `EADDRINUSE`) or bind `{dir}/vm{cid}_{port}` first and answer as
/// the orchestrator. The directory is created (with parents) `0700`, and an
/// existing directory is re-tightened to `0700` - the sockets inside are a
/// trust boundary, so a wider mode is not accepted. A directory that cannot be
/// made `0700` (one owned by another user, say) is an error the caller reports
/// rather than a reason to bind sockets in the open.
#[cfg(unix)]
pub fn create_uds_dir(path: &Path) -> std::io::Result<()> {
    create_private_dir(path)
}

/// Creates `path` (with its parents) as a `0700` directory, re-tightening one
/// that already exists.
///
/// `mode()` in `DirBuilder` is masked by the process umask, so the mode is set
/// explicitly afterwards as well. A directory that cannot be made `0700` (one
/// owned by another user, say) is an error the caller reports rather than a
/// reason to lay private files out in the open.
#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Errors from [`materialize_workspaces`].
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// The run's workspace directory could not be created.
    #[error("cannot create the workspace directory {path}: {source}")]
    Directory {
        /// The directory that could not be created.
        path: PathBuf,
        /// The error the creation failed with.
        source: std::io::Error,
    },
    /// A per-VM workspace image could not be copied.
    #[error("cannot copy the workspace image {base} to {copy}: {source}")]
    Copy {
        /// The base image the caller configured.
        base: PathBuf,
        /// The per-VM copy that could not be created.
        copy: PathBuf,
        /// The error the copy failed with.
        source: std::io::Error,
    },
}

/// The chunk [`copy_sparse`] moves through memory at a time.
#[cfg(unix)]
const COPY_CHUNK: usize = 1 << 20;

/// The granule of [`copy_sparse`]'s fallback: a block this size that is all
/// zero is skipped, so the copy keeps a hole there.
#[cfg(unix)]
const ZERO_BLOCK: usize = 64 << 10;

/// Copies `from` to `to` like `std::fs::copy`, but keeps the holes of a sparse
/// source as holes, so the copy costs only the blocks that hold data. Returns
/// the file's length, which is what `std::fs::copy` returns for the bytes it
/// copied.
///
/// The destination is created (truncating any file there), sized to the source
/// and given its permissions. On Linux the data extents come from
/// `SEEK_DATA`/`SEEK_HOLE`; a filesystem that cannot report holes (`EINVAL`),
/// and every other Unix, gets a fallback that reads the whole source but writes
/// only the 64 KiB blocks holding a non-zero byte.
#[cfg(unix)]
pub fn copy_sparse(from: &Path, to: &Path) -> std::io::Result<u64> {
    let src = std::fs::File::open(from)?;
    let meta = src.metadata()?;
    let len = meta.len();
    let dst = std::fs::File::create(to)?;
    dst.set_len(len)?;
    dst.set_permissions(meta.permissions())?;
    let mut buf = vec![0u8; COPY_CHUNK];
    #[cfg(target_os = "linux")]
    {
        if copy_data_extents(&src, &dst, len, &mut buf)? {
            return Ok(len);
        }
    }
    copy_skipping_zero_blocks(&src, &dst, len, &mut buf)?;
    Ok(len)
}

/// Copies the data extents of `src` into `dst`, found with `SEEK_DATA` and
/// `SEEK_HOLE`. `Ok(false)` means the filesystem cannot report holes (`EINVAL`):
/// the caller falls back and rewrites what was copied, identically.
#[cfg(target_os = "linux")]
fn copy_data_extents(
    src: &std::fs::File,
    dst: &std::fs::File,
    len: u64,
    buf: &mut [u8],
) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let fd = src.as_raw_fd();
    let mut at = 0u64;
    while at < len {
        let start = match seek_extent(fd, at, libc::SEEK_DATA) {
            Ok(Some(start)) => start,
            Ok(None) => break,
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => return Ok(false),
            Err(err) => return Err(err),
        };
        let end = seek_extent(fd, start, libc::SEEK_HOLE)?
            .unwrap_or(len)
            .min(len);
        if end <= start {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SEEK_HOLE did not move past the data it followed",
            ));
        }
        copy_range(src, dst, start, end, buf)?;
        at = end;
    }
    Ok(true)
}

/// `lseek` to the next data or hole at or after `at`. `Ok(None)` is `ENXIO`:
/// no data lies at or after `at`.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // the one syscall std does not expose: lseek with SEEK_DATA/SEEK_HOLE
fn seek_extent(
    fd: std::os::unix::io::RawFd,
    at: u64,
    whence: libc::c_int,
) -> std::io::Result<Option<u64>> {
    // SAFETY: lseek only moves the descriptor's file offset; `fd` is the live
    // descriptor of the caller's `File`, and the copy reads by offset (pread).
    let off = unsafe { libc::lseek(fd, at as libc::off_t, whence) };
    if off >= 0 {
        return Ok(Some(off as u64));
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ENXIO) {
        Ok(None)
    } else {
        Err(err)
    }
}

/// Copies `[start, end)` of `src` to the same offsets of `dst`, a buffer at a time.
#[cfg(target_os = "linux")]
fn copy_range(
    src: &std::fs::File,
    dst: &std::fs::File,
    start: u64,
    end: u64,
    buf: &mut [u8],
) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    let mut at = start;
    while at < end {
        let n = buf.len().min((end - at) as usize);
        src.read_exact_at(&mut buf[..n], at)?;
        write_fully_at(dst, &buf[..n], at)?;
        at += n as u64;
    }
    Ok(())
}

/// The fallback: reads all of `src` and writes only the `ZERO_BLOCK`s that hold
/// a non-zero byte. `dst` is a fresh file of the source's length, so a skipped
/// block already reads as zero.
#[cfg(unix)]
fn copy_skipping_zero_blocks(
    src: &std::fs::File,
    dst: &std::fs::File,
    len: u64,
    buf: &mut [u8],
) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    let mut at = 0u64;
    while at < len {
        let n = buf.len().min((len - at) as usize);
        src.read_exact_at(&mut buf[..n], at)?;
        for (i, block) in buf[..n].chunks(ZERO_BLOCK).enumerate() {
            if block.iter().any(|&b| b != 0) {
                write_fully_at(dst, block, at + (i * ZERO_BLOCK) as u64)?;
            }
        }
        at += n as u64;
    }
    Ok(())
}

/// Writes all of `data` to `dst` at `at`, looping over short writes.
#[cfg(unix)]
fn write_fully_at(dst: &std::fs::File, mut data: &[u8], mut at: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !data.is_empty() {
        let n = dst.write_at(data, at)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "the copy wrote no bytes",
            ));
        }
        data = &data[n..];
        at += n as u64;
    }
    Ok(())
}

/// Copies `base` into `dir` as `workspace-{0..n}.ext4`: one private workspace
/// image per VM of an `n`-VM run.
///
/// A workspace volume is *writable*, so the VMs of a run must not share one
/// image — each gets its own copy, in the run's own directory, and the caller
/// hands each VM its path through [`VmSpec::workspace`]. The directory is
/// created `0700` (see [`create_uds_dir`]): an agent's workspace is its private
/// data, and under the default `/tmp` prefix a wider mode would let any local
/// user read or rewrite it. [`copy_sparse`] keeps the base's holes as holes, so a copy costs the blocks
/// its VM writes, not the image's length. The copies happen once per run, before
/// its VMs boot.
pub fn materialize_workspaces(
    base: &Path,
    dir: &Path,
    n: usize,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    create_private_dir(dir).map_err(|source| WorkspaceError::Directory {
        path: dir.to_path_buf(),
        source,
    })?;
    (0..n)
        .map(|id| {
            let copy = dir.join(format!("workspace-{id}.ext4"));
            copy_sparse(base, &copy).map_err(|source| WorkspaceError::Copy {
                base: base.to_path_buf(),
                copy: copy.clone(),
                source,
            })?;
            Ok(copy)
        })
        .collect()
}

/// Hands the VM to the backend of the current platform.
///
/// Linux/KVM uses the arch-dispatched `crate::builder::boot`; Apple Silicon
/// uses `crate::hvf::boot(spec, id, runtime)`, which must:
///
/// * append guest console output to `runtime.serial`,
/// * install its stop control with [`VmRuntime::set_stop_control`],
/// * install its console input hook with [`VmRuntime::set_input_hook`],
/// * register every thread it spawned with [`VmRuntime::add_thread`],
/// * call [`VmRuntime::mark_guest_exited`] (or
///   [`VmRuntime::mark_guest_reset`], for a reset) when the guest stopped the
///   VM itself, and
/// * return once the VM is running (not when it stops).
///
/// [`SpawnMode::Process`] takes the other road first: the VM is built by a
/// child `apz-vmm` process (see [`process`]), and the parent's job is the
/// proxy that satisfies the same six rules from outside — which is what makes
/// the two modes interchangeable to a caller.
fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    if spawn_mode()? == SpawnMode::Process {
        #[cfg(target_os = "linux")]
        {
            return process::boot(spec, runtime);
        }
        // The per-VM process boots through `crate::builder`, which is the
        // Linux/KVM path. Windows has no `apz-vmm` binary either, so there the
        // request boots in-process (warned once); macOS refuses the mode rather
        // than silently downgrading it.
        #[cfg(windows)]
        {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                eprintln!("vmm: APZ_VMM_SPAWN=process is Linux-only; booting in-process");
            });
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = (spec, runtime);
            return Err(SpawnError::SpawnMode {
                reason: "the per-VM process mode is Linux-only: this platform has no `apz-vmm` \
                         binary to run"
                    .to_string(),
            });
        }
    }

    #[cfg(target_os = "linux")]
    {
        crate::builder::boot(spec, runtime)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        crate::hvf::boot(spec, runtime.id, runtime)
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        crate::whp::boot(spec, runtime)
    }

    #[cfg(not(any(
        target_os = "linux",
        all(target_os = "macos", target_arch = "aarch64"),
        all(windows, target_arch = "x86_64")
    )))]
    {
        let _ = (spec, runtime);
        Err(SpawnError::NoBackend)
    }
}

/// Creates `path` (with its parents). Windows has no mode bits, so the directory is not chmod'ed:
/// it sits in the user's profile (`%LOCALAPPDATA%\AgentPlaza\state\vsock`), whose inherited ACL already
/// restricts it to that user.
#[cfg(windows)]
pub fn create_uds_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Windows counterpart of the unix `0700` directory: see [`create_uds_dir`] for why the profile's ACL
/// is the protection here.
#[cfg(windows)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Copies `from` to `to` in full and returns the bytes copied. NTFS sparse ranges are not attempted,
/// so on Windows a workspace copy costs the image's full length, not just the blocks its VM writes.
#[cfg(windows)]
pub fn copy_sparse(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::copy(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this process and test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-spawner-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A run's workspace images are one per VM, byte-identical to the base
    /// image, named for the VM that owns them, inside a directory only the
    /// run's user can reach.
    #[test]
    fn a_run_gets_one_workspace_copy_per_vm() {
        let dir = scratch("workspaces");
        let base = dir.join("base.ext4");
        std::fs::write(&base, b"a workspace image").expect("the base image");
        let run_dir = dir.join("run-1");

        let copies = materialize_workspaces(&base, &run_dir, 3).expect("the run's copies");

        assert_eq!(copies.len(), 3, "one image per VM");
        for (index, copy) in copies.iter().enumerate() {
            assert_eq!(copy, &run_dir.join(format!("workspace-{index}.ext4")));
            assert_eq!(
                std::fs::read(copy).expect("a readable copy"),
                b"a workspace image",
                "a copy is the base's bytes"
            );
        }
        // No copy is the base, and the base is untouched: a VM's writes must
        // never reach the image the next run copies from.
        assert!(copies.iter().all(|copy| copy != &base));
        assert_eq!(
            std::fs::read(&base).expect("the base image"),
            b"a workspace image"
        );
        // The run's directory is private, exactly as the vsock sockets' is: a
        // workspace is an agent's own data.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&run_dir)
                .expect("the run's directory")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "the run's directory is 0700");
        }

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A base image that cannot be read is an error naming both paths, so the
    /// caller reports it instead of booting a guest with no volume.
    #[test]
    fn a_missing_base_image_is_an_error() {
        let dir = scratch("missing-base");
        let base = dir.join("missing.ext4");
        let run_dir = dir.join("run-2");

        let err = materialize_workspaces(&base, &run_dir, 1).expect_err("a missing base");
        let text = err.to_string();
        assert!(text.contains("missing.ext4"), "{text}");
        assert!(text.contains("workspace-0.ext4"), "{text}");
        assert!(
            !run_dir.join("workspace-0.ext4").exists(),
            "a failed copy leaves no image behind"
        );

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A sparse image copies without its holes: a 64 MiB file with 1 MiB of data
    /// at 32 MiB copies to the same bytes, and the copy allocates the data's
    /// blocks, not the file's length. The zero-block fallback makes the same copy.
    #[cfg(unix)]
    #[test]
    fn a_sparse_image_keeps_its_holes_in_the_copy() {
        use std::io::{Seek, SeekFrom, Write};
        use std::os::unix::fs::MetadataExt;
        let dir = scratch("sparse");
        let base = dir.join("sparse.ext4");
        {
            let mut file = std::fs::File::create(&base).expect("the sparse base");
            file.set_len(64 << 20).expect("64 MiB, mostly holes");
            file.seek(SeekFrom::Start(32 << 20))
                .expect("seek to 32 MiB");
            let data = vec![0xA5u8; 1 << 20];
            file.write_all(&data).expect("1 MiB of data at 32 MiB");
        }
        let want = std::fs::read(&base).expect("the base");

        let copy = dir.join("copy.ext4");
        assert_eq!(copy_sparse(&base, &copy).expect("a sparse copy"), 64 << 20);
        assert_eq!(std::fs::read(&copy).expect("the copy"), want);
        let allocated = std::fs::metadata(&copy)
            .expect("the copy's metadata")
            .blocks()
            * 512;
        assert!(allocated < 4 << 20, "the copy allocated {allocated} bytes");

        let fallback = dir.join("fallback.ext4");
        let src = std::fs::File::open(&base).expect("the base");
        let dst = std::fs::File::create(&fallback).expect("the fallback copy");
        dst.set_len(64 << 20).expect("the fallback's length");
        copy_skipping_zero_blocks(&src, &dst, 64 << 20, &mut vec![0u8; COPY_CHUNK])
            .expect("the fallback");
        drop(dst);
        assert_eq!(std::fs::read(&fallback).expect("the fallback copy"), want);

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    fn spec_with_cid(guest_cid: u64) -> VmSpec {
        VmSpec {
            vsock: Some(VsockConfig {
                guest_cid,
                uds_path: PathBuf::from("/tmp/plaza-vsock/vm-test"),
            }),
            ..VmSpec::default()
        }
    }

    /// F3: the CID range is enforced on the boot path, at both ends.
    #[test]
    fn guest_cids_outside_the_supported_range_are_refused() {
        for cid in [0, 1, 2, MAX_GUEST_CID + 1, u64::MAX] {
            assert!(
                matches!(
                    validate_spec(&spec_with_cid(cid)),
                    Err(SpawnError::GuestCidOutOfRange { guest_cid, min, max })
                        if guest_cid == cid && min == MIN_GUEST_CID && max == MAX_GUEST_CID
                ),
                "guest CID {cid} must be refused"
            );
        }
    }

    /// F3: the boundaries themselves are accepted.
    #[test]
    fn the_range_boundaries_are_accepted() {
        assert!(validate_spec(&spec_with_cid(MIN_GUEST_CID)).is_ok());
        assert!(validate_spec(&spec_with_cid(MAX_GUEST_CID)).is_ok());
    }

    /// A VM with no vsock device has no CID to validate.
    #[test]
    fn a_spec_without_vsock_is_accepted() {
        assert!(validate_spec(&VmSpec::default()).is_ok());
    }

    /// The boot mode an `APZ_VMM_SPAWN` value names: exactly the two modes,
    /// trimmed and case-insensitive — and *nothing else*, because a typo must
    /// stop the VM rather than pick a mode the operator did not ask for.
    #[test]
    fn a_boot_mode_env_value_that_names_no_mode_is_refused() {
        assert_eq!(
            parse_spawn_mode_env("in-process").expect("in-process"),
            SpawnMode::InProcess
        );
        assert_eq!(
            parse_spawn_mode_env("  PROCESS\n").expect("process"),
            SpawnMode::Process
        );
        for typo in ["", " ", "inprocess", "child", "off", "on", "in process"] {
            assert!(
                matches!(
                    parse_spawn_mode_env(typo),
                    Err(SpawnError::SpawnMode { .. })
                ),
                "{typo:?} names no boot mode and must be refused"
            );
        }
    }

    /// The switch is programmatic — the library half of a CLI or of a
    /// deployment's configuration — and the default (nothing set) is the v0
    /// shape, in this process.
    #[test]
    fn the_boot_mode_is_programmatic_and_defaults_in_process() {
        set_spawn_mode(SpawnMode::Process);
        assert_eq!(spawn_mode().expect("a set mode"), SpawnMode::Process);
        set_spawn_mode(SpawnMode::InProcess);
        assert_eq!(spawn_mode().expect("a set mode"), SpawnMode::InProcess);
        // Back to "read the environment" for any later test in this binary.
        MODE.store(MODE_FROM_ENV, Ordering::SeqCst);
        // Nothing is set in the test process's environment: the default is the
        // in-process shape. The environment path itself is driven by
        // `tests/per_vm_process.rs`, which runs the mode through a child.
        assert_eq!(spawn_mode().expect("the default"), SpawnMode::InProcess);
    }

    /// F4: the vsock socket directory is created `0700`, and an existing wider
    /// one is re-tightened - `0700` is what keeps another local user from
    /// squatting or impersonating a path under it.
    #[test]
    #[cfg(unix)]
    fn the_uds_dir_is_created_and_tightened_to_0700() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("plaza-uds-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // A nested path also proves the parents are created.
        let dir = root.join("nested").join("uds");
        create_uds_dir(&dir).expect("the socket directory is created");
        let mode = std::fs::metadata(&dir)
            .expect("the directory exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "a fresh socket directory must be 0700");

        // An existing, wider directory is re-tightened rather than accepted.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777))
            .expect("widen the directory for the test");
        create_uds_dir(&dir).expect("an existing directory is accepted");
        let mode = std::fs::metadata(&dir)
            .expect("the directory exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "a pre-existing socket directory is tightened"
        );

        std::fs::remove_dir_all(&root).expect("clean up");
    }
}
