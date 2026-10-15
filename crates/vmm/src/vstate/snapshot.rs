// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The state it carries is Firecracker's (`VmState`, `VcpuState`, the virtio
// transport and queue state, ported from src/vmm/src/{persist.rs,arch/*,devices/virtio/persist.rs});
// the on-disk layout below is this port's own.

//! VM snapshots on disk.
//!
//! A snapshot is a **directory** the caller names:
//!
//! | file | what it holds |
//! |---|---|
//! | [`STATE_FILE`] | [`VmSnapshot`] as JSON: the format version, the VMM and architecture it was taken on, the guest memory layout, the VM's and every vCPU's KVM state, and the device state |
//! | [`MEMORY_FILE`] | the guest memory, each region at its [`RegionMeta::offset`]; pages the guest never touched are holes, so the file is sparse |
//!
//! A snapshot is only restored by the VMM that wrote it: [`read_state`] refuses another format,
//! another VMM version or another architecture ([`SnapshotError::Incompatible`]) rather than restore
//! state whose layout may have changed — a mismatch would corrupt the guest silently.
//!
//! The memory file is restored by mapping it **private** (`MAP_PRIVATE`), so a restored guest reads
//! the file's pages lazily and its writes stay in its own anonymous memory. That is also what makes a
//! [`SnapshotKind::Diff`] cheap: the next snapshot of a restored VM only writes the pages it changed
//! (its anonymous pages), over a reflinked copy of the memory file it was restored from
//! (`vstate::snapshot_memory`). The workspace volume is not part of the snapshot: the caller copies
//! (reflinks) it beside the snapshot while the VM is paused, and hands the copy back on restore.

#[cfg(unix)]
use std::fs::{self, File};
use std::io;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use std::io::Write;
#[cfg(unix)]
use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use crate::devices::virtio::mmio::MmioDeviceState;
use crate::spawner::SnapshotKind;

/// The format this VMM writes and reads. Bumped whenever a state struct changes shape.
pub const FORMAT: u32 = 1;
/// The state file in a snapshot directory.
pub const STATE_FILE: &str = "vmstate.json";
/// The guest memory file in a snapshot directory.
pub const MEMORY_FILE: &str = "memory.img";

/// The architecture snapshots are taken and restored on.
#[cfg(target_arch = "x86_64")]
pub const ARCH: &str = "x86_64";
/// The architecture snapshots are taken and restored on.
#[cfg(target_arch = "aarch64")]
pub const ARCH: &str = "aarch64";
/// The architecture snapshots are taken and restored on (none here: this build takes no snapshots).
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const ARCH: &str = "unsupported";

/// The architecture's vCPU state.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub use crate::arch::aarch64::vcpu::VcpuState;
/// The architecture's VM state.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub use crate::arch::aarch64::vm::VmState;
/// The architecture's vCPU state (registers, MSRs, LAPIC, … on x86_64; the core registers on aarch64).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use crate::arch::x86_64::vcpu::VcpuState;
/// The architecture's VM state (irqchip, clock, PIT on x86_64; the GIC on aarch64).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use crate::arch::x86_64::vm::VmState;

/// One guest memory region and where it sits in [`MEMORY_FILE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionMeta {
    /// The region's guest physical address.
    pub guest_addr: u64,
    /// The region's length in bytes.
    pub len: u64,
    /// The region's offset in the memory file.
    pub offset: u64,
}

/// The hypervisor this VMM's snapshots are taken on: each keeps its own vCPU and interrupt
/// controller state, so a snapshot restores only on the backend that wrote it.
#[cfg(target_os = "linux")]
pub const BACKEND: &str = "kvm";
#[cfg(windows)]
pub const BACKEND: &str = "whp";
#[cfg(target_os = "macos")]
pub const BACKEND: &str = "hvf";
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub const BACKEND: &str = "unsupported";

fn kvm() -> String {
    "kvm".to_string()
}

/// What a snapshot is and what it was taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// [`FORMAT`] when written.
    pub format: u32,
    /// [`BACKEND`] when written. Snapshots from before backends other than KVM took them are KVM's.
    #[serde(default = "kvm")]
    pub backend: String,
    /// The `vmm` crate version that wrote it.
    pub vmm_version: String,
    /// [`ARCH`] when written.
    pub arch: String,
    pub kind: SnapshotKind,
    /// When the snapshot was taken, in milliseconds since the Unix epoch.
    pub taken_at_ms: u64,
    pub vcpus: u8,
    pub memory_mib: usize,
    /// The guest memory layout, in guest-address order.
    pub regions: Vec<RegionMeta>,
    /// The bytes of guest memory this snapshot wrote (a diff writes only the changed pages).
    pub memory_written: u64,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
/// The state of the VM's devices. A device the VM does not have is `None`; a restore refuses a
/// snapshot whose devices do not match the spec it is restored with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceStates {
    /// The legacy serial console's UART registers (`vm_superio::serial::SerialState`'s fields).
    pub serial: SerialRegisters,
    pub vsock: Option<MmioDeviceState>,
    pub workspace: Option<MmioDeviceState>,
    pub balloon: Option<MmioDeviceState>,
    /// The shared data volumes' transports, in the spec's order (`VmSpec::volumes`). Older snapshots
    /// have none.
    #[serde(default)]
    pub volumes: Vec<MmioDeviceState>,
}

