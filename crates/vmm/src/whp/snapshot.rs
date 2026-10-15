//! Snapshots of WHP VMs: the vCPUs' registers and hypervisor state, the guest memory as slices, and
//! the VM's pause/resume/snapshot control. The state's shape and the memory file are
//! [`super::snapshot_state`]'s.
//!
//! A vCPU's state is three reads of the paused processor: its registers by name (the list QEMU's
//! WHPX accelerator moves, plus the TSC, XCR0, the APIC base and the activity and pending-event
//! registers), its local APIC (`InterruptControllerState2`) and its XSAVE area. A restore writes
//! them back in the order the hypervisor needs: the registers (XCR0 and CR4 first enable the XSAVE
//! components), then the XSAVE area, then the local APIC (whose base the registers set).
//!
//! WHP has no dirty-page log and the guest memory is anonymous, so every snapshot is a full one:
//! every page that is not zero.
//!
//! Not run on hardware here: WHP needs a Windows host with the Windows Hypervisor Platform. The
//! pure half's tests run everywhere; this half is compile-checked for `x86_64-pc-windows-msvc`.
#![allow(unsafe_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Hypervisor::*;

use super::snapshot_state::{WhpSnapshot, WhpVcpuState, write_memory, write_state};
use super::vcpu::WhpVm;
use super::vm::Partition;
use crate::devices::legacy::SerialDevice;
use crate::devices::virtio::mmio::VirtioMmioTransport;
use crate::devices::virtio::vsock::event_thread::DeviceGate;
use crate::spawner::{SnapshotInfo, SnapshotKind, SpawnError, VmControl};
use crate::vstate::memory::GuestMemoryMmap;
use crate::vstate::snapshot::meta_now;

/// How long a pause waits for every vCPU to park.
const PAUSE_LIMIT: Duration = Duration::from_secs(5);

/// The registers every vCPU's state is moved by, written back in this order in one call.
const REGISTERS: &[WHV_REGISTER_NAME] = &[
    WHvX64RegisterRax,
    WHvX64RegisterRcx,
    WHvX64RegisterRdx,
    WHvX64RegisterRbx,
    WHvX64RegisterRsp,
    WHvX64RegisterRbp,
    WHvX64RegisterRsi,
    WHvX64RegisterRdi,
    WHvX64RegisterR8,
    WHvX64RegisterR9,
    WHvX64RegisterR10,
    WHvX64RegisterR11,
    WHvX64RegisterR12,
    WHvX64RegisterR13,
    WHvX64RegisterR14,
    WHvX64RegisterR15,
    WHvX64RegisterRip,
    WHvX64RegisterRflags,
    WHvX64RegisterEs,
    WHvX64RegisterCs,
    WHvX64RegisterSs,
    WHvX64RegisterDs,
    WHvX64RegisterFs,
    WHvX64RegisterGs,
    WHvX64RegisterLdtr,
    WHvX64RegisterTr,
    WHvX64RegisterIdtr,
    WHvX64RegisterGdtr,
    WHvX64RegisterCr0,
    WHvX64RegisterCr2,
    WHvX64RegisterCr3,
    WHvX64RegisterCr4,
    WHvX64RegisterCr8,
    WHvX64RegisterXCr0,
    WHvX64RegisterEfer,
    WHvX64RegisterKernelGsBase,
    WHvX64RegisterApicBase,
    WHvX64RegisterPat,
    WHvX64RegisterSysenterCs,
    WHvX64RegisterSysenterEip,
    WHvX64RegisterSysenterEsp,
    WHvX64RegisterStar,
    WHvX64RegisterLstar,
    WHvX64RegisterCstar,
    WHvX64RegisterSfmask,
];

/// Registers that are part of the processor's state but that a hypervisor may refuse to write on
/// some hosts or in some states (the debug registers, TSC_AUX without RDTSCP, an event already
/// injected, a processor still waiting for its SIPI). Written one by one after the others; a
/// refusal is logged and the restore goes on. The TSC is here too: a guest whose TSC was not put
/// back sees time jump, which it survives (the plaza sets its clock after every restore).
const SOFT_REGISTERS: &[WHV_REGISTER_NAME] = &[
    WHvX64RegisterTsc,
    WHvX64RegisterTscAux,
    WHvX64RegisterDr0,
    WHvX64RegisterDr1,
    WHvX64RegisterDr2,
    WHvX64RegisterDr3,
    WHvX64RegisterDr6,
    WHvX64RegisterDr7,
    WHvRegisterInternalActivityState,
    WHvRegisterInterruptState,
    WHvRegisterPendingInterruption,
    WHvRegisterPendingEvent,
];

