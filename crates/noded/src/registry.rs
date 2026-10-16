//! The run registry: run ids, the run state machine and the VM handles behind
//! a run.
//!
//! A run follows `core.run_transitions` (`docs/orchestrator-draft/schema.sql`)
//! restricted to the states noded v0 implements - `starting` may become
//! `running`, `failed` or `killed`, and `running` may become `done`, `failed`
//! or `killed`. Every other pair is refused, exactly as `core.check_run`
//! refuses it: a caller that asks for one gets a [`RefusedTransition`] rather
//! than a state the schema does not allow. Runs are created in `starting`
//! (where the schema says a run begins) and end with `ended_at` stamped, so a
//! run in `done`, `failed` or `killed` always has one.
//!
//! One thread per run watches its VMs and ends the run as `done` once every
//! one of them has stopped by itself. Because a [`VmHandle`]'s `wait` takes
//! `&mut` and blocks, exactly one thread may be inside a handle at a time: the
//! watcher polls with a bounded timeout under the run's lock, which is the
//! same lock a kill takes, so a kill and a wait never block on the same handle
//! at once.
//!
//! Every VM of every run carries the agent harness, and so every VM has a
//! guest CID. A create request that names no `vsock_cid` draws one span from
//! the node's run range ([`RUN_CID_BASE`]..=[`RUN_CID_END`]); the span is held
//! for the run's lifetime ([`CidBook`]) and released when the run ends, and a
//! create the node cannot give CIDs to is refused before any VM boots.
//!
//! Whether a run's guests are long-lived agents follows who chose the CIDs (see
//! [`RunSession`]) unless the request says otherwise: a caller who pins
//! `vsock_cid` gets the one-shot session, and a run the node allocated CIDs for
//! is an operator run of keepalive agents. A run with a `role` is always a
//! keepalive agent of that role, booted exactly as the role fleet boots its VMs.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::fleet::{Fleet, agent_flags};
use crate::snapshots::{self, SnapshotStore, SnapshotView};
use crate::volumes::VolumeStore;
use vmm::clonefile::clone_file;
use vmm::spawner::{
    self, DataVolume, MAX_GUEST_CID, MIN_GUEST_CID, SnapshotInfo, SnapshotKind, SpawnError, VmExit,
    VmHandle, VmRuntime, VmSpec, VsockConfig,
};

/// The most forks one `fork` request may make.
pub const MAX_FORKS: u32 = 8;

/// The states a VM may be in while a restart, wipe or stop operates on it: every state but asleep,
/// which has no process to stop.
const AWAKE: [VmState; 4] = [
    VmState::Running,
    VmState::Paused,
    VmState::Exited,
    VmState::Killed,
];

/// The most VMs one run may have (`n`).
pub const MAX_VMS: u32 = 64;

/// The most vCPUs one VM may have (`vcpus`).
pub const MAX_VCPUS: u32 = 64;

/// The least guest memory one VM may have, in MiB (`memory_mib`).
pub const MIN_MEMORY_MIB: u32 = 128;

/// The first guest CID of the range a run draws from when its create request
/// names no `vsock_cid`.
///
/// Every VM of a run has a CID - the guest's harness is the vsock client, and
/// the guest cannot have a harness without one - so a request that names none
/// is given a span of `RUN_CID_BASE..=RUN_CID_END`. The range is the node's
/// own: it is disjoint from the role fleet's default first CID (`--agents-cid-base`,
/// 100) and from the hub's long-standing served set (10..=15), it holds more
/// than one [`MAX_VMS`] run ([`MAX_VMS`] is 64 and the range is 80 wide, so a
/// full-size run always fits), and the hub has to serve it for the guests to
/// reach it (`vsockd --cids`).
pub const RUN_CID_BASE: u64 = 20;

/// The last guest CID of the run range, **inclusive**: a run's CIDs are
/// `RUN_CID_BASE..=RUN_CID_END` at the widest, so a run of [`MAX_VMS`] VMs
/// still fits.
pub const RUN_CID_END: u64 = 99;

/// The guest's console device: the first flag of every run's command line.
const CONSOLE: &str = "console=ttyS0";

/// How soon the run watcher looks at its VMs again after something changed: the
/// shortest it sleeps between passes. The watcher sleeps for its interval
/// *outside* the run's lock, so its polls stay non-blocking and no other caller
/// of the run can be starved by them.
const WATCH_INTERVAL_MIN: Duration = Duration::from_millis(5);

/// The longest the run watcher waits while nothing changes: a VM's exit or
/// serial output is seen at most this late.
const WATCH_INTERVAL_MAX: Duration = Duration::from_millis(250);

/// The newest a snapshot may be for a recovery to boot from it, in milliseconds: a day. An older
/// snapshot is a stale copy of an agent, so the recovery boots the VM cold instead.
const RECOVERY_SNAPSHOT_AGE_MS: u64 = 24 * 60 * 60 * 1000;

/// How many recoveries one VM may have within [`RECOVERY_WINDOW_MS`]. A VM that keeps crashing
/// is a crash loop: past this it stays exited, so the node does not boot it again in a tight loop.
const RECOVERY_LIMIT: usize = 3;

/// The window [`RECOVERY_LIMIT`] counts recoveries in, in milliseconds: ten minutes.
const RECOVERY_WINDOW_MS: u64 = 10 * 60 * 1000;

/// Whether noded recovers a VM that stops by itself (see [`set_crash_recovery`]). Set once at
/// start-up from `--no-recover`, and on by default.
static CRASH_RECOVERY: AtomicBool = AtomicBool::new(true);

/// Turns crash recovery on or off for the whole daemon: `--no-recover` turns it off. A VM that
/// stops by itself while it is off stays exited, as it did before recovery existed.
pub fn set_crash_recovery(on: bool) {
    CRASH_RECOVERY.store(on, Ordering::SeqCst);
}

/// Whether crash recovery is on (see [`set_crash_recovery`]).
fn crash_recovery() -> bool {
    CRASH_RECOVERY.load(Ordering::SeqCst)
}

/// The guest images every run boots from.
#[derive(Debug, Clone)]
pub struct VmImages {
    /// The guest kernel image.
    pub kernel: PathBuf,
    /// The guest initrd.
    pub initrd: PathBuf,
    /// The base image of every run's writable workspace volume, or `None` for
    /// runs whose guests have no block device.
    ///
    /// It is the *base*: a run never shares it. Each run gets one copy per VM
    /// (see [`Registry::create_run`]), because a workspace volume is writable
    /// and two guests sharing one image would corrupt each other's filesystem.
    pub workspace: Option<PathBuf>,
}

/// The `POST /v1/runs` body, as the client sent it.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateRun {
    /// How many VMs to boot.
    pub n: u32,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// Heartbeats before the guest reboots itself (churn mode); `None` runs
    /// the guest long-lived.
    #[serde(default)]
    pub beats: Option<u32>,
    /// Base guest CID for the run's VMs (`vsock_cid + i` for VM `i`), or
    /// `None` to let the node allocate a span from its run range
    /// ([`RUN_CID_BASE`]..=[`RUN_CID_END`]).
    ///
    /// Pinning the base is the caller's choice and is validated as it always
    /// was; a request that names none is given the first free span of the run
    /// range, and refused before any VM boots when the range has no room. The
    /// choice also selects the run's session (see [`RunSession`]): a pinned
    /// run is the demo task's frozen one-shot session, an allocated one is an
    /// operator run of long-running agents.
    #[serde(default)]
    pub vsock_cid: Option<u64>,
    /// The role every VM of the run runs as (`plaza.role=`), or `None` for a
    /// run with no role: 1 to 64 characters of `a-z`, `0-9` and `-`.
    #[serde(default)]
    pub role: Option<String>,
    /// Whether each guest is a long-lived keepalive agent. `None` takes the
    /// default for the run's CIDs (see [`RunSession`]); a roled run is always
    /// keepalive, so `false` is refused for one.
    #[serde(default)]
    pub keepalive: Option<bool>,
    /// The guest's usable memory per VM, in MiB, at boot. A VM given one boots
    /// with a balloon, and its usable memory can be resized later
    /// (`POST /v1/runs/{id}/vms/{index}/memory`). `None` boots every VM without
    /// a balloon.
    #[serde(default)]
    pub balloon_usable_mib: Option<u32>,
    /// Whether the run's guests are sandboxes: E2B-compatible VMs whose guest runs the envd-compatible
    /// daemon (`plaza.sandbox=1`) in place of an agent session. A sandbox has no role and is never a
    /// keepalive agent; its workspace is a per-run copy of the base image, never a persistent one.
    #[serde(default)]
    pub sandbox: bool,
    /// Shared data volumes (see [`crate::volumes`]) every VM of the run mounts read-only at
    /// `/data/<name>`, by name. A VM with volumes boots cold: warm templates carry none.
    #[serde(default)]
    pub volumes: Vec<String>,
}

/// The longest role name a run may carry.
pub const MAX_ROLE_LEN: usize = 64;

/// The harness session a run's guests speak when the request leaves
/// `keepalive` unset.
///
/// Both are started by `plaza.harness=1`; which one is the default follows who
/// chose the run's CIDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunSession {
    /// The frozen v1 session - `hello`, `welcome`, `echo`, `uptime`, `bye` -
    /// after which the guest's `init` reboots it, so the run ends as `done`.
    /// This is the demo task's contract, and it is what a run whose CIDs the
    /// caller **pinned** speaks by default: the acceptance suites drive it byte
    /// for byte, and it is why `beats` has any meaning there only as an upper
    /// bound.
    Frozen,
    /// The keepalive agent session: the guest registers on the hub, beats,
    /// serves the hub's tool calls, and its `init` never reboots it. A run
    /// whose CIDs the **node allocated** is an operator run - the owner's
    /// console creates one by leaving `vsock_cid` empty - so its VMs are
    /// agents the plaza can see and drive until the run is killed or `beats`
    /// churns the guest.
    Agent,
}

impl CreateRun {
    /// Checks the request's counts, its role, its keepalive choice and, when
    /// the caller named one, its `vsock_cid`; returns how many VMs the run has
    /// and the base the caller named.
    ///
    /// `n` is 1..=64, `vcpus` is 1..=64, `memory_mib` is at least 128, and a
    /// named `vsock_cid` must be a base whose whole span `base..base + n` fits
    /// the vsock transport. Anything else is refused with the reason, before
    /// any run exists - and before any CID is allocated, so a malformed shape
    /// never reaches the allocator.
    fn checked(&self) -> Result<(usize, Option<u64>), String> {
        let vms = checked_count("n", self.n, 1, MAX_VMS)?;
        // The shape is checked here at the edge as well as in `boot_run`,
        // which every creation path passes: a malformed count is refused
        // before a run id, a CID or a workspace copy is spent on it.
        spawner_vcpus(self.vcpus)?;
        spawner_memory_mib(self.memory_mib)?;
        if let Some(usable) = self.balloon_usable_mib {
            if usable == 0 || usable > self.memory_mib {
                return Err(format!(
                    "balloon_usable_mib must be between 1 and memory_mib ({})",
                    self.memory_mib
                ));
            }
        }
        if self.sandbox {
            if self.role.is_some() {
                return Err("a sandbox has no role".to_string());
            }
            if self.keepalive == Some(true) {
                return Err(
                    "a sandbox is not a keepalive agent: keepalive cannot be true".to_string(),
                );
            }
        }
        if let Some(role) = &self.role {
            checked_role(role)?;
            if self.keepalive == Some(false) {
                return Err(
                    "a run with a role is a keepalive agent: keepalive cannot be false".to_string(),
                );
            }
        }
        let named = match self.vsock_cid {
            Some(base) => Some(checked_base(base, vms)?),
            None => None,
        };
        Ok((vms, named))
    }

    /// Whether the run's guests are keepalive agents: always for a roled run,
    /// never for a sandbox, otherwise the request's choice or the default its
    /// session implies.
    fn keepalive(&self, session: RunSession) -> bool {
        if self.sandbox {
            return false;
        }
        self.role.is_some() || self.keepalive.unwrap_or(session == RunSession::Agent)
    }

    /// The kernel command line of VM `index` of the run.
    ///
    /// A sandbox's VM names the harness and the sandbox flag, which makes its
    /// guest run the envd-compatible daemon instead of an agent session; it is
    /// never a keepalive agent, so it carries no keepalive flag.
    ///
    /// A roled VM is the role fleet's VM, flag for flag (see [`agent_flags`]),
    /// behind the console device and the churn count when the run has one. Any
    /// other VM names the harness, and the keepalive flag when it is a
    /// keepalive agent, and carries its index for its identity within the run.
    fn vm_cmdline(&self, index: usize, keepalive: bool) -> String {
        let beats = match self.beats {
            Some(beats) => format!(" plaza.beats={beats}"),
            None => String::new(),
        };
        if self.sandbox {
            return format!("{CONSOLE}{beats} plaza.harness=1 plaza.sandbox=1 plaza.vm={index}");
        }
        match &self.role {
            // An agent's console carries its session: kernel chatter below warnings
            // (the balloon's inflate retries, for one) stays out of it.
            Some(role) => format!("{CONSOLE}{beats} loglevel=4 {}", agent_flags(role, index)),
            None => {
                let keepalive = if keepalive { " plaza.keepalive=1" } else { "" };
                format!("{CONSOLE}{beats} plaza.harness=1{keepalive} plaza.vm={index}")
            }
        }
    }
}

/// Checks a run's role: 1 to [`MAX_ROLE_LEN`] characters of `a-z`, `0-9` and
/// `-`, so it is one `plaza.role=` value on the guest's command line.
fn checked_role(role: &str) -> Result<(), String> {
    let valid = role
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid && (1..=MAX_ROLE_LEN).contains(&role.len()) {
        return Ok(());
    }
    Err(format!(
        "role must be 1 to {MAX_ROLE_LEN} characters of a-z, 0-9 and '-', not {role:?}"
    ))
}

/// Checks an explicit `vsock_cid` base, and the run of CIDs it roots, against
/// the vsock transport: `base` is at least [`MIN_GUEST_CID`] (1 and 2 are
/// reserved, and 0 is the hypervisor), and the run's last VM's CID
/// (`base + n - 1`) still fits `MAX_GUEST_CID`.
fn checked_base(base: u64, vms: usize) -> Result<u64, String> {
    if base < MIN_GUEST_CID {
        return Err(format!(
            "vsock_cid must be at least {MIN_GUEST_CID} (1 and 2 are reserved)"
        ));
    }
    // A CID the kernel's vsock stack always refuses — VMADDR_CID_ANY
    // (u32::MAX) and anything above it — is rejected here rather than left to
    // a boot that would fail on it.
    if base > MAX_GUEST_CID {
        return Err(format!(
            "vsock_cid must be at most {MAX_GUEST_CID}: the kernel refuses \
             VMADDR_CID_ANY and above"
        ));
    }
    // Every VM's CID, up to the run's last, must fit the transport.
    let last = base
        .checked_add(u64::try_from(vms).map_err(|_| "n is too large".to_string())? - 1)
        .ok_or_else(|| "vsock_cid is too large for this many VMs".to_string())?;
    if last > MAX_GUEST_CID {
        return Err("vsock_cid and n together run past the last guest CID".to_string());
    }
    Ok(base)
}

/// The `count` guest CIDs a run rooted at `base` holds: `base`, `base + 1`,
/// ... .
///
/// The span is checked against the vsock transport before this (see
/// [`checked_base`]), and an allocated base is inside [`RUN_CID_BASE`]..=
/// [`RUN_CID_END`] by construction, so the addition cannot overflow.
fn cid_span(base: u64, count: u32) -> Vec<u64> {
    (0..count).map(|offset| base + u64::from(offset)).collect()
}

/// vCPUs per VM as the spawner wants them, or why the request is outside the
/// contract's range.
fn spawner_vcpus(vcpus: u32) -> Result<u8, String> {
    let vcpus = checked_count("vcpus", vcpus, 1, MAX_VCPUS)?;
    u8::try_from(vcpus).map_err(|_| format!("vcpus must be between 1 and {MAX_VCPUS}"))
}

/// Guest memory per VM, in MiB, as the spawner wants it, or why the request is
/// outside the contract's range.
fn spawner_memory_mib(memory_mib: u32) -> Result<usize, String> {
    if memory_mib < MIN_MEMORY_MIB {
        return Err(format!("memory_mib must be at least {MIN_MEMORY_MIB}"));
    }
    usize::try_from(memory_mib).map_err(|_| "memory_mib is too large for this host".to_string())
}

/// The guest CIDs this node's runs hold, and the run range they are drawn
/// from.
///
/// Every VM of every run has a CID, so a create request always needs one - or
/// a span of them, one per VM. A request that names a `vsock_cid` holds exactly
/// its own span (the caller's choice, validated before this); a request that
/// names none is given the first free span of [`RUN_CID_BASE`]..=[`RUN_CID_END`].
/// The book is what makes an allocation honest: two live runs are never given
/// the same CID, so two VMs never race the same vsock muxer socket, and a run
/// that cannot get a span is refused here - before any VM boots, and before
/// the run is registered. A run releases its span when it ends, so a finished
/// run's CIDs are the next run's to draw.
#[derive(Debug)]
struct CidBook {
    /// The run range allocations are drawn from (inclusive).
    range: std::ops::RangeInclusive<u64>,
    /// One entry per live run: its id and every CID it holds, in order.
    held: Mutex<Vec<(String, Vec<u64>)>>,
}

impl Default for CidBook {
    fn default() -> Self {
        Self::new(RUN_CID_BASE..=RUN_CID_END)
    }
}

impl CidBook {
    /// An empty book that allocates from `range`.
    fn new(range: std::ops::RangeInclusive<u64>) -> Self {
        Self {
            range,
            held: Mutex::new(Vec::new()),
        }
    }

    /// The book, poison-proof: a panicking run must not take the node's other
    /// runs with it.
    fn held(&self) -> MutexGuard<'_, Vec<(String, Vec<u64>)>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Holds `cids` for `run`.
    ///
    /// Holding the same run twice replaces its earlier span (every creation
    /// path holds once, before its VMs boot). A span that names a CID another
    /// live run holds is held as it stands: two callers may pin overlapping
    /// ranges - the caller's choice, exactly as before this book existed - and
    /// the book's job is to keep an *allocation* off every CID a run already
    /// owns.
    fn hold(&self, run: &str, cids: Vec<u64>) {
        put(&mut self.held(), run, cids);
    }

    /// Gives `run` the first free span of `count` consecutive CIDs of
    /// [`RUN_CID_BASE`]..=[`RUN_CID_END`] and returns its base.
    ///
    /// The search and the hold happen under one lock, so two runs created at
    /// once can never be handed the same span. `Err` names the range and every
    /// CID a live run holds: exhaustion is a refusal the caller reads, not a
    /// VM booted without a CID.
    fn allocate(&self, run: &str, count: u32) -> Result<u64, String> {
        let mut held = self.held();
        let busy = busy(&held);
        let (first, last) = (*self.range.start(), *self.range.end());
        let free = (first..=last).find(|base| {
            (0..count).all(|offset| {
                base.checked_add(u64::from(offset))
                    .is_some_and(|cid| cid <= last && !busy.contains(&cid))
            })
        });
        match free {
            Some(base) => {
                let cids = cid_span(base, count);
                put(&mut held, run, cids);
                Ok(base)
            }
            None => Err(format!(
                "the run range {first}..={last} has no free span of {count} \
                 guest CIDs (in use: {})",
                busy.iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Releases `run`'s span and returns whether the run held one.
    ///
    /// Idempotent: every path that ends a run calls it, and only the first
    /// call finds the span.
    fn release(&self, run: &str) -> bool {
        let mut held = self.held();
        let before = held.len();
        held.retain(|(id, _)| id != run);
        held.len() != before
    }

    /// Holds `cids` for `run` unless one of them is held already, and returns whether it did. The
    /// check and the hold are one step under the book's lock, so two imports racing for one CID
    /// cannot both take it.
    fn hold_unless_busy(&self, run: &str, cids: Vec<u64>) -> bool {
        let mut held = self.held();
        let busy = busy(&held);
        if cids.iter().any(|cid| busy.contains(cid)) {
            return false;
        }
        put(&mut held, run, cids);
        true
    }
}

/// Every CID the runs in `held` hold.
fn busy(held: &[(String, Vec<u64>)]) -> BTreeSet<u64> {
    held.iter()
        .flat_map(|(_, cids)| cids.iter().copied())
        .collect()
}

/// Holds `cids` for `run` in `held`, replacing whatever span it held before.
fn put(held: &mut Vec<(String, Vec<u64>)>, run: &str, cids: Vec<u64>) {
    match held.iter_mut().find(|(id, _)| id == run) {
        Some(entry) => entry.1 = cids,
        None => held.push((run.to_string(), cids)),
    }
}

/// The run-level shape every VM of a run shares.
#[derive(Debug, Clone)]
struct RunMeta {
    /// vCPUs per VM, as the API reports it.
    vcpus: u32,
    /// Guest memory per VM, in MiB, as the API reports it.
    memory_mib: u32,
    /// Heartbeats before a guest reboots itself, when the run churns.
    beats: Option<u32>,
    /// The role every VM runs as, when the run has one.
    role: Option<String>,
    /// Whether the run's guests are keepalive agents: their workspace images
    /// persist when `--persist-dir` is set.
    keepalive: bool,
    /// The usable guest memory per VM at boot, when the VMs get a balloon.
    balloon_usable_mib: Option<u32>,
    /// Whether the run's guests are sandboxes (see [`CreateRun::sandbox`]).
    sandbox: bool,
    /// The shared data volumes every VM of the run carries (see [`CreateRun::volumes`]).
    volumes: Vec<DataVolume>,
}

/// One VM of a run, as the creation paths build it: its own command line, its
/// vsock CID, and its role when it has one.
///
/// The per-VM plan is the only thing the two creation paths differ in — a
/// request's VMs share `plaza.vm=<i>` on one CID span, the role fleet's VMs
/// each carry their role — so everything else about a run's life is shared.
///
/// Every plan has a CID: the harness is the guest's vsock client, so a VM
/// without one could not be an agent at all.
#[derive(Debug, Clone)]
struct VmPlan {
    /// The VM's whole kernel command line.
    cmdline: String,
    /// The VM's guest CID.
    vsock_cid: u64,
    /// The role this VM's guest runs as, when it runs as one.
    role: Option<String>,
}

/// Checks one of the create request's counts against its contract range and
/// returns it as a `usize`.
fn checked_count(name: &str, value: u32, min: u32, max: u32) -> Result<usize, String> {
    if !(min..=max).contains(&value) {
        return Err(format!("{name} must be between {min} and {max}"));
    }
    usize::try_from(value).map_err(|_| format!("{name} must be between {min} and {max}"))
}

/// The states noded v0 gives a run (`core.runs.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// The run exists and its VMs are being booted.
    Starting,
    /// Every VM of the run is up.
    Running,
    /// The run ended because its VMs stopped by themselves.
    Done,
    /// The run ended because its VMs could not be booted.
    Failed,
    /// The run ended because the host killed its VMs.
    Killed,
}

impl RunState {
    /// The name the API and the schema use.
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Starting => "starting",
            RunState::Running => "running",
            RunState::Done => "done",
            RunState::Failed => "failed",
            RunState::Killed => "killed",
        }
    }

    /// Whether `self -> next` is one of the transitions `core.run_transitions`
    /// allows for the states this daemon implements.
    pub fn can_transition_to(self, next: RunState) -> bool {
        matches!(
            (self, next),
            (RunState::Starting, RunState::Running)
                | (RunState::Starting, RunState::Failed)
                | (RunState::Starting, RunState::Killed)
                | (RunState::Running, RunState::Done)
                | (RunState::Running, RunState::Failed)
                | (RunState::Running, RunState::Killed)
        )
    }

    /// Whether the run has ended: the schema pairs these states with a
    /// non-null `ended_at`.
    pub fn is_finished(self) -> bool {
        match self {
            RunState::Starting | RunState::Running => false,
            RunState::Done | RunState::Failed | RunState::Killed => true,
        }
    }
}

/// A transition the run state machine does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedTransition {
    /// The state the run was in.
    pub from: RunState,
    /// The state the caller asked for.
    pub to: RunState,
}