/// The UART's registers, mirrored from `vm_superio::serial::SerialState` (which has no serde).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialRegisters {
    pub baud_divisor_low: u8,
    pub baud_divisor_high: u8,
    pub interrupt_enable: u8,
    pub interrupt_identification: u8,
    pub line_control: u8,
    pub line_status: u8,
    pub modem_control: u8,
    pub modem_status: u8,
    pub scratch: u8,
    pub in_buffer: Vec<u8>,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
/// Everything [`STATE_FILE`] holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VmSnapshot {
    pub meta: SnapshotMeta,
    pub vm: VmState,
    /// One per vCPU, in index order.
    pub vcpus: Vec<VcpuState>,
    pub devices: DeviceStates,
}

/// Errors reading or writing a snapshot directory.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// An I/O error on a snapshot file.
    #[error("snapshot {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    /// The state file is not valid JSON of this format.
    #[error("snapshot state {path} is malformed: {source}")]
    Malformed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    /// The snapshot was written by another format, VMM version or architecture.
    #[error("snapshot is incompatible: {0}")]
    Incompatible(String),
    /// The snapshot does not fit the VM it is restored into (devices, vCPUs, memory).
    #[error("snapshot does not match the VM: {0}")]
    Mismatch(String),
    /// Saving or restoring a piece of state failed.
    #[error("{0}")]
    State(String),
}

impl SnapshotError {
    #[cfg(unix)]
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// The meta of a snapshot taken now on this VMM.
pub fn meta_now(
    kind: SnapshotKind,
    vcpus: u8,
    memory_mib: usize,
    regions: Vec<RegionMeta>,
) -> SnapshotMeta {
    let taken_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    SnapshotMeta {
        format: FORMAT,
        backend: BACKEND.to_string(),
        vmm_version: env!("CARGO_PKG_VERSION").to_string(),
        arch: ARCH.to_string(),
        kind,
        taken_at_ms,
        vcpus,
        memory_mib,
        regions,
        memory_written: 0,
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
/// Writes [`STATE_FILE`] into `dir` (created if missing, mode 0700): to a temporary name first, then
/// renamed, so a crash never leaves a half-written state beside a complete memory file. The file is
/// mode 0600: guest state is the agent's data.
pub fn write_state(dir: &Path, snapshot: &VmSnapshot) -> Result<(), SnapshotError> {
    create_private_dir(dir)?;
    let path = dir.join(STATE_FILE);
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    let bytes = serde_json::to_vec(snapshot).map_err(|source| SnapshotError::Malformed {
        path: path.display().to_string(),
        source,
    })?;
    let mut file = private_file(&tmp)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| SnapshotError::io(&tmp, err))?;
    fs::rename(&tmp, &path).map_err(|err| SnapshotError::io(&path, err))
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
/// Reads [`STATE_FILE`] from `dir` and refuses a snapshot this VMM cannot restore.
pub fn read_state(dir: &Path) -> Result<VmSnapshot, SnapshotError> {
    let path = dir.join(STATE_FILE);
    let bytes = fs::read(&path).map_err(|err| SnapshotError::io(&path, err))?;
    let snapshot: VmSnapshot =
        serde_json::from_slice(&bytes).map_err(|source| SnapshotError::Malformed {
            path: path.display().to_string(),
            source,
        })?;
    check_compatible(&snapshot.meta)?;
    Ok(snapshot)
}

/// Refuses a snapshot from another format, VMM version or architecture.
pub fn check_compatible(meta: &SnapshotMeta) -> Result<(), SnapshotError> {
    if meta.format != FORMAT {
        return Err(SnapshotError::Incompatible(format!(
            "format {} (this VMM reads {FORMAT})",
            meta.format
        )));
    }
    if meta.vmm_version != env!("CARGO_PKG_VERSION") {
        return Err(SnapshotError::Incompatible(format!(
            "taken by vmm {} (this is {})",
            meta.vmm_version,
            env!("CARGO_PKG_VERSION")
        )));
    }
    if meta.backend != BACKEND {
        return Err(SnapshotError::Incompatible(format!(
            "taken on {} (this VMM runs on {BACKEND})",
            meta.backend
        )));
    }
    if meta.arch != ARCH {
        return Err(SnapshotError::Incompatible(format!(
            "taken on {} (this is {ARCH})",
            meta.arch
        )));
    }
    Ok(())
}

#[cfg(unix)]
/// Creates `dir` (and its parents) with mode 0700 for the leaf.
pub fn create_private_dir(dir: &Path) -> Result<(), SnapshotError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|err| SnapshotError::io(dir, err))?;
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|err| SnapshotError::io(dir, err))
}

#[cfg(unix)]
/// Creates (truncating) `path` with mode 0600.
pub fn private_file(path: &Path) -> Result<File, SnapshotError> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| SnapshotError::io(path, err))
}
