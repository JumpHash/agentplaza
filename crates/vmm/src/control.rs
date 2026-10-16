// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The sequence — pause the vCPUs, save their state, the VM's and the
// devices', dump the memory, resume — is Firecracker's (src/vmm/src/persist.rs
// `create_snapshot`); the pieces it calls are the ported state code.

//! Pausing, resuming and snapshotting a running in-process VM (Linux/KVM): the [`VmControl`] the
//! builder installs on a VM's runtime.
//!
//! **What a snapshot freezes.** The vCPUs are paused (out of `KVM_RUN`), so no guest code runs. The
//! block and balloon devices only act on a vCPU's MMIO exit, so they are quiet too. The vsock device
//! has its own event thread that writes guest memory when host data arrives; the snapshot holds the
//! write half of the VM's **device gate** ([`DeviceGate`]) for its whole length, and the event thread
//! takes the read half around each pass, so no packet lands between the device state and the memory
//! dump.
//!
//! **What memory is written.** A VM that was booted has anonymous memory, and its snapshot writes
//! every page it touched ([`SnapshotKind::Full`]). A VM that was itself restored maps its snapshot's
//! memory file privately: pages it never read are only in that file, so a `Full` write would leave
//! holes where they belong. Its snapshots are therefore always a reflinked copy of the file it was
//! restored from plus the pages it changed ([`SnapshotKind::Diff`]), whatever the caller asked —
//! complete either way. The control keeps that file open, so the copy works even after the file's
//! directory was removed.
//!
//! **What a snapshot records for a restore.** The guest's resident pages, as ranges of the memory file
//! ([`WORKING_SET_FILE`]), taken before the memory is written. A restore reads them ahead, so the
//! guest's working set is in the page cache when it wakes. A reclaimed VM's pages are gone from RAM by
//! the time it is snapshotted, so `reclaim` keeps the set it saw first (see the `reclaimed_working_set`
//! field).
//!
//! **What it does not hold.** The control keeps the VM and its devices *weakly*: the VM's exit
//! watcher owns them, and a control that kept them alive would keep a stopped VM's memory mapped.
//! Every call on a stopped VM fails with "the VM has stopped".

