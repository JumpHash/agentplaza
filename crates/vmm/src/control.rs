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
//! **What it does not hold.** The control keeps the VM and its devices *weakly*: the VM's exit
//! watcher owns them, and a control that kept them alive would keep a stopped VM's memory mapped.
//! Every call on a stopped VM fails with "the VM has stopped".

use std::fmt::Display;
use std::fs::{self, File, OpenOptions, Permissions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Instant;

use crate::clonefile::clone_file;
use crate::devices::legacy::SerialDevice;
use crate::devices::virtio::mmio::{MmioDeviceState, VirtioMmioTransport};
use crate::spawner::{SnapshotInfo, SnapshotKind, SpawnError, VmControl};
use crate::vstate::bus::lock;
use crate::vstate::snapshot::{
    self, DeviceStates, MEMORY_FILE, RegionMeta, SerialRegisters, VcpuState, VmSnapshot, VmState,
    meta_now, write_state,
};
use crate::vstate::snapshot_memory::{layout, write_memory};
use crate::vstate::vm::{KvmVm, KvmVmError};

/// What every call on a VM that has stopped fails with.
const VM_STOPPED: &str = "the VM has stopped";

/// The VM's device gate: the vsock event thread holds the read half while it processes, a snapshot
/// holds the write half from before the device state is saved until the memory is written.
pub type DeviceGate = Arc<RwLock<()>>;

/// The VM's devices, as the control reaches them (weakly; see the module docs).
pub struct VmDevices {
    pub serial: Weak<Mutex<SerialDevice>>,
    pub vsock: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub workspace: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub balloon: Option<Weak<Mutex<VirtioMmioTransport>>>,
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
        let guard = lock(&serial);
        let state = guard.serial.state();
        Ok(SerialRegisters {
            baud_divisor_low: state.baud_divisor_low,
            baud_divisor_high: state.baud_divisor_high,
            interrupt_enable: state.interrupt_enable,
            interrupt_identification: state.interrupt_identification,
            line_control: state.line_control,
            line_status: state.line_status,
            modem_control: state.modem_control,
            modem_status: state.modem_status,
            scratch: state.scratch,
            in_buffer: state.in_buffer,
        })
    }

    /// The state of the device set: the serial console always, each virtio transport the VM has.
    fn save_devices(&self) -> Result<DeviceStates, SpawnError> {
        Ok(DeviceStates {
            serial: self.save_serial()?,
            vsock: save_transport(self.devices.vsock.as_ref(), "vsock")?,
            workspace: save_transport(self.devices.workspace.as_ref(), "workspace")?,
            balloon: save_transport(self.devices.balloon.as_ref(), "balloon")?,
        })
    }

    /// Writes the memory file of the snapshot in `dir`, returning its kind and the bytes written.
    ///
    /// A VM restored from a file gets a reflinked copy of that file with its changed pages written
    /// over it (a diff); a booted VM gets every page it touched (full). See the module docs.
    fn save_memory(
        &self,
        vm: &KvmVm,
        dir: &Path,
        regions: &[RegionMeta],
    ) -> Result<(SnapshotKind, u64), SpawnError> {
        let path = dir.join(MEMORY_FILE);
        let (kind, file) = match &self.restored_from {
            Some(base) => {
                // The base is the file the VM still maps: copying onto it would truncate the pages
                // the VM has not read yet.
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
                // The copy goes through the open descriptor, so it works after the base was unlinked.
                let source = PathBuf::from(format!("/proc/self/fd/{}", base.as_raw_fd()));
                clone_file(&source, &path)
                    .map_err(|err| step("copy the restored memory file", err))?;
                fs::set_permissions(&path, Permissions::from_mode(0o600))
                    .map_err(|err| step("set the memory file's mode", err))?;
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .map_err(|err| step("open the memory file", err))?;
                (SnapshotKind::Diff, file)
            }
            None => {
                let file = snapshot::private_file(&path)
                    .map_err(|err| step("create the memory file", err))?;
                (SnapshotKind::Full, file)
            }
        };
        let written = write_memory(vm.guest_memory(), regions, &file, kind)
            .map_err(|err| step("write the guest memory", err))?;
        Ok((kind, written))
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
    /// so an interrupted snapshot leaves no state for a restore to read.
    fn snapshot(&self, dir: &Path, _kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        let _busy = lock(&self.busy);
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
        let (kind, memory_written) = self.save_memory(&vm, dir, &regions)?;

        let mut meta = meta_now(kind, self.vcpus, self.memory_mib, regions);
        meta.memory_written = memory_written;
        let state = VmSnapshot {
            meta,
            vm: vm_state,
            vcpus,
            devices,
        };
        write_state(dir, &state).map_err(|err| step("write the state file", err))?;

        Ok(SnapshotInfo {
            dir: dir.to_path_buf(),
            kind,
            memory_written,
            paused_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
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
    let mpidrs: Vec<u64> = vcpus.iter().map(|vcpu| vcpu.mpidr).collect();
    vm.save_state(&mpidrs)
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
                balloon: None,
            },
            Arc::new(RwLock::new(())),
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
        assert_stopped(
            control
                .snapshot(Path::new("/nonexistent/apz-control"), SnapshotKind::Full)
                .map(|_| ()),
        );
    }

    /// A scratch directory unique to this test run, removed by the caller.
    #[cfg(target_arch = "x86_64")]
    fn scratch(name: &str) -> PathBuf {
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
                balloon: None,
            },
            Arc::new(RwLock::new(())),
            0,
            1,
            restored_from,
        )
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn pause_and_resume_are_idempotent_and_track_the_flag() {
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
    fn a_booted_vm_snapshot_writes_private_state_and_memory_and_stays_paused() {
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

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_restored_vm_snapshot_is_a_diff_over_a_copy_of_its_base() {
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
}
