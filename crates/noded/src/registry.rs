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

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
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

/// The `POST /api/v0/runs` body, as the client sent it.
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
    /// Base guest CID for the run's vsock devices (`vsock_cid + i` for VM `i`);
    /// `None` boots the VMs without a vsock device.
    #[serde(default)]
    pub vsock_cid: Option<u64>,
}

impl CreateRun {
    /// Checks the contract's ranges and turns the request into what the
    /// spawner needs: the VMs' spec and how many of them to boot.
    ///
    /// `n` is 1..=64, `vcpus` is 1..=64 and `memory_mib` is at least 128;
    /// anything else is refused with the reason, before any run exists.
    fn spec(&self, images: &VmImages) -> Result<(VmSpec, usize, Option<u64>), String> {
        let vms = checked_count("n", self.n, 1, MAX_VMS)?;
        let vcpus = checked_count("vcpus", self.vcpus, 1, MAX_VCPUS)?;
        if self.memory_mib < MIN_MEMORY_MIB {
            return Err(format!("memory_mib must be at least {MIN_MEMORY_MIB}"));
        }
        let vcpus = u8::try_from(vcpus)
            .map_err(|_| format!("vcpus must be between 1 and {MAX_VCPUS}"))?;
        let memory_mib = usize::try_from(self.memory_mib)
            .map_err(|_| "memory_mib is too large for this host".to_string())?;
        let vsock_cid = match self.vsock_cid {
            Some(cid) => {
                if cid < MIN_GUEST_CID {
                    return Err(format!(
                        "vsock_cid must be at least {MIN_GUEST_CID} (1 and 2 are reserved)"
                    ));
                }
                // A CID the kernel's vsock stack always refuses — VMADDR_CID_ANY
                // (u32::MAX) and anything above it — is rejected here rather
                // than left to a boot that would fail on it.
                if cid > MAX_GUEST_CID {
                    return Err(format!(
                        "vsock_cid must be at most {MAX_GUEST_CID}: the kernel refuses \
                         VMADDR_CID_ANY and above"
                    ));
                }
                // Every VM's CID, up to the run's last, must fit the transport.
                let last = cid
                    .checked_add(u64::try_from(vms).map_err(|_| "n is too large".to_string())? - 1)
                    .ok_or_else(|| "vsock_cid is too large for this many VMs".to_string())?;
                if last > MAX_GUEST_CID {
                    return Err(
                        "vsock_cid and n together run past the last guest CID".to_string(),
                    );
                }
                Some(cid)
            }
            None => None,
        };
        // A run with vsock devices boots the agent harness in its guests: the
        // harness is the vsock client (handbook: every tool call goes over
        // vsock), and it exits by rebooting the guest when its session ends.
        let cmdline = match (self.beats, vsock_cid) {
            (Some(beats), Some(_)) => format!("console=ttyS0 plaza.beats={beats} plaza.harness=1"),
            (Some(beats), None) => format!("console=ttyS0 plaza.beats={beats}"),
            (None, Some(_)) => "console=ttyS0 plaza.harness=1".to_string(),
            (None, None) => "console=ttyS0".to_string(),
        };
        let spec = VmSpec {
            kernel: images.kernel.clone(),
            cmdline,
            vcpus,
            memory_mib,
            initrd: Some(images.initrd.clone()),
            gic: None,
            vsock: None,
            // Filled in per VM by `create_run`, from the run's image copies.
            workspace: None,
        };
        Ok((spec, vms, vsock_cid))
    }
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
    /// The VM's vsock guest CID (`vsock_cid + index`); absent when the run has
    /// no vsock devices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_cid: Option<u64>,
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
    /// Where the registry's run, VM and serial-delta events are published.
    events: Arc<EventBus>,
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
            events,
            inner: Mutex::new(RegistryInner {
                next_seq: 0,
                runs: Vec::new(),
            }),
        }
    }

    /// Creates and boots a run.
    ///
    /// The run is registered in `starting` before its VMs are booted, so a
    /// concurrent GET can see it, and moves to `running` once they are up (or
    /// `failed` when they are not). A `DELETE` that arrives while the VMs are
    /// booting moves the run to `killed`, and the VMs that come up afterwards
    /// are killed rather than left behind.
    pub fn create_run(&self, request: &CreateRun) -> Result<Arc<Run>, String> {
        let (spec, vms, vsock_cid) = request.spec(&self.images)?;
        // A run's workspace volume is writable, so every VM of the run gets its
        // own copy of the base image: one copy per VM, in the run's own
        // directory under the daemon's UDS root. The directory is created
        // `0700` (an agent's workspace is its private data), and the copies are
        // made before the run is registered, so a failure to prepare them is
        // reported to the caller without leaving a run behind. The copies stay
        // when the run ends: they hold the guests' work.
        let id = self.reserve_run_id();
        let workspaces = match &self.images.workspace {
            Some(base) => {
                let run_dir = self.uds_dir.join(format!("run-{id}"));
                Some(
                    spawner::materialize_workspaces(base, &run_dir, vms).map_err(|err| {
                        format!("cannot prepare the run's workspace images: {err}")
                    })?,
                )
            }
            None => None,
        };
        let run = self.insert(id, request);
        let boot = spawner::spawn_each(vms, |i| {
            let mut per_vm = spec.clone();
            if vsock_cid.is_some() || workspaces.is_some() {
                // One command line is shared by every VM of a spawn, so the
                // per-VM index rides as a flag the guest harness can trust.
                per_vm.cmdline = format!("{} plaza.vm={}", spec.cmdline, i);
            }
            if let Some(base) = vsock_cid {
                let guest_cid = base + u64::from(i);
                per_vm.vsock = Some(VsockConfig {
                    guest_cid,
                    uds_path: self.uds_dir.join(format!("vm{guest_cid}")),
                });
            }
            // One image per VM, in the order `materialize_workspaces` produced
            // them: the spawn's ids are `0..vms` and the run has exactly `vms`
            // copies.
            per_vm.workspace = workspaces.as_ref().and_then(|images| {
                usize::try_from(i)
                    .ok()
                    .and_then(|index| images.get(index))
                    .cloned()
            });
            per_vm
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
                inner.vms = handles
                    .into_iter()
                    .enumerate()
                    .map(|(index, handle)| VmSlot {
                        id: handle.id(),
                        guest_cid: vsock_cid.map(|base| {
                            base + u64::try_from(index).unwrap_or(u64::MAX)
                        }),
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
                    run.events.publish(Event::Run {
                        id: run.id.clone(),
                        state: RunState::Failed.as_str(),
                    });
                }
            }
        }
        Ok(run)
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

    /// Registers a new run in `starting`, which is where the schema says a run
    /// begins (`core.check_run`).
    /// Reserves the next run id (`r{seq}`), consuming it.
    ///
    /// A run's id names its own directory under the UDS root (`run-{id}`, which
    /// holds the run's workspace-image copies), so an id is never handed out
    /// twice: a caller that reserves one and then fails leaves a gap in the
    /// numbering rather than letting a later run share its directory.
    fn reserve_run_id(&self) -> String {
        let mut inner = self.lock();
        inner.next_seq += 1;
        format!("r{}", inner.next_seq)
    }

    fn insert(&self, id: String, request: &CreateRun) -> Arc<Run> {
        let mut inner = self.lock();
        let run = Arc::new(Run {
            id,
            vcpus: request.vcpus,
            memory_mib: request.memory_mib,
            beats: request.beats,
            created_at: now_ms(),
            events: Arc::clone(&self.events),
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
    created_at: u64,
    /// Where this run's state, VM and serial-delta events are published.
    events: Arc<EventBus>,
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
            created_at: self.created_at,
            ended_at: inner.ended_at,
            vms: inner
                .vms
                .iter()
                .map(|slot| VmView {
                    id: slot.id,
                    guest_cid: slot.guest_cid,
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
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
            })
            .expect("the run exists even when its VMs cannot boot");

        let run_dir = uds_dir.join(format!("run-{}", run.id));
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

    #[test]
    fn a_create_request_is_range_checked_before_a_run_exists() {
        let request = |n: u32, vcpus: u32, memory_mib: u32| CreateRun {
            n,
            vcpus,
            memory_mib,
            beats: None,
            vsock_cid: None,
        };

        let (spec, vms, vsock_cid) = request(2, 2, 512)
            .spec(&images())
            .expect("a request inside the ranges");
        assert_eq!(vsock_cid, None);
        assert_eq!(vms, 2);
        assert_eq!(spec.vcpus, 2);
        assert_eq!(spec.memory_mib, 512);
        assert_eq!(spec.cmdline, "console=ttyS0");
        assert_eq!(spec.initrd.as_deref(), Some(Path::new("initrd")));

        for bad in [
            request(0, 1, 128),
            request(65, 1, 128),
            request(1, 0, 128),
            request(1, 65, 128),
            request(1, 1, 127),
        ] {
            assert!(bad.spec(&images()).is_err(), "should be refused: {bad:?}");
        }

        let edges = [
            request(1, 1, 128),
            request(64, 64, 128),
            request(1, 1, u32::MAX),
        ];
        for edge in edges {
            assert!(edge.spec(&images()).is_ok(), "should be allowed: {edge:?}");
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
        };

        // A CID inside the valid range is accepted, base + n - 1 included.
        assert!(request(Some(3)).spec(&images()).is_ok());
        assert!(request(Some(MAX_GUEST_CID - 1)).spec(&images()).is_ok());

        // The kernel refuses VMADDR_CID_ANY (u32::MAX) and anything above it.
        assert!(request(Some(u32::MAX as u64)).spec(&images()).is_err());
        assert!(request(Some(u64::from(u32::MAX) + 1)).spec(&images()).is_err());

        // The run's last VM (base + n - 1) must still fit the transport.
        assert!(request(Some(MAX_GUEST_CID)).spec(&images()).is_err());
    }

    #[test]
    fn beats_becomes_the_guest_cmdline() {
        let request = CreateRun {
            n: 1,
            vcpus: 1,
            memory_mib: 128,
            beats: Some(2),
            vsock_cid: None,
        };
        let (spec, _, _) = request.spec(&images()).expect("a request inside the ranges");
        assert_eq!(spec.cmdline, "console=ttyS0 plaza.beats=2");
    }
}