use std::fmt::Display;
use std::fs::{self, File, OpenOptions, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Instant;

use crate::devices::legacy::SerialDevice;
use crate::devices::virtio::mmio::{MmioDeviceState, VirtioMmioTransport};
use vm_memory::GuestMemoryBackend;

use crate::spawner::{SnapshotInfo, SnapshotKind, SpawnError, VmControl};
use crate::sys;
use crate::vstate::bus::lock;
use crate::vstate::memory::{
    Address, GuestMemoryRegion, MemoryRegionAddress, host_page_size, u64_to_usize, usize_to_u64,
};
use crate::vstate::snapshot::{
    self, DeviceStates, MEMORY_FILE, RegionMeta, Run, SerialRegisters, VcpuState, VmSnapshot,
    VmState, merge_runs, meta_now, write_state, write_working_set,
};
use crate::vstate::snapshot_memory::{
    CapturedMemory, capture_memory, layout, resident_runs, write_captured, write_memory,
};
use crate::vstate::vm::{KvmVm, KvmVmError};

/// What every call on a VM that has stopped fails with.
const VM_STOPPED: &str = "the VM has stopped";

/// The most guest memory a capture keeps in RAM. A capture whose pages would pass it writes the
/// memory file at once, as a snapshot does.
const CAPTURE_LIMIT: u64 = 256 << 20;

/// The VM's device gate (see the vsock event thread, where it is defined for every backend).
pub use crate::devices::virtio::vsock::event_thread::DeviceGate;

/// The VM's devices, as the control reaches them (weakly; see the module docs).
pub struct VmDevices {
    pub serial: Weak<Mutex<SerialDevice>>,
    pub vsock: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub workspace: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub balloon: Option<Weak<Mutex<VirtioMmioTransport>>>,
    /// The shared data volumes' transports, in the spec's order.
    pub volumes: Vec<Weak<Mutex<VirtioMmioTransport>>>,
}

/// The in-process [`VmControl`] of one VM.
pub struct InProcessControl {
    vm: Weak<KvmVm>,
    devices: VmDevices,
    gate: DeviceGate,
    vcpus: u8,
    memory_mib: usize,
    /// The memory file this VM was restored from, kept open (see the module docs); `None` for a VM
    /// that was booted.
    restored_from: Option<File>,
    /// Serializes control calls: one pause, resume or snapshot at a time.
    busy: Mutex<()>,
    /// Whether the control has left the VM's vCPUs paused. Pause and resume check it, so both are
    /// idempotent; a VM starts running.
    paused: AtomicBool,
    /// The guest's resident pages, as memory-file runs, as the last `reclaim` found them before it
    /// paged them out. A snapshot of a reclaimed VM would otherwise record almost nothing; the next
    /// snapshot takes this set (merged with what is resident then) and clears it.
    reclaimed_working_set: Mutex<Option<Vec<Run>>>,
    /// The last capture, kept in RAM until `write_captured` writes it (see [`VmControl::capture`]).
    captured: Mutex<Option<Captured>>,
}

/// What a capture holds, all of it in RAM: what `write_captured` writes into the snapshot directory.
struct Captured {
    dir: PathBuf,
    memory: CapturedMemory,
    working_set: Vec<Run>,
    state: VmSnapshot,
    durable: bool,
}

impl InProcessControl {
    /// The control of `vm`, built by the builder once the VM's devices exist.
    pub fn new(
        vm: Weak<KvmVm>,
        devices: VmDevices,
        gate: DeviceGate,
        vcpus: u8,
        memory_mib: usize,
        restored_from: Option<File>,
    ) -> Self {
        Self {
            vm,
            devices,
            gate,
            vcpus,
            memory_mib,
            restored_from,
            busy: Mutex::new(()),
            paused: AtomicBool::new(false),
            reclaimed_working_set: Mutex::new(None),
            captured: Mutex::new(None),
        }
    }

    /// The VM, if it has not stopped.
    fn live_vm(&self) -> Result<Arc<KvmVm>, SpawnError> {
        self.vm.upgrade().ok_or_else(|| snapshot_error(VM_STOPPED))
    }

    /// Pauses the vCPUs of `vm` unless they are paused already. A failed pause resumes the vCPUs
    /// that did stop (best effort): the control owns the pause, so it does not leave the VM
    /// half-stopped for its caller to untangle.
    fn pause_vm(&self, vm: &KvmVm) -> Result<(), SpawnError> {
        if self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Err(err) = vm.pause_vcpus() {
            let _ = vm.resume_vcpus();
            return Err(snapshot_error(format!("pause the vCPUs: {err}")));
        }
        self.paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// The state of the serial console, field for field from the UART's state.
    fn save_serial(&self) -> Result<SerialRegisters, SpawnError> {
        let serial = alive(&self.devices.serial, "serial")?;
        let registers = lock(&serial).registers();
        Ok(registers)
    }

    /// The state of the device set: the serial console always, each virtio transport the VM has.
    fn save_devices(&self) -> Result<DeviceStates, SpawnError> {
        Ok(DeviceStates {
            serial: self.save_serial()?,
            vsock: save_transport(self.devices.vsock.as_ref(), "vsock")?,
            workspace: save_transport(self.devices.workspace.as_ref(), "workspace")?,
            balloon: save_transport(self.devices.balloon.as_ref(), "balloon")?,
            volumes: self
                .devices
                .volumes
                .iter()
                .map(|volume| {
                    save_transport(Some(volume), "data volume")?
                        .ok_or_else(|| snapshot_error("a data volume's transport is gone"))
                })
                .collect::<Result<Vec<_>, SpawnError>>()?,
        })
    }

    /// The kind of memory file a snapshot of this VM holds: a VM restored from a file gets a diff, a
    /// booted VM a full snapshot. See the module docs.
    fn memory_kind(&self) -> SnapshotKind {
        match self.restored_from {
            Some(_) => SnapshotKind::Diff,
            None => SnapshotKind::Full,
        }
    }

    /// Refuses a snapshot whose memory file would be the file the VM was restored from: the base is
    /// the file the VM still maps, and copying onto it would truncate the pages the VM has not read.
    fn check_memory_target(&self, dir: &Path) -> Result<(), SpawnError> {
        let Some(base) = &self.restored_from else {
            return Ok(());
        };
        let path = dir.join(MEMORY_FILE);
        let base_meta = base
            .metadata()
            .map_err(|err| step("read the restored memory file", err))?;
        if let Ok(existing) = fs::metadata(&path)
            && existing.dev() == base_meta.dev()
            && existing.ino() == base_meta.ino()
        {
            return Err(snapshot_error(format!(
                "write the memory file: {} is the file the VM was restored from",
                path.display()
            )));
        }
        Ok(())
    }

    /// Creates the memory file of the snapshot in `dir`, empty for a booted VM, or a reflinked copy
    /// of the base with the diff written over it for a restored one. Returns the file's kind.
    fn memory_file(&self, dir: &Path) -> Result<(SnapshotKind, File), SpawnError> {
        let path = dir.join(MEMORY_FILE);
        match &self.restored_from {
            Some(base) => {
                // The copy goes through the open descriptor, so it works after the base was unlinked
                // and inside a jail with no `/proc`.
                crate::clonefile::clone_open_file(base, &path)
                    .map_err(|err| step("copy the restored memory file", err))?;
                fs::set_permissions(&path, Permissions::from_mode(0o600))
                    .map_err(|err| step("set the memory file's mode", err))?;
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .map_err(|err| step("open the memory file", err))?;
                Ok((SnapshotKind::Diff, file))
            }
            None => {
                let file = snapshot::private_file(&path)
                    .map_err(|err| step("create the memory file", err))?;
                Ok((SnapshotKind::Full, file))
            }
        }
    }

    /// Writes the memory file of the snapshot in `dir`, returning its kind and the bytes written.
    fn save_memory(
        &self,
        vm: &KvmVm,
        dir: &Path,
        regions: &[RegionMeta],
    ) -> Result<(SnapshotKind, u64), SpawnError> {
        let (kind, file) = self.memory_file(dir)?;
        let written = write_memory(vm.guest_memory(), regions, &file, kind)
            .map_err(|err| step("write the guest memory", err))?;
        Ok((kind, written))
    }

    /// Writes the snapshot's memory and state: the shared body of `snapshot` and `capture`. With
    /// `keep` the memory is captured into RAM (`capture`), and the state file waits for
    /// `write_captured`; a capture past [`CAPTURE_LIMIT`] writes both at once, as a snapshot does.
    fn take(
        &self,
        dir: &Path,
        requested: SnapshotKind,
        keep: bool,
    ) -> Result<SnapshotInfo, SpawnError> {
        self.check_memory_target(dir)?;
        let vm = self.live_vm()?;
        // The clock starts before the pause: `paused_ms` is how long the guest was stopped.
        let started = Instant::now();
        self.pause_vm(&vm)?;
        // Held to the end: the vsock event thread must not land a packet between the device state
        // and the memory dump.
        let _gate = self.gate.write().unwrap_or_else(PoisonError::into_inner);

        snapshot::create_private_dir(dir)
            .map_err(|err| step("create the snapshot directory", err))?;
        let vcpus = vm
            .save_vcpu_states()
            .map_err(|err| step("save the vCPU states", err))?;
        let vm_state = save_vm_state(&vm, &vcpus).map_err(|err| step("save the VM state", err))?;
        let devices = self.save_devices()?;
        let regions = layout(vm.guest_memory());
        // Read before the memory is written: the write reads the touched pages back in.
        // The working set is a hint for the restore: failing to read or write it never fails the snapshot.
        let working_set = {
            let kept = lock(&self.reclaimed_working_set);
            merge_runs(
                kept.as_deref().unwrap_or(&[]),
                &resident_or_none(&vm, &regions),
            )
        };
        let kept = if keep {
            capture_memory(
                vm.guest_memory(),
                &regions,
                self.memory_kind(),
                CAPTURE_LIMIT,
            )
            .map_err(|err| step("capture the guest memory", err))?
        } else {
            None
        };
        let (kind, memory_written, memory) = match kept {
            Some(memory) => (self.memory_kind(), memory.bytes, Some(memory)),
            None => {
                let (kind, written) = self.save_memory(&vm, dir, &regions)?;
                (kind, written, None)
            }
        };

        let mut meta = meta_now(kind, self.vcpus, self.memory_mib, regions);
        meta.memory_written = memory_written;
        let state = VmSnapshot {
            meta,
            vm: vm_state,
            vcpus,
            devices,
        };
        let durable = requested.is_durable();
        match memory {
            Some(memory) => {
                *lock(&self.captured) = Some(Captured {
                    dir: dir.to_path_buf(),
                    memory,
                    working_set,
                    state,
                    durable,
                });
            }
            None => {
                if let Err(err) = write_working_set(dir, &working_set) {
                    eprintln!("warning: snapshot: the working-set hint was not written: {err}");
                }
                write_state(dir, &state, durable)
                    .map_err(|err| step("write the state file", err))?;
            }
        }
        // The snapshot has taken what the reclaim kept; a later snapshot records what is resident then.
        *lock(&self.reclaimed_working_set) = None;

        Ok(SnapshotInfo {
            dir: dir.to_path_buf(),
            kind,
            memory_written,
            paused_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

impl VmControl for InProcessControl {
    fn pause(&self) -> Result<(), SpawnError> {
        let _busy = lock(&self.busy);
        let vm = self.live_vm()?;
        self.pause_vm(&vm)
    }

    fn resume(&self) -> Result<(), SpawnError> {
        let _busy = lock(&self.busy);
        let vm = self.live_vm()?;
        if !self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        vm.resume_vcpus()
            .map_err(|err| snapshot_error(format!("resume the vCPUs: {err}")))?;
        self.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Takes a snapshot into `dir` and leaves the VM paused.
    ///
    /// The `kind` the caller asks for does not change what is written: a restored VM's snapshot is
    /// always a diff (a full write would leave holes where its untouched pages are), and a booted
    /// VM's is always full (the same pages a diff of it would hold). The state file is written last,
    /// so an interrupted snapshot leaves no state for a restore to read. Only the durability of the
    /// `requested` kind is honored: a `DiffUnsynced` state file is not fsynced.
    fn snapshot(&self, dir: &Path, requested: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        let _busy = lock(&self.busy);
        // A snapshot supersedes a capture that was never written.
        *lock(&self.captured) = None;
        self.take(dir, requested, false)
    }

    /// Takes a snapshot like [`snapshot`](Self::snapshot), but keeps the memory and the state in RAM
    /// (see [`VmControl::capture`]). A capture while an earlier one is unwritten is refused.
    fn capture(&self, dir: &Path, requested: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        let _busy = lock(&self.busy);
        if lock(&self.captured).is_some() {
            return Err(snapshot_error(
                "a previous capture is not written yet; write it before capturing again",
            ));
        }
        self.take(dir, requested, true)
    }

    /// Writes the capture the VM holds, with the VM running: the memory pages, the working-set hint,
    /// then the state file last, as a snapshot writes them. Nothing to write is `Ok(0)`.
    fn write_captured(&self) -> Result<u64, SpawnError> {
        let _busy = lock(&self.busy);
        let Some(captured) = lock(&self.captured).take() else {
            return Ok(0);
        };
        let Captured {
            dir,
            memory,
            working_set,
            state,
            durable,
        } = captured;
        let (kind, file) = self.memory_file(&dir)?;
        let written = write_captured(&memory, &file, kind)
            .map_err(|err| step("write the guest memory", err))?;
        if let Err(err) = write_working_set(&dir, &working_set) {
            eprintln!("warning: snapshot: the working-set hint was not written: {err}");
        }
        write_state(&dir, &state, durable).map_err(|err| step("write the state file", err))?;
        Ok(written)
    }

    /// Pages every guest region out of host RAM. The vCPUs and devices are stopped, so nothing
    /// touches the guest meanwhile; the pages fault back in when the guest next reads them.
    fn reclaim(&self) -> Result<u64, SpawnError> {
        let _busy = lock(&self.busy);
        let vm = self.live_vm()?;
        if !self.paused.load(Ordering::SeqCst) {
            return Err(snapshot_error(
                "the VM is running; pause it before reclaiming its memory",
            ));
        }
        // Keep what is resident before any page goes out, for the next snapshot's working set. A second
        // reclaim finds little resident, so it adds to the set the first one kept.
        {
            let regions = layout(vm.guest_memory());
            let mut kept = lock(&self.reclaimed_working_set);
            let resident = resident_or_none(&vm, &regions);
            *kept = Some(merge_runs(kept.as_deref().unwrap_or(&[]), &resident));
        }
        let mut advised = 0;
        for region in vm.guest_memory().iter() {
            let host = region
                .get_host_address(MemoryRegionAddress(0))
                .map_err(|err| step("find the guest memory", err))?;
            let len = region.len();
            sys::page_out(host, u64_to_usize(len))
                .map_err(|err| step("page out the guest memory", err))?;
            advised += len;
        }
        Ok(advised)
    }
}

/// The guest memory's resident pages, as runs of memory-file offsets. `regions` is the snapshot's
/// [`layout`] of `vm`'s memory.
/// [`resident_pages`], or no runs (and one warning) when they cannot be read: the set is only a hint.
fn resident_or_none(vm: &KvmVm, regions: &[RegionMeta]) -> Vec<Run> {
    resident_pages(vm, regions).unwrap_or_else(|err| {
        eprintln!("warning: snapshot: the guest's resident pages could not be read: {err}");
        Vec::new()
    })
}

fn resident_pages(vm: &KvmVm, regions: &[RegionMeta]) -> Result<Vec<Run>, SpawnError> {
    let page = usize_to_u64(host_page_size());
    let mut runs = Vec::new();
    for meta in regions {
        let region = vm
            .guest_memory()
            .iter()
            .find(|r| r.start_addr().raw_value() == meta.guest_addr && r.len() == meta.len)
            .ok_or_else(|| {
                snapshot_error(format!("no guest memory region at {:#x}", meta.guest_addr))
            })?;
        let host = region
            .get_host_address(MemoryRegionAddress(0))
            .map_err(|err| step("find the guest memory", err))?;
        let vec = sys::resident_pages(host, u64_to_usize(meta.len))
            .map_err(|err| step("read the resident pages", err))?;
        runs.extend(resident_runs(&vec, meta.offset, page));
    }
    Ok(merge_runs(&runs, &[]))
}

/// A snapshot failure, as the control reports it.
fn snapshot_error(reason: impl Into<String>) -> SpawnError {
    SpawnError::Snapshot {
        reason: reason.into(),
    }
}

/// A snapshot step's failure, naming the step.
fn step(what: &str, err: impl Display) -> SpawnError {
    snapshot_error(format!("{what}: {err}"))
}

/// The device a weak reference names, or the failure to save it.
fn alive<T>(weak: &Weak<T>, name: &str) -> Result<Arc<T>, SpawnError> {
    weak.upgrade()
        .ok_or_else(|| snapshot_error(format!("the {name} device has stopped")))
}

/// The state of one virtio transport the VM has, or `None` for a device it does not have.
fn save_transport(
    weak: Option<&Weak<Mutex<VirtioMmioTransport>>>,
    name: &str,
) -> Result<Option<MmioDeviceState>, SpawnError> {
    let Some(weak) = weak else {
        return Ok(None);
    };
    let transport = alive(weak, name)?;
    let state = lock(&transport).save_state();
    Ok(Some(state))
}

/// The VM's own state. x86_64 saves its irqchip, clock and PIT directly.
#[cfg(target_arch = "x86_64")]
fn save_vm_state(vm: &KvmVm, _vcpus: &[VcpuState]) -> Result<VmState, KvmVmError> {
    vm.save_state()
}

/// The VM's own state. aarch64 saves its GIC per vCPU, keyed by the vCPUs' MPIDRs.
#[cfg(target_arch = "aarch64")]
fn save_vm_state(vm: &KvmVm, vcpus: &[VcpuState]) -> Result<VmState, KvmVmError> {
    vm.save_state(&crate::arch::aarch64::vcpu::kvm_mpidrs(vcpus))
}

#[cfg(test)]
mod tests {
    use super::*;
    // The KVM-backed tests below are x86_64 only (an aarch64 VM needs its GIC set up first), so
    // their imports are too.
    #[cfg(target_arch = "x86_64")]
    use crate::vstate::kvm::Kvm;
    #[cfg(target_arch = "x86_64")]
    use crate::vstate::memory::{GuestAddress, anonymous};
    #[cfg(target_arch = "x86_64")]
    use crate::vstate::snapshot::{STATE_FILE, read_state};
    #[cfg(target_arch = "x86_64")]
    use crate::vstate::snapshot_memory::layout as memory_layout;

    /// A control whose VM is gone: nothing can upgrade the weak reference.
    fn stopped() -> InProcessControl {
        InProcessControl::new(
            Weak::new(),
            VmDevices {
                serial: Weak::new(),
                vsock: None,
                workspace: None,
                volumes: Vec::new(),
                balloon: None,
            },
            DeviceGate::default(),
            0,
            1,
            None,
        )
    }

    fn assert_stopped(result: Result<(), SpawnError>) {
        match result {
            Err(SpawnError::Snapshot { reason }) => assert_eq!(reason, VM_STOPPED),
            other => panic!("expected the VM-stopped error, got {other:?}"),
        }
    }

    #[test]
    fn a_stopped_vm_fails_every_call() {
        let control = stopped();
        assert_stopped(control.pause());
        assert_stopped(control.resume());
        assert_stopped(control.reclaim().map(|_| ()));
        assert_stopped(
            control
                .snapshot(Path::new("/nonexistent/apz-control"), SnapshotKind::Full)
                .map(|_| ()),
        );
    }

    /// A scratch directory unique to this test run, removed by the caller.
    #[cfg(target_arch = "x86_64")]
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-control-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(target_arch = "x86_64")]
    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A UART the VM's control reaches, as the builder shares it.
    #[cfg(target_arch = "x86_64")]
    fn serial() -> Arc<Mutex<SerialDevice>> {
        use crate::devices::legacy::{SerialOut, SerialOutInner};
        Arc::new(Mutex::new(
            SerialDevice::new(SerialOut::new(SerialOutInner::Sink)).unwrap(),
        ))
    }

    /// A KVM VM with its irqchip and `len` bytes of anonymous guest memory at guest address 0, and
    /// no vCPUs: enough to save a VM without a guest running in it.
    #[cfg(target_arch = "x86_64")]
    fn vm_without_vcpus(len: usize) -> Arc<KvmVm> {
        let mut vm = KvmVm::new(Kvm::new().unwrap()).unwrap();
        vm.setup_irqchip().unwrap();
        vm.register_dram_memory_regions(anonymous(&[(GuestAddress(0), len)]).unwrap())
            .unwrap();
        Arc::new(vm)
    }

    #[cfg(target_arch = "x86_64")]
    fn control_of(
        vm: &Arc<KvmVm>,
        serial: &Arc<Mutex<SerialDevice>>,
        restored_from: Option<File>,
    ) -> InProcessControl {
        InProcessControl::new(
            Arc::downgrade(vm),
            VmDevices {
                serial: Arc::downgrade(serial),
                vsock: None,
                workspace: None,
                volumes: Vec::new(),
                balloon: None,
            },
            DeviceGate::default(),
            0,
            1,
            restored_from,
        )
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn pause_and_resume_are_idempotent_and_track_the_flag() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_without_vcpus(1 << 20);
        let control = control_of(&vm, &serial(), None);
        control.pause().unwrap();
        control.pause().unwrap();
        assert!(control.paused.load(Ordering::SeqCst));
        control.resume().unwrap();
        control.resume().unwrap();
        assert!(!control.paused.load(Ordering::SeqCst));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn reclaim_is_refused_while_running_and_advises_every_region_when_paused() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_without_vcpus(1 << 20);
        let control = control_of(&vm, &serial(), None);
        assert!(control.reclaim().is_err(), "a running VM is not paged out");
        control.pause().unwrap();
        assert_eq!(control.reclaim().unwrap(), 1 << 20);
        control.resume().unwrap();
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_booted_vm_snapshot_writes_private_state_and_memory_and_stays_paused() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_without_vcpus(1 << 20);
        let serial = serial();
        serial.lock().unwrap().queue_input(b"ab").unwrap();
        let control = control_of(&vm, &serial, None);
        let root = scratch("booted");
        let dir = root.join("snap");

        // A diff asked of a booted VM is a full snapshot: the same pages either way.
        let info = control.snapshot(&dir, SnapshotKind::Diff).unwrap();
        assert_eq!(info.kind, SnapshotKind::Full);
        assert_eq!(info.dir, dir);
        assert!(
            control.paused.load(Ordering::SeqCst),
            "the VM is left paused"
        );

        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(STATE_FILE)), 0o600);
        assert_eq!(mode(&dir.join(MEMORY_FILE)), 0o600);
        let state = read_state(&dir).unwrap();
        assert_eq!(state.meta.kind, SnapshotKind::Full);
        assert_eq!(state.meta.memory_written, info.memory_written);
        assert_eq!(state.meta.regions, memory_layout(vm.guest_memory()));
        assert!(state.vcpus.is_empty());
        assert!(state.devices.vsock.is_none() && state.devices.balloon.is_none());
        assert_eq!(state.devices.serial.in_buffer, b"ab");
        // The memory file spans the whole layout, even where nothing was written.
        assert_eq!(fs::metadata(dir.join(MEMORY_FILE)).unwrap().len(), 1 << 20);

        control.resume().unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    /// A snapshot records the guest's resident pages as its working-set hint. A reclaim records them
    /// before it pages them out, and the next snapshot takes that record and clears it.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_snapshot_records_the_resident_pages_and_a_reclaim_keeps_them() {
        use crate::vstate::memory::Bytes;
        use crate::vstate::snapshot::{WORKING_SET_FILE, read_working_set};
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_without_vcpus(1 << 20);
        let serial = serial();
        let control = control_of(&vm, &serial, None);
        // Write the fifth page: it is resident; the rest of the region is never touched.
        vm.guest_memory()
            .write_slice(&[0xAA; 4096], GuestAddress(4 * 4096))
            .unwrap();
        let root = scratch("working-set");
        let dir = root.join("snap");

        control.pause().unwrap();
        control.reclaim().unwrap();
        control.snapshot(&dir, SnapshotKind::Full).unwrap();
        let runs = read_working_set(&dir);
        assert!(
            runs.iter()
                .any(|&(offset, len)| offset <= 4 * 4096 && 5 * 4096 <= offset + len),
            "the touched page is in the working set: {runs:?}"
        );
        assert_eq!(mode(&dir.join(WORKING_SET_FILE)), 0o600);
        assert!(
            control.reclaimed_working_set.lock().unwrap().is_none(),
            "the snapshot takes the set the reclaim kept"
        );
        control.resume().unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_restored_vm_snapshot_is_a_diff_over_a_copy_of_its_base() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_without_vcpus(1 << 20);
        let serial = serial();
        let root = scratch("restored");
        let base_dir = root.join("base");
        let base_control = control_of(&vm, &serial, None);
        base_control
            .snapshot(&base_dir, SnapshotKind::Full)
            .unwrap();
        let base_file = base_dir.join(MEMORY_FILE);
        let base_len = fs::metadata(&base_file).unwrap().len();

        // The same VM, as if restored from that base: a full request still gives a diff.
        let control = control_of(&vm, &serial, Some(File::open(&base_file).unwrap()));
        let next_dir = root.join("next");
        let info = control.snapshot(&next_dir, SnapshotKind::Full).unwrap();
        assert_eq!(info.kind, SnapshotKind::Diff);
        assert_eq!(read_state(&next_dir).unwrap().meta.kind, SnapshotKind::Diff);
        assert_eq!(
            fs::metadata(next_dir.join(MEMORY_FILE)).unwrap().len(),
            base_len
        );
        assert_eq!(mode(&next_dir.join(MEMORY_FILE)), 0o600);

        // Snapshotting over the base itself would truncate the pages the VM still maps: refused,
        // and the base is left as it was.
        let err = control
            .snapshot(&base_dir, SnapshotKind::Diff)
            .map(|_| ())
            .unwrap_err();
        match err {
            SpawnError::Snapshot { reason } => {
                assert!(reason.contains("restored from"), "{reason}");
            }
            other => panic!("expected a snapshot error, got {other:?}"),
        }
        assert_eq!(fs::metadata(&base_file).unwrap().len(), base_len);

        control.resume().unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    /// A capture is a point-in-time copy: the guest writes after the resume, and the memory file that
    /// `write_captured` writes still holds the page from before it. The state file waits for the
    /// write, and a second capture is refused while the first is unwritten.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_capture_is_written_after_the_resume_with_the_memory_it_held() {
        use crate::vstate::memory::Bytes;
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_without_vcpus(1 << 20);
        let serial = serial();
        let control = control_of(&vm, &serial, None);
        vm.guest_memory()
            .write_slice(&[0xAA; 4096], GuestAddress(4 * 4096))
            .unwrap();
        let root = scratch("capture");
        let dir = root.join("snap");

        let info = control.capture(&dir, SnapshotKind::Full).unwrap();
        assert_eq!(info.kind, SnapshotKind::Full);
        assert!(
            control.paused.load(Ordering::SeqCst),
            "the VM is left paused"
        );
        assert!(
            !dir.join(STATE_FILE).exists(),
            "the state file waits for the write"
        );
        let other = root.join("other");
        assert!(
            control.capture(&other, SnapshotKind::Full).is_err(),
            "a capture is refused while the first is unwritten"
        );
        assert!(!other.exists(), "a refused capture writes nothing");

        control.resume().unwrap();
        vm.guest_memory()
            .write_slice(&[0xBB; 4096], GuestAddress(4 * 4096))
            .unwrap();
        assert_eq!(control.write_captured().unwrap(), info.memory_written);
        assert_eq!(
            control.write_captured().unwrap(),
            0,
            "nothing is left to write"
        );

        let memory = fs::read(dir.join(MEMORY_FILE)).unwrap();
        assert_eq!(memory.len(), 1 << 20);
        assert!(
            memory[4 * 4096..5 * 4096].iter().all(|&b| b == 0xAA),
            "the captured page is the one from before the resume"
        );
        let state = read_state(&dir).unwrap();
        assert_eq!(state.meta.memory_written, info.memory_written);
        assert_eq!(state.devices.serial.in_buffer, Vec::<u8>::new());
        assert_eq!(mode(&dir.join(MEMORY_FILE)), 0o600);
        fs::remove_dir_all(&root).unwrap();
    }
}
