//! The public multi-VM API: [`spawn`] boots one microVM, [`spawn_n`] boots
//! `n` of them (ids `0..n`), and each returned [`VmHandle`] exposes the VM's
//! id, its guest serial capture, and blocking `wait`/`kill` control.
//!
//! Every VM is built and booted by the backend of the current platform
//! (`crate::builder` on Linux, `crate::hvf` on Apple Silicon); the state the
//! spawner and the backend share — the serial sink, the stop control and the
//! backend's thread joins — lives in [`VmRuntime`], which is backend-neutral.
//! A backend's `boot` fills the runtime in and returns once the VM is
//! running; the spawner then owns the runtime and joins the backend's
//! threads when the VM stops.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
        }
    }
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
    #[error("no VM backend is compiled in for this platform")]
    NoBackend,
}

impl SpawnError {
    /// Wraps a backend's error into a [`SpawnError`].
    pub fn backend(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Boot(Box::new(err))
    }
}

/// A thread the backend must join before the VM is considered stopped.
type StopControl = Box<dyn Fn() + Send + Sync>;

/// State shared between the spawner, the VM handle and the active backend.
///
/// The spawner creates one per VM and hands it to the backend's `boot`, which
/// installs the stop control, registers the threads it spawned and, when the
/// guest stops the VM by itself, marks the exit as guest-initiated. The
/// spawner then owns the VM: [`VmHandle::wait`] joins the registered threads
/// and [`VmHandle::kill`] first invokes the stop control.
pub struct VmRuntime {
    /// 0-based id of the VM this runtime belongs to.
    pub id: u32,
    /// Guest serial (console) capture. The backend's UART appends to it; the
    /// VM handle reads it (see [`VmHandle::serial`]).
    pub serial: Arc<Mutex<Vec<u8>>>,
    /// Stop control installed by the backend in `boot`: asks the VM's vCPU
    /// loop to finish. Left installed after it is invoked, so that
    /// [`VmHandle::kill`] can invoke it again when the first request did not
    /// stop the VM.
    stop: Mutex<Option<StopControl>>,
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
            stop: Mutex::new(None),
            stop_requested: AtomicBool::new(false),
            guest_exited: AtomicBool::new(false),
            guest_reset: AtomicBool::new(false),
            threads: Mutex::new(Vec::new()),
        }
    }

    /// Installs the stop control. Called by the backend from `boot`, before
    /// it returns.
    pub fn set_stop_control(&self, stop: impl Fn() + Send + Sync + 'static) {
        *self.stop.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(stop));
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
}

/// Boots one microVM.
pub fn spawn(spec: VmSpec) -> Result<VmHandle, SpawnError> {
    spawn_with_id(spec, 0)
}

/// Boots `n` microVMs, with ids `0..n`.
///
/// If one VM fails to boot, the ones that did boot are killed before the
/// error is returned; when one of those cannot be stopped either, the returned
/// error is a [`SpawnError::PartialCleanup`] that names it.
pub fn spawn_n(spec: VmSpec, n: usize) -> Result<Vec<VmHandle>, SpawnError> {
    let mut handles = Vec::with_capacity(n.min(1024));
    for id in 0..n {
        let id = u32::try_from(id).map_err(|_| SpawnError::TooManyVms)?;
        match spawn_with_id(spec.clone(), id) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                let err = cleanup_after_boot_failure(err, &mut handles);
                return Err(err);
            }
        }
    }
    Ok(handles)
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
    let runtime = Arc::new(VmRuntime::new(id));
    boot(&spec, Arc::clone(&runtime))?;
    Ok(VmHandle { id, runtime })
}

/// Hands the VM to the backend of the current platform.
///
/// Linux/KVM uses the arch-dispatched `crate::builder::boot`; Apple Silicon
/// uses `crate::hvf::boot(spec, id, runtime)`, which must:
///
/// * append guest console output to `runtime.serial`,
/// * install its stop control with [`VmRuntime::set_stop_control`],
/// * register every thread it spawned with [`VmRuntime::add_thread`],
/// * call [`VmRuntime::mark_guest_exited`] (or
///   [`VmRuntime::mark_guest_reset`], for a reset) when the guest stopped the
///   VM itself, and
/// * return once the VM is running (not when it stops).
fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    #[cfg(target_os = "linux")]
    {
        crate::builder::boot(spec, runtime)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        crate::hvf::boot(spec, runtime.id, runtime)
    }

    #[cfg(not(any(
        target_os = "linux",
        all(target_os = "macos", target_arch = "aarch64")
    )))]
    {
        let _ = (spec, runtime);
        Err(SpawnError::NoBackend)
    }
}
