//! Snapshots of Hypervisor.framework VMs: the VM's pause/resume/snapshot control and the guest
//! memory as byte slices. The state's shape is [`super::snapshot_state`]'s, the memory file the
//! portable full one (`vstate::snapshot::write_memory_file`).
//!
//! Hypervisor.framework only lets a vCPU's own thread read or write its registers, so a snapshot is
//! a pause in which each vCPU thread reads its own state before it parks (`SharedVm::pause_vcpus`),
//! and a restore hands each thread its state to write before its first run. The virtio transports
//! are saved with the vCPUs parked (a block request runs on the vCPU that rang its doorbell, so none
//! is in flight) and the vsock event thread gated, as on WHP. The framework has no
//! dirty-page log and the guest memory is anonymous, so every snapshot is a full one.
//!
//! Not run on hardware here: this needs an Apple Silicon Mac. The pure half's tests run everywhere;
//! this half is compile-checked for `aarch64-apple-darwin`.
#![allow(unsafe_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use vm_memory::{Address, GuestMemoryBackend, GuestMemoryRegion};

use super::snapshot_state::{HvfSnapshot, write_state};
use super::vcpu::SharedVm;
use crate::devices::virtio::mmio::VirtioMmioTransport;
use crate::devices::virtio::vsock::event_thread::DeviceGate;
use crate::spawner::{SnapshotInfo, SnapshotKind, SpawnError, VmControl};
use crate::vstate::memory::GuestMemoryMmap;
use crate::vstate::snapshot::{meta_now, read_memory_file, write_memory_file};

/// How long a pause waits for every vCPU to park.
const PAUSE_LIMIT: Duration = Duration::from_secs(5);

/// The guest's RAM regions as `(guest address, bytes)`.
///
/// # Safety
///
/// The caller holds `memory` for as long as it uses the slices, and the vCPUs are paused.
unsafe fn regions(memory: &GuestMemoryMmap) -> Vec<(u64, &[u8])> {
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

/// Fills the guest's RAM from the snapshot in `dir`, before the VM maps it and any vCPU runs.
pub fn fill_memory(
    memory: &GuestMemoryMmap,
    dir: &Path,
    snapshot: &HvfSnapshot,
) -> Result<(), String> {
    let mut slices: Vec<(u64, &mut [u8])> = memory
        .iter()
        .map(|region| {
            // SAFETY: the region is a committed, writable mapping of `len` bytes that `memory` owns
            // and that nothing else touches yet.
            let bytes =
                unsafe { std::slice::from_raw_parts_mut(region.as_ptr(), region.len() as usize) };
            (region.start_addr().raw_value(), bytes)
        })
        .collect();
    read_memory_file(dir, &snapshot.meta.regions, &mut slices).map_err(|err| err.to_string())
}

/// The VM's [`VmControl`]: a weak handle on the shared VM state (the vCPU threads own it).
pub struct HvfControl {
    pub vm: Weak<SharedVm>,
    /// The virtio transports, weakly (the VM holds them).
    pub vsock: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub workspace: Option<Weak<Mutex<VirtioMmioTransport>>>,
    pub volumes: Vec<Weak<Mutex<VirtioMmioTransport>>>,
    /// The vsock event thread's gate: held for writing while the devices and memory are saved.
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

impl HvfControl {
    fn live_vm(&self) -> Result<std::sync::Arc<SharedVm>, SpawnError> {
        self.vm
            .upgrade()
            .ok_or_else(|| failed("the VM has stopped"))
    }
}

impl VmControl for HvfControl {
    fn pause(&self) -> Result<(), SpawnError> {
        let vm = self.live_vm()?;
        if !self.paused.load(Ordering::SeqCst) {
            vm.pause_vcpus(false, PAUSE_LIMIT)
                .map_err(|err| failed(format!("pause the vCPUs: {err}")))?;
            self.paused.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn resume(&self) -> Result<(), SpawnError> {
        let vm = self.live_vm()?;
        vm.resume_vcpus();
        self.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn snapshot(&self, dir: &Path, _kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        let _busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        let vm = self.live_vm()?;
        let started = Instant::now();
        // Each vCPU reads its own state as it parks (or, already parked, when asked).
        let vcpus = vm
            .pause_vcpus(true, PAUSE_LIMIT)
            .map_err(|err| failed(format!("pause the vCPUs: {err}")))?;
        self.paused.store(true, Ordering::SeqCst);
        // Held to the end: the vsock event thread must not land a packet between the device state
        // and the memory dump.
        let _gate = self.gate.write().unwrap_or_else(PoisonError::into_inner);
        let guest_counter = vm.guest_counter();
        let serial = vm
            .serial_registers()
            .ok_or_else(|| failed("the serial console is gone"))?;
        let save = |transport: &Weak<Mutex<VirtioMmioTransport>>, what: &str| {
            let transport: Arc<Mutex<VirtioMmioTransport>> = transport
                .upgrade()
                .ok_or_else(|| failed(format!("the {what} device is gone")))?;
            let state = transport
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .save_state();
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
        // The interrupt controller with the devices, before the memory dump: the relay still runs.
        let gic = vm.gic_state();
        std::fs::create_dir_all(dir).map_err(|err| failed(format!("{}: {err}", dir.display())))?;
        // SAFETY: `vm` holds the memory, and its vCPUs are paused.
        let (regions, memory_written) = write_memory_file(dir, &unsafe { regions(vm.memory()) })
            .map_err(|err| failed(format!("write the memory file: {err}")))?;
        let mut meta = meta_now(SnapshotKind::Full, self.vcpus, self.memory_mib, regions);
        meta.memory_written = memory_written;
        write_state(
            dir,
            &HvfSnapshot {
                meta,
                vcpus,
                running: vm.running(),
                guest_counter,
                gic,
                serial,
                vsock,
                workspace,
                volumes,
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
