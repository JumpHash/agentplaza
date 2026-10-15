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

use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock, Weak};

use crate::devices::legacy::SerialDevice;
use crate::devices::virtio::mmio::VirtioMmioTransport;
use crate::spawner::{SnapshotInfo, SnapshotKind, SpawnError, VmControl};
use crate::vstate::vm::KvmVm;

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
        }
    }
}

impl VmControl for InProcessControl {
    fn pause(&self) -> Result<(), SpawnError> {
        todo!("in-process pause")
    }

    fn resume(&self) -> Result<(), SpawnError> {
        todo!("in-process resume")
    }

    fn snapshot(&self, dir: &Path, kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        let _ = (
            dir,
            kind,
            &self.vm,
            &self.devices,
            &self.gate,
            self.vcpus,
            self.memory_mib,
            &self.restored_from,
            &self.busy,
        );
        todo!("in-process snapshot")
    }
}