impl std::fmt::Display for RefusedTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "a run may not go from {} to {}",
            self.from.as_str(),
            self.to.as_str()
        )
    }
}

impl std::error::Error for RefusedTransition {}

/// The states noded v0 gives a VM of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    /// The VM is up.
    Running,
    /// The VM is up but paused: its vCPUs and devices are stopped, its process is not.
    Paused,
    /// The VM is stopped with a sleep snapshot, and wakes from it (see [`Registry::wake_vm`]).
    Asleep,
    /// The VM stopped by itself (a guest shutdown, reset or fault).
    Exited,
    /// The VM was stopped by the host.
    Killed,
}

impl VmState {
    /// The name the API uses.
    pub fn as_str(self) -> &'static str {
        match self {
            VmState::Running => "running",
            VmState::Paused => "paused",
            VmState::Asleep => "asleep",
            VmState::Exited => "exited",
            VmState::Killed => "killed",
        }
    }

    /// How a VM's stop maps to the state the API reports.
    fn from_exit(exit: VmExit) -> Self {
        match exit {
            VmExit::GuestExited { .. } => VmState::Exited,
            VmExit::Killed => VmState::Killed,
        }
    }
}

/// Why a run refused console input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    /// The run has no VM with that index.
    UnknownVm(u32),
    /// The run is not running, so there is no console to write to.
    NotRunning {
        /// The state the run is in.
        state: RunState,
    },
    /// The VM's console could not take the bytes: the guest has not drained
    /// its input queue yet, or the VM has stopped.
    Console {
        /// The VM that refused.
        vm: u32,
        /// Why the console refused.
        reason: String,
    },
}

impl InputError {
    /// The HTTP status this error is reported as.
    pub fn status(&self) -> u16 {
        match self {
            InputError::UnknownVm(_) => 400,
            InputError::NotRunning { .. } | InputError::Console { .. } => 409,
        }
    }
}

impl std::fmt::Display for InputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::UnknownVm(vm) => write!(formatter, "the run has no VM {vm}"),
            InputError::NotRunning { state } => write!(
                formatter,
                "the run is {}: console input needs a running run",
                state.as_str()
            ),
            InputError::Console { vm, reason } => {
                write!(
                    formatter,
                    "VM {vm} could not take the console input: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for InputError {}

/// How much of a run a view carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// What the list endpoint returns: no serial captures, so the list stays
    /// small.
    Summary,
    /// What a single-run GET returns: every VM's serial capture.
    Full,
}

/// A run as the API reports it.
#[derive(Debug, Serialize)]
pub struct RunView<'a> {
    /// The run's id (`r<seq>`).
    pub id: &'a str,
    /// The run's state.
    pub state: &'static str,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// Heartbeats before the guest reboots itself, or `null`.
    pub beats: Option<u32>,
    /// The role every VM of the run runs as, or `null`.
    pub role: Option<&'a str>,
    /// Whether the run's guests are sandboxes (see [`CreateRun::sandbox`]).
    pub sandbox: bool,
    /// When the run was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When the run ended, in milliseconds since the Unix epoch, or `null`.
    pub ended_at: Option<u64>,
    /// The run's VMs.
    pub vms: Vec<VmView>,
}

/// A VM as the API reports it.
#[derive(Debug, Serialize)]
pub struct VmView {
    /// The VM's index among the run's VMs.
    pub id: u32,
    /// The VM's vsock guest CID (`vsock_cid + index`), or the base the node
    /// allocated for a run whose request named none.
    ///
    /// Every VM of every run has one now - the harness is the vsock client -
    /// so a run's VM row always carries it; it stays optional in the JSON
    /// because a client reads its absence as "no vsock device", a reading the
    /// daemon no longer produces but the console's parsers still model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_cid: Option<u64>,
    /// The role this VM's guest runs as (the role fleet's VM rows carry one);
    /// absent for a VM that was not started as a role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub state: &'static str,
    /// The VM's serial capture; present only in a single-run GET.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// The VM's guest memory as its balloon reports it, or `null` for a VM with
    /// no balloon (or one whose handle is taken while it is being killed).
    pub memory: Option<MemoryView>,
}

/// A VM's guest memory as the API reports it (see `vmm::spawner::MemoryStats`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryView {
    /// The VM's whole memory, in MiB.
    pub max_mib: u32,
    /// What the guest may use now, in MiB (the host's target).
    pub usable_mib: u32,
    /// What the balloon holds, in MiB (the guest's acknowledged size).
    pub held_mib: u64,
    /// Free memory inside the guest, in MiB, once it has reported it.
    pub free_mib: Option<u64>,
    /// Memory available without swapping inside the guest, in MiB, once reported.
    pub available_mib: Option<u64>,
}

impl From<vmm::spawner::MemoryStats> for MemoryView {
    fn from(stats: vmm::spawner::MemoryStats) -> Self {
        Self {
            max_mib: stats.max_mib,
            usable_mib: stats.usable_mib,
            held_mib: stats.held_mib,
            free_mib: stats.free_mib,
            available_mib: stats.available_mib,
        }
    }
}

/// Why a VM's memory could not be resized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// The node has no run with that id.
    UnknownRun,
    /// The run has no VM with that index.
    UnknownVm(u32),
    /// The VM was booted without a balloon, so its memory cannot be resized.
    NoBalloon(u32),
    /// The VM is not running, so there is no balloon to resize.
    Stopped(u32),
    /// The VM's memory control refused the resize.
    Refused(String),
}

impl MemoryError {
    /// The HTTP status this error is reported as: 404 for a run or VM the node
    /// does not have, 409 for one whose memory cannot be resized now, 502 when
    /// the VM's own memory control failed.
    pub fn status(&self) -> u16 {
        match self {
            MemoryError::UnknownRun | MemoryError::UnknownVm(_) => 404,
            MemoryError::NoBalloon(_) | MemoryError::Stopped(_) => 409,
            MemoryError::Refused(_) => 502,
        }
    }
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryError::UnknownRun => write!(formatter, "no run with that id"),
            MemoryError::UnknownVm(vm) => write!(formatter, "the run has no VM {vm}"),
            MemoryError::NoBalloon(vm) => {
                write!(
                    formatter,
                    "VM {vm} has no balloon, so its memory cannot be resized"
                )
            }
            MemoryError::Stopped(vm) => write!(formatter, "VM {vm} is not running"),
            MemoryError::Refused(reason) => {
                write!(formatter, "VM memory control refused the resize: {reason}")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

/// The list endpoint's body: `{"runs": [...]}`.
#[derive(Debug, Serialize)]
pub struct RunsView<'a> {
    /// Every run, oldest first.
    pub runs: Vec<RunView<'a>>,
}

/// The persistent workspace image of the keepalive VM with guest CID `cid`:
/// `persist_dir/workspaces/cid-{cid}.ext4`. The first call copies `base` there;
/// a later call (a restarted daemon's run on the same CID) returns the image as
/// it is and never overwrites it, so the agent's files survive. The copy is
/// made under a partial name and renamed into place, so an interrupted copy is
/// never reused.
fn prepare_persistent_workspace(
    base: &std::path::Path,
    persist_dir: &std::path::Path,
    cid: u64,
) -> std::io::Result<PathBuf> {
    let dir = persist_dir.join("workspaces");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir)?;
    let image = persistent_workspace_path(persist_dir, cid);
    if !image.exists() {
        let partial = dir.join(format!("cid-{cid}.ext4.partial"));
        vmm::spawner::copy_sparse(base, &partial)?;
        std::fs::rename(&partial, &image)?;
    }
    Ok(image)
}

/// The persistent workspace image of the keepalive VM with guest CID `cid`
/// under `persist_dir`: `persist_dir/workspaces/cid-{cid}.ext4`.
fn persistent_workspace_path(persist_dir: &std::path::Path, cid: u64) -> PathBuf {
    persist_dir
        .join("workspaces")
        .join(format!("cid-{cid}.ext4"))
}

/// Whether a VM's kernel command line makes its guest a sandbox (`plaza.sandbox=1`).
fn is_sandbox_cmdline(cmdline: &str) -> bool {
    cmdline
        .split_whitespace()
        .any(|word| word == "plaza.sandbox=1")
}

/// The workspace image of VM `index` of a sandbox run: `run_dir/workspace-{index}.ext4`. It is the
/// name [`spawner::materialize_workspaces`] gives a run's copies, so a sandbox's copy and a template's
/// clone of the same VM land at one path.
fn sandbox_workspace_path(run_dir: &Path, index: usize) -> PathBuf {
    run_dir.join(format!("workspace-{index}.ext4"))
}

/// The template start of one new VM of the shape `shape` (guest CID `cid`), whose workspace is `target`
/// (`None` for a VM with no workspace volume): the template for the shape (built on first use) with
/// `target` laid down as a clone of its disk. The result is the template's snapshot to restore the VM
/// from, or `None` when the VM boots cold.
fn template_start(
    templates: &crate::templates::Templates,
    shape: &crate::templates::Shape,
    cid: u64,
    target: Option<&Path>,
) -> Option<PathBuf> {
    let template = match templates.get_or_build(shape) {
        Ok(template) => template,
        Err(err) => {
            eprintln!("noded: no warm template for cid {cid} ({err}); booting it cold");
            return None;
        }
    };
    let laid = match (target, template.workspace.as_deref()) {
        (Some(target), Some(disk)) => target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| vmm::clonefile::clone_file(disk, target))
            .map(|_| ()),
        // No disk on either side: the VM has no workspace volume, so there is nothing to lay down.
        (None, None) => Ok(()),
        // The template is of the VM's own shape, so its disk and the VM's always agree.
        _ => Err(std::io::Error::other(
            "the template and the VM disagree on a workspace volume",
        )),
    };
    match laid {
        Ok(()) => Some(template.snapshot),
        Err(err) => {
            if let Some(target) = target {
                let _ = std::fs::remove_file(target);
            }
            eprintln!(
                "noded: cannot copy the template disk for cid {cid} ({err}); booting it cold"
            );
            None
        }
    }
}

/// Replaces the workspace image `image` with a fresh copy of the base image
/// `base`. The copy is made under a partial name and renamed over the image, as
/// [`prepare_persistent_workspace`] makes its copies, so an interrupted wipe
/// never leaves a half-copied image behind.
fn replace_workspace(base: &std::path::Path, image: &std::path::Path) -> std::io::Result<()> {
    let partial = PathBuf::from(format!("{}.partial", image.display()));
    match std::fs::remove_file(&partial) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    vmm::spawner::copy_sparse(base, &partial)?;
    std::fs::rename(&partial, image)
}

/// A VM an import brings to this node, its files already verified and staged (see `crate::migrate`).
#[derive(Debug, Clone)]
pub struct ImportedVm {
    /// The guest CID the VM keeps (the hub's choice).
    pub cid: u64,
    /// The role the VM runs as, when it has one.
    pub role: Option<String>,
    /// vCPUs, as the source reported them.
    pub vcpus: u32,
    /// Guest memory in MiB, as the source reported it.
    pub memory_mib: u32,
    /// The usable guest memory at boot, when the VM has a balloon.
    pub balloon_usable_mib: Option<u32>,
    /// The VM's kernel command line.
    pub cmdline: String,
    /// A staged directory holding `memory.img` and `vmstate.json`: moved into the CID's sleep snapshot.
    pub sleep_dir: PathBuf,
    /// The staged `workspace.img`, moved to the VM's workspace image on this node. `None` for a VM
    /// with no workspace volume: its export carries no workspace image.
    pub workspace: Option<PathBuf>,
}

/// An asleep VM, as an export reads it (see [`Registry::asleep_vm`]).
#[derive(Debug, Clone)]
pub struct AsleepVm {
    /// The VM's guest CID.
    pub cid: u64,
    /// The role the VM runs as, when it has one.
    pub role: Option<String>,
    /// The VM's cold spec: its command line, shape, balloon and workspace image.
    pub spec: VmSpec,
    /// The directory holding the VM's sleep snapshot.
    pub sleep_dir: PathBuf,
}

/// Checks the shape an imported VM must have, as a create request would: a CID the guest range
/// holds, a role, and the vCPUs and memory the spawner takes.
pub fn check_import_shape(
    cid: u64,
    role: Option<&str>,
    vcpus: u32,
    memory_mib: u32,
) -> Result<(), VmOpError> {
    checked_base(cid, 1).map_err(VmOpError::Invalid)?;
    if let Some(role) = role {
        checked_role(role).map_err(VmOpError::Invalid)?;
    }
    spawner_vcpus(vcpus).map_err(VmOpError::Invalid)?;
    spawner_memory_mib(memory_mib).map_err(VmOpError::Invalid)?;
    Ok(())
}

/// Moves the file `from` to `to`, replacing it. A rename that cannot cross the file system is a
/// sparse copy, then a removal of the original.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let partial = PathBuf::from(format!("{}.partial", to.display()));
    vmm::spawner::copy_sparse(from, &partial)?;
    fs::rename(&partial, to)?;
    fs::remove_file(from)
}

/// A VM's control handle, and what an operation on the VM needs, taken out of its slot so the
/// operation runs outside the run's lock. The slot is seated again by the operation's end.
struct Held {
    handle: VmHandle,
    /// The spec the VM was booted from (its cold command line and its workspace image).
    spec: VmSpec,
    /// The snapshot directory the VM was restored from: the base its next snapshot is a diff over.
    restored_from: Option<PathBuf>,
    /// The state the VM was in when it was taken.
    state: VmState,
}

/// A fork's VM, booted but not yet a run: it is registered only once every fork has booted.
struct PreparedFork {
    /// The fork run's id.
    id: String,
    cid: u64,
    handle: VmHandle,
    spec: VmSpec,
    /// The fork's own workspace image, when the fork has a workspace volume.
    workspace: Option<PathBuf>,
    /// The fork's diff base (see [`SnapshotStore::stage_base`]); none for a fork booted cold from a
    /// disk-only checkpoint.
    base: Option<PathBuf>,
}

/// The state a VM is in once an operation that paused it is over: a running VM is resumed (a
/// snapshot leaves its VM paused), a paused one stays paused.
fn thaw(state: VmState, handle: &VmHandle) -> VmState {
    match state {
        VmState::Running => match handle.resume() {
            Ok(()) => VmState::Running,
            Err(err) => {
                eprintln!("noded: a VM did not resume after its snapshot: {err}");
                VmState::Paused
            }
        },
        other => other,
    }
}

/// Creates `dir` for a snapshot of a VM. A VM restored from a snapshot writes a diff over the memory
/// it was restored from; the VMM copies that memory into `dir` itself, through the descriptor it keeps
/// (which also works inside a jail), so nothing is copied here: a second copy cost a full read and write
/// of the memory file on a filesystem without reflinks.
fn stage_base_memory(base: Option<&Path>, dir: &Path) -> Result<(), VmOpError> {
    if base.is_some() {
        snapshots::private_dir(dir)
            .map_err(|err| VmOpError::Failed(format!("cannot create {}: {err}", dir.display())))?;
    }
    Ok(())
}

/// Writes a snapshot of a VM into `dir` (created), with the memory staged as [`stage_base_memory`]
/// does. `kind` is what the VM is asked for (see [`SnapshotKind::is_durable`]).
fn write_snapshot(
    handle: &VmHandle,
    base: Option<&Path>,
    dir: &Path,
    kind: SnapshotKind,
) -> Result<SnapshotInfo, VmOpError> {
    stage_base_memory(base, dir)?;
    handle
        .snapshot(dir, kind)
        .map_err(|err| VmOpError::Failed(format!("the VM did not snapshot: {err}")))
}

/// A checkpoint whose directory is written, except (when `captured`) the VM's memory and state file,
/// which wait in RAM until [`Registry::finish_checkpoint`] writes them.
struct PendingCheckpoint {
    name: String,
    /// How long the VM was paused for the capture, in milliseconds.
    paused_ms: u64,
    captured: bool,
}

/// Boots a VM from the checkpoint directory `snapshot`, with its workspace copied back from it, and
/// returns the VM with the diff base its next snapshot is written over (staged from `snapshot`'s
/// memory). Used by a restore (and its fallback) and by a fork. A disk-only checkpoint boots the VM
/// cold from its disk, with no diff base.
fn boot_from(
    store: &SnapshotStore,
    cid: u64,
    spec: &VmSpec,
    snapshot: &Path,
) -> Result<(VmHandle, Option<PathBuf>), String> {
    if let Some(workspace) = &spec.workspace {
        clone_file(&snapshot.join(snapshots::WORKSPACE_FILE), workspace)
            .map_err(|err| format!("cannot copy the workspace back: {err}"))?;
    }
    if snapshots::is_disk_only(snapshot) {
        let handle = spawn_timed(spec.clone()).map_err(|err| err.to_string())?;
        return Ok((handle, None));
    }
    let base = store
        .stage_base(cid, &snapshot.join(snapshots::MEMORY_FILE))
        .map_err(|err| format!("cannot stage the snapshot's memory: {err}"))?;
    let handle = spawn_timed(VmSpec {
        restore: Some(snapshot.to_path_buf()),
        ..spec.clone()
    })
    .map_err(|err| err.to_string())?;
    Ok((handle, Some(base)))
}

/// What a recovery of a VM that stopped by itself boots it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// The snapshot named `name`: the sleep snapshot, or a checkpoint.
    From(String),
    /// No snapshot is fresh enough: the VM boots cold from its spec.
    Cold,
    /// The VM has been recovered too often lately: it stays exited.
    GiveUp,
}

/// Decides how to recover a VM that stopped by itself, from the snapshots it has (`snapshots`, its
/// checkpoints and sleep snapshot) and the times its earlier recoveries started (`recent`, all of
/// them; only those inside [`RECOVERY_WINDOW_MS`] count). The rate limit comes first: a VM in a
/// crash loop is given up on whatever it has. Otherwise the newest snapshot taken within
/// [`RECOVERY_SNAPSHOT_AGE_MS`] that has memory is the one to boot from, and with none the VM boots
/// cold. A disk-only checkpoint (kind `disk`) has no memory to resume a running VM from, so it is
/// never picked.
pub(crate) fn recovery_decision(now: u64, snapshots: &[SnapshotView], recent: &[u64]) -> Recovery {
    let recovered = recent
        .iter()
        .filter(|&&at| now.saturating_sub(at) < RECOVERY_WINDOW_MS)
        .count();
    if recovered >= RECOVERY_LIMIT {
        return Recovery::GiveUp;
    }
    snapshots
        .iter()
        .filter(|view| view.kind != "disk")
        .filter(|view| now.saturating_sub(view.taken_at_ms) <= RECOVERY_SNAPSHOT_AGE_MS)
        .max_by_key(|view| view.taken_at_ms)
        .map_or(Recovery::Cold, |view| Recovery::From(view.name.clone()))
}

/// Whether a run's VMs are recovered when they stop by themselves: a keepalive agent or a sandbox
/// that does not churn. A frozen demo session and a churn run (`beats`) end by design, so their
/// exit is their normal end and is never recovered.
pub(crate) fn is_recoverable_run(keepalive: bool, sandbox: bool, beats: Option<u32>) -> bool {
    (keepalive || sandbox) && beats.is_none()
}

/// Whether a VM's stop is one recovery takes over. Only a guest-initiated exit of a VM that was
/// running counts: a kill (the host's stop, a wipe, a restart, a sleep, a move) is reported as
/// [`VmExit::Killed`], and a paused or asleep VM is not one that ran on its own.
pub(crate) fn recovers(exit: VmExit, state: VmState, recoverable: bool, enabled: bool) -> bool {
    matches!(exit, VmExit::GuestExited { .. })
        && state == VmState::Running
        && recoverable
        && enabled
}

/// Boots a recovered VM from its snapshot `name` in `store`, with the same CID and spec. A
/// checkpoint's workspace is copied back, as a restore does; the sleep snapshot leaves the
/// workspace as the stopped VM had it, as a wake does. Returns the VM and the diff base its next
/// snapshot is written over.
fn boot_recovered(
    store: &SnapshotStore,
    cid: u64,
    spec: &VmSpec,
    name: &str,
) -> Result<(VmHandle, PathBuf), String> {
    let snapshot = store.dir(cid, name);
    let base = if name == snapshots::SLEEP {
        snapshot.clone()
    } else {
        if let Some(workspace) = &spec.workspace {
            let saved = snapshot.join(snapshots::WORKSPACE_FILE);
            if saved.is_file() {
                clone_file(&saved, workspace)
                    .map_err(|err| format!("cannot copy the workspace back: {err}"))?;
            }
        }
        store
            .stage_base(cid, &snapshot.join(snapshots::MEMORY_FILE))
            .map_err(|err| format!("cannot stage the snapshot's memory: {err}"))?
    };
    let handle = spawn_timed(VmSpec {
        restore: Some(snapshot),
        ..spec.clone()
    })
    .map_err(|err| err.to_string())?;
    Ok((handle, base))
}

/// The daemon's runs, in creation order.
pub struct Registry {
    images: VmImages,
    /// Directory holding the per-VM vsock Unix sockets (`{uds_dir}/vm{cid}`).
    uds_dir: PathBuf,
    /// Where keepalive runs' workspace images persist (`--persist-dir`), or
    /// `None` for the per-run copies.
    persist_dir: Option<PathBuf>,
    /// Where VM snapshots are kept (`--snapshot-dir`), or `None` for a node that keeps none: every
    /// snapshot operation is then refused with [`VmOpError::NoSnapshots`].
    snapshots: Option<SnapshotStore>,
    /// Warm templates (`crate::templates`): a new keepalive agent VM starts by restoring one instead
    /// of booting. `None` boots every VM cold.
    templates: Option<Arc<crate::templates::Templates>>,
    /// The node's shared data volumes (`--volume-dir`), or `None` for a node that keeps none.
    volumes: Option<Arc<VolumeStore>>,
    /// When this daemon started, in milliseconds since the Unix epoch: part of
    /// a run's workspace-directory name, because run ids restart with the
    /// daemon (a bare `run-{id}` would collide with the previous daemon's
    /// retained copies).
    epoch_ms: u64,
    /// Where the registry's run, VM and serial-delta events are published.
    events: Arc<EventBus>,
    /// The guest CIDs the node's runs hold (see [`CidBook`]). It is shared
    /// with every run, so a run that ends releases its own span without the
    /// registry having to watch for it.
    cids: Arc<CidBook>,
    inner: Mutex<RegistryInner>,
}

struct RegistryInner {
    next_seq: u64,
    runs: Vec<Arc<Run>>,
}

/// `spawner::spawn`, timed into `crate::metrics::SPAWN`.
fn spawn_timed(spec: VmSpec) -> Result<VmHandle, SpawnError> {
    let started = std::time::Instant::now();
    let spawned = spawner::spawn(spec);
    crate::metrics::SPAWN.observe(started.elapsed());
    spawned
}

impl Registry {
    /// A registry with no runs that boots every run from `images`, lays the
    /// runs' vsock sockets out under `uds_dir`, and publishes every state
    /// transition and serial delta to `events`.
    pub fn new(images: VmImages, uds_dir: PathBuf, events: Arc<EventBus>) -> Self {
        Self {
            images,
            uds_dir,
            persist_dir: None,
            snapshots: None,
            templates: None,
            volumes: None,
            epoch_ms: now_ms(),
            events,
            cids: Arc::new(CidBook::default()),
            inner: Mutex::new(RegistryInner {
                next_seq: 0,
                runs: Vec::new(),
            }),
        }
    }

    /// Keeps keepalive runs' workspace images under `persist_dir` (see
    /// [`prepare_persistent_workspace`]); `None` keeps every run's copies in its
    /// own directory.
    pub fn with_persist_dir(mut self, persist_dir: Option<PathBuf>) -> Self {
        self.persist_dir = persist_dir;
        self
    }

    /// Keeps VM snapshots in `store` (see [`crate::snapshots`]); `None` keeps none.
    pub fn with_snapshots(mut self, store: Option<SnapshotStore>) -> Self {
        self.snapshots = store;
        self
    }

