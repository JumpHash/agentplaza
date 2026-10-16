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
//! | [`WORKING_SET_FILE`] | the memory ranges that were resident in host RAM when the snapshot was taken, as [`encode_runs`] writes them; a hint for the restore's read-ahead, not fsynced |
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
/// The working-set hint in a snapshot directory: the ranges of [`MEMORY_FILE`] that were resident in
/// host RAM when the snapshot was taken (see [`encode_runs`]). A restore reads them ahead; a missing or
/// malformed file is ignored, since it is only a hint.
pub const WORKING_SET_FILE: &str = "workingset.bin";
/// The most runs a [`WORKING_SET_FILE`] holds. A snapshot keeps the first runs it finds, in file
/// order, and drops the rest.
pub const MAX_WORKING_SET_RUNS: usize = 65_536;

/// A byte range of a memory file: `(offset, len)`.
pub type Run = (u64, u64);

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
/// mode 0600: guest state is the agent's data. With `durable` false the file is not fsynced (see
/// [`SnapshotKind::DiffUnsynced`]).
pub fn write_state(dir: &Path, snapshot: &VmSnapshot, durable: bool) -> Result<(), SnapshotError> {
    create_private_dir(dir)?;
    let path = dir.join(STATE_FILE);
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    let bytes = serde_json::to_vec(snapshot).map_err(|source| SnapshotError::Malformed {
        path: path.display().to_string(),
        source,
    })?;
    let mut file = private_file(&tmp)?;
    file.write_all(&bytes)
        .and_then(|()| if durable { file.sync_all() } else { Ok(()) })
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

/// The page size a full memory file skips zero pages by.
const ZERO_PAGE: usize = 4096;

fn file_error(path: &std::path::Path, source: io::Error) -> SnapshotError {
    SnapshotError::State(format!("{}: {source}", path.display()))
}

// The full memory file of a backend without a dirty-page log or a file-backed guest memory (WHP,
// Hypervisor.framework): every page that is not zero, written from and read into the guest's RAM
// as plain byte slices. Portable; the KVM backend's sparse, diff-capable file is
// `vstate::snapshot_memory`'s.

/// Writes the guest's RAM regions (`(guest address, bytes)`) to [`MEMORY_FILE`] in `dir`, one after
/// another, leaving out pages that are all zero. Returns each region's place in the file and the
/// bytes written.
pub fn write_memory_file(
    dir: &std::path::Path,
    regions: &[(u64, &[u8])],
) -> Result<(Vec<RegionMeta>, u64), SnapshotError> {
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};
    let path = dir.join(MEMORY_FILE);
    let mut file = File::create(&path).map_err(|err| file_error(&path, err))?;
    let mut metas = Vec::with_capacity(regions.len());
    let mut offset = 0u64;
    let mut written = 0u64;
    for (guest_addr, bytes) in regions {
        metas.push(RegionMeta {
            guest_addr: *guest_addr,
            len: bytes.len() as u64,
            offset,
        });
        for (index, page) in bytes.chunks(ZERO_PAGE).enumerate() {
            if page.iter().all(|byte| *byte == 0) {
                continue;
            }
            file.seek(SeekFrom::Start(offset + (index * ZERO_PAGE) as u64))
                .and_then(|_| file.write_all(page))
                .map_err(|err| file_error(&path, err))?;
            written += page.len() as u64;
        }
        offset += bytes.len() as u64;
    }
    file.set_len(offset)
        .and_then(|()| file.sync_all())
        .map_err(|err| file_error(&path, err))?;
    Ok((metas, written))
}

