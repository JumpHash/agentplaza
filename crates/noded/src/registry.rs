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
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::fleet::{Fleet, agent_flags};
use vmm::spawner::{
    self, MAX_GUEST_CID, MIN_GUEST_CID, SpawnError, VmExit, VmHandle, VmRuntime, VmSpec,
    VsockConfig,
};

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

/// How often the run watcher looks at its VMs. The watcher sleeps for this
/// long *outside* the run's lock, so its polls stay non-blocking and no other
/// caller of the run can be starved by them.
const WATCH_INTERVAL: Duration = Duration::from_millis(5);

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
        if let Some(role) = &self.role {
            checked_role(role)?;
            if self.keepalive == Some(false) {
                return Err("a run with a role is a keepalive agent: keepalive cannot be false".to_string());
            }
        }
        let named = match self.vsock_cid {
            Some(base) => Some(checked_base(base, vms)?),
            None => None,
        };
        Ok((vms, named))
    }

    /// Whether the run's guests are keepalive agents: always for a roled run,
    /// otherwise the request's choice or the default its session implies.
    fn keepalive(&self, session: RunSession) -> bool {
        self.role.is_some() || self.keepalive.unwrap_or(session == RunSession::Agent)
    }

    /// The kernel command line of VM `index` of the run.
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
        match &self.role {
            Some(role) => format!("{CONSOLE}{beats} {}", agent_flags(role, index)),
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
#[derive(Debug, Default)]
struct CidBook {
    /// One entry per live run: its id and every CID it holds, in order.
    held: Mutex<Vec<(String, Vec<u64>)>>,
}

impl CidBook {
    /// The book, poison-proof: a panicking run must not take the node's other
    /// runs with it.
    fn held(&self) -> MutexGuard<'_, Vec<(String, Vec<u64>)>> {
        self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let free = (RUN_CID_BASE..=RUN_CID_END).find(|base| {
            (0..count).all(|offset| {
                base.checked_add(u64::from(offset))
                    .is_some_and(|cid| cid <= RUN_CID_END && !busy.contains(&cid))
            })
        });
        match free {
            Some(base) => {
                let cids = cid_span(base, count);
                put(&mut held, run, cids);
                Ok(base)
            }
            None => Err(format!(
                "the run range {RUN_CID_BASE}..={RUN_CID_END} has no free span of {count} \
                 guest CIDs (in use: {})",
                busy.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
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
                write!(formatter, "VM {vm} could not take the console input: {reason}")
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
}

/// The list endpoint's body: `{"runs": [...]}`.
#[derive(Debug, Serialize)]
pub struct RunsView<'a> {
    /// Every run, oldest first.
    pub runs: Vec<RunView<'a>>,
}

/// The daemon's runs, in creation order.
pub struct Registry {
    images: VmImages,
    /// Directory holding the per-VM vsock Unix sockets (`{uds_dir}/vm{cid}`).
    uds_dir: PathBuf,
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

impl Registry {
    /// A registry with no runs that boots every run from `images`, lays the
    /// runs' vsock sockets out under `uds_dir`, and publishes every state
    /// transition and serial delta to `events`.
    pub fn new(images: VmImages, uds_dir: PathBuf, events: Arc<EventBus>) -> Self {
        Self {
            images,
            uds_dir,
            epoch_ms: now_ms(),
            events,
            cids: Arc::new(CidBook::default()),
            inner: Mutex::new(RegistryInner {
                next_seq: 0,
                runs: Vec::new(),
            }),
        }
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
            },
            plans,
        )
    }

    /// Boots the initial role fleet: one long-lived keepalive VM per role of
    /// `fleet`, in the manifest's alphabetical role order, as one run whose VM
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
        let memory_mib = spawner_memory_mib(meta.memory_mib).map_err(|err| self.give_back(&id, err))?;
        let vms = plans.len();
        // A run's workspace volume is writable, so every VM of the run gets its
        // own copy of the base image: one copy per VM, in the run's own
        // directory under the daemon's UDS root. The directory is created
        // `0700` (an agent's workspace is its private data), and the copies are
        // made before the run is registered, so a failure to prepare them is
        // reported to the caller without leaving a run behind. The copies stay
        // when the run ends: they hold the guests' work.
        let workspaces = match &self.images.workspace {
            Some(base) => {
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
            None => None,
        };
        let run = self.insert(id, meta);
        let boot = spawner::spawn_each(vms, |i| {
            // `spawn_each` calls this exactly `vms` times with the ids
            // `0..vms`, and `plans` has `vms` entries: the index is in range.
            let plan = &plans[i as usize];
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
                // produced them: the spawn's ids are `0..vms` and the run has
                // exactly `vms` copies.
                workspace: workspaces
                    .as_ref()
                    .and_then(|images| images.get(i as usize))
                    .cloned(),
            }
        });
        match boot {
            Ok(handles) => {
                let mut inner = run.lock();
                if let Err(refused) = inner.transition(RunState::Running) {
                    // A DELETE ended the run while its VMs were booting: the
                    // run is already over, so the VMs that just came up must
                    // not be left behind.
                    drop(inner);
                    eprintln!("noded: run {}: {refused}; killing the VMs it just booted", run.id);
                    for mut handle in handles {
                        if let Err(err) = handle.kill() {
                            eprintln!("noded: run {}: VM {} did not stop: {err}", run.id, handle.id());
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
                        state: VmState::Running,
                        // The capture was empty at boot (the serial delta
                        // ticker starts at the first byte the guest writes).
                        serial_seen: 0,
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
                    eprintln!("noded: run {}: {refused} (a kill raced the failed boot)", run.id);
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

    /// The registry's event bus, so a stream endpoint can subscribe to it.
    pub fn events_bus(&self) -> &EventBus {
        &self.events
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
            created_at: now_ms(),
            events: Arc::clone(&self.events),
            cids: Arc::clone(&self.cids),
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

/// One run: what the API reports about it and the VMs behind it.
pub struct Run {
    id: String,
    vcpus: u32,
    memory_mib: u32,
    beats: Option<u32>,
    /// The role every VM of the run runs as, when the run has one.
    role: Option<String>,
    created_at: u64,
    /// Where this run's state, VM and serial-delta events are published.
    events: Arc<EventBus>,
    /// The node's CID book: this run's span is held there for the run's
    /// lifetime and released when it ends ([`Run::release_cids`]).
    cids: Arc<CidBook>,
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
    /// How the VM stopped (read and written under the run's lock).
    state: VmState,
    /// How much of the serial capture the event bus has already published
    /// (read and written under the run's lock): the serial-delta ticker's
    /// bookmark.
    serial_seen: usize,
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

    /// Whether every VM of the run has stopped.
    fn all_vms_stopped(&self) -> bool {
        self.vms.iter().all(|slot| slot.state != VmState::Running)
    }
}

impl Run {
    /// The run's id (`r<seq>`).
    pub fn id(&self) -> &str {
        &self.id
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
                VmState::Running => slot.handle.take().map(|handle| (index, handle)),
                VmState::Exited | VmState::Killed => None,
            })
            .collect();
        drop(inner);

        let mut stopped = Vec::with_capacity(taken.len());
        for (index, mut handle) in taken {
            stopped.push((index, handle.kill(), handle));
        }

        let mut inner = self.lock();
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

fn watch_run(run: &Run) {
    loop {
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
                publish_serial_delta(&run.events, &run.id, slot);
                if slot.state != VmState::Running {
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
                match handle.wait_timeout(Duration::ZERO) {
                    Ok(Some(exit)) => {
                        slot.state = VmState::from_exit(exit);
                        run.events.publish(Event::Vm {
                            run: run.id.clone(),
                            vm: slot.id,
                            state: slot.state.as_str(),
                        });
                    }
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
        thread::sleep(WATCH_INTERVAL);
    }
}

/// Publishes the part of the VM's serial capture that grew since the last
/// look, as a `serial` event whose `offset` is the capture byte the `text`
/// starts at. The capture is bounded, and so are the events: once the capture
/// stops growing, no further events fire for it.
fn publish_serial_delta(events: &EventBus, run: &str, slot: &mut VmSlot) {
    let buffer = slot
        .runtime
        .serial
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if buffer.len() <= slot.serial_seen {
        return;
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
}

/// The current time in milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
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
            std::fs::read_dir(&run_dir).expect("the run's directory").count(),
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
    fn a_refused_transition_leaves_the_run_alone() {
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
        assert_eq!(refused.to_string(), "a run may not go from killed to running");
        assert_eq!(run.state, RunState::Killed);
        assert_eq!(run.ended_at, Some(7));
    }

    #[test]
    fn an_ending_transition_stamps_ended_at_and_a_start_does_not() {
        let mut run = inner(RunState::Starting);
        run.transition(RunState::Running).expect("starting -> running");
        assert_eq!(run.state, RunState::Running);
        assert_eq!(run.ended_at, None);

        run.transition(RunState::Done).expect("running -> done");
        assert!(run.ended_at.is_some());
        assert!(run.state.is_finished());

        let mut failed = inner(RunState::Running);
        failed.transition(RunState::Failed).expect("running -> failed");
        assert!(failed.ended_at.is_some());

        let mut killed = inner(RunState::Starting);
        killed.transition(RunState::Killed).expect("starting -> killed");
        assert!(killed.ended_at.is_some());
    }

    #[test]
    fn finishing_is_a_no_op_once_the_run_has_ended() {
        let mut run = inner(RunState::Killed);
        run.ended_at = Some(7);
        run.finish();
        assert_eq!(run.state, RunState::Killed);
        assert_eq!(run.ended_at, Some(7));
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
            created_at: now_ms(),
            events: EventBus::new(),
            cids,
            inner: Mutex::new(RunInner {
                state: run_state,
                ended_at: None,
                vms: vec![VmSlot {
                    id: 0,
                    guest_cid: cid,
                    role: role.map(str::to_string),
                    runtime: Arc::new(VmRuntime::new(0)),
                    handle: None,
                    state: vm_state,
                    serial_seen: 0,
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
        };
        assert_eq!(request(None, None).vm_cmdline(0, false), "console=ttyS0 plaza.harness=1 plaza.vm=0");
        assert_eq!(
            request(Some(2), None).vm_cmdline(1, true),
            "console=ttyS0 plaza.beats=2 plaza.harness=1 plaza.keepalive=1 plaza.vm=1"
        );
        let roled = request(None, Some("builder")).vm_cmdline(0, true);
        assert!(roled.contains("plaza.role=builder") && roled.contains("plaza.keepalive=1"), "{roled}");
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
        let request = |n: u32, vcpus: u32, memory_mib: u32| CreateRun {
            n,
            vcpus,
            memory_mib,
            beats: None,
            vsock_cid: None,
            role: None,
            keepalive: None,
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

    #[test]
    fn vsock_cid_is_range_checked_before_a_run_exists() {
        let request = |vsock_cid: Option<u64>| CreateRun {
            n: 2,
            vcpus: 1,
            memory_mib: 128,
            beats: None,
            vsock_cid,
            role: None,
            keepalive: None,
        };

        // A CID inside the valid range is accepted, base + n - 1 included.
        assert_eq!(
            request(Some(3)).checked().expect("a valid CID").1,
            Some(3)
        );
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

    /// The book hands out the first free span of the run range, one span per
    /// run, and gives a released span back.
    #[test]
    fn an_allocation_takes_the_first_free_span_and_gives_it_back() {
        let book = CidBook::default();
        assert_eq!(book.allocate("r1", 1).expect("the range is empty"), RUN_CID_BASE);
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
        assert_eq!(book.allocate("r4", 1).expect("the hole is free"), RUN_CID_BASE);
        assert!(!book.release("r1"), "release is idempotent");

        // Every live run's span is its own: no two allocations overlap.
        let held = book.held();
        let mut all: Vec<u64> = held.iter().flat_map(|(_, cids)| cids.iter().copied()).collect();
        let count = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), count, "two runs were given the same CID: {held:?}");
    }

    /// A span the caller pinned is never allocated to another run: the
    /// caller's choice wins, and the allocator works around it.
    #[test]
    fn a_pinned_span_is_never_allocated() {
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

    /// Exhaustion is a refusal that names the range and every busy CID, not a
    /// span quietly handed out twice.
    #[test]
    fn a_full_range_refuses_the_next_allocation_and_names_it() {
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
        assert!(refused.contains(&format!("in use: {RUN_CID_BASE}, ")), "{refused}");
        assert!(refused.contains(&format!(", {RUN_CID_END}")), "{refused}");

        // A span wider than the whole range is refused the same way, and a
        // released CID makes room for it again.
        assert!(book.allocate("wide", width + 1).is_err());
        assert!(book.release("r0"));
        assert_eq!(book.allocate("late", 1).expect("the hole is free"), RUN_CID_BASE);
    }

    /// A create that cannot prepare its workspace copies is refused with no
    /// run - and, because it had already reserved a span, with those CIDs back
    /// in the book: a request that never became a run holds nothing.
    #[test]
    fn a_boot_that_never_registers_a_run_holds_no_cids() {
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
            })
            .err()
            .expect("the workspace copies cannot be prepared");
        assert!(
            refused.contains("cannot prepare the run's workspace images"),
            "{refused}"
        );
        assert!(registry.list().is_empty(), "a refused create left a run behind");
        assert!(
            busy(&registry.cids.held()).is_empty(),
            "a create that never became a run kept its CIDs: {:?}",
            registry.cids.held()
        );

        std::fs::remove_dir_all(&dir).expect("cleanup");
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
            })
            .err()
            .expect("the run range is full");
        assert!(refused.contains(&format!("{RUN_CID_BASE}..={RUN_CID_END}")), "{refused}");
        assert!(refused.contains("in use: "), "{refused}");
        // Nothing was created and nothing was laid out: the refusal came
        // before a run id became a run directory.
        assert!(registry.list().is_empty(), "a refused create left a run behind");
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
            })
            .expect("a pinned run does not need the run range");

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A run that ends - here, a boot that fails, which the tests can produce
    /// without a hypervisor - gives its CIDs back, so the next run draws the
    /// same span instead of the range leaking a span per attempt.
    #[test]
    fn a_finished_run_gives_its_cids_back() {
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
}