    /// Keeps the node's shared data volumes in `store` (see [`crate::volumes`]); `None` keeps none, and
    /// a run that names a volume is refused.
    pub fn with_volumes(mut self, store: Option<VolumeStore>) -> Self {
        self.volumes = store.map(Arc::new);
        self
    }

    /// The node's volume store, if it keeps volumes.
    pub fn volume_store(&self) -> Option<&VolumeStore> {
        self.volumes.as_deref()
    }

    /// The data volumes a live run's VMs carry: volume name to the guest CIDs that mount it.
    pub fn volumes_in_use(&self) -> std::collections::BTreeMap<String, Vec<u64>> {
        let mut used: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
        for run in self.list() {
            if run.volumes.is_empty() {
                continue;
            }
            let inner = run.lock();
            if !matches!(inner.state, RunState::Starting | RunState::Running) {
                continue;
            }
            for cid in inner.vms.iter().filter_map(|slot| slot.guest_cid) {
                for volume in &run.volumes {
                    used.entry(volume.name.clone()).or_default().push(cid);
                }
            }
        }
        used
    }

    /// The volumes `names` name, as the VMs' specs carry them: each must be in the node's store.
    fn resolve_volumes(&self, names: &[String]) -> Result<Vec<DataVolume>, String> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let store = self
            .volumes
            .as_deref()
            .ok_or_else(|| "this node keeps no volumes".to_string())?;
        let volumes = names
            .iter()
            .map(|name| {
                store
                    .image(name)
                    .map(|image| DataVolume {
                        name: name.clone(),
                        image,
                    })
                    .ok_or_else(|| format!("no volume {name}"))
            })
            .collect::<Result<Vec<DataVolume>, String>>()?;
        spawner::check_volumes(&volumes)?;
        Ok(volumes)
    }

    /// Starts new keepalive agent VMs from warm templates kept in `templates` (see
    /// [`crate::templates`]); `None` boots every VM cold.
    pub fn with_templates(mut self, templates: Option<crate::templates::Templates>) -> Self {
        self.templates = templates.map(Arc::new);
        self
    }

    /// The node's warm templates, when it keeps them.
    pub fn templates(&self) -> Option<&crate::templates::Templates> {
        self.templates.as_deref()
    }

    /// For each planned VM of a keepalive run on a node with templates and persistent workspaces, and
    /// for each VM of a sandbox run on a node with templates: the template to restore it from, when
    /// its workspace could be laid down as a clone of the template's disk. A keepalive VM is new when
    /// it has no persistent workspace yet; one that already has one (an agent coming back after a
    /// restart of the node) boots cold from its own disk. A sandbox's workspace is its run's own, so
    /// it is always new. A template that cannot be built is logged and the VM boots cold too.
    ///
    /// A node with no base image gives its keepalive VMs no workspace volume at all (their
    /// `/workspace` lives in guest RAM): their template has no disk to lay down, and a new VM is
    /// simply one being created now, whether or not the node has a persist directory.
    fn template_starts(
        &self,
        id: &str,
        meta: &RunMeta,
        plans: &[VmPlan],
        vcpus: u8,
        memory_mib: usize,
    ) -> Vec<Option<PathBuf>> {
        let none = || vec![None; plans.len()];
        let Some(templates) = &self.templates else {
            return none();
        };
        // Templates carry no data volumes, and a restore must describe the machine it was taken of.
        if !meta.volumes.is_empty() {
            return none();
        }
        let base = self.images.workspace.as_deref();
        if meta.sandbox {
            // A sandbox's copy is the base image cloned into its run's directory, so without a base
            // image there is no disk for its template to lay down.
            let Some(base) = base else {
                return none();
            };
            let run_dir = self.run_dir(id);
            if let Err(err) = snapshots::private_dir(&run_dir) {
                eprintln!(
                    "noded: run {id}: cannot create {} ({err}); its sandboxes boot cold",
                    run_dir.display()
                );
                return none();
            }
            return plans
                .iter()
                .enumerate()
                .map(|(index, plan)| {
                    let target = sandbox_workspace_path(&run_dir, index);
                    let shape = self.template_shape(Some(base), meta, plan, vcpus, memory_mib);
                    template_start(templates, &shape, plan.vsock_cid, Some(&target))
                })
                .collect();
        }
        if !meta.keepalive {
            return none();
        }
        match (base, &self.persist_dir) {
            (None, _) => plans
                .iter()
                .map(|plan| {
                    let shape = self.template_shape(None, meta, plan, vcpus, memory_mib);
                    template_start(templates, &shape, plan.vsock_cid, None)
                })
                .collect(),
            (Some(_), None) => none(),
            (Some(base), Some(persist)) => plans
                .iter()
                .map(|plan| {
                    let image = persistent_workspace_path(persist, plan.vsock_cid);
                    if image.exists() {
                        return None;
                    }
                    let shape = self.template_shape(Some(base), meta, plan, vcpus, memory_mib);
                    template_start(templates, &shape, plan.vsock_cid, Some(&image))
                })
                .collect(),
        }
    }

    /// The template shape of one planned VM: the run's kernel, initrd and base image (none for a VM
    /// with no workspace volume), and the VM's own command line (sandbox or agent, whichever the plan
    /// carries).
    fn template_shape(
        &self,
        base: Option<&Path>,
        meta: &RunMeta,
        plan: &VmPlan,
        vcpus: u8,
        memory_mib: usize,
    ) -> crate::templates::Shape {
        crate::templates::Shape {
            kernel: self.images.kernel.clone(),
            initrd: Some(self.images.initrd.clone()),
            workspace_base: base.map(Path::to_path_buf),
            vcpus,
            memory_mib,
            balloon: meta.balloon_usable_mib,
            cmdline: plan.cmdline.clone(),
        }
    }

    /// Draws the runs' CIDs (those whose create request names none) from
    /// `range` instead of [`RUN_CID_BASE`]..=[`RUN_CID_END`]: a node joined to a
    /// hub uses the range the hub gave it.
    pub fn with_run_cids(mut self, range: std::ops::RangeInclusive<u64>) -> Self {
        self.cids = Arc::new(CidBook::new(range));
        self
    }

    /// The directory a run's workspace copies live in: `run-{id}-{epoch_ms}`.
    /// Run ids restart with the daemon, so the start stamp is what keeps a
    /// new `r1` from copying over the previous daemon's `r1` workspace.
    fn run_dir(&self, id: &str) -> PathBuf {
        self.uds_dir.join(format!("run-{id}-{}", self.epoch_ms))
    }

    /// Creates and boots a run from a `POST /v1/runs` request.
    ///
    /// The run's guest CIDs are taken first - the caller's own span when it
    /// named a `vsock_cid`, otherwise the first free span of the run range
    /// ([`RUN_CID_BASE`]..=[`RUN_CID_END`], held in the node's [`CidBook`]) -
    /// so a request the node cannot give CIDs to is refused with no run, no
    /// workspace copies and no VM behind it. The run is then registered in
    /// `starting` before its VMs are booted, so a concurrent GET can see it,
    /// and moves to `running` once they are up (or `failed` when they are
    /// not). A `DELETE` that arrives while the VMs are booting moves the run to
    /// `killed`, and the VMs that come up afterwards are killed rather than
    /// left behind.
    ///
    /// Every VM carries the harness and so every VM has a CID; which session
    /// its guest speaks follows who chose the CIDs (see [`RunSession`]). A
    /// request refused for lack of CIDs has already taken the next run id (the
    /// book keys a span by the run that holds it), so the refusal leaves a gap
    /// in the run numbering - the same gap [`Registry::reserve_run_id`] has
    /// always documented, and the run's id names nothing until the run exists.
    pub fn create_run(&self, request: &CreateRun) -> Result<Arc<Run>, String> {
        let (vms, named) = request.checked()?;
        let count = u32::try_from(vms).map_err(|_| "n is too large".to_string())?;
        let volumes = self.resolve_volumes(&request.volumes)?;
        // The id is reserved before the CIDs, because the book keys a span by
        // the run that holds it: allocating and holding happen under the
        // book's lock, so two creates racing here never draw the same span.
        let id = self.reserve_run_id();
        let (base, session) = match named {
            Some(base) => {
                self.cids.hold(&id, cid_span(base, count));
                (base, RunSession::Frozen)
            }
            None => (self.cids.allocate(&id, count)?, RunSession::Agent),
        };
        // One plan per VM. Every VM's CID is the run's base plus its index,
        // and its index rides on the command line as the flag the guest
        // harness reads for its identity within the run.
        let keepalive = request.keepalive(session);
        let plans = (0..vms)
            .map(|index| VmPlan {
                cmdline: request.vm_cmdline(index, keepalive),
                vsock_cid: base + index as u64,
                role: request.role.clone(),
            })
            .collect();
        self.boot_run(
            id,
            RunMeta {
                vcpus: request.vcpus,
                memory_mib: request.memory_mib,
                beats: request.beats,
                role: request.role.clone(),
                keepalive,
                balloon_usable_mib: request.balloon_usable_mib,
                sandbox: request.sandbox,
                volumes,
            },
            plans,
        )
    }

    /// Boots the initial role fleet: one long-lived keepalive VM per role of
    /// `fleet`, in the fleet's boot order (slot order), as one run whose VM
    /// rows carry the roles.
    ///
    /// This is the service's initial setup, so it is booted exactly like a
    /// requested run — the same states, events, workspace copies and kill
    /// semantics — with `beats` unset: a keepalive agent does not exit on its
    /// own, so the run stays `running` until it is killed, and only guests that
    /// stop by themselves can end it. A fleet that cannot be booted ends as
    /// `failed` with the VMs that had come up already stopped (the spawner's
    /// all-or-nothing boot), and the caller reports that the setup does not
    /// stand rather than serving a node without its agents.
    ///
    /// The fleet's own CIDs are held like any run's, so an allocated run or
    /// another caller's pinned run never draws a CID a live fleet owns.
    pub fn create_agent_fleet(&self, fleet: &Fleet) -> Result<Arc<Run>, String> {
        let plans = Self::fleet_plans(fleet);
        let id = self.reserve_run_id();
        self.cids
            .hold(&id, plans.iter().map(|plan| plan.vsock_cid).collect());
        self.boot_run(
            id,
            RunMeta {
                vcpus: u32::from(fleet.vcpus()),
                memory_mib: fleet.memory_mib(),
                beats: None,
                // The fleet's VMs carry their own roles; the run has none.
                role: None,
                // Fleet VMs are long-lived keepalive agents.
                keepalive: true,
                balloon_usable_mib: fleet.balloon_usable_mib(),
                sandbox: false,
                volumes: Vec::new(),
            },
            plans,
        )
    }

    /// The per-VM plans of a role fleet: the console device, then the role's own
    /// flags, then the role itself and the CID the fleet gives its VM.
    fn fleet_plans(fleet: &Fleet) -> Vec<VmPlan> {
        fleet
            .vms()
            .into_iter()
            .map(|vm| VmPlan {
                cmdline: format!("{CONSOLE} {}", vm.flags),
                vsock_cid: vm.guest_cid,
                role: Some(vm.role),
            })
            .collect()
    }

    /// Boots one run from its `plans`: reserves its workspace copies,
    /// registers it in `starting`, spawns every VM, and moves it to `running`
    /// (or `failed`).
    ///
    /// Every creation path goes through here, so a run's life is the same
    /// however it was asked for; only the plans and the reserved `id` differ.
    /// The caller has already taken the run's guest CIDs in the node's
    /// [`CidBook`] - it needs the id to do that - so this never boots a VM the
    /// book does not cover, and every refusal *before* a run is registered
    /// hands that span back.
    fn boot_run(&self, id: String, meta: RunMeta, plans: Vec<VmPlan>) -> Result<Arc<Run>, String> {
        // The shape is checked here, at the one place every path passes, as
        // well as at the edge where the request is read.
        let vcpus = spawner_vcpus(meta.vcpus).map_err(|err| self.give_back(&id, err))?;
        let memory_mib =
            spawner_memory_mib(meta.memory_mib).map_err(|err| self.give_back(&id, err))?;
        let vms = plans.len();
        // New agent VMs and sandboxes start from a warm template where the node keeps them: their
        // workspace is laid down here as a clone of the template's disk, which the workspace step
        // below then finds in place (a keepalive one by `prepare_persistent_workspace`, a sandbox's
        // by the sandbox branch).
        let starts = self.template_starts(&id, &meta, &plans, vcpus, memory_mib);
        // A run's workspace volume is writable, so every VM of the run gets its
        // own copy of the base image: one copy per VM, in the run's own
        // directory under the daemon's UDS root. The directory is created
        // `0700` (an agent's workspace is its private data), and the copies are
        // made before the run is registered, so a failure to prepare them is
        // reported to the caller without leaving a run behind. The copies stay
        // when the run ends: they hold the guests' work.
        let workspaces = match (&self.images.workspace, &self.persist_dir) {
            // A keepalive run's workspace images persist: each VM's image is
            // keyed by its guest CID, copied once, reused by every later run on
            // that CID, and never deleted with the run.
            (Some(base), Some(persist)) if meta.keepalive => Some(
                plans
                    .iter()
                    .map(|plan| prepare_persistent_workspace(base, persist, plan.vsock_cid))
                    .collect::<std::io::Result<Vec<PathBuf>>>()
                    .map_err(|err| {
                        self.give_back(
                            &id,
                            format!("cannot prepare the run's workspace images: {err}"),
                        )
                    })?,
            ),
            // A sandbox's workspaces are per run, never the persistent images: each VM's copy is the
            // base image cloned into the run's directory, except a VM a template started, whose copy
            // is already the template's disk (see `template_starts`).
            (Some(base), _) if meta.sandbox => {
                let run_dir = self.run_dir(&id);
                let copies = snapshots::private_dir(&run_dir)
                    .map_err(|err| format!("cannot create {}: {err}", run_dir.display()))
                    .and_then(|()| {
                        (0..vms)
                            .map(|index| {
                                let copy = sandbox_workspace_path(&run_dir, index);
                                if starts[index].is_some() {
                                    return Ok(copy);
                                }
                                vmm::clonefile::clone_file(base, &copy)
                                    .map(|_| copy)
                                    .map_err(|err| {
                                        format!("cannot prepare the run's workspace images: {err}")
                                    })
                            })
                            .collect::<Result<Vec<PathBuf>, String>>()
                    });
                Some(copies.map_err(|err| self.give_back(&id, err))?)
            }
            (Some(base), _) => {
                // The directory name carries the daemon's start stamp as well
                // as the run id: run ids restart with the daemon, so a bare
                // `run-{id}` would let a new `r1` overwrite the workspace
                // copies the previous `r1` left behind (`materialize_workspaces`
                // copies over same-named images).
                let run_dir = self.run_dir(&id);
                Some(
                    spawner::materialize_workspaces(base, &run_dir, vms).map_err(|err| {
                        self.give_back(
                            &id,
                            format!("cannot prepare the run's workspace images: {err}"),
                        )
                    })?,
                )
            }
            (None, _) => None,
        };
        // Every VM of the run gets the same balloon, at the same usable size.
        let balloon = meta.balloon_usable_mib;
        // Each VM's spec is kept in its slot: a restart or wipe boots the VM
        // again from exactly the spec it was booted from here.
        let specs: Vec<VmSpec> = (0..vms)
            .map(|i| {
                // `plans` has one entry per VM, so `i` is in range.
                let plan = &plans[i];
                VmSpec {
                    kernel: self.images.kernel.clone(),
                    cmdline: plan.cmdline.clone(),
                    vcpus,
                    memory_mib,
                    initrd: Some(self.images.initrd.clone()),
                    gic: None,
                    vsock: Some(VsockConfig {
                        guest_cid: plan.vsock_cid,
                        uds_path: self.uds_dir.join(format!("vm{}", plan.vsock_cid)),
                    }),
                    // One image per VM, in the order `materialize_workspaces`
                    // produced them: the run has exactly `vms` copies.
                    workspace: workspaces
                        .as_ref()
                        .and_then(|images| images.get(i))
                        .cloned(),
                    balloon,
                    restore: None,
                    volumes: meta.volumes.clone(),
                }
            })
            .collect();
        let run = self.insert(id, meta);
        // `spawn_each` calls this exactly `vms` times with the ids `0..vms`, and
        // `specs` has `vms` entries: the index is in range. A VM with a template start is restored
        // from it; the spec kept in its slot stays the cold one, so a restart boots it as usual.
        let boot = spawner::spawn_each(vms, |i| {
            let mut spec = specs[i as usize].clone();
            spec.restore = starts[i as usize].clone();
            spec
        });
        match boot {
            Ok(handles) => {
                let mut inner = run.lock();
                if let Err(refused) = inner.transition(RunState::Running) {
                    // A DELETE ended the run while its VMs were booting: the
                    // run is already over, so the VMs that just came up must
                    // not be left behind.
                    drop(inner);
                    eprintln!(
                        "noded: run {}: {refused}; killing the VMs it just booted",
                        run.id
                    );
                    for mut handle in handles {
                        if let Err(err) = handle.kill() {
                            eprintln!(
                                "noded: run {}: VM {} did not stop: {err}",
                                run.id,
                                handle.id()
                            );
                        }
                    }
                    return Ok(run);
                }
                // Every handle's id is its index in the spawn, and the plans
                // are one per VM in that same order.
                inner.vms = handles
                    .into_iter()
                    .enumerate()
                    .map(|(index, handle)| VmSlot {
                        id: handle.id(),
                        guest_cid: Some(plans[index].vsock_cid),
                        role: plans[index].role.clone(),
                        runtime: handle.runtime(),
                        handle: Some(handle),
                        spec: Some(specs[index].clone()),
                        state: VmState::Running,
                        // The capture was empty at boot (the serial delta
                        // ticker starts at the first byte the guest writes).
                        serial_seen: 0,
                        restored_from: None,
                    })
                    .collect();
                // The run is `running` and every VM with it: published under
                // the run's lock so a stream sees the run and its VMs appear
                // together, in order.
                run.events.publish(Event::Run {
                    id: run.id.clone(),
                    state: RunState::Running.as_str(),
                });
                for slot in inner.vms.iter() {
                    run.events.publish(Event::Vm {
                        run: run.id.clone(),
                        vm: slot.id,
                        state: VmState::Running.as_str(),
                    });
                }
                drop(inner);
                watch(Arc::clone(&run));
            }
            Err(err) => {
                eprintln!("noded: run {} failed to boot: {err}", run.id);
                let mut inner = run.lock();
                if let Err(refused) = inner.transition(RunState::Failed) {
                    eprintln!(
                        "noded: run {}: {refused} (a kill raced the failed boot)",
                        run.id
                    );
                } else {
                    // The run ended here, so its CIDs go back to the book: the
                    // next run may draw them, and none of this run's VMs ever
                    // reached a guest.
                    run.release_cids();
                    run.events.publish(Event::Run {
                        id: run.id.clone(),
                        state: RunState::Failed.as_str(),
                    });
                }
            }
        }
        Ok(run)
    }

    /// Gives the span reserved for `id` back to the node's [`CidBook`] and
    /// hands `err` on: what every refusal in [`Registry::boot_run`] that
    /// happens *before* a run is registered does, so a request that never
    /// became a run never holds CIDs.
    fn give_back(&self, id: &str, err: String) -> String {
        self.cids.release(id);
        err
    }

    /// Restarts VM `vm` of run `run`: stops it, then boots it again from the
    /// spec it was booted from (same CID, same workspace image).
    pub fn restart_vm(&self, run: &str, vm: u32) -> Result<Arc<Run>, VmOpError> {
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        self.cycle_vm(&run, vm, false)?;
        Ok(run)
    }

    /// Wipes VM `vm` of run `run`: stops it, replaces its workspace image with a
    /// fresh copy of the base image, and boots it again.
    pub fn wipe_vm(&self, run: &str, vm: u32) -> Result<Arc<Run>, VmOpError> {
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        self.cycle_vm(&run, vm, true)?;
        Ok(run)
    }

    /// Stops VM `vm` of run `run` for good: it is killed and left stopped, and
    /// the run's other VMs carry on. Nothing boots it again.
    pub fn stop_vm(&self, run: &str, vm: u32) -> Result<Arc<Run>, VmOpError> {
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let mut handle = run.take_vm(vm, &AWAKE)?.handle;
        if let Err(err) = handle.kill() {
            run.return_vm(vm, handle);
            return Err(VmOpError::Refused(err.to_string()));
        }
        run.set_vm_killed(vm, handle);
        Ok(run)
    }

    /// Pauses VM `vm` of run `run`: its vCPUs and devices stop, its process stays. Only a running VM
    /// pauses; [`Registry::resume_vm`] lets it run again.
    pub fn pause_vm(&self, run: &str, vm: u32) -> Result<Arc<Run>, VmOpError> {
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let held = run.take_vm(vm, &[VmState::Running])?;
        if let Err(err) = held.handle.pause() {
            run.return_vm(vm, held.handle);
            return Err(VmOpError::Refused(err.to_string()));
        }
        run.return_vm_in(vm, held.handle, VmState::Paused);
        Ok(run)
    }

    /// Lets paused VM `vm` of run `run` run again.
    pub fn resume_vm(&self, run: &str, vm: u32) -> Result<Arc<Run>, VmOpError> {
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let held = run.take_vm(vm, &[VmState::Paused])?;
        if let Err(err) = held.handle.resume() {
            run.return_vm(vm, held.handle);
            return Err(VmOpError::Refused(err.to_string()));
        }
        run.return_vm_in(vm, held.handle, VmState::Running);
        Ok(run)
    }

    /// Dozes VM `vm` of run `run`: pauses it if it runs, then pages its guest memory out of host RAM
    /// (`VmHandle::reclaim`). The VM stays paused, and [`Registry::resume_vm`] wakes it. A reclaim that
    /// fails leaves the VM paused and is refused. Returns the run and the bytes advised.
    pub fn doze_vm(&self, run: &str, vm: u32) -> Result<(Arc<Run>, u64), VmOpError> {
        let mut phases = apz_metrics::Phases::start("doze", format!("{run}/{vm}"));
        let result = self.doze_vm_timed(run, vm, &mut phases);
        let ok = result.is_ok();
        let reclaimed = result.as_ref().map_or(0, |(_, bytes)| *bytes);
        phases.finish(
            if ok {
                Some(&crate::metrics::DOZE)
            } else {
                None
            },
            &[("ok", &ok), ("reclaimed_bytes", &reclaimed)],
        );
        result
    }

    /// The work of [`Registry::doze_vm`], with its pause and reclaim marked on `phases`.
    fn doze_vm_timed(
        &self,
        run: &str,
        vm: u32,
        phases: &mut apz_metrics::Phases,
    ) -> Result<(Arc<Run>, u64), VmOpError> {
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let held = run.take_vm(vm, &[VmState::Running, VmState::Paused])?;
        if let Err(err) = held.handle.pause() {
            run.return_vm(vm, held.handle);
            return Err(VmOpError::Refused(err.to_string()));
        }
        phases.mark("pause");
        let reclaimed = held.handle.reclaim();
        // Paused in every case: a reclaim that failed leaves the VM as the pause left it.
        run.return_vm_in(vm, held.handle, VmState::Paused);
        phases.mark("reclaim");
        match reclaimed {
            Ok(bytes) => Ok((run, bytes)),
            Err(err) => Err(VmOpError::Refused(err.to_string())),
        }
    }

    /// Puts VM `vm` of run `run` to sleep (see `sleep_vm_inner`), timed: one `timing op=sleep` line and
    /// `crate::metrics::SLEEP`.
    pub fn sleep_vm(
        &self,
        run: &str,
        vm: u32,
        keep_memory: bool,
    ) -> Result<(Arc<Run>, Option<SnapshotView>), VmOpError> {
        let mut phases = apz_metrics::Phases::start("sleep", format!("{run}/{vm}"));
        let result = self.sleep_vm_inner(run, vm, keep_memory);
        phases.mark("snapshot_and_stop");
        let ok = result.is_ok();
        phases.finish(
            if ok {
                Some(&crate::metrics::SLEEP)
            } else {
                None
            },
            &[("ok", &ok)],
        );
        result
    }

    /// Puts VM `vm` of run `run` to sleep: its state goes to the sleep snapshot, its process stops,
    /// and it waits as `asleep` until [`Registry::wake_vm`]. A paused VM is snapshotted as it is; a
    /// running one is paused for the snapshot. The sleep snapshot is written beside the old one and
    /// renamed over it once the VM is stopped (see [`crate::snapshots`]). With `keep_memory` false
    /// the VM is stopped without a memory image, any sleep snapshot is removed, and it returns no
    /// snapshot: it wakes cold.
    fn sleep_vm_inner(
        &self,
        run: &str,
        vm: u32,
        keep_memory: bool,
    ) -> Result<(Arc<Run>, Option<SnapshotView>), VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let cid = run.vm_cid(vm)?;
        let held = run.take_vm(vm, &[VmState::Running, VmState::Paused])?;
        let Held {
            mut handle,
            restored_from,
            state,
            ..
        } = held;
        if !keep_memory {
            if let Err(err) = handle.kill() {
                let state = thaw(state, &handle);
                run.return_vm_in(vm, handle, state);
                return Err(VmOpError::Refused(err.to_string()));
            }
            if store.view(cid, snapshots::SLEEP).is_some() {
                if let Err(err) = store.remove(cid, snapshots::SLEEP) {
                    run.set_vm_killed(vm, handle);
                    return Err(VmOpError::Failed(format!(
                        "VM {vm} stopped, but its stale sleep snapshot could not be removed: {err}"
                    )));
                }
            }
            run.return_vm_in(vm, handle, VmState::Asleep);
            return Ok((run, None));
        }
        let partial = store.dir(cid, snapshots::SLEEP_PARTIAL);
        let _ = fs::remove_dir_all(&partial);
        let info = match write_snapshot(
            &handle,
            restored_from.as_deref(),
            &partial,
            SnapshotKind::Diff,
        ) {
            Ok(info) => info,
            Err(err) => {
                let _ = fs::remove_dir_all(&partial);
                let state = thaw(state, &handle);
                run.return_vm_in(vm, handle, state);
                return Err(err);
            }
        };
        if let Err(err) = handle.kill() {
            let _ = fs::remove_dir_all(&partial);
            let state = thaw(state, &handle);
            run.return_vm_in(vm, handle, state);
            return Err(VmOpError::Refused(err.to_string()));
        }
        if let Err(err) = store.install(cid, &partial, snapshots::SLEEP) {
            run.set_vm_killed(vm, handle);
            return Err(VmOpError::Failed(format!(
                "VM {vm} stopped, but its sleep snapshot could not be kept: {err}"
            )));
        }
        run.return_vm_in(vm, handle, VmState::Asleep);
        let Some(mut view) = store.view(cid, snapshots::SLEEP) else {
            return Err(VmOpError::Failed(format!(
                "VM {vm} is asleep, but its sleep snapshot cannot be read"
            )));
        };
        view.paused_ms = info.paused_ms;
        Ok((run, Some(view)))
    }

    /// Wakes VM `vm` of run `run` (see `wake_vm_inner`), timed: one `timing op=wake` line and
    /// `crate::metrics::WAKE`. The bool is true when the warm restore failed and the VM booted cold instead.
    pub fn wake_vm(&self, run: &str, vm: u32, cold: bool) -> Result<(Arc<Run>, bool), VmOpError> {
        let mut phases = apz_metrics::Phases::start("wake", format!("{run}/{vm}"));
        let result = self.wake_vm_inner(run, vm, cold);
        phases.mark("spawn");
        let fallback = matches!(result, Ok((_, true)));
        let ok = result.is_ok();
        phases.finish(
            if ok {
                Some(&crate::metrics::WAKE)
            } else {
                None
            },
            &[("cold", &cold), ("fallback", &fallback), ("ok", &ok)],
        );
        if fallback {
            crate::metrics::WAKE_COLD.inc();
        }
        result
    }

    /// Wakes asleep VM `vm` of run `run` from its sleep snapshot: it runs again with the CID and
    /// workspace it slept with. A snapshot that will not restore is not fatal: the VM boots cold from
    /// its spec, so an agent is never lost to a bad snapshot (the cause is logged).
    /// The bool is true only when that cold boot after a failed warm restore happened.
    fn wake_vm_inner(&self, run: &str, vm: u32, cold: bool) -> Result<(Arc<Run>, bool), VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let cid = run.vm_cid(vm)?;
        let sleep = store.dir(cid, snapshots::SLEEP);
        // A cold wake, or a VM with no sleep snapshot, boots from its spec; the sleep snapshot itself
        // is left as it is.
        let warm = (!cold && store.view(cid, snapshots::SLEEP).is_some()).then(|| sleep.clone());
        // A VM that arrived asleep from another node has never run here: there is no old handle to
        // swap out, only its spec.
        if let Some(spec) = run.take_unbooted(vm)? {
            return match spawn_timed(VmSpec {
                restore: warm.clone(),
                ..spec.clone()
            }) {
                Ok(fresh) => {
                    run.seat_first(vm, spec, fresh, warm);
                    Ok((run, false))
                }
                Err(err) => {
                    run.untake_unbooted(vm, spec);
                    Err(VmOpError::Failed(format!(
                        "VM {vm} did not wake from its imported snapshot: {err}"
                    )))
                }
            };
        }
        let Held {
            handle: killed,
            spec,
            ..
        } = run.take_vm(vm, &[VmState::Asleep])?;
        let boot_cold = |spec: VmSpec, warm_err: Option<String>| match spawn_timed(VmSpec {
            restore: None,
            ..spec
        }) {
            Ok(fresh) => Ok((fresh, None)),
            Err(cold) => Err(VmOpError::Failed(match warm_err {
                Some(err) => format!("VM {vm} did not wake ({err}) and did not boot ({cold})"),
                None => format!("VM {vm} did not boot ({cold})"),
            })),
        };
        let woken = match warm {
            Some(base) => match spawn_timed(VmSpec {
                restore: Some(base.clone()),
                ..spec.clone()
            }) {
                Ok(fresh) => Ok((fresh, Some(base), false)),
                Err(err) => {
                    eprintln!(
                        "noded: run {}: VM {vm} did not wake from its sleep snapshot ({err}); booting it cold",
                        run.id
                    );
                    boot_cold(spec, Some(err.to_string())).map(|(fresh, base)| (fresh, base, true))
                }
            },
            None => boot_cold(spec, None).map(|(fresh, base)| (fresh, base, false)),
        };
        match woken {
            Ok((fresh, base, fallback)) => {
                run.seat_vm(vm, killed, fresh, base);
                Ok((run, fallback))
            }
            Err(err) => {
                run.return_vm(vm, killed);
                Err(err)
            }
        }
    }

    /// Takes checkpoint `label` of VM `vm` of run `run` (see `checkpoint_vm_inner`), timed: one
    /// `timing op=checkpoint` line and `crate::metrics::CHECKPOINT`.
    pub fn checkpoint_vm(
        &self,
        run: &str,
        vm: u32,
        label: &str,
        keep_memory: bool,
    ) -> Result<SnapshotView, VmOpError> {
        let mut phases = apz_metrics::Phases::start("checkpoint", format!("{run}/{vm}"));
        let result = self.checkpoint_vm_inner(run, vm, label, keep_memory, &mut phases);
        let ok = result.is_ok();
        let paused_ms = result.as_ref().map_or(0, |view| view.paused_ms);
        phases.finish(
            if ok {
                Some(&crate::metrics::CHECKPOINT)
            } else {
                None
            },
            &[
                ("memory", &keep_memory),
                ("ok", &ok),
                ("paused_ms", &paused_ms),
            ],
        );
        result
    }

    /// Takes checkpoint `label` of VM `vm` of run `run`: a copy of its memory and its workspace, from
    /// which the VM can be restored or forked. The VM is paused for the capture of its memory and the
    /// copy of its workspace, and runs on after them; the memory is written while it runs. With
    /// `keep_memory == false` only the workspace is copied (a disk-only checkpoint): a restore
    /// or a fork of it boots cold from that disk. A VM with no workspace volume has no disk to keep.
    /// noded does not flush the guest's page cache: a caller taking a disk-only checkpoint of a
    /// running VM flushes the guest first (apid's E2B does it with the daemon's `POST /sync`).
    fn checkpoint_vm_inner(
        &self,
        run: &str,
        vm: u32,
        label: &str,
        keep_memory: bool,
        phases: &mut apz_metrics::Phases,
    ) -> Result<SnapshotView, VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let cid = run.vm_cid(vm)?;
        let held = run.take_vm(vm, &[VmState::Running, VmState::Paused])?;
        let taken = if keep_memory {
            Self::checkpoint_held(store, cid, &held, label, SnapshotKind::Diff)
        } else {
            Self::checkpoint_disk(store, cid, &held, label)
        };
        phases.mark("capture");
        let state = thaw(held.state, &held.handle);
        let view = taken.and_then(|taken| Self::finish_checkpoint(store, cid, &held.handle, taken));
        phases.mark("write");
        run.return_vm_in(vm, held.handle, state);
        let view = view?;
        store.prune(cid);
        Ok(view)
    }

    /// A disk-only checkpoint of a held VM: its workspace copied while the VM is paused (left paused,
    /// as [`Registry::checkpoint_held`] leaves it), and the marker that says there is no memory.
    fn checkpoint_disk(
        store: &SnapshotStore,
        cid: u64,
        held: &Held,
        label: &str,
    ) -> Result<PendingCheckpoint, VmOpError> {
        let Some(workspace) = &held.spec.workspace else {
            return Err(VmOpError::Invalid(
                "this VM has no workspace volume, so there is no disk to keep".to_string(),
            ));
        };
        store.prepare(cid).map_err(|err| {
            VmOpError::Failed(format!("cannot create the snapshot directory: {err}"))
        })?;
        let taken_at_ms = now_ms();
        let name = SnapshotStore::checkpoint_name(label, taken_at_ms);
        let dir = store.dir(cid, &name);
        held.handle
            .pause()
            .map_err(|err| VmOpError::Refused(err.to_string()))?;
        let kept = snapshots::private_dir(&dir)
            .and_then(|()| clone_file(workspace, &dir.join(snapshots::WORKSPACE_FILE)).map(|_| ()))
            .and_then(|()| {
                fs::File::open(dir.join(snapshots::WORKSPACE_FILE)).and_then(|copy| copy.sync_all())
            })
            .and_then(|()| snapshots::write_disk_marker(&dir, taken_at_ms));
        if let Err(err) = kept {
            let _ = fs::remove_dir_all(&dir);
            return Err(VmOpError::Failed(format!(
                "cannot copy the workspace into checkpoint {name}: {err}"
            )));
        }
        Ok(PendingCheckpoint {
            name,
            paused_ms: 0,
            captured: false,
        })
    }

    /// Completes a checkpoint: writes the memory its capture kept in RAM, from `handle` (the VM it was
    /// taken from, running or paused), then returns the checkpoint's view. A failed write removes the
    /// checkpoint, as a failed snapshot does.
    fn finish_checkpoint(
        store: &SnapshotStore,
        cid: u64,
        handle: &VmHandle,
        taken: PendingCheckpoint,
    ) -> Result<SnapshotView, VmOpError> {
        if taken.captured {
            if let Err(err) = handle.write_captured() {
                let _ = fs::remove_dir_all(store.dir(cid, &taken.name));
                return Err(VmOpError::Failed(format!(
                    "cannot write the VM's memory into checkpoint {}: {err}",
                    taken.name
                )));
            }
        }
        let mut view = store.view(cid, &taken.name).ok_or_else(|| {
            VmOpError::Failed(format!("checkpoint {} cannot be read back", taken.name))
        })?;
        view.paused_ms = taken.paused_ms;
        Ok(view)
    }

    /// The checkpoints of VM `vm` of run `run`, newest first.
    pub fn checkpoints(&self, run: &str, vm: u32) -> Result<Vec<SnapshotView>, VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        Ok(store.checkpoints(run.vm_cid(vm)?))
    }

    /// Deletes checkpoint `name` of VM `vm` of run `run`. The sleep snapshot is not a checkpoint and
    /// is not deleted this way.
    pub fn delete_checkpoint(&self, run: &str, vm: u32, name: &str) -> Result<(), VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let cid = run.vm_cid(vm)?;
        if !snapshots::is_checkpoint_name(name) || store.view(cid, name).is_none() {
            return Err(VmOpError::UnknownCheckpoint(name.to_string()));
        }
        store
            .remove(cid, name)
            .map_err(|err| VmOpError::Failed(format!("cannot delete checkpoint {name}: {err}")))
    }

    /// Restores VM `vm` of run `run` to checkpoint `name`: the VM is checkpointed first (the
    /// `before-restore` checkpoint, returned), then stopped and booted from `name` with its workspace
    /// copied back. If the restore fails, the VM is put back on the `before-restore` checkpoint (or
    /// booted cold, if that fails too) and the restore's error is returned.
    pub fn restore_checkpoint(
        &self,
        run: &str,
        vm: u32,
        name: &str,
    ) -> Result<(Arc<Run>, SnapshotView), VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let cid = run.vm_cid(vm)?;
        if !snapshots::is_checkpoint_name(name) || store.view(cid, name).is_none() {
            return Err(VmOpError::UnknownCheckpoint(name.to_string()));
        }
        let held = run.take_vm(vm, &[VmState::Running, VmState::Paused])?;
        // The VM is killed next, and a capture's memory lives in the VM's process, so it is written
        // here, while the VM is still paused.
        let safety =
            match Self::checkpoint_held(store, cid, &held, "before-restore", SnapshotKind::Diff)
                .and_then(|taken| Self::finish_checkpoint(store, cid, &held.handle, taken))
            {
                Ok(view) => view,
                Err(err) => {
                    let state = thaw(held.state, &held.handle);
                    run.return_vm_in(vm, held.handle, state);
                    return Err(err);
                }
            };
        let Held {
            mut handle,
            spec,
            state,
            ..
        } = held;
        if let Err(err) = handle.kill() {
            let state = thaw(state, &handle);
            run.return_vm_in(vm, handle, state);
            return Err(VmOpError::Refused(err.to_string()));
        }
        let snapshot = store.dir(cid, name);
        match boot_from(store, cid, &spec, &snapshot) {
            Ok((fresh, base)) => {
                run.seat_vm(vm, handle, fresh, base);
                store.prune(cid);
                Ok((run, safety))
            }
            Err(err) => {
                let before = store.dir(cid, &safety.name);
                let (fresh, base, back) = match boot_from(store, cid, &spec, &before) {
                    Ok((fresh, base)) => (fresh, base, safety.name.clone()),
                    Err(back) => match spawn_timed(spec.clone()) {
                        Ok(fresh) => {
                            eprintln!(
                                "noded: run {}: VM {vm} did not return to {} ({back}); booted it cold",
                                run.id, safety.name
                            );
                            (fresh, None, "a cold boot".to_string())
                        }
                        Err(cold) => {
                            run.set_vm_killed(vm, handle);
                            return Err(VmOpError::Failed(format!(
                                "cannot restore checkpoint {name} ({err}), and VM {vm} did not start again ({back}; {cold})"
                            )));
                        }
                    },
                };
                run.seat_vm(vm, handle, fresh, base);
                Err(VmOpError::Failed(format!(
                    "cannot restore checkpoint {name}: {err}; VM {vm} runs again from {back}"
                )))
            }
        }
    }

    /// Forks VM `vm` of run `run` into `n` new single-VM runs (see `fork_vm_inner`), timed: one
    /// `timing op=fork` line and `crate::metrics::FORK`.
    pub fn fork_vm(
        &self,
        run: &str,
        vm: u32,
        n: u32,
        from: Option<&str>,
    ) -> Result<(SnapshotView, Vec<(Arc<Run>, u64)>), VmOpError> {
        let mut phases = apz_metrics::Phases::start("fork", format!("{run}/{vm}"));
        let result = self.fork_vm_inner(run, vm, n, from, &mut phases);
        phases.mark("boot");
        let ok = result.is_ok();
        let paused_ms = result
            .as_ref()
            .map_or(0, |(checkpoint, _)| checkpoint.paused_ms);
        phases.finish(
            if ok {
                Some(&crate::metrics::FORK)
            } else {
                None
            },
            &[("copies", &n), ("ok", &ok), ("paused_ms", &paused_ms)],
        );
        result
    }

    /// Forks VM `vm` of run `run` into `n` new single-VM runs: a checkpoint of the VM is taken, and each
    /// fork boots from it with a CID of its own and a copy of its workspace. The forks are all made or
    /// none is: a fork that cannot boot takes the ones made before it down. Returns the checkpoint and
    /// each fork's run and CID.
    fn fork_vm_inner(
        &self,
        run: &str,
        vm: u32,
        n: u32,
        from: Option<&str>,
        phases: &mut apz_metrics::Phases,
    ) -> Result<(SnapshotView, Vec<(Arc<Run>, u64)>), VmOpError> {
        if !(1..=MAX_FORKS).contains(&n) {
            return Err(VmOpError::Invalid(format!(
                "n must be between 1 and {MAX_FORKS}"
            )));
        }
        let store = self.snapshot_store()?;
        let source = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let cid = source.vm_cid(vm)?;
        let (checkpoint, meta, has_workspace) = match from {
            // A named checkpoint is forked as it is: the source VM is not taken, so it may be running,
            // paused or asleep.
            Some(name) => {
                if !snapshots::is_checkpoint_name(name) {
                    return Err(VmOpError::UnknownCheckpoint(name.to_string()));
                }
                let view = store
                    .view(cid, name)
                    .ok_or_else(|| VmOpError::UnknownCheckpoint(name.to_string()))?;
                let (meta, has_workspace) = source.fork_shape(vm)?;
                (view, meta, has_workspace)
            }
            None => {
                let held = source.take_vm(vm, &[VmState::Running, VmState::Paused])?;
                let meta = source.meta(held.spec.balloon);
                let has_workspace = held.spec.workspace.is_some();
                // A fork's checkpoint only has to outlive the restores made from it on this host, so
                // it is written without the fsyncs.
                let taken =
                    Self::checkpoint_held(store, cid, &held, "fork", SnapshotKind::DiffUnsynced);
                phases.mark("capture");
                let state = thaw(held.state, &held.handle);
                // The copies boot from the checkpoint, so its memory is written before they do.
                let checkpoint = taken
                    .and_then(|taken| Self::finish_checkpoint(store, cid, &held.handle, taken));
                phases.mark("write");
                source.return_vm_in(vm, held.handle, state);
                let checkpoint = checkpoint?;
                store.prune(cid);
                (checkpoint, meta, has_workspace)
            }
        };
        let snapshot = store.dir(cid, &checkpoint.name);
        // The copies boot concurrently: each boot has its own run id, CID, workspace and base. The
        // results come back in index order, so the forks are returned in that order too.
        let booted: Vec<Result<PreparedFork, VmOpError>> = std::thread::scope(|scope| {
            let boots: Vec<_> = (0..n)
                .map(|_| scope.spawn(|| self.boot_fork(store, &meta, &snapshot, has_workspace)))
                .collect();
            boots
                .into_iter()
                .map(|boot| {
                    boot.join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        });
        let mut forks = Vec::with_capacity(n as usize);
        let mut failure = None;
        for result in booted {
            match result {
                Ok(fork) => forks.push(fork),
                Err(err) => {
                    failure.get_or_insert(err);
                }
            }
        }
        if let Some(err) = failure {
            self.discard_forks(store, forks);
            return Err(err);
        }
        let made = forks
            .into_iter()
            .map(|fork| {
                let cid = fork.cid;
                (self.adopt_fork(fork, &meta), cid)
            })
            .collect();
        Ok((checkpoint, made))
    }

    /// The store, or [`VmOpError::NoSnapshots`] when the node keeps none.
    fn snapshot_store(&self) -> Result<&SnapshotStore, VmOpError> {
        self.snapshots.as_ref().ok_or(VmOpError::NoSnapshots)
    }

    /// The node's snapshot root, where a transfer stages the files it receives (see `crate::migrate`).
    pub fn snapshot_root(&self) -> Result<&Path, VmOpError> {
        Ok(self.snapshot_store()?.root())
    }

    /// Whether a live run of this node holds guest CID `cid`.
    pub fn cid_in_use(&self, cid: u64) -> bool {
        busy(&self.cids.held()).contains(&cid)
    }

    /// The asleep VM `vm` of run `run`, as an export reads it: its guest CID, role, cold spec and
    /// the directory holding its sleep snapshot. The VM must be asleep.
    pub fn asleep_vm(&self, run: &str, vm: u32) -> Result<AsleepVm, VmOpError> {
        let store = self.snapshot_store()?;
        let run = self.find(run).ok_or(VmOpError::UnknownRun)?;
        let (cid, role, spec) = run.asleep_slot(vm)?;
        Ok(AsleepVm {
            sleep_dir: store.dir(cid, snapshots::SLEEP),
            cid,
            role,
            spec,
        })
    }

    /// Makes an imported VM this node's: its sleep snapshot and workspace image are moved into
    /// place, its CID is held, and a run of its own holds it asleep - a `running` run whose one VM
    /// sleeps, as a sleep leaves it, so `wake` restores it here. The files are already verified
    /// (see `crate::migrate`); a failure here leaves the node as it was, with the CID released.
    pub fn import_run(&self, vm: ImportedVm) -> Result<Arc<Run>, VmOpError> {
        check_import_shape(vm.cid, vm.role.as_deref(), vm.vcpus, vm.memory_mib)?;
        let vcpus = spawner_vcpus(vm.vcpus).map_err(VmOpError::Invalid)?;
        let memory_mib = spawner_memory_mib(vm.memory_mib).map_err(VmOpError::Invalid)?;
        let store = self.snapshot_store()?;
        let id = self.reserve_run_id();
        // A sandbox travels with its command line: its import is a sandbox, never a keepalive agent.
        let sandbox = is_sandbox_cmdline(&vm.cmdline);
        // The CID is the hub's choice, so it may sit outside the run range; the book holds it all
        // the same, for as long as the run lives.
        if !self.cids.hold_unless_busy(&id, vec![vm.cid]) {
            return Err(VmOpError::CidInUse(vm.cid));
        }
        let workspace = match self.place_import(&id, &vm, store) {
            Ok(workspace) => workspace,
            Err(err) => {
                self.cids.release(&id);
                return Err(err);
            }
        };
        let spec = VmSpec {
            kernel: self.images.kernel.clone(),
            cmdline: vm.cmdline.clone(),
            vcpus,
            memory_mib,
            initrd: Some(self.images.initrd.clone()),
            gic: None,
            vsock: Some(VsockConfig {
                guest_cid: vm.cid,
                uds_path: self.uds_dir.join(format!("vm{}", vm.cid)),
            }),
            workspace,
            balloon: vm.balloon_usable_mib,
            restore: None,
            volumes: Vec::new(),
        };
        let run = self.insert(
            id,
            RunMeta {
                vcpus: vm.vcpus,
                memory_mib: vm.memory_mib,
                beats: None,
                role: vm.role.clone(),
                keepalive: !sandbox,
                balloon_usable_mib: vm.balloon_usable_mib,
                sandbox,
                volumes: Vec::new(),
            },
        );
        {
            let mut inner = run.lock();
            // `insert` registered the run in `starting`; an imported run begins `running` with its
            // one VM asleep, the state a sleep leaves a run in.
            inner.state = RunState::Running;
            inner.vms = vec![VmSlot::asleep(0, vm.cid, vm.role, spec)];
            run.events.publish(Event::Run {
                id: run.id.clone(),
                state: RunState::Running.as_str(),
            });
            run.events.publish(Event::Vm {
                run: run.id.clone(),
                vm: 0,
                state: VmState::Asleep.as_str(),
            });
        }
        Ok(run)
    }

    /// Moves an imported VM's files into the node's places for guest CID `vm.cid` and returns the
    /// workspace image's path, or `None` for a VM with no workspace volume: the sleep snapshot takes
    /// the CID's `sleep` directory, and the workspace image the CID's persistent image (or the run's
    /// directory without a persist dir).
    fn place_import(
        &self,
        id: &str,
        vm: &ImportedVm,
        store: &SnapshotStore,
    ) -> Result<Option<PathBuf>, VmOpError> {
        store.prepare(vm.cid).map_err(|err| {
            VmOpError::Failed(format!("cannot create VM {}'s directory: {err}", vm.cid))
        })?;
        store
            .install(vm.cid, &vm.sleep_dir, snapshots::SLEEP)
            .map_err(|err| VmOpError::Failed(format!("cannot keep the sleep snapshot: {err}")))?;
        let Some(staged) = &vm.workspace else {
            return Ok(None);
        };
        let dest = match &self.persist_dir {
            // A sandbox's workspace is its run's own copy, never the persistent image.
            Some(persist) if !is_sandbox_cmdline(&vm.cmdline) => {
                let image = persistent_workspace_path(persist, vm.cid);
                if let Some(dir) = image.parent() {
                    snapshots::private_dir(dir).map_err(|err| {
                        VmOpError::Failed(format!("cannot create {}: {err}", dir.display()))
                    })?;
                }
                image
            }
            _ => {
                let dir = self.run_dir(id);
                snapshots::private_dir(&dir).map_err(|err| {
                    VmOpError::Failed(format!("cannot create {}: {err}", dir.display()))
                })?;
                // The run's copies are named by VM index, and this run's one VM is index 0.
                dir.join("workspace-0.ext4")
            }
        };
        if let Err(err) = move_file(staged, &dest) {
            // The sleep snapshot was kept for this VM; it goes with the workspace it cannot be paired with.
            let _ = store.remove(vm.cid, snapshots::SLEEP);
            return Err(VmOpError::Failed(format!(
                "cannot place the workspace image: {err}"
            )));
        }
        Ok(Some(dest))
    }

    /// A running run whose one VM is in `state`, with no handle or spec: a stand-in for the
    /// registry's tests of the operations that refuse a VM by its state.
    #[cfg(test)]
    pub(crate) fn stand_in_run(&self, cid: u64, state: VmState) -> Arc<Run> {
        let id = self.reserve_run_id();
        let run = self.insert(
            id,
            RunMeta {
                vcpus: 1,
                memory_mib: MIN_MEMORY_MIB,
                beats: None,
                role: None,
                keepalive: true,
                balloon_usable_mib: None,
                sandbox: false,
                volumes: Vec::new(),
            },
        );
        {
            let mut inner = run.lock();
            inner.state = RunState::Running;
            inner.vms = vec![VmSlot {
                id: 0,
                guest_cid: Some(cid),
                role: None,
                runtime: Arc::new(VmRuntime::new(0)),
                handle: None,
                spec: None,
                state,
                serial_seen: 0,
                restored_from: None,
            }];
        }
        run
    }

    /// Removes the snapshots of every VM of `run`: what deleting the run does with them.
    pub fn drop_snapshots(&self, run: &Run) {
        let Some(store) = &self.snapshots else {
            return;
        };
        let cids: Vec<u64> = run
            .lock()
            .vms
            .iter()
            .filter_map(|slot| slot.guest_cid)
            .collect();
        for cid in cids {
            if let Err(err) = store.remove_all(cid) {
                eprintln!(
                    "noded: run {}: cannot remove VM CID {cid}'s snapshots: {err}",
                    run.id
                );
            }
        }
    }

    /// Captures a checkpoint of a held VM: its memory (a diff, when it was restored) and a copy of its
    /// workspace, both taken while the VM is paused. `kind` is the snapshot kind asked for. The memory
    /// waits in RAM for [`Registry::finish_checkpoint`]. The VM is left paused; the caller resumes it.
    fn checkpoint_held(
        store: &SnapshotStore,
        cid: u64,
        held: &Held,
        label: &str,
        kind: SnapshotKind,
    ) -> Result<PendingCheckpoint, VmOpError> {
        store.prepare(cid).map_err(|err| {
            VmOpError::Failed(format!("cannot create the snapshot directory: {err}"))
        })?;
        let name = SnapshotStore::checkpoint_name(label, now_ms());
        let dir = store.dir(cid, &name);
        // The base memory is the file the VM was restored from, which never changes: it is staged
        // before the pause.
        let staged = snapshots::private_dir(&dir)
            .map_err(|err| VmOpError::Failed(format!("cannot create {}: {err}", dir.display())))
            .and_then(|()| stage_base_memory(held.restored_from.as_deref(), &dir));
        if let Err(err) = staged {
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }
        match held.handle.capture(&dir, kind) {
            Ok(info) => {
                // The workspace is copied inside the pause the capture left, so the disk and the
                // memory are of the same instant (a disk copied while the guest ran could be behind its
                // page cache). A failed copy writes the capture out and discards it with the dir.
                if let Some(workspace) = &held.spec.workspace {
                    if let Err(err) = clone_file(workspace, &dir.join(snapshots::WORKSPACE_FILE)) {
                        let _ = held.handle.write_captured();
                        let _ = fs::remove_dir_all(&dir);
                        return Err(VmOpError::Failed(format!(
                            "cannot copy the workspace into checkpoint {name}: {err}"
                        )));
                    }
                }
                Ok(PendingCheckpoint {
                    name,
                    paused_ms: info.paused_ms,
                    captured: true,
                })
            }
            Err(err) => {
                // A capture that failed after keeping the memory (a reply lost in transit) would
                // refuse the VM's next capture: its memory is written out, and discarded with the dir.
                let _ = held.handle.write_captured();
                let _ = fs::remove_dir_all(&dir);
                Err(VmOpError::Failed(format!("the VM did not snapshot: {err}")))
            }
        }
    }

    /// Boots one fork of the checkpoint at `snapshot`: a single-VM run with a CID of its own and, when
    /// the forked VM has a workspace volume (`workspace`), a copy of the checkpoint's workspace. A fork
    /// of a VM with no workspace volume has none either. Nothing is registered until every fork has
    /// booted.
    fn boot_fork(
        &self,
        store: &SnapshotStore,
        meta: &RunMeta,
        snapshot: &Path,
        workspace: bool,
    ) -> Result<PreparedFork, VmOpError> {
        let id = self.reserve_run_id();
        let cid = self.cids.allocate(&id, 1).map_err(VmOpError::Failed)?;
        let workspace = workspace.then(|| self.fork_workspace(&id, cid, meta.keepalive));
        match self.boot_fork_vm(store, meta, snapshot, cid, workspace.as_deref()) {
            Ok((handle, spec, base)) => Ok(PreparedFork {
                id,
                cid,
                handle,
                spec,
                workspace,
                base,
            }),
            Err(err) => {
                self.cids.release(&id);
                if let Some(workspace) = &workspace {
                    let _ = fs::remove_file(workspace);
                }
                let _ = fs::remove_dir_all(store.dir(cid, snapshots::BASE));
                Err(err)
            }
        }
    }

    /// The VM of a fork: restored from `snapshot`, with its workspace copied from it when it has one.
    fn boot_fork_vm(
        &self,
        store: &SnapshotStore,
        meta: &RunMeta,
        snapshot: &Path,
        cid: u64,
        workspace: Option<&Path>,
    ) -> Result<(VmHandle, VmSpec, Option<PathBuf>), VmOpError> {
        if let Some(parent) = workspace.and_then(Path::parent) {
            snapshots::private_dir(parent).map_err(|err| {
                VmOpError::Failed(format!("cannot create {}: {err}", parent.display()))
            })?;
        }
        // The fork's spec is the one a restart would boot it from: the cold command line, no restore.
        let request = CreateRun {
            n: 1,
            vcpus: meta.vcpus,
            memory_mib: meta.memory_mib,
            beats: meta.beats,
            vsock_cid: None,
            role: meta.role.clone(),
            keepalive: Some(meta.keepalive),
            balloon_usable_mib: meta.balloon_usable_mib,
            // A fork of a sandbox is a sandbox: its command line is the sandbox's, so a restart of
            // the fork boots the same daemon.
            sandbox: meta.sandbox,
            volumes: meta.volumes.iter().map(|v| v.name.clone()).collect(),
        };
        let spec = VmSpec {
            kernel: self.images.kernel.clone(),
            cmdline: request.vm_cmdline(0, meta.keepalive),
            vcpus: spawner_vcpus(meta.vcpus).map_err(VmOpError::Failed)?,
            memory_mib: spawner_memory_mib(meta.memory_mib).map_err(VmOpError::Failed)?,
            initrd: Some(self.images.initrd.clone()),
            gic: None,
            vsock: Some(VsockConfig {
                guest_cid: cid,
                uds_path: self.uds_dir.join(format!("vm{cid}")),
            }),
            workspace: workspace.map(Path::to_path_buf),
            balloon: meta.balloon_usable_mib,
            restore: None,
            volumes: meta.volumes.clone(),
        };
        let (handle, base) = boot_from(store, cid, &spec, snapshot).map_err(VmOpError::Failed)?;
        Ok((handle, spec, base))
    }

    /// The workspace image of a fork on `cid`: the CID's persistent image for a keepalive run on a
    /// node with `--persist-dir` when none exists yet (so the image persists like any keepalive
    /// agent's), otherwise a file in the fork's run directory. A persistent image that already exists
    /// belongs to an earlier agent on that CID, and is never overwritten.
    fn fork_workspace(&self, id: &str, cid: u64, keepalive: bool) -> PathBuf {
        if let (Some(persist), true) = (&self.persist_dir, keepalive) {
            let image = persistent_workspace_path(persist, cid);
            if !image.exists() {
                return image;
            }
        }
        self.run_dir(id).join(format!("fork-{cid}.ext4"))
    }

    /// Registers a fork as a run: `starting`, then `running` with its one VM, watched like any run.
    fn adopt_fork(&self, fork: PreparedFork, meta: &RunMeta) -> Arc<Run> {
        let PreparedFork {
            id,
            cid,
            handle,
            spec,
            base,
            ..
        } = fork;
        let run = self.insert(id, meta.clone());
        let mut inner = run.lock();
        if let Err(refused) = inner.transition(RunState::Running) {
            eprintln!("noded: run {}: {refused}", run.id);
        }
        inner.vms = vec![VmSlot {
            id: 0,
            guest_cid: Some(cid),
            role: meta.role.clone(),
            runtime: handle.runtime(),
            handle: Some(handle),
            spec: Some(spec),
            state: VmState::Running,
            serial_seen: 0,
            restored_from: base,
        }];
        run.events.publish(Event::Run {
            id: run.id.clone(),
            state: RunState::Running.as_str(),
        });
        run.events.publish(Event::Vm {
            run: run.id.clone(),
            vm: 0,
            state: VmState::Running.as_str(),
        });
        drop(inner);
        watch(Arc::clone(&run));
        run
    }

    /// Stops and removes forks that were booted before a later one failed: their VMs stop, their CIDs
    /// go back to the book and their workspace copies and bases are removed.
    fn discard_forks(&self, store: &SnapshotStore, forks: Vec<PreparedFork>) {
        for mut fork in forks {
            if let Err(err) = fork.handle.kill() {
                eprintln!("noded: fork {}: VM did not stop: {err}", fork.id);
            }
            self.cids.release(&fork.id);
            if let Some(workspace) = &fork.workspace {
                let _ = fs::remove_file(workspace);
            }
            let _ = fs::remove_dir_all(store.dir(fork.cid, snapshots::BASE));
        }
    }

    /// The restart and wipe of one VM: stops it, resets its workspace when
    /// `wipe` is set, boots it again from its spec and seats it in its slot.
    fn cycle_vm(&self, run: &Run, vm: u32, wipe: bool) -> Result<(), VmOpError> {
        let base = match (wipe, &self.images.workspace) {
            (true, None) => return Err(VmOpError::NoBaseImage),
            (true, Some(base)) => Some(base.clone()),
            (false, _) => None,
        };
        let Held {
            handle: mut killed,
            spec,
            ..
        } = run.take_vm(vm, &AWAKE)?;
        if let Err(err) = killed.kill() {
            run.return_vm(vm, killed);
            return Err(VmOpError::Refused(err.to_string()));
        }
        if let Some(base) = &base {
            if let Err(err) = self.reset_workspace(base, run, &spec) {
                run.set_vm_killed(vm, killed);
                return Err(VmOpError::Failed(format!(
                    "cannot reset VM {vm}'s workspace: {err}"
                )));
            }
        }
        match spawn_timed(spec) {
            Ok(fresh) => {
                run.seat_vm(vm, killed, fresh, None);
                Ok(())
            }
            Err(err) => {
                run.set_vm_killed(vm, killed);
                Err(VmOpError::Failed(format!(
                    "VM {vm} did not boot again: {err}"
                )))
            }
        }
    }

    /// Gives a VM's workspace a fresh copy of `base`. A keepalive run's
    /// persistent image is deleted and copied again; any other image is copied
    /// over in place.
    fn reset_workspace(
        &self,
        base: &std::path::Path,
        run: &Run,
        spec: &VmSpec,
    ) -> std::io::Result<()> {
        match (run.keepalive, &self.persist_dir, &spec.vsock) {
            (true, Some(persist), Some(vsock)) => {
                let image = persistent_workspace_path(persist, vsock.guest_cid);
                match std::fs::remove_file(&image) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
                prepare_persistent_workspace(base, persist, vsock.guest_cid).map(|_| ())
            }
            _ => match &spec.workspace {
                Some(image) => replace_workspace(base, image),
                None => Ok(()),
            },
        }
    }

    /// The registry's event bus, so a stream endpoint can subscribe to it.
    pub fn events_bus(&self) -> &EventBus {
        &self.events
    }

    /// Resizes VM `vm` of run `run` to `usable_mib` (see [`Run::set_memory`]).
    pub fn set_vm_memory(
        &self,
        run: &str,
        vm: u32,
        usable_mib: u32,
    ) -> Result<MemoryView, MemoryError> {
        let run = self.find(run).ok_or(MemoryError::UnknownRun)?;
        run.set_memory(vm, usable_mib)
    }

    /// The run with that id, if there is one.
    pub fn find(&self, id: &str) -> Option<Arc<Run>> {
        self.lock().runs.iter().find(|run| run.id == id).cloned()
    }

    /// Every run, oldest first.
    pub fn list(&self) -> Vec<Arc<Run>> {
        self.lock().runs.clone()
    }

    /// Reserves the next run id (`r{seq}`), consuming it.
    ///
    /// A run's id names its own directory under the UDS root
    /// (`run-{id}-{epoch_ms}`, which holds the run's workspace-image copies),
    /// so an id is never handed out twice within one daemon's life: a caller
    /// that reserves one and then fails leaves a gap in the numbering rather
    /// than letting a later run share its directory. Across a restart the id
    /// sequence begins again, which is exactly why the directory name carries
    /// the daemon's start stamp too — a new `r1` must not copy over the
    /// previous daemon's `r1` workspace.
    fn reserve_run_id(&self) -> String {
        let mut inner = self.lock();
        inner.next_seq += 1;
        format!("r{}", inner.next_seq)
    }

    /// Registers a new run in `starting`, which is where the schema says a run
    /// begins (`core.check_run`).
    fn insert(&self, id: String, meta: RunMeta) -> Arc<Run> {
        let mut inner = self.lock();
        let run = Arc::new(Run {
            id,
            vcpus: meta.vcpus,
            memory_mib: meta.memory_mib,
            beats: meta.beats,
            role: meta.role,
            keepalive: meta.keepalive,
            sandbox: meta.sandbox,
            volumes: meta.volumes,
            created_at: now_ms(),
            events: Arc::clone(&self.events),
            cids: Arc::clone(&self.cids),
            snapshots: self.snapshots.clone(),
            recoveries: Mutex::new(Vec::new()),
            inner: Mutex::new(RunInner {
                state: RunState::Starting,
                ended_at: None,
                vms: Vec::new(),
            }),
        });
        inner.runs.push(Arc::clone(&run));
        // The run is registered in `starting`: the first event a stream
        // carries for it. Registered under the registry's lock, so a
        // subscriber sees the run appear exactly once and in order.
        self.events.publish(Event::Run {
            id: run.id.clone(),
            state: RunState::Starting.as_str(),
        });
        run
    }

    fn lock(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Why a per-VM operation (restart, wipe or stop) did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmOpError {
    /// The node has no run with that id.
    UnknownRun,
    /// The run has no VM with that index.
    UnknownVm(u32),
    /// The run is not running, so its VMs cannot be operated on.
    RunEnded(RunState),
    /// Another restart, wipe or stop of the VM has not finished yet.
    Busy(u32),
    /// The node has no base workspace image to wipe a VM back to.
    NoBaseImage,
    /// The VM did not stop, so it was left as it was.
    Refused(String),
    /// The VM could not be prepared or booted again.
    Failed(String),
    /// The node keeps no snapshots (it was started with `--no-snapshots`).
    NoSnapshots,
    /// The VM has no snapshot of that name.
    UnknownCheckpoint(String),
    /// The request asks for something the node cannot do (an out-of-range count).
    Invalid(String),
    /// The VM is in a state the operation does not apply to (a paused VM cannot be paused again).
    WrongState { vm: u32, state: VmState },
    /// The VM has no vsock CID, which the snapshot operation needs.
    Unsupported(String),
    /// An import named a guest CID a run of this node already holds.
    CidInUse(u64),
}

impl VmOpError {
    /// The HTTP status this error is reported as: 404 for a run, VM or checkpoint the node does
    /// not have, 400 for a request out of range, 409 for one whose state refuses the operation
    /// now, 500 when the VM itself could not be stopped or booted.
    pub fn status(&self) -> u16 {
        match self {
            VmOpError::UnknownRun | VmOpError::UnknownVm(_) | VmOpError::UnknownCheckpoint(_) => {
                404
            }
            VmOpError::Invalid(_) => 400,
            VmOpError::RunEnded(_)
            | VmOpError::Busy(_)
            | VmOpError::NoBaseImage
            | VmOpError::NoSnapshots
            | VmOpError::WrongState { .. }
            | VmOpError::Unsupported(_) => 409,
            VmOpError::CidInUse(_) => 409,
            VmOpError::Refused(_) | VmOpError::Failed(_) => 500,
        }
    }
}

impl std::fmt::Display for VmOpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmOpError::UnknownRun => write!(formatter, "no run with that id"),
            VmOpError::UnknownVm(vm) => write!(formatter, "the run has no VM {vm}"),
            VmOpError::RunEnded(state) => {
                write!(
                    formatter,
                    "the run is {}, so its VMs cannot be operated on",
                    state.as_str()
                )
            }
            VmOpError::Busy(vm) => write!(
                formatter,
                "VM {vm} is already being restarted, wiped or stopped"
            ),
            VmOpError::NoBaseImage => {
                write!(
                    formatter,
                    "this node has no base workspace image to wipe a VM back to"
                )
            }
            VmOpError::Refused(reason) => write!(formatter, "VM did not stop: {reason}"),
            VmOpError::Failed(reason) => write!(formatter, "{reason}"),
            VmOpError::NoSnapshots => write!(
                formatter,
                "this node keeps no snapshots: it was started with --no-snapshots"
            ),
            VmOpError::UnknownCheckpoint(name) => {
                write!(formatter, "the VM has no snapshot named {name}")
            }
            VmOpError::Invalid(reason) => write!(formatter, "{reason}"),
            VmOpError::WrongState { vm, state } => write!(
                formatter,
                "VM {vm} is {}, so this operation does not apply to it",
                state.as_str()
            ),
            VmOpError::Unsupported(reason) => write!(formatter, "{reason}"),
            VmOpError::CidInUse(cid) => {
                write!(
                    formatter,
                    "guest CID {cid} is already held by a run of this node"
                )
            }
        }
    }
}

