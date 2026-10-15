//! What a snapshot of a WHP VM holds, and its memory file. Pure: no FFI, so it builds and its tests
//! run on every host; the Windows half (`whp::snapshot`) reads and writes the hypervisor's state.
//!
//! The directory has the KVM backend's two files ([`STATE_FILE`] and [`MEMORY_FILE`]), but the state
//! is WHP's own: each vCPU's registers by `WHV_REGISTER_NAME`, its local APIC
//! (`WHvVirtualProcessorStateTypeInterruptControllerState2`) and XSAVE area as the hypervisor
//! hands them out, the userspace I/O APIC, the UART and the vsock transport. The meta's `backend`
//! is `whp`, so neither backend restores the other's snapshot.
//!
//! The memory file holds the guest's RAM regions one after another, at the offsets the meta lists.
//! Pages that are all zero are not written (the file has holes there, or zeros), and a restore reads
//! the file back into freshly allocated memory.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use crate::vstate::snapshot::{
    read_memory_file as read_memory, write_memory_file as write_memory,
};

use super::ioapic::IoApic;
use crate::devices::virtio::mmio::MmioDeviceState;
use crate::vstate::snapshot::{
    STATE_FILE, SerialRegisters, SnapshotError, SnapshotMeta, check_compatible,
};

/// One vCPU's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhpVcpuState {
    /// `(WHV_REGISTER_NAME, the 128-bit WHV_REGISTER_VALUE as two little-endian halves)`, in the
    /// order they are written back.
    pub registers: Vec<(i32, [u64; 2])>,
    /// The local APIC's state (`InterruptControllerState2`), as WHP returns it.
    pub lapic: Vec<u8>,
    /// The XSAVE area (`XsaveState`), as WHP returns it.
    pub xsave: Vec<u8>,
}

/// Everything [`STATE_FILE`] holds for a WHP VM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhpSnapshot {
    pub meta: SnapshotMeta,
    pub vcpus: Vec<WhpVcpuState>,
    pub ioapic: IoApic,
    pub serial: SerialRegisters,
    pub vsock: Option<MmioDeviceState>,
    /// The workspace volume's transport.
    #[serde(default)]
    pub workspace: Option<MmioDeviceState>,
    /// The shared data volumes' transports, in the spec's order.
    #[serde(default)]
    pub volumes: Vec<MmioDeviceState>,
    /// The data volumes' names, in slot order, when the snapshot was taken. A restore refuses a spec
    /// whose volumes differ in name or order. Empty for a snapshot from before they were recorded,
    /// which a restore checks by count alone.
    #[serde(default)]
    pub volume_names: Vec<String>,
}

fn io_error(path: &Path, source: io::Error) -> SnapshotError {
    SnapshotError::State(format!("{}: {source}", path.display()))
}

/// Writes [`STATE_FILE`] into `dir` (created if missing): to a temporary name, then renamed.
pub fn write_state(dir: &Path, snapshot: &WhpSnapshot) -> Result<(), SnapshotError> {
    fs::create_dir_all(dir).map_err(|err| io_error(dir, err))?;
    let path = dir.join(STATE_FILE);
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    let bytes = serde_json::to_vec(snapshot).map_err(|source| SnapshotError::Malformed {
        path: path.display().to_string(),
        source,
    })?;
    let mut file = File::create(&tmp).map_err(|err| io_error(&tmp, err))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error(&tmp, err))?;
    drop(file);
    fs::rename(&tmp, &path).map_err(|err| io_error(&path, err))
}

/// Reads [`STATE_FILE`] from `dir`, refusing a snapshot this VMM cannot restore (another format,
/// version, architecture or backend).
pub fn read_state(dir: &Path) -> Result<WhpSnapshot, SnapshotError> {
    let path = dir.join(STATE_FILE);
    let bytes = fs::read(&path).map_err(|err| io_error(&path, err))?;
    let snapshot: WhpSnapshot =
        serde_json::from_slice(&bytes).map_err(|source| SnapshotError::Malformed {
            path: path.display().to_string(),
            source,
        })?;
    check_compatible(&snapshot.meta)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawner::SnapshotKind;
    use crate::vstate::snapshot::MEMORY_FILE;

    const PAGE: usize = 4096;
    use crate::vstate::snapshot::meta_now;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-whp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn memory_round_trips_and_zero_pages_are_not_written() {
        let dir = scratch("memory");
        let mut low = vec![0u8; 4 * PAGE];
        low[PAGE + 7] = 0xaa;
        let mut high = vec![0u8; 2 * PAGE];
        high[2 * PAGE - 1] = 0x55;
        let (metas, written) = write_memory(&dir, &[(0, &low), (1 << 32, &high)]).expect("written");
        assert_eq!(written, 2 * PAGE as u64, "only the two pages with data");
        assert_eq!(metas[1].offset, low.len() as u64);
        assert_eq!(
            fs::metadata(dir.join(MEMORY_FILE)).unwrap().len(),
            (low.len() + high.len()) as u64
        );
        let mut low_back = vec![0xffu8; low.len()];
        let mut high_back = vec![0xffu8; high.len()];
        read_memory(
            &dir,
            &metas,
            &mut [(0, &mut low_back), (1 << 32, &mut high_back)],
        )
        .expect("read");
        assert_eq!(low_back, low);
        assert_eq!(high_back, high);
        let mut wrong = vec![0u8; PAGE];
        assert!(matches!(
            read_memory(&dir, &metas, &mut [(0, &mut wrong)]),
            Err(SnapshotError::Mismatch(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_state_round_trips_and_another_backend_is_refused() {
        let dir = scratch("state");
        let mut meta = meta_now(SnapshotKind::Full, 2, 256, Vec::new());
        meta.backend = crate::vstate::snapshot::BACKEND.to_string();
        let snapshot = WhpSnapshot {
            meta,
            vcpus: vec![WhpVcpuState {
                registers: vec![(0, [1, 2]), (0x2001, [u64::MAX, 0])],
                lapic: vec![1, 2, 3],
                xsave: vec![4; 16],
            }],
            ioapic: IoApic::new(3),
            serial: SerialRegisters::default(),
            vsock: None,
            workspace: None,
            volumes: Vec::new(),
            volume_names: vec!["alpha".to_string(), "beta".to_string()],
        };
        write_state(&dir, &snapshot).expect("written");
        assert_eq!(read_state(&dir).expect("read back"), snapshot);
        let mut other = snapshot.clone();
        other.meta.backend = "somewhere-else".to_string();
        write_state(&dir, &other).expect("written");
        assert!(matches!(
            read_state(&dir),
            Err(SnapshotError::Incompatible(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