// A register value is a 128-bit union; the state keeps it as two halves.
const _: () = assert!(std::mem::size_of::<WHV_REGISTER_VALUE>() == 16);

fn to_halves(value: &WHV_REGISTER_VALUE) -> [u64; 2] {
    // SAFETY: `WHV_REGISTER_VALUE` is 16 bytes (checked above), every bit pattern of which is a
    // valid `[u64; 2]`; the read is unaligned-safe.
    unsafe {
        (value as *const WHV_REGISTER_VALUE)
            .cast::<[u64; 2]>()
            .read_unaligned()
    }
}

fn from_halves(halves: [u64; 2]) -> WHV_REGISTER_VALUE {
    let mut value = WHV_REGISTER_VALUE::default();
    // SAFETY: as above: the union is 16 plain bytes, written whole.
    unsafe {
        (&mut value as *mut WHV_REGISTER_VALUE)
            .cast::<[u64; 2]>()
            .write_unaligned(halves);
    }
    value
}

/// Reads paused processor `index`'s state.
pub fn save_vcpu(partition: &Partition, index: u32) -> Result<WhpVcpuState, String> {
    let names: Vec<WHV_REGISTER_NAME> = REGISTERS.iter().chain(SOFT_REGISTERS).copied().collect();
    let mut values = vec![WHV_REGISTER_VALUE::default(); names.len()];
    partition
        .get_registers(index, &names, &mut values)
        .map_err(|err| format!("vCPU {index}: the registers: {err}"))?;
    let registers = names
        .iter()
        .zip(&values)
        .map(|(name, value)| (*name, to_halves(value)))
        .collect();
    let lapic = partition
        .get_state(index, WHvVirtualProcessorStateTypeInterruptControllerState2)
        .map_err(|err| format!("vCPU {index}: the local APIC: {err}"))?;
    let xsave = partition
        .get_state(index, WHvVirtualProcessorStateTypeXsaveState)
        .map_err(|err| format!("vCPU {index}: the XSAVE area: {err}"))?;
    Ok(WhpVcpuState {
        registers,
        lapic,
        xsave,
    })
}

/// Writes processor `index`'s state back, before it first runs.
pub fn restore_vcpu(partition: &Partition, index: u32, state: &WhpVcpuState) -> Result<(), String> {
    let (hard, soft): (Vec<&(i32, [u64; 2])>, Vec<&(i32, [u64; 2])>) = state
        .registers
        .iter()
        .partition(|(name, _)| !SOFT_REGISTERS.contains(name));
    let names: Vec<WHV_REGISTER_NAME> = hard.iter().map(|entry| entry.0).collect();
    let values: Vec<WHV_REGISTER_VALUE> = hard.iter().map(|entry| from_halves(entry.1)).collect();
    partition
        .set_registers(index, &names, &values)
        .map_err(|err| format!("vCPU {index}: the registers: {err}"))?;
    partition
        .set_state(index, WHvVirtualProcessorStateTypeXsaveState, &state.xsave)
        .map_err(|err| format!("vCPU {index}: the XSAVE area: {err}"))?;
    partition
        .set_state(
            index,
            WHvVirtualProcessorStateTypeInterruptControllerState2,
            &state.lapic,
        )
        .map_err(|err| format!("vCPU {index}: the local APIC: {err}"))?;
    for &&(name, value) in &soft {
        if let Err(err) = partition.set_registers(index, &[name], &[from_halves(value)]) {
            eprintln!("whp: vCPU {index}: register {name:#x} not restored: {err}");
        }
    }
    Ok(())
}

/// The guest's RAM regions as `(guest address, bytes)`.
///
/// # Safety
///
/// The caller holds `memory` (and so the mappings) for as long as it uses the slices, and the
/// vCPUs are paused (or not started), so nothing writes the memory meanwhile.
unsafe fn regions(memory: &GuestMemoryMmap) -> Vec<(u64, &[u8])> {
    use vm_memory::{Address, GuestMemoryBackend, GuestMemoryRegion};
    memory
        .iter()
        .map(|region| {
            // SAFETY: the region is a committed mapping of `len` bytes that `memory` owns (see the
            // function's contract).
            let bytes =
                unsafe { std::slice::from_raw_parts(region.as_ptr(), region.len() as usize) };
            (region.start_addr().raw_value(), bytes)
        })
        .collect()
}