impl std::error::Error for VmOpError {}

/// One run: what the API reports about it and the VMs behind it.
pub struct Run {
    id: String,
    vcpus: u32,
    memory_mib: u32,
    beats: Option<u32>,
    /// The role every VM of the run runs as, when the run has one.
    role: Option<String>,
    /// Whether the run's guests are keepalive agents: their workspace images
    /// are persistent, so a wipe copies the base image over the persistent one.
    keepalive: bool,
    /// Whether the run's guests are sandboxes: never keepalive, and their
    /// workspaces are per-run copies of the base image.
    sandbox: bool,
    /// The shared data volumes every VM of the run carries.
    volumes: Vec<DataVolume>,
    created_at: u64,
    /// Where this run's state, VM and serial-delta events are published.
    events: Arc<EventBus>,
    /// The node's CID book: this run's span is held there for the run's
    /// lifetime and released when it ends ([`Run::release_cids`]).
    cids: Arc<CidBook>,
    /// The node's snapshot store, or `None` for a node that keeps none: where a
    /// recovery looks for the VM's newest snapshot.
    snapshots: Option<SnapshotStore>,
    /// The recoveries this run's VMs have started, as `(VM index, when)`, kept
    /// for the last [`RECOVERY_WINDOW_MS`] (the crash-loop limit counts them).
    recoveries: Mutex<Vec<(u32, u64)>>,
    inner: Mutex<RunInner>,
}