/// Reads [`MEMORY_FILE`] in `dir` back into the guest's RAM regions (`(guest address, bytes)`), which
/// must be the regions the snapshot's meta lists, in its order.
pub fn read_memory_file(
    dir: &std::path::Path,
    metas: &[RegionMeta],
    regions: &mut [(u64, &mut [u8])],
) -> Result<(), SnapshotError> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    if metas.len() != regions.len()
        || metas
            .iter()
            .zip(regions.iter())
            .any(|(meta, (addr, bytes))| meta.guest_addr != *addr || meta.len != bytes.len() as u64)
    {
        return Err(SnapshotError::Mismatch(
            "the guest memory layout is not the snapshot's".to_string(),
        ));
    }
    let path = dir.join(MEMORY_FILE);
    let mut file = File::open(&path).map_err(|err| file_error(&path, err))?;
    for (meta, (_, bytes)) in metas.iter().zip(regions.iter_mut()) {
        file.seek(SeekFrom::Start(meta.offset))
            .and_then(|_| file.read_exact(bytes))
            .map_err(|err| file_error(&path, err))?;
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

/// The union of two sets of runs: sorted, with the runs that overlap or touch merged. Empty runs are
/// dropped.
pub fn merge_runs(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut all: Vec<Run> = a
        .iter()
        .chain(b)
        .copied()
        .filter(|&(_, len)| len > 0)
        .collect();
    all.sort_unstable();
    let mut merged: Vec<Run> = Vec::with_capacity(all.len());
    for (offset, len) in all {
        match merged.last_mut() {
            Some(last) if offset <= last.0.saturating_add(last.1) => {
                last.1 = last
                    .0
                    .saturating_add(last.1)
                    .max(offset.saturating_add(len))
                    - last.0;
            }
            _ => merged.push((offset, len)),
        }
    }
    merged
}

/// The bytes a [`WORKING_SET_FILE`] holds for `runs`: at most [`MAX_WORKING_SET_RUNS`] of them, each
/// as a little-endian `offset` then a little-endian `len`.
pub fn encode_runs(runs: &[Run]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(runs.len().min(MAX_WORKING_SET_RUNS) * 16);
    for &(offset, len) in runs.iter().take(MAX_WORKING_SET_RUNS) {
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&len.to_le_bytes());
    }
    bytes
}

/// The runs a [`WORKING_SET_FILE`]'s bytes hold, or `None` if they are not a whole number of runs, or
/// name more than [`MAX_WORKING_SET_RUNS`].
pub fn decode_runs(bytes: &[u8]) -> Option<Vec<Run>> {
    if !bytes.len().is_multiple_of(16) || bytes.len() / 16 > MAX_WORKING_SET_RUNS {
        return None;
    }
    let mut runs = Vec::with_capacity(bytes.len() / 16);
    for chunk in bytes.as_chunks::<16>().0 {
        let offset = u64::from_le_bytes(chunk[..8].try_into().ok()?);
        let len = u64::from_le_bytes(chunk[8..].try_into().ok()?);
        runs.push((offset, len));
    }
    Some(runs)
}