/// Fills the guest's RAM from the snapshot in `dir`, before any vCPU runs.
pub fn fill_memory(
    memory: &GuestMemoryMmap,
    dir: &Path,
    snapshot: &WhpSnapshot,
) -> Result<(), String> {
    use vm_memory::{Address, GuestMemoryBackend, GuestMemoryRegion};
    let mut slices: Vec<(u64, &mut [u8])> = memory
        .iter()
        .map(|region| {
            // SAFETY: the region is a committed, writable mapping of `len` bytes that `memory` owns
            // and that nothing else touches yet: no vCPU has run and no device is attached.
            let bytes =
                unsafe { std::slice::from_raw_parts_mut(region.as_ptr(), region.len() as usize) };
            (region.start_addr().raw_value(), bytes)
        })
        .collect();
    super::snapshot_state::read_memory(dir, &snapshot.meta.regions, &mut slices)
        .map_err(|err| err.to_string())
}

/// The WHP VM's [`VmControl`]: weak handles on the VM and its devices (the exit watcher owns
/// them), and the vsock event thread's gate.
pub struct WhpControl {
    pub vm: Weak<WhpVm>,
    pub serial: Weak<Mutex<SerialDevice>>,
    pub vsock: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub workspace: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub volumes: Vec<Weak<Mutex<VirtioMmioTransport>>>,
    /// The data volumes' names, in the spec's order, recorded in the snapshot.
    pub volume_names: Vec<String>,
    pub gate: DeviceGate,
    pub vcpus: u8,
    pub memory_mib: usize,
    /// One snapshot at a time.
    pub busy: Mutex<()>,
    pub paused: AtomicBool,
}

fn failed(reason: impl Into<String>) -> SpawnError {
    SpawnError::Snapshot {
        reason: reason.into(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WhpControl {
    fn live_vm(&self) -> Result<std::sync::Arc<WhpVm>, SpawnError> {
        self.vm
            .upgrade()
            .ok_or_else(|| failed("the VM has stopped"))
    }

    fn pause_vm(&self, vm: &WhpVm) -> Result<(), SpawnError> {
        if self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        vm.pause_vcpus(PAUSE_LIMIT)
            .map_err(|err| failed(format!("pause the vCPUs: {err}")))?;
        self.paused.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl VmControl for WhpControl {
    fn pause(&self) -> Result<(), SpawnError> {
        let vm = self.live_vm()?;
        self.pause_vm(&vm)
    }

    fn resume(&self) -> Result<(), SpawnError> {
        let vm = self.live_vm()?;
        vm.resume_vcpus();
        self.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn snapshot(&self, dir: &Path, _kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        let _busy = lock(&self.busy);
        let vm = self.live_vm()?;
        let started = Instant::now();
        self.pause_vm(&vm)?;
        // Held to the end: the vsock event thread must not land a packet between the device state
        // and the memory dump.
        let _gate = self.gate.write().unwrap_or_else(PoisonError::into_inner);

        std::fs::create_dir_all(dir).map_err(|err| failed(format!("{}: {err}", dir.display())))?;
        let vcpus = (0..vm.vcpu_count)
            .map(|index| save_vcpu(&vm.partition, index))
            .collect::<Result<Vec<_>, String>>()
            .map_err(failed)?;
        let ioapic = vm.interrupts.ioapic();
        let serial = self
            .serial
            .upgrade()
            .ok_or_else(|| failed("the serial console is gone"))?;
        let serial = lock(&serial).registers();
        let save = |transport: &Weak<Mutex<VirtioMmioTransport>>, what: &str| {
            let transport = transport
                .upgrade()
                .ok_or_else(|| failed(format!("the {what} device is gone")))?;
            let state = lock(&transport).save_state();
            Ok::<_, SpawnError>(state)
        };
        let vsock = self.vsock.as_ref().map(|t| save(t, "vsock")).transpose()?;
        let workspace = self
            .workspace
            .as_ref()
            .map(|t| save(t, "workspace"))
            .transpose()?;
        let volumes = self
            .volumes
            .iter()
            .map(|t| save(t, "data volume"))
            .collect::<Result<Vec<_>, SpawnError>>()?;
        // SAFETY: `vm` holds the memory, and the vCPUs are paused and the devices gated.
        let (regions, memory_written) = write_memory(dir, &unsafe { regions(&vm.memory) })
            .map_err(|err| failed(format!("write the memory file: {err}")))?;
        let mut meta = meta_now(SnapshotKind::Full, self.vcpus, self.memory_mib, regions);
        meta.memory_written = memory_written;
        write_state(
            dir,
            &WhpSnapshot {
                meta,
                vcpus,
                ioapic,
                serial,
                vsock,
                workspace,
                volumes,
                volume_names: self.volume_names.clone(),
            },
        )
        .map_err(|err| failed(format!("write the state file: {err}")))?;
        Ok(SnapshotInfo {
            dir: dir.to_path_buf(),
            kind: SnapshotKind::Full,
            memory_written,
            paused_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}