struct RunInner {
    state: RunState,
    ended_at: Option<u64>,
    vms: Vec<VmSlot>,
}

/// One VM of a run.
///
/// The serial capture and the console input are reached through the shared
/// runtime, and the control handle is taken out while the VM is being killed:
struct VmSlot {
    /// The VM's index among the run's VMs, as the API reports it.
    id: u32,
    /// The VM's vsock guest CID, when the run has vsock devices.
    guest_cid: Option<u64>,
    /// The role this VM's guest runs as, when it was started as one (the role
    /// fleet's VMs are).
    role: Option<String>,
    /// The VM's shared runtime: its serial capture and its console input.
    runtime: Arc<VmRuntime>,
    /// The VM's control handle (`wait`/`kill`), absent only while a kill has
    /// taken it out to stop the VM outside the run's lock.
    handle: Option<VmHandle>,
    /// The spec the VM was booted from, kept so a restart or wipe boots it
    /// again exactly as it was. Absent only for a slot that was not booted from
    /// a spec (the unit tests' stand-in VMs).
    spec: Option<VmSpec>,
    /// How the VM stopped (read and written under the run's lock).
    state: VmState,
    /// How much of the serial capture the event bus has already published
    /// (read and written under the run's lock): the serial-delta ticker's
    /// bookmark.
    serial_seen: usize,
    /// The snapshot directory holding the memory this VM was restored from (its
    /// wake's sleep snapshot, or its restore's staged base): the base its next
    /// snapshot is written as a diff over. `None` for a VM booted cold.
    restored_from: Option<PathBuf>,
}

impl VmSlot {
    /// A VM that is asleep on this node: stopped, its sleep snapshot kept under its CID's directory,
    /// and no control handle (no process runs it). `spec` is the cold spec a wake restores from.
    fn asleep(id: u32, cid: u64, role: Option<String>, spec: VmSpec) -> Self {
        Self {
            id,
            guest_cid: Some(cid),
            role,
            runtime: Arc::new(VmRuntime::new(id)),
            handle: None,
            spec: Some(spec),
            state: VmState::Asleep,
            serial_seen: 0,
            restored_from: None,
        }
    }
}

impl RunInner {
    /// Attempts `state -> to`. A refused transition changes nothing.
    fn transition(&mut self, to: RunState) -> Result<(), RefusedTransition> {
        let from = self.state;
        if !from.can_transition_to(to) {
            return Err(RefusedTransition { from, to });
        }
        self.state = to;
        if to.is_finished() {
            self.ended_at = Some(now_ms());
        }
        Ok(())
    }

    /// Ends the run as `done` once every VM has stopped by itself. A no-op
    /// when a kill already ended the run.
    fn finish(&mut self) {
        if self.state.can_transition_to(RunState::Done) {
            self.state = RunState::Done;
            self.ended_at = Some(now_ms());
        }
    }

    /// Whether every VM of the run has stopped. A paused or asleep VM has not: it is the run's
    /// to wake or resume, so its run goes on.
    fn all_vms_stopped(&self) -> bool {
        self.vms
            .iter()
            .all(|slot| matches!(slot.state, VmState::Exited | VmState::Killed))
    }
}