#[cfg(unix)]
/// The working set `dir` records for a restore. A missing file is an empty set, silently. A file that
/// cannot be read or decoded is reported on stderr and is also an empty set: it is only a hint.
pub fn read_working_set(dir: &Path) -> Vec<Run> {
    use std::io::Read;
    let path = dir.join(WORKING_SET_FILE);
    // One byte past the largest valid file, so an oversized file is seen as oversized, not read whole.
    let limit = (MAX_WORKING_SET_RUNS * 16 + 1) as u64;
    let mut bytes = Vec::new();
    let read = fs::File::open(&path).and_then(|file| file.take(limit).read_to_end(&mut bytes));
    match read {
        Ok(_) => match decode_runs(&bytes) {
            Some(runs) => runs,
            None => {
                eprintln!(
                    "vmm: {} is malformed; restoring without a read-ahead",
                    path.display()
                );
                Vec::new()
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            eprintln!(
                "vmm: cannot read {}: {err}; restoring without a read-ahead",
                path.display()
            );
            Vec::new()
        }
    }
}

/// Writes `runs` as the working-set file of `dir` (mode 0600, created or truncated). Not fsynced: a
/// lost hint only costs the restore its read-ahead.
#[cfg(unix)]
pub fn write_working_set(dir: &Path, runs: &[Run]) -> Result<(), SnapshotError> {
    use std::io::Write;
    let path = dir.join(WORKING_SET_FILE);
    let mut file = private_file(&path)?;
    file.write_all(&encode_runs(runs))
        .map_err(|err| SnapshotError::io(&path, err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_joins_adjacent_and_overlapping_runs_and_keeps_gaps() {
        // Touching runs join; overlapping ones join; a gap stays a gap.
        assert_eq!(
            merge_runs(&[(0, 4096), (4096, 4096)], &[]),
            vec![(0, 8192)],
            "adjacent runs join"
        );
        assert_eq!(
            merge_runs(&[(0, 8192)], &[(4096, 8192)]),
            vec![(0, 12288)],
            "overlapping runs join"
        );
        assert_eq!(
            merge_runs(&[(0, 4096)], &[(8192, 4096)]),
            vec![(0, 4096), (8192, 4096)],
            "a gap stays"
        );
        assert_eq!(
            merge_runs(&[(8192, 4096)], &[(0, 4096)]),
            vec![(0, 4096), (8192, 4096)]
        );
    }

    #[test]
    fn merge_is_a_union_and_drops_empty_runs() {
        let a = [(40960, 4096), (0, 4096)];
        let b = [(4096, 4096), (0, 0), (40960, 8192)];
        assert_eq!(merge_runs(&a, &b), vec![(0, 8192), (40960, 8192)]);
        assert_eq!(merge_runs(&[], &[]), Vec::<Run>::new());
        assert_eq!(merge_runs(&[(16, 0)], &[]), Vec::<Run>::new());
    }

    #[test]
    fn runs_round_trip_through_their_encoding() {
        let runs = vec![(0, 4096), (u64::MAX - 4095, 4096), (1 << 40, 1 << 30)];
        let bytes = encode_runs(&runs);
        assert_eq!(bytes.len(), runs.len() * 16);
        assert_eq!(
            &bytes[..8],
            &0u64.to_le_bytes(),
            "little-endian offset first"
        );
        assert_eq!(decode_runs(&bytes), Some(runs));
        assert_eq!(decode_runs(&[]), Some(Vec::new()));
    }

    #[test]
    fn a_malformed_encoding_decodes_to_nothing() {
        assert_eq!(decode_runs(&[0u8; 15]), None, "a partial run");
        assert_eq!(decode_runs(&[0u8; 17]), None, "a stray byte");
        let too_many = vec![0u8; (MAX_WORKING_SET_RUNS + 1) * 16];
        assert_eq!(decode_runs(&too_many), None, "more runs than the cap");
    }

    #[test]
    fn encoding_keeps_only_the_first_runs_up_to_the_cap() {
        let runs: Vec<Run> = (0..(MAX_WORKING_SET_RUNS as u64 + 3))
            .map(|i| (i * 8192, 4096))
            .collect();
        let decoded = decode_runs(&encode_runs(&runs)).expect("a capped file decodes");
        assert_eq!(decoded.len(), MAX_WORKING_SET_RUNS);
        assert_eq!(decoded.last(), Some(&runs[MAX_WORKING_SET_RUNS - 1]));
    }

    #[cfg(unix)]
    #[test]
    fn the_working_set_file_is_read_back_and_a_missing_one_is_empty() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("apz-workingset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            read_working_set(&dir),
            Vec::<Run>::new(),
            "no file, no runs"
        );

        write_working_set(&dir, &[(0, 4096), (8192, 8192)]).unwrap();
        let mode = fs::metadata(dir.join(WORKING_SET_FILE))
            .unwrap()
            .permissions();
        assert_eq!(mode.mode() & 0o777, 0o600);
        assert_eq!(read_working_set(&dir), vec![(0, 4096), (8192, 8192)]);

        // A malformed file is an empty set, not an error.
        fs::write(dir.join(WORKING_SET_FILE), [1u8, 2, 3]).unwrap();
        assert_eq!(read_working_set(&dir), Vec::<Run>::new());
        fs::remove_dir_all(&dir).unwrap();
    }
}