impl Run {
    /// The run's id (`r<seq>`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The process id of each running VM of the run that has one, as
    /// `(VM index, pid)`. A VM in process-less form, or mid-kill, has none.
    pub fn process_ids(&self) -> Vec<(u32, u32)> {
        let inner = self.lock();
        inner
            .vms
            .iter()
            .filter(|slot| slot.state == VmState::Running)
            .filter_map(|slot| Some((slot.id, slot.handle.as_ref()?.process_id()?)))
            .collect()
    }

    /// The run as the API reports it.
    pub fn view(&self, detail: Detail) -> RunView<'_> {
        let inner = self.lock();
        RunView {
            id: &self.id,
            state: inner.state.as_str(),
            vcpus: self.vcpus,
            memory_mib: self.memory_mib,
            beats: self.beats,
            role: self.role.as_deref(),
            sandbox: self.sandbox,
            created_at: self.created_at,
            ended_at: inner.ended_at,
            vms: inner
                .vms
                .iter()
                .map(|slot| VmView {
                    id: slot.id,
                    guest_cid: slot.guest_cid,
                    role: slot.role.clone(),
                    state: slot.state.as_str(),
                    memory: slot
                        .handle
                        .as_ref()
                        .and_then(VmHandle::memory_stats)
                        .map(MemoryView::from),
                    serial: match detail {
                        Detail::Summary => None,
                        Detail::Full => Some(serial_of(&slot.runtime)),
                    },
                })
                .collect(),
        }
    }

    /// Sends `data` to one of the run's VMs (`vm` is the VM's index).
    ///
    /// Input needs a running run: a run that has ended has no console to write
    /// to, and asking anyway is a conflict with its state, not a bad request.
    pub fn send_input(&self, vm: u32, data: &[u8]) -> Result<(), InputError> {
        let mut inner = self.lock();
        if inner.state != RunState::Running {
            return Err(InputError::NotRunning { state: inner.state });
        }
        let index = usize::try_from(vm).map_err(|_| InputError::UnknownVm(vm))?;
        let Some(slot) = inner.vms.get_mut(index) else {
            return Err(InputError::UnknownVm(vm));
        };
        slot.runtime
            .serial_input(data)
            .map_err(|err| InputError::Console {
                vm,
                reason: console_reason(err),
            })
    }

    /// Resizes one of the run's VMs' balloon to `usable_mib` and reports the
    /// VM's memory as the balloon now reads it.
    ///
    /// A VM with no balloon, or one that is no longer running (its handle is
    /// absent or it has stopped), is a conflict with its state, not a bad
    /// request.
    pub fn set_memory(&self, vm: u32, usable_mib: u32) -> Result<MemoryView, MemoryError> {
        let inner = self.lock();
        let slot = inner
            .vms
            .iter()
            .find(|slot| slot.id == vm)
            .ok_or(MemoryError::UnknownVm(vm))?;
        let handle = match (slot.state, slot.handle.as_ref()) {
            (VmState::Running, Some(handle)) => handle,
            _ => return Err(MemoryError::Stopped(vm)),
        };
        handle
            .set_usable_memory_mib(usable_mib)
            .map_err(|err| match err {
                SpawnError::NoBalloon => MemoryError::NoBalloon(vm),
                other => MemoryError::Refused(other.to_string()),
            })?;
        handle
            .memory_stats()
            .map(MemoryView::from)
            .ok_or(MemoryError::NoBalloon(vm))
    }

    /// Kills every VM of the run and marks it `killed`.
    ///
    /// Idempotent: a run that has already ended is left exactly as it is, and
    /// killing it again reports the same run. A VM that refuses to stop is
    /// reported (loudly, and in the run's JSON as still `running`) rather than
    /// claimed to be dead.
    pub fn kill(&self) {
        let mut inner = self.lock();
        if inner.state.is_finished() {
            return;
        }
        // `starting -> killed` and `running -> killed` are the only
        // transitions left here, so this cannot be refused.
        if let Err(refused) = inner.transition(RunState::Killed) {
            eprintln!("noded: run {}: {refused}", self.id);
            return;
        }
        // The kill took: publish it before the handles are stopped, so a
        // stream sees the run's `killed` before any VM's `killed`.
        self.events.publish(Event::Run {
            id: self.id.clone(),
            state: RunState::Killed.as_str(),
        });
        // Take the live handles out of the run before stopping them: a VM that
        // will not stop must not hold the run's lock, or every GET of the run
        // would wait behind it.
        let taken: Vec<(usize, VmHandle)> = inner
            .vms
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| match slot.state {
                VmState::Running | VmState::Paused => {
                    slot.handle.take().map(|handle| (index, handle))
                }
                VmState::Asleep | VmState::Exited | VmState::Killed => None,
            })
            .collect();
        drop(inner);

        let mut stopped = Vec::with_capacity(taken.len());
        for (index, mut handle) in taken {
            stopped.push((index, handle.kill(), handle));
        }

        let mut inner = self.lock();
        // A sleeping VM has no process to stop: its sleep snapshot is its whole life, and the kill
        // ends it.
        for slot in inner.vms.iter_mut() {
            if slot.state == VmState::Asleep {
                slot.state = VmState::Killed;
                self.events.publish(Event::Vm {
                    run: self.id.clone(),
                    vm: slot.id,
                    state: VmState::Killed.as_str(),
                });
            }
        }
        for (index, outcome, handle) in stopped {
            let Some(slot) = inner.vms.get_mut(index) else {
                continue;
            };
            slot.handle = Some(handle);
            let next_state = match outcome {
                Ok(()) => Some(VmState::Killed),
                // The VM is still running: the run is over either way, but
                // saying the VM stopped when it did not would hide a leak.
                Err(err) => {
                    eprintln!("noded: run {}: VM {} did not stop: {err}", self.id, slot.id);
                    None
                }
            };
            if let Some(next_state) = next_state {
                slot.state = next_state;
                self.events.publish(Event::Vm {
                    run: self.id.clone(),
                    vm: slot.id,
                    state: next_state.as_str(),
                });
            }
        }
        // The run is over once its VMs have been stopped, so its CIDs go back
        // to the book and the next run may draw them. A VM that refused to
        // stop still holds its CID's muxer socket, so its span is *not* given
        // back: the leak is reported above, and handing that CID to a new run
        // would make the new run's muxer bind fail on a path a live guest
        // still owns.
        let all_stopped = inner.all_vms_stopped();
        drop(inner);
        if all_stopped {
            self.release_cids();
        }
    }

    /// Takes VM `vm`'s control handle out of the run, with the spec it was
    /// booted from, so the VM can be stopped outside the run's lock. The slot
    /// keeps its state; the watcher skips a slot with no handle until
    /// [`Run::seat_vm`] or [`Run::set_vm_killed`] puts one back.
    fn take_vm(&self, vm: u32, allowed: &[VmState]) -> Result<Held, VmOpError> {
        let mut inner = self.lock();
        if inner.state != RunState::Running {
            return Err(VmOpError::RunEnded(inner.state));
        }
        let slot = inner
            .vms
            .iter_mut()
            .find(|slot| slot.id == vm)
            .ok_or(VmOpError::UnknownVm(vm))?;
        if !allowed.contains(&slot.state) {
            return Err(VmOpError::WrongState {
                vm,
                state: slot.state,
            });
        }
        let spec = slot.spec.clone().ok_or_else(|| {
            VmOpError::Failed(format!("VM {vm} has no saved spec to boot again from"))
        })?;
        let handle = slot.handle.take().ok_or(VmOpError::Busy(vm))?;
        Ok(Held {
            handle,
            spec,
            restored_from: slot.restored_from.clone(),
            state: slot.state,
        })
    }

    /// The guest CID of VM `vm`: the key of its snapshots.
    fn vm_cid(&self, vm: u32) -> Result<u64, VmOpError> {
        let inner = self.lock();
        let slot = inner
            .vms
            .iter()
            .find(|slot| slot.id == vm)
            .ok_or(VmOpError::UnknownVm(vm))?;
        slot.guest_cid.ok_or_else(|| {
            VmOpError::Unsupported(format!("VM {vm} has no vsock CID, so it has no snapshots"))
        })
    }

    /// The meta and workspace flag a fork of VM `vm` takes, read from its spec without taking the VM.
    fn fork_shape(&self, vm: u32) -> Result<(RunMeta, bool), VmOpError> {
        let inner = self.lock();
        if inner.state != RunState::Running {
            return Err(VmOpError::RunEnded(inner.state));
        }
        let slot = inner
            .vms
            .iter()
            .find(|slot| slot.id == vm)
            .ok_or(VmOpError::UnknownVm(vm))?;
        let spec = slot.spec.as_ref().ok_or_else(|| {
            VmOpError::Failed(format!("VM {vm} has no saved spec to boot again from"))
        })?;
        Ok((self.meta(spec.balloon), spec.workspace.is_some()))
    }

    /// The run's shape as a run booted from it would be: what a fork of one of its VMs shares.
    fn meta(&self, balloon_usable_mib: Option<u32>) -> RunMeta {
        RunMeta {
            vcpus: self.vcpus,
            memory_mib: self.memory_mib,
            beats: self.beats,
            role: self.role.clone(),
            keepalive: self.keepalive,
            balloon_usable_mib,
            sandbox: self.sandbox,
            volumes: self.volumes.clone(),
        }
    }

    /// Puts back the handle of a VM whose stop was refused: it is still running.
    fn return_vm(&self, vm: u32, handle: VmHandle) {
        let mut inner = self.lock();
        if let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) {
            slot.handle = Some(handle);
        }
    }

    /// Puts back the handle of VM `vm` and sets its state to `state` (a paused VM, a resumed one).
    fn return_vm_in(&self, vm: u32, handle: VmHandle, state: VmState) {
        let mut inner = self.lock();
        let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) else {
            return;
        };
        slot.handle = Some(handle);
        if slot.state != state {
            slot.state = state;
            self.events.publish(Event::Vm {
                run: self.id.clone(),
                vm,
                state: state.as_str(),
            });
        }
    }

    /// Records that VM `vm` was stopped: its killed handle goes back into the
    /// slot and the slot becomes `killed`. The run's CIDs are released once no
    /// VM of the run is left running (the watcher then ends the run).
    fn set_vm_killed(&self, vm: u32, handle: VmHandle) {
        let mut inner = self.lock();
        let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) else {
            return;
        };
        slot.handle = Some(handle);
        slot.state = VmState::Killed;
        self.events.publish(Event::Vm {
            run: self.id.clone(),
            vm,
            state: VmState::Killed.as_str(),
        });
        let all_stopped = inner.all_vms_stopped();
        drop(inner);
        if all_stopped {
            self.release_cids();
        }
    }

    /// Seats a restarted VM: `fresh` becomes the slot's running VM, and
    /// `killed` (the handle of the VM it replaces) stays with the slot as a
    /// stopped VM. A run that ended while the VM was booting takes it back down.
    fn seat_vm(
        &self,
        vm: u32,
        killed: VmHandle,
        mut fresh: VmHandle,
        restored_from: Option<PathBuf>,
    ) {
        let mut inner = self.lock();
        if inner.state != RunState::Running {
            drop(inner);
            if let Err(err) = fresh.kill() {
                eprintln!("noded: run {}: VM {vm} did not stop: {err}", self.id);
            }
            self.set_vm_killed(vm, killed);
            return;
        }
        if let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) {
            // The new VM has a new serial capture, which starts empty.
            slot.runtime = fresh.runtime();
            slot.handle = Some(fresh);
            slot.state = VmState::Running;
            slot.serial_seen = 0;
            slot.restored_from = restored_from;
            self.events.publish(Event::Vm {
                run: self.id.clone(),
                vm,
                state: VmState::Running.as_str(),
            });
        }
    }

    /// For an asleep VM that has never run on this node (imported, so its slot has no handle): takes
    /// its spec out of the slot, so a second wake finds it busy, and returns it. `None` for a VM that
    /// has a handle (the usual wake path).
    fn take_unbooted(&self, vm: u32) -> Result<Option<VmSpec>, VmOpError> {
        let mut inner = self.lock();
        if inner.state != RunState::Running {
            return Err(VmOpError::RunEnded(inner.state));
        }
        let slot = inner
            .vms
            .iter_mut()
            .find(|slot| slot.id == vm)
            .ok_or(VmOpError::UnknownVm(vm))?;
        if slot.handle.is_some() {
            return Ok(None);
        }
        if slot.state != VmState::Asleep {
            return Err(VmOpError::WrongState {
                vm,
                state: slot.state,
            });
        }
        slot.spec.take().map(Some).ok_or(VmOpError::Busy(vm))
    }

    /// Puts back the spec [`Run::take_unbooted`] took (its wake failed).
    fn untake_unbooted(&self, vm: u32, spec: VmSpec) {
        let mut inner = self.lock();
        if let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) {
            slot.spec = Some(spec);
        }
    }

    /// Seats the first VM an imported slot runs: its spec back, its handle, running.
    fn seat_first(&self, vm: u32, spec: VmSpec, fresh: VmHandle, restored_from: Option<PathBuf>) {
        let mut inner = self.lock();
        if let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) {
            slot.spec = Some(spec);
            slot.runtime = fresh.runtime();
            slot.handle = Some(fresh);
            slot.state = VmState::Running;
            slot.serial_seen = 0;
            slot.restored_from = restored_from;
            self.events.publish(Event::Vm {
                run: self.id.clone(),
                vm,
                state: VmState::Running.as_str(),
            });
        }
    }

    /// The guest CID, role and cold spec of VM `vm`, which must be asleep: what an export reads. The
    /// run must be running, as a sleep leaves it.
    fn asleep_slot(&self, vm: u32) -> Result<(u64, Option<String>, VmSpec), VmOpError> {
        let inner = self.lock();
        if inner.state != RunState::Running {
            return Err(VmOpError::RunEnded(inner.state));
        }
        let slot = inner
            .vms
            .iter()
            .find(|slot| slot.id == vm)
            .ok_or(VmOpError::UnknownVm(vm))?;
        if slot.state != VmState::Asleep {
            return Err(VmOpError::WrongState {
                vm,
                state: slot.state,
            });
        }
        let cid = slot
            .guest_cid
            .ok_or_else(|| VmOpError::Unsupported(format!("VM {vm} has no vsock CID")))?;
        let spec = slot
            .spec
            .clone()
            .ok_or_else(|| VmOpError::Failed(format!("VM {vm} has no saved spec to export")))?;
        Ok((cid, slot.role.clone(), spec))
    }

    /// Releases this run's guest CIDs: the run has ended, so its span is the
    /// next run's to draw.
    ///
    /// Idempotent, and called by every path that ends a run (the boot's
    /// failure, a kill, and the watcher's `done`); only the first call finds
    /// the span. A run that has not ended holds it, which is what keeps a
    /// second run off its CIDs.
    fn release_cids(&self) {
        self.cids.release(&self.id);
    }

    /// Whether this run's VMs are recovered when they stop by themselves (see [`is_recoverable_run`]).
    fn recoverable(&self) -> bool {
        is_recoverable_run(self.keepalive, self.sandbox, self.beats)
    }

    /// Whether the run is still running: a recovery that boots a VM for it checks this first.
    fn is_running(&self) -> bool {
        self.lock().state == RunState::Running
    }

    /// Records that a recovery of VM `vm` started at `at`, and forgets the recoveries outside the
    /// window (the crash-loop limit only counts the ones inside it).
    fn record_recovery(&self, vm: u32, at: u64) {
        let mut recoveries = self
            .recoveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recoveries.retain(|(_, when)| at.saturating_sub(*when) < RECOVERY_WINDOW_MS);
        recoveries.push((vm, at));
    }

    /// When VM `vm`'s recent recoveries started, for [`recovery_decision`].
    fn recent_recoveries(&self, vm: u32) -> Vec<u64> {
        let recoveries = self
            .recoveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recoveries
            .iter()
            .filter(|(index, _)| *index == vm)
            .map(|(_, at)| *at)
            .collect()
    }

    /// Puts back the handle of VM `vm` that a recovery could not boot, and sets it `exited`: it
    /// stays as the guest left it. A run a kill ended while the recovery ran leaves the VM `killed`
    /// instead, and gives its CIDs back once its VMs are all stopped.
    fn set_vm_exited(&self, vm: u32, handle: VmHandle) {
        let mut inner = self.lock();
        let run_state = inner.state;
        let Some(slot) = inner.vms.iter_mut().find(|slot| slot.id == vm) else {
            return;
        };
        slot.handle = Some(handle);
        slot.state = if run_state == RunState::Running {
            VmState::Exited
        } else {
            VmState::Killed
        };
        self.events.publish(Event::Vm {
            run: self.id.clone(),
            vm,
            state: slot.state.as_str(),
        });
        let release = run_state.is_finished() && inner.all_vms_stopped();
        drop(inner);
        if release {
            self.release_cids();
        }
    }

    fn lock(&self) -> MutexGuard<'_, RunInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Why a VM's console refused input, without the spawner's own "VM {id} could
/// not take the console input" prefix: [`InputError::Console`] adds one.
///
/// A console refusal is a [`SpawnError::SerialInput`]; any other error is
/// reported as the spawner words it.
fn console_reason(err: SpawnError) -> String {
    match err {
        SpawnError::SerialInput { reason, .. } => reason,
        other => other.to_string(),
    }
}

/// A VM's captured console output (lossy UTF-8), read through the shared
/// runtime rather than through the VM's handle: a GET of the run must not wait
/// behind a handle a kill is holding.
fn serial_of(runtime: &VmRuntime) -> String {
    let buffer = runtime
        .serial
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Watches a run's VMs and ends the run as `done` once every one of them has
/// stopped by itself.
fn watch(run: Arc<Run>) {
    thread::spawn(move || watch_run(&run));
}

fn watch_run(run: &Arc<Run>) {
    let mut interval = WATCH_INTERVAL_MIN;
    loop {
        let mut changed = false;
        {
            let mut inner = run.lock();
            if inner.state != RunState::Running {
                return;
            }
            for slot in inner.vms.iter_mut() {
                // The serial delta first, for every VM and whatever its
                // state: a VM that stopped between two passes grew its
                // capture one last time, and that last stretch is exactly
                // what the run's transcript wants.
                if publish_serial_delta(&run.events, &run.id, slot) {
                    changed = true;
                }
                // A paused VM's process is still there to exit, so it is watched too.
                if !matches!(slot.state, VmState::Running | VmState::Paused) {
                    continue;
                }
                let Some(handle) = slot.handle.as_mut() else {
                    // Only a kill in flight takes a handle out, and that has
                    // already ended the run, so this is unreachable - but a
                    // watcher must not assume it.
                    continue;
                };
                // A non-blocking look, so the run's lock is held for the check
                // and nothing else: a GET or a kill of the run never waits
                // behind the watcher.
                let polled = handle.wait_timeout(Duration::ZERO);
                if matches!(polled, Ok(Some(_)) | Err(_)) {
                    changed = true;
                }
                match polled {
                    Ok(Some(exit)) => match (slot.spec.clone(), slot.guest_cid) {
                        // A keepalive agent or sandbox that stopped by itself is recovered, not
                        // left exited: its handle goes to the recovery, and the slot keeps its
                        // state with no handle, which the watcher skips and the run waits on.
                        (Some(spec), Some(cid))
                            if recovers(exit, slot.state, run.recoverable(), crash_recovery()) =>
                        {
                            if let Some(exited) = slot.handle.take() {
                                let run = Arc::clone(run);
                                let vm = slot.id;
                                thread::spawn(move || recover_vm(run, vm, cid, spec, exited));
                            }
                        }
                        _ => {
                            slot.state = VmState::from_exit(exit);
                            run.events.publish(Event::Vm {
                                run: run.id.clone(),
                                vm: slot.id,
                                state: slot.state.as_str(),
                            });
                        }
                    },
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!(
                            "noded: run {}: VM {} could not be waited on: {err}",
                            run.id, slot.id
                        );
                        slot.state = VmState::Killed;
                        run.events.publish(Event::Vm {
                            run: run.id.clone(),
                            vm: slot.id,
                            state: VmState::Killed.as_str(),
                        });
                    }
                }
            }
            if inner.all_vms_stopped() {
                inner.finish();
                // `finish` only moves a `running` run; publish what the run
                // actually ended as — a kill that raced the last exit has
                // already moved it, and the stream must not be told `done`.
                run.events.publish(Event::Run {
                    id: run.id.clone(),
                    state: inner.state.as_str(),
                });
                // The run is over, so its CIDs go back to the book and the
                // next run may draw them. Release is idempotent (a kill that
                // raced this last exit released them already), and it is done
                // outside the run's lock: the book has its own.
                drop(inner);
                run.release_cids();
                return;
            }
        }
        // Outside the lock. Re-acquiring it in a tight loop starves every
        // other caller of the run (measured: a GET right after a run started
        // waited for the run's whole life), so the watcher gives the lock up
        // for longer than it holds it.
        interval = if changed {
            WATCH_INTERVAL_MIN
        } else {
            (interval * 2).min(WATCH_INTERVAL_MAX)
        };
        thread::sleep(interval);
    }
}

/// Recovers VM `vm` of `run` after it stopped by itself: boots it again with the CID, workspace and
/// spec it had, from its newest snapshot taken in the last day or cold (see [`recovery_decision`]).
///
/// It runs on its own thread, outside the run's lock, since a boot takes a while. Meanwhile the VM's
/// slot keeps its state with no handle, which is how the watcher and the host's own operations leave
/// a VM they are working on alone, and the run stays up for it.
fn recover_vm(run: Arc<Run>, vm: u32, cid: u64, spec: VmSpec, exited: VmHandle) {
    let mut exited = exited;
    // The stop is idempotent for a process that has exited. A failure means it is still there, and a
    // boot on the same CID would fail on its muxer socket, so the VM stays exited.
    if let Err(err) = exited.kill() {
        eprintln!(
            "noded: run {}: VM {vm} stopped by itself, and its process did not go ({err}); it stays exited",
            run.id
        );
        run.set_vm_exited(vm, exited);
        return;
    }
    let views = match &run.snapshots {
        Some(store) => {
            let mut views = store.checkpoints(cid);
            views.extend(store.view(cid, snapshots::SLEEP));
            views
        }
        None => Vec::new(),
    };
    let now = now_ms();
    let decision = recovery_decision(now, &views, &run.recent_recoveries(vm));
    if decision == Recovery::GiveUp {
        eprintln!(
            "noded: run {}: VM {vm} stopped by itself again, after {RECOVERY_LIMIT} recoveries in {} minutes; it stays exited",
            run.id,
            RECOVERY_WINDOW_MS / 60_000
        );
        run.set_vm_exited(vm, exited);
        return;
    }
    run.record_recovery(vm, now);
    // A kill that landed while the snapshots were read ended the run: the VM is the kill's to stop.
    if !run.is_running() {
        run.set_vm_killed(vm, exited);
        return;
    }
    let restored = match (&decision, &run.snapshots) {
        (Recovery::From(name), Some(store)) => match boot_recovered(store, cid, &spec, name) {
            Ok((fresh, base)) => Some((fresh, base, name.clone())),
            Err(err) => {
                eprintln!(
                    "noded: run {}: VM {vm} did not recover from {name} ({err}); booting it cold",
                    run.id
                );
                None
            }
        },
        _ => None,
    };
    let (fresh, base, from) = match restored {
        Some((fresh, base, name)) if name == snapshots::SLEEP => {
            (fresh, Some(base), "the sleep snapshot".to_string())
        }
        Some((fresh, base, name)) => (fresh, Some(base), format!("checkpoint {name}")),
        None => match spawn_timed(spec.clone()) {
            Ok(fresh) => (fresh, None, "a cold boot".to_string()),
            Err(err) => {
                eprintln!(
                    "noded: run {}: VM {vm} stopped by itself and did not boot again ({err}); it stays exited",
                    run.id
                );
                run.set_vm_exited(vm, exited);
                return;
            }
        },
    };
    eprintln!(
        "noded: run {}: VM {vm} stopped by itself; recovered from {from}",
        run.id
    );
    run.seat_vm(vm, exited, fresh, base);
}

/// Publishes the part of the VM's serial capture that grew since the last
/// look, as a `serial` event whose `offset` is the capture byte the `text`
/// starts at. The capture is bounded, and so are the events: once the capture
/// stops growing, no further events fire for it. Returns whether it published
/// anything, that is, whether the capture grew since the last look.
fn publish_serial_delta(events: &EventBus, run: &str, slot: &mut VmSlot) -> bool {
    let buffer = slot
        .runtime
        .serial
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if buffer.len() <= slot.serial_seen {
        return false;
    }
    let offset = slot.serial_seen;
    let text = String::from_utf8_lossy(&buffer[offset..]).into_owned();
    slot.serial_seen = buffer.len();
    drop(buffer);
    events.publish(Event::Serial {
        run: run.to_string(),
        vm: slot.id,
        offset: offset as u64,
        text,
    });
    true
}

/// The current time in milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [RunState; 5] = [
        RunState::Starting,
        RunState::Running,
        RunState::Done,
        RunState::Failed,
        RunState::Killed,
    ];

    /// `core.run_transitions`, restricted to the states noded v0 implements.
    const ALLOWED: [(RunState, RunState); 6] = [
        (RunState::Starting, RunState::Running),
        (RunState::Starting, RunState::Failed),
        (RunState::Starting, RunState::Killed),
        (RunState::Running, RunState::Done),
        (RunState::Running, RunState::Failed),
        (RunState::Running, RunState::Killed),
    ];

    fn inner(state: RunState) -> RunInner {
        RunInner {
            state,
            ended_at: None,
            vms: Vec::new(),
        }
    }

    fn images() -> VmImages {
        VmImages {
            kernel: PathBuf::from("kernel"),
            initrd: PathBuf::from("initrd"),
            workspace: None,
        }
    }

    /// A scratch directory unique to this process and test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-noded-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn a_wipe_copies_the_base_image_over_the_workspace_and_leaves_no_partial() {
        let dir = scratch("wipe-copy");
        let base = dir.join("base.ext4");
        let image = dir.join("cid-7.ext4");
        std::fs::write(&base, b"fresh base image").expect("the base image");
        std::fs::write(&image, b"the agent's own files").expect("the workspace image");
        replace_workspace(&base, &image).expect("the workspace is replaced");
        assert_eq!(
            std::fs::read(&image).expect("the image"),
            b"fresh base image"
        );
        assert!(!dir.join("cid-7.ext4.partial").exists());
    }

    #[test]
    fn a_persistent_workspace_is_named_by_its_guest_cid() {
        assert_eq!(
            persistent_workspace_path(std::path::Path::new("/persist"), 7),
            PathBuf::from("/persist/workspaces/cid-7.ext4")
        );
    }

    /// A daemon with a workspace base image lays one copy per VM of a run out
    /// in the run's own directory under its UDS root, before the run's VMs
    /// boot — so no two VMs of a run ever share the one writable volume. The
    /// images come from the real spawner path (the boot itself fails here: the
    /// test has no guest kernel), and a failed boot does not delete them.
    #[test]
    fn a_run_with_a_workspace_lays_out_one_image_per_vm() {
        let dir = scratch("workspace-run");
        let base = dir.join("workspace.ext4");
        std::fs::write(&base, b"workspace bytes").expect("the base image");
        let uds_dir = dir.join("uds");
        let events = EventBus::new();
        let images = VmImages {
            kernel: PathBuf::from("no-such-kernel"),
            initrd: PathBuf::from("no-such-initrd"),
            workspace: Some(base.clone()),
        };
        let registry = Registry::new(images, uds_dir.clone(), events);

        let run = registry
            .create_run(&CreateRun {
                n: 2,
                vcpus: 1,
                memory_mib: MIN_MEMORY_MIB,
                beats: None,
                vsock_cid: None,
                role: None,
                keepalive: None,
                sandbox: false,
                balloon_usable_mib: None,
                volumes: Vec::new(),
            })
            .expect("the run exists even when its VMs cannot boot");

        let run_dir = registry.run_dir(&run.id);
        for index in 0..2 {
            let image = run_dir.join(format!("workspace-{index}.ext4"));
            assert_eq!(
                std::fs::read(&image).expect("the VM's own image"),
                b"workspace bytes"
            );
        }
        assert_eq!(
            std::fs::read_dir(&run_dir)
                .expect("the run's directory")
                .count(),
            2,
            "one image per VM, and nothing else"
        );
        // The base image the next run copies from is untouched.
        assert_eq!(
            std::fs::read(&base).expect("the base image"),
            b"workspace bytes"
        );

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_transition_matrix_matches_the_schema() {
        for from in STATES {
            for to in STATES {
                assert_eq!(
                    from.can_transition_to(to),
                    ALLOWED.contains(&(from, to)),
                    "{} -> {}",
                    from.as_str(),
                    to.as_str()
                );
            }
        }
    }

    #[test]
    fn run_transitions_and_finish_follow_the_state() {
        // A refused transition leaves the run alone: the error names both
        // states, and the state and end stamp are unchanged.
        {
            let mut run = inner(RunState::Killed);
            run.ended_at = Some(7);
            let refused = run
                .transition(RunState::Running)
                .expect_err("killed -> running is refused");
            assert_eq!(
                refused,
                RefusedTransition {
                    from: RunState::Killed,
                    to: RunState::Running
                }
            );
            assert_eq!(
                refused.to_string(),
                "a run may not go from killed to running"
            );
            assert_eq!(run.state, RunState::Killed);
            assert_eq!(run.ended_at, Some(7));
        }
        // An ending transition stamps ended_at, and a start does not.
        {
            let mut run = inner(RunState::Starting);
            run.transition(RunState::Running)
                .expect("starting -> running");
            assert_eq!(run.state, RunState::Running);
            assert_eq!(run.ended_at, None);

            run.transition(RunState::Done).expect("running -> done");
            assert!(run.ended_at.is_some());
            assert!(run.state.is_finished());

            let mut failed = inner(RunState::Running);
            failed
                .transition(RunState::Failed)
                .expect("running -> failed");
            assert!(failed.ended_at.is_some());

            let mut killed = inner(RunState::Starting);
            killed
                .transition(RunState::Killed)
                .expect("starting -> killed");
            assert!(killed.ended_at.is_some());
        }
        // Finishing is a no-op once the run has ended.
        {
            let mut run = inner(RunState::Killed);
            run.ended_at = Some(7);
            run.finish();
            assert_eq!(run.state, RunState::Killed);
            assert_eq!(run.ended_at, Some(7));
        }
    }

    #[test]
    fn the_state_names_are_the_schema_names() {
        assert_eq!(RunState::Starting.as_str(), "starting");
        assert_eq!(RunState::Running.as_str(), "running");
        assert_eq!(RunState::Done.as_str(), "done");
        assert_eq!(RunState::Failed.as_str(), "failed");
        assert_eq!(RunState::Killed.as_str(), "killed");
        assert_eq!(VmState::Running.as_str(), "running");
        assert_eq!(VmState::Exited.as_str(), "exited");
        assert_eq!(VmState::Killed.as_str(), "killed");
        assert!(!RunState::Starting.is_finished());
        assert!(!RunState::Running.is_finished());
    }

    /// The same run, holding `cids` as its CID book: what a run whose CIDs are
    /// in the node's book is (see [`an_ending_run_gives_its_cids_back`]).
    fn run_with_one_vm_and_book(
        run_state: RunState,
        vm_state: VmState,
        cid: Option<u64>,
        role: Option<&str>,
        cids: Arc<CidBook>,
    ) -> Arc<Run> {
        Arc::new(Run {
            id: "r1".to_string(),
            vcpus: 1,
            memory_mib: MIN_MEMORY_MIB,
            beats: None,
            role: role.map(str::to_string),
            keepalive: false,
            sandbox: false,
            volumes: Vec::new(),
            created_at: now_ms(),
            events: EventBus::new(),
            cids,
            snapshots: None,
            recoveries: Mutex::new(Vec::new()),
            inner: Mutex::new(RunInner {
                state: run_state,
                ended_at: None,
                vms: vec![VmSlot {
                    id: 0,
                    spec: None,
                    guest_cid: cid,
                    role: role.map(str::to_string),
                    runtime: Arc::new(VmRuntime::new(0)),
                    handle: None,
                    state: vm_state,
                    serial_seen: 0,
                    restored_from: None,
                }],
            }),
        })
    }

    #[test]
    fn a_vm_exit_maps_to_the_state_the_api_reports() {
        assert_eq!(
            VmState::from_exit(VmExit::GuestExited { reset: true }),
            VmState::Exited
        );
        assert_eq!(
            VmState::from_exit(VmExit::GuestExited { reset: false }),
            VmState::Exited
        );
        assert_eq!(VmState::from_exit(VmExit::Killed), VmState::Killed);
    }

    const MINUTE: u64 = 60 * 1000;
    const HOUR: u64 = 60 * MINUTE;
    const NOW: u64 = 1_000 * HOUR;

    /// A snapshot named `name`, taken at `taken_at_ms`, as a view reads it back from disk.
    fn snapshot(name: &str, taken_at_ms: u64) -> SnapshotView {
        SnapshotView {
            name: name.to_string(),
            cid: 7,
            label: String::new(),
            kind: "diff".to_string(),
            taken_at_ms,
            memory_written: 0,
            disk_bytes: 0,
            paused_ms: 0,
        }
    }

    #[test]
    fn a_recovery_boots_from_a_fresh_snapshot_or_gives_up() {
        // The newest snapshot of the last day is booted from, or the VM goes cold.
        {
            assert_eq!(recovery_decision(NOW, &[], &[]), Recovery::Cold);

            let checkpoints = [
                snapshot("cp-a", NOW - 5 * HOUR),
                snapshot("cp-b", NOW - 2 * HOUR),
            ];
            assert_eq!(
                recovery_decision(NOW, &checkpoints, &[]),
                Recovery::From("cp-b".to_string()),
                "the newest checkpoint"
            );

            let sleep_newer = [
                snapshot("cp-b", NOW - 2 * HOUR),
                snapshot(snapshots::SLEEP, NOW - HOUR),
            ];
            assert_eq!(
                recovery_decision(NOW, &sleep_newer, &[]),
                Recovery::From(snapshots::SLEEP.to_string()),
                "the sleep snapshot, when it is newer"
            );

            let sleep_older = [
                snapshot(snapshots::SLEEP, NOW - 3 * HOUR),
                snapshot("cp-b", NOW - 2 * HOUR),
            ];
            assert_eq!(
                recovery_decision(NOW, &sleep_older, &[]),
                Recovery::From("cp-b".to_string()),
                "a checkpoint newer than the sleep snapshot"
            );
        }
        // A snapshot older than a day is never booted from.
        {
            let stale = [snapshot("cp-stale", NOW - 25 * HOUR)];
            assert_eq!(recovery_decision(NOW, &stale, &[]), Recovery::Cold);

            let mixed = [
                snapshot("cp-stale", NOW - 30 * HOUR),
                snapshot("cp-day", NOW - 24 * HOUR),
            ];
            assert_eq!(
                recovery_decision(NOW, &mixed, &[]),
                Recovery::From("cp-day".to_string()),
                "a snapshot a day old still counts; the older one does not"
            );
        }
        // A VM recovered three times in ten minutes stays exited.
        {
            let views = [snapshot("cp-a", NOW - HOUR)];
            let from_a = Recovery::From("cp-a".to_string());

            assert_eq!(
                recovery_decision(NOW, &views, &[NOW - MINUTE, NOW - 5 * MINUTE]),
                from_a,
                "two recent recoveries are under the limit"
            );
            assert_eq!(
                recovery_decision(
                    NOW,
                    &views,
                    &[NOW - MINUTE, NOW - 5 * MINUTE, NOW - 9 * MINUTE]
                ),
                Recovery::GiveUp,
                "the third recovery in the window is refused"
            );
            assert_eq!(
                recovery_decision(
                    NOW,
                    &[],
                    &[NOW - MINUTE, NOW - 2 * MINUTE, NOW - 3 * MINUTE]
                ),
                Recovery::GiveUp,
                "the limit holds with no snapshot to boot from too"
            );
            assert_eq!(
                recovery_decision(
                    NOW,
                    &views,
                    &[
                        NOW - MINUTE,
                        NOW - 2 * MINUTE,
                        NOW - 11 * MINUTE,
                        NOW - 12 * MINUTE
                    ]
                ),
                from_a,
                "recoveries older than ten minutes do not count"
            );
        }
    }

    #[test]
    fn a_disk_only_checkpoint_is_never_booted_from() {
        let disk_only = SnapshotView {
            kind: "disk".to_string(),
            ..snapshot("cp-disk", NOW - MINUTE)
        };
        assert_eq!(
            recovery_decision(NOW, std::slice::from_ref(&disk_only), &[]),
            Recovery::Cold,
            "a disk-only checkpoint alone leaves the VM cold"
        );

        let views = [disk_only, snapshot("cp-mem", NOW - HOUR)];
        assert_eq!(
            recovery_decision(NOW, &views, &[]),
            Recovery::From("cp-mem".to_string()),
            "the newest checkpoint with memory, past a newer disk-only one"
        );
    }

    #[test]
    fn recoveries_are_counted_per_vm_and_the_old_ones_are_forgotten() {
        let run = run_with_one_vm_and_book(
            RunState::Running,
            VmState::Running,
            Some(7),
            None,
            Arc::new(CidBook::default()),
        );
        run.record_recovery(0, 1_000);
        run.record_recovery(1, 2_000);
        assert_eq!(run.recent_recoveries(0), vec![1_000]);
        assert_eq!(run.recent_recoveries(1), vec![2_000]);

        let later = 1_000 + 11 * MINUTE;
        run.record_recovery(0, later);
        assert_eq!(
            run.recent_recoveries(0),
            vec![later],
            "the recovery eleven minutes back is out of the window"
        );
    }

    #[test]
    fn only_a_guest_stop_of_a_running_recoverable_vm_is_recovered() {
        let guest = VmExit::GuestExited { reset: false };
        assert!(recovers(guest, VmState::Running, true, true));
        assert!(
            recovers(
                VmExit::GuestExited { reset: true },
                VmState::Running,
                true,
                true
            ),
            "a guest reset is a crash too"
        );
        // A host's stop, kill, wipe, restart, sleep or move kills the VM's process, which reports
        // as `Killed`, never as a guest exit.
        assert!(!recovers(VmExit::Killed, VmState::Running, true, true));
        // A paused or asleep VM is not one that ran on its own.
        assert!(!recovers(guest, VmState::Paused, true, true));
        assert!(!recovers(guest, VmState::Asleep, true, true));
        assert!(!recovers(guest, VmState::Killed, true, true));
        // Not a keepalive agent or sandbox, or with recovery turned off (`--no-recover`).
        assert!(!recovers(guest, VmState::Running, false, true));
        assert!(!recovers(guest, VmState::Running, true, false));
    }

    #[test]
    fn keepalive_agents_and_sandboxes_are_recovered_unless_the_run_churns() {
        assert!(is_recoverable_run(true, false, None));
        assert!(is_recoverable_run(false, true, None));
        assert!(
            !is_recoverable_run(false, false, None),
            "the frozen demo session ends by design"
        );
        assert!(!is_recoverable_run(true, false, Some(3)), "a churn run");
        assert!(!is_recoverable_run(false, true, Some(3)), "a churn sandbox");
    }

    /// A run whose CIDs the caller pins is the demo task's frozen session, and
    /// its VMs carry the harness; a run the node allocates CIDs for is an
    /// operator run, and its VMs are keepalive agents. Both command lines name
    /// the harness, which is what makes `plaza.vm=<i>` meaningful at all.
    #[test]
    fn the_cmdline_names_the_harness_its_session_and_the_role() {
        let request = |beats: Option<u32>, role: Option<&str>| CreateRun {
            n: 1,
            vcpus: 1,
            memory_mib: 128,
            beats,
            vsock_cid: None,
            role: role.map(str::to_string),
            keepalive: None,
            sandbox: false,
            balloon_usable_mib: None,
            volumes: Vec::new(),
        };
        assert_eq!(
            request(None, None).vm_cmdline(0, false),
            "console=ttyS0 plaza.harness=1 plaza.vm=0"
        );
        assert_eq!(
            request(Some(2), None).vm_cmdline(1, true),
            "console=ttyS0 plaza.beats=2 plaza.harness=1 plaza.keepalive=1 plaza.vm=1"
        );
        let roled = request(None, Some("builder")).vm_cmdline(0, true);
        assert!(
            roled.contains("plaza.role=builder") && roled.contains("plaza.keepalive=1"),
            "{roled}"
        );
        assert!(request(None, Some("Bad Role")).checked().is_err());
    }

    /// The run range must hold one full-size run: a request of [`MAX_VMS`] VMs
    /// with no `vsock_cid` is one the node has to be able to serve.
    #[test]
    fn the_run_range_holds_a_full_size_run() {
        let width = RUN_CID_END - RUN_CID_BASE + 1;
        assert!(
            width >= u64::from(MAX_VMS),
            "the run range is {width} wide and a run may have {MAX_VMS} VMs"
        );
        // The range is the node's own: it must not collide with the role
        // fleet's default first CID.
        assert!(
            RUN_CID_END < crate::DEFAULT_AGENTS_CID_BASE,
            "the run range runs into the fleet's default CID base"
        );
        assert!(RUN_CID_BASE >= MIN_GUEST_CID);
    }

    #[test]
    fn a_create_request_is_range_checked_before_a_run_exists() {
        // A create request is range-checked: the range edges are allowed, anything
        // outside is refused, and an unnamed CID is allocated by the node.
        {
            let request = |n: u32, vcpus: u32, memory_mib: u32| CreateRun {
                n,
                vcpus,
                memory_mib,
                beats: None,
                vsock_cid: None,
                role: None,
                keepalive: None,
                sandbox: false,
                balloon_usable_mib: None,
                volumes: Vec::new(),
            };

            let (vms, named) = request(2, 2, 512)
                .checked()
                .expect("a request inside the ranges");
            assert_eq!(vms, 2);
            assert_eq!(named, None, "no CID was named, so the node allocates one");

            for bad in [
                request(0, 1, 128),
                request(65, 1, 128),
                request(1, 0, 128),
                request(1, 65, 128),
                request(1, 1, 127),
            ] {
                assert!(bad.checked().is_err(), "should be refused: {bad:?}");
            }

            let edges = [
                request(1, 1, 128),
                request(64, 64, 128),
                request(1, 1, u32::MAX),
            ];
            for edge in edges {
                assert!(edge.checked().is_ok(), "should be allowed: {edge:?}");
            }
        }
        // A vsock CID is range-checked before a run exists.
        {
            let request = |vsock_cid: Option<u64>| CreateRun {
                n: 2,
                vcpus: 1,
                memory_mib: 128,
                beats: None,
                vsock_cid,
                role: None,
                keepalive: None,
                sandbox: false,
                balloon_usable_mib: None,
                volumes: Vec::new(),
            };

            // A CID inside the valid range is accepted, base + n - 1 included.
            assert_eq!(request(Some(3)).checked().expect("a valid CID").1, Some(3));
            assert_eq!(
                request(Some(MAX_GUEST_CID - 1))
                    .checked()
                    .expect("a valid CID")
                    .1,
                Some(MAX_GUEST_CID - 1)
            );

            // The kernel refuses VMADDR_CID_ANY (u32::MAX) and anything above it.
            assert!(request(Some(u32::MAX as u64)).checked().is_err());
            assert!(request(Some(u64::from(u32::MAX) + 1)).checked().is_err());

            // The run's last VM (base + n - 1) must still fit the transport.
            assert!(request(Some(MAX_GUEST_CID)).checked().is_err());

            // A CID the caller names is kept, not replaced by an allocation.
            assert_eq!(
                request(Some(RUN_CID_BASE)).checked().expect("in range").1,
                Some(RUN_CID_BASE)
            );
        }
    }

    #[test]
    fn the_cid_book_allocates_releases_and_refuses_when_full() {
        // The book hands out the first free span of the run range, one span per
        // run, and gives a released span back.
        {
            let book = CidBook::default();
            assert_eq!(
                book.allocate("r1", 1).expect("the range is empty"),
                RUN_CID_BASE
            );
            assert_eq!(
                book.allocate("r2", 1).expect("the next span is free"),
                RUN_CID_BASE + 1
            );
            let span = book.allocate("r3", 3).expect("three more are free");
            assert_eq!(span, RUN_CID_BASE + 2);
            assert_eq!(busy(&book.held()).len(), 5, "three runs, five CIDs");

            // Releasing gives exactly that run's span back: the next allocation
            // reuses the hole, not the tail.
            assert!(book.release("r1"));
            assert_eq!(
                book.allocate("r4", 1).expect("the hole is free"),
                RUN_CID_BASE
            );
            assert!(!book.release("r1"), "release is idempotent");

            // Every live run's span is its own: no two allocations overlap.
            let held = book.held();
            let mut all: Vec<u64> = held
                .iter()
                .flat_map(|(_, cids)| cids.iter().copied())
                .collect();
            let count = all.len();
            all.sort_unstable();
            all.dedup();
            assert_eq!(
                all.len(),
                count,
                "two runs were given the same CID: {held:?}"
            );
        }
        // A span the caller pinned is never allocated to another run: the
        // caller's choice wins, and the allocator works around it.
        {
            let book = CidBook::default();
            book.hold("pinned", cid_span(RUN_CID_BASE, 2));
            assert_eq!(
                book.allocate("r1", 1).expect("room above the pinned span"),
                RUN_CID_BASE + 2
            );
            // A pinned span *outside* the run range costs the allocator nothing.
            book.hold("elsewhere", cid_span(crate::DEFAULT_AGENTS_CID_BASE, 3));
            assert_eq!(
                book.allocate("r2", 1).expect("the range is still free"),
                RUN_CID_BASE + 3
            );
        }
        // Exhaustion is a refusal that names the range and every busy CID, not a
        // span quietly handed out twice.
        {
            let book = CidBook::default();
            let width = u32::try_from(RUN_CID_END - RUN_CID_BASE + 1).expect("the range fits");
            for index in 0..width {
                book.allocate(&format!("r{index}"), 1)
                    .expect("every CID of the range is free at the start");
            }
            let refused = book.allocate("late", 1).expect_err("the range is full");
            assert!(
                refused.contains(&format!("{RUN_CID_BASE}..={RUN_CID_END}")),
                "{refused}"
            );
            assert!(
                refused.contains(&format!("in use: {RUN_CID_BASE}, ")),
                "{refused}"
            );
            assert!(refused.contains(&format!(", {RUN_CID_END}")), "{refused}");

            // A span wider than the whole range is refused the same way, and a
            // released CID makes room for it again.
            assert!(book.allocate("wide", width + 1).is_err());
            assert!(book.release("r0"));
            assert_eq!(
                book.allocate("late", 1).expect("the hole is free"),
                RUN_CID_BASE
            );
        }
    }

    #[test]
    fn a_run_that_never_registers_or_ends_gives_its_cids_back() {
        // A create that cannot prepare its workspace copies is refused with no
        // run - and, because it had already reserved a span, with those CIDs back
        // in the book: a request that never became a run holds nothing.
        {
            let dir = scratch("cid-unbooted");
            let images = VmImages {
                kernel: PathBuf::from("no-such-kernel"),
                initrd: PathBuf::from("no-such-initrd"),
                // A base image that is not there: `materialize_workspaces` fails,
                // so the run is never registered.
                workspace: Some(dir.join("no-such-base.ext4")),
            };
            let registry = Registry::new(images, dir.join("uds"), EventBus::new());

            let refused = registry
                .create_run(&CreateRun {
                    n: 2,
                    vcpus: 1,
                    memory_mib: MIN_MEMORY_MIB,
                    beats: None,
                    vsock_cid: None,
                    role: None,
                    keepalive: None,
                    sandbox: false,
                    balloon_usable_mib: None,
                    volumes: Vec::new(),
                })
                .err()
                .expect("the workspace copies cannot be prepared");
            assert!(
                refused.contains("cannot prepare the run's workspace images"),
                "{refused}"
            );
            assert!(
                registry.list().is_empty(),
                "a refused create left a run behind"
            );
            assert!(
                busy(&registry.cids.held()).is_empty(),
                "a create that never became a run kept its CIDs: {:?}",
                registry.cids.held()
            );

            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        // A run that ends - here, a boot that fails, which the tests can produce
        // without a hypervisor - gives its CIDs back, so the next run draws the
        // same span instead of the range leaking a span per attempt.
        {
            let registry = Registry::new(images(), PathBuf::from("uds"), EventBus::new());
            for _ in 0..3 {
                let run = registry
                    .create_run(&CreateRun {
                        n: 2,
                        vcpus: 1,
                        memory_mib: MIN_MEMORY_MIB,
                        beats: None,
                        vsock_cid: None,
                        role: None,
                        keepalive: None,
                        sandbox: false,
                        balloon_usable_mib: None,
                        volumes: Vec::new(),
                    })
                    .expect("the run exists even when its VMs cannot boot");
                assert_eq!(run.view(Detail::Summary).state, "failed");
                // The failed boot released the span: the book is empty again.
                assert!(
                    busy(&registry.cids.held()).is_empty(),
                    "a failed run kept its CIDs"
                );
            }
        }
    }

    /// A run whose create request cannot get CIDs is refused *before* anything
    /// is spent on it: no run in the register, and - with a workspace base, the
    /// first thing a boot would do - no run directory either.
    #[test]
    fn a_run_that_cannot_get_cids_is_refused_before_anything_is_booted() {
        let dir = scratch("cid-exhausted");
        let base = dir.join("workspace.ext4");
        std::fs::write(&base, b"workspace bytes").expect("the base image");
        let uds_dir = dir.join("uds");
        let images = VmImages {
            kernel: PathBuf::from("no-such-kernel"),
            initrd: PathBuf::from("no-such-initrd"),
            workspace: Some(base),
        };
        let registry = Registry::new(images, uds_dir.clone(), EventBus::new());
        // Take the whole run range the way another live run would.
        let width = u32::try_from(RUN_CID_END - RUN_CID_BASE + 1).expect("the range fits");
        registry
            .cids
            .hold("elsewhere", cid_span(RUN_CID_BASE, width));

        let refused = registry
            .create_run(&CreateRun {
                n: 1,
                vcpus: 1,
                memory_mib: MIN_MEMORY_MIB,
                beats: None,
                vsock_cid: None,
                role: None,
                keepalive: None,
                sandbox: false,
                balloon_usable_mib: None,
                volumes: Vec::new(),
            })
            .err()
            .expect("the run range is full");
        assert!(
            refused.contains(&format!("{RUN_CID_BASE}..={RUN_CID_END}")),
            "{refused}"
        );
        assert!(refused.contains("in use: "), "{refused}");
        // Nothing was created and nothing was laid out: the refusal came
        // before a run id became a run directory.
        assert!(
            registry.list().is_empty(),
            "a refused create left a run behind"
        );
        let laid_out = std::fs::read_dir(&uds_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| entry.file_name().to_string_lossy().starts_with("run-"))
            })
            .unwrap_or(false);
        assert!(!laid_out, "a refused create laid out a workspace copy");

        // A pinned request still works while the range is full: the caller's
        // own CID is not drawn from the range.
        let _pinned = registry
            .create_run(&CreateRun {
                n: 2,
                vcpus: 1,
                memory_mib: MIN_MEMORY_MIB,
                beats: None,
                vsock_cid: Some(crate::DEFAULT_AGENTS_CID_BASE),
                role: None,
                keepalive: None,
                sandbox: false,
                balloon_usable_mib: None,
                volumes: Vec::new(),
            })
            .expect("a pinned run does not need the run range");

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A run that ends by itself, or is killed, gives its CIDs back exactly
    /// once - the watcher's `done` and the kill both go through
    /// [`Run::release_cids`].
    #[test]
    fn an_ending_run_gives_its_cids_back() {
        // The watcher: every VM has stopped, so the run finishes as `done`.
        let book = Arc::new(CidBook::default());
        book.hold("r1", cid_span(RUN_CID_BASE, 2));
        let run = run_with_one_vm_and_book(
            RunState::Running,
            VmState::Exited,
            Some(RUN_CID_BASE),
            None,
            Arc::clone(&book),
        );
        watch_run(&run);
        assert_eq!(run.view(Detail::Summary).state, "done");
        assert!(busy(&book.held()).is_empty(), "a `done` run kept its CIDs");

        // The kill: a run whose VMs have already stopped is marked `killed`
        // and releases the span the same way.
        let book = Arc::new(CidBook::default());
        book.hold("r1", cid_span(RUN_CID_BASE, 2));
        let run = run_with_one_vm_and_book(
            RunState::Running,
            VmState::Exited,
            Some(RUN_CID_BASE),
            None,
            Arc::clone(&book),
        );
        run.kill();
        assert_eq!(run.view(Detail::Summary).state, "killed");
        assert!(busy(&book.held()).is_empty(), "a killed run kept its CIDs");

        // A run that has *not* ended keeps them: this is what stops a second
        // run from being handed a live run's CIDs.
        let book = Arc::new(CidBook::default());
        book.hold("r1", cid_span(RUN_CID_BASE, 1));
        let run = run_with_one_vm_and_book(
            RunState::Running,
            VmState::Running,
            Some(RUN_CID_BASE),
            None,
            Arc::clone(&book),
        );
        assert_eq!(run.view(Detail::Summary).state, "running");
        assert!(busy(&book.held()).contains(&RUN_CID_BASE));
    }

    /// A two-role manifest, as the daemon reads it.
    fn manifest(roles: &[&str]) -> String {
        let entries: Vec<String> = roles
            .iter()
            .map(|name| {
                format!(
                    r#""{name}":{{"role":"{name}","title":"The {name}","permits":[],"toolsets":[],"budget":1,"variant":null}}"#
                )
            })
            .collect();
        format!(r#"{{"roles":{{{}}}}}"#, entries.join(","))
    }

    #[test]
    fn the_fleets_plans_are_one_keepalive_vm_per_role() {
        let fleet = Fleet::parse(&manifest(&["reviewer", "builder"]), 100, 128, 1)
            .expect("a two-role manifest");
        let plans = Registry::fleet_plans(&fleet);
        assert_eq!(plans.len(), 2);
        // Alphabetical order is the boot order, and the CIDs follow it.
        assert_eq!(
            plans[0].cmdline,
            "console=ttyS0 plaza.role=builder plaza.keepalive=1 plaza.vm=0"
        );
        assert_eq!(plans[0].vsock_cid, 100);
        assert_eq!(plans[0].role.as_deref(), Some("builder"));
        assert_eq!(
            plans[1].cmdline,
            "console=ttyS0 plaza.role=reviewer plaza.keepalive=1 plaza.vm=1"
        );
        assert_eq!(plans[1].vsock_cid, 101);
        assert_eq!(plans[1].role.as_deref(), Some("reviewer"));
    }

    #[test]
    fn a_fleet_that_cannot_boot_leaves_a_failed_run_and_claims_no_vm() {
        // The boot fails here (this test has no guest kernel): the honest
        // report is a `failed` run whose VMs are the ones that came up, which
        // is none - the spawner stopped the ones it had started.
        let dir = scratch("fleet-failed");
        let base = dir.join("workspace.ext4");
        std::fs::write(&base, b"workspace bytes").expect("the base image");
        let uds_dir = dir.join("uds");
        let images = VmImages {
            kernel: PathBuf::from("no-such-kernel"),
            initrd: PathBuf::from("no-such-initrd"),
            workspace: Some(base),
        };
        let registry = Registry::new(images, uds_dir.clone(), EventBus::new());
        let fleet = Fleet::parse(&manifest(&["builder", "reviewer"]), 100, 128, 1)
            .expect("a two-role manifest");

        let run = registry
            .create_agent_fleet(&fleet)
            .expect("the run exists even when its VMs cannot boot");
        let view = run.view(Detail::Full);
        assert_eq!(view.state, "failed", "{}", view.state);
        assert!(view.ended_at.is_some());
        assert_eq!(view.vms.len(), 0, "no VM is claimed to be up");
        // The workspace copies were laid out before the boot, one per role,
        // and are not deleted by a failed boot.
        let run_dir = registry.run_dir(&run.id);
        assert_eq!(
            std::fs::read_dir(&run_dir)
                .expect("the run's directory")
                .count(),
            2
        );

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A memory resize names a run and a VM the node has, and a VM it cannot
    /// reach is a conflict, not a success. None of this boots a VM.
    #[test]
    fn a_memory_resize_of_an_unknown_or_unballooned_vm_is_refused() {
        let registry = Registry::new(images(), PathBuf::from("uds"), EventBus::new());
        let unknown = registry
            .set_vm_memory("r9", 0, 256)
            .expect_err("there is no run r9");
        assert_eq!(unknown, MemoryError::UnknownRun);
        assert_eq!(unknown.status(), 404);

        // A running run with one VM that has no handle to resize through.
        let run = run_with_one_vm_and_book(
            RunState::Running,
            VmState::Running,
            Some(RUN_CID_BASE),
            None,
            Arc::new(CidBook::default()),
        );
        let missing = run.set_memory(3, 256).expect_err("the run has no VM 3");
        assert_eq!(missing, MemoryError::UnknownVm(3));
        assert_eq!(missing.status(), 404);
        let stopped = run.set_memory(0, 256).expect_err("the VM has no handle");
        assert_eq!(stopped, MemoryError::Stopped(0));
        assert_eq!(stopped.status(), 409);
    }

    /// A registry holding run `r1` with one stand-in VM in `state` (no handle, no spec), with the
    /// snapshot store at `store` when given.
    fn registry_with_run(state: VmState, store: Option<SnapshotStore>) -> (Registry, Arc<Run>) {
        let registry =
            Registry::new(images(), PathBuf::from("uds"), EventBus::new()).with_snapshots(store);
        let run = run_with_one_vm_and_book(
            RunState::Running,
            state,
            Some(RUN_CID_BASE),
            None,
            Arc::new(CidBook::default()),
        );
        registry.lock().runs.push(Arc::clone(&run));
        (registry, run)
    }

    #[test]
    fn the_snapshot_states_and_refusals_have_their_api_names() {
        assert_eq!(VmState::Paused.as_str(), "paused");
        assert_eq!(VmState::Asleep.as_str(), "asleep");
        let refused = VmOpError::NoSnapshots;
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "this node keeps no snapshots: it was started with --no-snapshots"
        );
        assert_eq!(VmOpError::UnknownCheckpoint("cp-1".into()).status(), 404);
        assert_eq!(VmOpError::Invalid("n".into()).status(), 400);
    }

    #[test]
    fn snapshot_operations_are_refused_with_their_reason() {
        // A node with no snapshot dir refuses every snapshot operation.
        {
            let (registry, _run) = registry_with_run(VmState::Running, None);
            let none = Some(VmOpError::NoSnapshots);
            assert_eq!(registry.checkpoints("r1", 0).err(), none);
            assert_eq!(registry.checkpoint_vm("r1", 0, "x", true).err(), none);
            assert_eq!(registry.sleep_vm("r1", 0, true).err(), none);
            assert_eq!(registry.wake_vm("r1", 0, false).err(), none);
            assert_eq!(registry.restore_checkpoint("r1", 0, "cp-1").err(), none);
            assert_eq!(registry.delete_checkpoint("r1", 0, "cp-1").err(), none);
            assert_eq!(registry.fork_vm("r1", 0, 2, None).err(), none);
        }
        // An unknown run, VM or checkpoint is a 404.
        {
            let dir = scratch("unknown");
            let (registry, _run) =
                registry_with_run(VmState::Running, Some(SnapshotStore::new(dir.clone())));
            assert_eq!(
                registry.pause_vm("r9", 0).err(),
                Some(VmOpError::UnknownRun)
            );
            assert_eq!(registry.doze_vm("r9", 0).err(), Some(VmOpError::UnknownRun));
            assert_eq!(
                registry.sleep_vm("r1", 3, true).err(),
                Some(VmOpError::UnknownVm(3))
            );
            let missing = VmOpError::UnknownCheckpoint("cp-1-nope".into());
            assert_eq!(missing.status(), 404);
            assert_eq!(
                registry.restore_checkpoint("r1", 0, "cp-1-nope").err(),
                Some(missing.clone())
            );
            assert_eq!(
                registry.delete_checkpoint("r1", 0, "cp-1-nope").err(),
                Some(missing)
            );
            assert_eq!(
                registry.restore_checkpoint("r1", 0, "sleep").err(),
                Some(VmOpError::UnknownCheckpoint("sleep".into()))
            );
            assert_eq!(registry.checkpoints("r1", 0).ok(), Some(Vec::new()));
            let _ = fs::remove_dir_all(dir);
        }
        // A fork count out of range is a 400, and takes no checkpoint.
        {
            let dir = scratch("fork-count");
            let (registry, _run) =
                registry_with_run(VmState::Running, Some(SnapshotStore::new(dir.clone())));
            for n in [0, MAX_FORKS + 1] {
                let refused = registry
                    .fork_vm("r1", 0, n, None)
                    .err()
                    .expect("out of range");
                assert_eq!(refused.status(), 400);
            }
            assert!(
                registry
                    .checkpoints("r1", 0)
                    .ok()
                    .unwrap_or_default()
                    .is_empty()
            );
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_vm_in_the_wrong_state_is_refused_with_that_state() {
        let dir = scratch("wrong-state");
        let (asleep, _run) =
            registry_with_run(VmState::Asleep, Some(SnapshotStore::new(dir.clone())));
        let state = VmState::Asleep;
        let wrong = Some(VmOpError::WrongState { vm: 0, state });
        assert_eq!(asleep.pause_vm("r1", 0).err(), wrong);
        assert_eq!(asleep.doze_vm("r1", 0).err(), wrong);
        assert_eq!(asleep.resume_vm("r1", 0).err(), wrong);
        assert_eq!(asleep.sleep_vm("r1", 0, true).err(), wrong);
        assert_eq!(asleep.checkpoint_vm("r1", 0, "x", true).err(), wrong);

        let (running, _run) =
            registry_with_run(VmState::Running, Some(SnapshotStore::new(dir.clone())));
        let state = VmState::Running;
        assert_eq!(
            running.wake_vm("r1", 0, false).err(),
            Some(VmOpError::WrongState { vm: 0, state })
        );
        assert_eq!(
            running.resume_vm("r1", 0).err(),
            Some(VmOpError::WrongState { vm: 0, state })
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_paused_or_asleep_vm_keeps_its_run_going() {
        let run = run_with_one_vm_and_book(
            RunState::Running,
            VmState::Paused,
            Some(RUN_CID_BASE),
            None,
            Arc::new(CidBook::default()),
        );
        assert!(!run.lock().all_vms_stopped());
        run.lock().vms[0].state = VmState::Asleep;
        assert!(!run.lock().all_vms_stopped());
        run.lock().vms[0].state = VmState::Exited;
        assert!(run.lock().all_vms_stopped());
    }

    #[test]
    fn a_fork_takes_the_cids_persistent_image_only_when_none_exists() {
        let dir = scratch("fork-workspace");
        let registry = Registry::new(images(), PathBuf::from("uds"), EventBus::new())
            .with_persist_dir(Some(dir.clone()));
        let image = persistent_workspace_path(&dir, 40);
        assert_eq!(registry.fork_workspace("r7", 40, true), image);
        fs::create_dir_all(image.parent().expect("the workspaces directory"))
            .expect("the workspaces directory");
        fs::write(&image, b"an earlier agent's files").expect("the earlier image");
        let fallback = registry.fork_workspace("r7", 40, true);
        assert_ne!(
            fallback, image,
            "an earlier agent's image is never overwritten"
        );
        assert!(fallback.starts_with(registry.run_dir("r7")));
        assert_eq!(
            registry.fork_workspace("r7", 40, false),
            registry.run_dir("r7").join("fork-40.ext4")
        );
        let _ = fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod sandbox_tests {
    use super::*;

    /// A one-VM sandbox request on the given CID span.
    fn sandbox_request() -> CreateRun {
        CreateRun {
            n: 1,
            vcpus: 1,
            memory_mib: MIN_MEMORY_MIB,
            beats: None,
            vsock_cid: None,
            role: None,
            keepalive: None,
            balloon_usable_mib: None,
            sandbox: true,
            volumes: Vec::new(),
        }
    }

    fn sandbox_meta() -> RunMeta {
        RunMeta {
            vcpus: 1,
            memory_mib: MIN_MEMORY_MIB,
            beats: None,
            role: None,
            keepalive: false,
            balloon_usable_mib: None,
            sandbox: true,
            volumes: Vec::new(),
        }
    }

    #[test]
    fn a_sandbox_is_refused_with_a_role_and_with_keepalive() {
        let request = |role: Option<&str>, keepalive: Option<bool>| CreateRun {
            role: role.map(str::to_string),
            keepalive,
            ..sandbox_request()
        };
        assert_eq!(
            request(Some("builder"), None).checked(),
            Err("a sandbox has no role".to_string())
        );
        assert_eq!(
            request(None, Some(true)).checked(),
            Err("a sandbox is not a keepalive agent: keepalive cannot be true".to_string())
        );
        assert!(request(None, None).checked().is_ok());
        assert!(request(None, Some(false)).checked().is_ok());
    }

    /// A sandbox boots the harness in sandbox mode with no keepalive flag, under either session: a
    /// pinned CID (the frozen session) and an allocated one (the agent session) both give it no keepalive.
    #[test]
    fn a_sandbox_boots_the_sandbox_cmdline_and_is_never_keepalive() {
        let request = sandbox_request();
        assert_eq!(
            request.vm_cmdline(0, false),
            "console=ttyS0 plaza.harness=1 plaza.sandbox=1 plaza.vm=0"
        );
        let beaten = CreateRun {
            beats: Some(3),
            ..sandbox_request()
        };
        assert_eq!(
            beaten.vm_cmdline(2, false),
            "console=ttyS0 plaza.beats=3 plaza.harness=1 plaza.sandbox=1 plaza.vm=2"
        );
        assert!(!request.keepalive(RunSession::Agent));
        assert!(!request.keepalive(RunSession::Frozen));
    }

    #[test]
    fn the_sandbox_flag_is_optional_on_the_wire_and_defaults_to_false() {
        let plain: CreateRun =
            serde_json::from_str(r#"{"n":1,"vcpus":1,"memory_mib":128}"#).expect("a create body");
        assert!(!plain.sandbox);
        let sandbox: CreateRun =
            serde_json::from_str(r#"{"n":1,"vcpus":1,"memory_mib":128,"sandbox":true}"#)
                .expect("a sandbox body");
        assert!(sandbox.sandbox);
        assert!(sandbox.checked().is_ok());
    }

    #[test]
    fn a_sandbox_run_view_reports_its_sandbox_flag() {
        let run = |sandbox: bool| Run {
            id: "r1".to_string(),
            vcpus: 1,
            memory_mib: MIN_MEMORY_MIB,
            beats: None,
            role: None,
            keepalive: false,
            sandbox,
            volumes: Vec::new(),
            created_at: now_ms(),
            events: EventBus::new(),
            cids: Arc::new(CidBook::default()),
            snapshots: None,
            recoveries: Mutex::new(Vec::new()),
            inner: Mutex::new(RunInner {
                state: RunState::Running,
                ended_at: None,
                vms: Vec::new(),
            }),
        };
        let view = |run: &Run| {
            serde_json::to_value(run.view(Detail::Summary)).expect("a run view")["sandbox"].clone()
        };
        assert_eq!(view(&run(true)), serde_json::Value::Bool(true));
        assert_eq!(view(&run(false)), serde_json::Value::Bool(false));
    }

    /// A sandbox's template shape is the sandbox's own command line over the run's base image, so a
    /// template built for it is never one an agent restores, and the other way round.
    #[test]
    fn a_sandbox_template_shape_is_built_from_the_sandbox_cmdline() {
        let dir =
            std::env::temp_dir().join(format!("apz-noded-sandbox-shape-{}", std::process::id()));
        let registry = Registry::new(
            VmImages {
                kernel: PathBuf::from("kernel"),
                initrd: PathBuf::from("initrd"),
                workspace: Some(PathBuf::from("base.ext4")),
            },
            dir,
            EventBus::new(),
        );
        let meta = sandbox_meta();
        let plans: Vec<VmPlan> = (0..2)
            .map(|index| VmPlan {
                cmdline: sandbox_request().vm_cmdline(index, false),
                vsock_cid: 20 + index as u64,
                role: None,
            })
            .collect();
        let shape = registry.template_shape(Some(Path::new("base.ext4")), &meta, &plans[1], 1, 128);
        assert_eq!(shape.cmdline, plans[1].cmdline);
        assert!(shape.cmdline.contains("plaza.sandbox=1") && !shape.cmdline.contains("keepalive"));
        assert_eq!(shape.workspace_base, Some(PathBuf::from("base.ext4")));
        assert_eq!(shape.balloon, None);
        // The agent shape of the same index is a different one.
        let agent = VmPlan {
            cmdline: CreateRun {
                sandbox: false,
                ..sandbox_request()
            }
            .vm_cmdline(1, true),
            vsock_cid: 21,
            role: None,
        };
        assert_ne!(
            registry.template_shape(Some(Path::new("base.ext4")), &meta, &agent, 1, 128),
            shape
        );
    }

    /// A sandbox's workspace is its run's own copy, and it is never a persistent image: with a
    /// stand-in template store whose kernel is missing, no template can be had, so every VM boots
    /// cold and no workspace is laid down.
    #[test]
    fn a_sandbox_with_no_template_boots_cold_and_lays_no_workspace() {
        let dir =
            std::env::temp_dir().join(format!("apz-noded-sandbox-cold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let base = dir.join("base.ext4");
        std::fs::write(&base, b"base").expect("the base image");
        let registry = Registry::new(
            VmImages {
                kernel: dir.join("missing-kernel"),
                initrd: dir.join("initrd"),
                workspace: Some(base),
            },
            dir.join("uds"),
            EventBus::new(),
        )
        .with_templates(Some(crate::templates::Templates::new(
            dir.join("snapshots"),
        )))
        .with_persist_dir(Some(dir.join("persist")));
        let meta = sandbox_meta();
        let plans = vec![VmPlan {
            cmdline: sandbox_request().vm_cmdline(0, false),
            vsock_cid: 20,
            role: None,
        }];
        let starts = registry.template_starts("r1", &meta, &plans, 1, MIN_MEMORY_MIB as usize);
        assert_eq!(starts, vec![None]);
        assert!(!registry.run_dir("r1").join("workspace-0.ext4").exists());
        assert!(
            !dir.join("persist")
                .join("workspaces")
                .join("cid-20.ext4")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sandbox_copies_and_imports_are_recognised_by_their_command_line() {
        assert!(is_sandbox_cmdline(
            "console=ttyS0 plaza.harness=1 plaza.sandbox=1 plaza.vm=0"
        ));
        assert!(!is_sandbox_cmdline(
            "console=ttyS0 plaza.harness=1 plaza.keepalive=1 plaza.vm=0"
        ));
        assert_eq!(
            sandbox_workspace_path(Path::new("/run"), 2),
            PathBuf::from("/run/workspace-2.ext4")
        );
    }
}

#[cfg(test)]
mod persistent_workspace_tests {
    use super::prepare_persistent_workspace;

    #[test]
    fn a_persistent_workspace_is_copied_once_and_reused_for_its_cid() {
        let dir = std::env::temp_dir().join(format!("apz-noded-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let base = dir.join("workspace.ext4");
        std::fs::write(&base, b"workspace bytes").expect("the base image");
        let persist = dir.join("persist");

        let first = prepare_persistent_workspace(&base, &persist, 42).expect("the first image");
        assert_eq!(first, persist.join("workspaces").join("cid-42.ext4"));
        assert_eq!(std::fs::read(&first).expect("the copy"), b"workspace bytes");
        // The agent's run writes a marker; the next run on the same CID must
        // find it, not a fresh copy of the base.
        std::fs::write(&first, b"M").expect("a marker write");

        let second = prepare_persistent_workspace(&base, &persist, 42).expect("the reused image");
        assert_eq!(second, first);
        assert_eq!(std::fs::read(&second).expect("the reused image"), b"M");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
