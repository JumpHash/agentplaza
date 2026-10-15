//! What a snapshot of a Hypervisor.framework VM holds. Pure: no FFI, so it builds and its tests run
//! on every host; the Apple Silicon half (`hvf::vcpu`, `hvf::snapshot`) reads and writes the
//! framework's state.
//!
//! The directory has the KVM backend's two files: [`STATE_FILE`] (this module's [`HvfSnapshot`],
//! with the meta's `backend` set to `hvf`, so no other backend restores it) and the full memory file
//! (`vstate::snapshot::write_memory_file`). Each vCPU keeps its general registers, its SIMD/FP
//! registers and the EL0/EL1 system registers the guest owns (the identification registers and
//! `MPIDR_EL1` are set by the VMM on every start and are not kept); the VM keeps the userspace GIC,
//! the UART, which vCPUs were powered on (PSCI), the guest's virtual counter, so a restored
//! guest's counter goes on from where it stopped, and the virtio-mmio transports (vsock, workspace,
//! data volumes) as the KVM and WHP backends keep them.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::gic::GicState;
use crate::devices::virtio::mmio::MmioDeviceState;
use crate::vstate::snapshot::{
    STATE_FILE, SerialRegisters, SnapshotError, SnapshotMeta, check_compatible,
};

/// One vCPU's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HvfVcpuState {
    /// `(hv_reg_t, value)`: X0..X30, PC, FPCR, FPSR and CPSR.
    pub regs: Vec<(u32, u64)>,
    /// Q0..Q31, as their 16 bytes.
    pub simd: Vec<[u8; 16]>,
    /// `(hv_sys_reg_t, value)`, written back in this order.
    pub sys_regs: Vec<(u16, u64)>,
    /// Whether the framework had the virtual timer masked (it fired and the guest had not yet
    /// cleared it).
    pub vtimer_masked: bool,
}

/// Everything [`STATE_FILE`] holds for a Hypervisor.framework VM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HvfSnapshot {
    pub meta: SnapshotMeta,
    pub vcpus: Vec<HvfVcpuState>,
    /// Which vCPUs were powered on (a secondary that never got its `CPU_ON` was not).
    pub running: Vec<bool>,
    /// The guest's `CNTVCT_EL0` when the vCPUs stopped.
    pub guest_counter: u64,
    pub gic: GicState,
    pub serial: SerialRegisters,
    /// The vsock device's transport.
    #[serde(default)]
    pub vsock: Option<MmioDeviceState>,
    /// The workspace volume's transport.
    #[serde(default)]
    pub workspace: Option<MmioDeviceState>,
    /// The shared data volumes' transports, in the spec's order.
    #[serde(default)]
    pub volumes: Vec<MmioDeviceState>,
}

fn io_error(path: &Path, source: io::Error) -> SnapshotError {
    SnapshotError::State(format!("{}: {source}", path.display()))
}

/// Writes [`STATE_FILE`] into `dir` (created if missing): to a temporary name, then renamed.
pub fn write_state(dir: &Path, snapshot: &HvfSnapshot) -> Result<(), SnapshotError> {
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

/// Reads [`STATE_FILE`] from `dir`, refusing a snapshot this VMM cannot restore.
pub fn read_state(dir: &Path) -> Result<HvfSnapshot, SnapshotError> {
    let path = dir.join(STATE_FILE);
    let bytes = fs::read(&path).map_err(|err| io_error(&path, err))?;
    let snapshot: HvfSnapshot =
        serde_json::from_slice(&bytes).map_err(|source| SnapshotError::Malformed {
            path: path.display().to_string(),
            source,
        })?;
    check_compatible(&snapshot.meta)?;
    Ok(snapshot)
}

/// The virtual-timer offset that makes the guest's counter read `guest_counter` when the host's reads
/// `host_now` (the guest's counter is the host's minus the offset).
pub fn vtimer_offset_for(host_now: u64, guest_counter: u64) -> u64 {
    host_now.saturating_sub(guest_counter)
}

/// The SPIs to raise again after a restore: those of the transports whose interrupt the guest had
/// not acknowledged when the snapshot was taken (`InterruptStatus` non-zero). Each item is a
/// transport's saved state (`None`: no such device) and its SPI.
pub fn unacknowledged_spis<'a>(
    transports: impl IntoIterator<Item = (Option<&'a MmioDeviceState>, u32)>,
) -> Vec<u32> {
    transports
        .into_iter()
        .filter(|(state, _)| state.is_some_and(|state| state.transport.interrupt_status != 0))
        .map(|(_, spi)| spi)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::mmio::{
        DeviceSnapshot, MmioTransportState, QueueConfigState, QueuePosition,
    };
    use crate::hvf::gic::Gic;
    use crate::spawner::SnapshotKind;
    use crate::vstate::snapshot::{BACKEND, meta_now};

    #[test]
    fn the_state_round_trips() {
        let dir = std::env::temp_dir().join(format!("apz-hvf-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut meta = meta_now(SnapshotKind::Full, 2, 512, Vec::new());
        meta.backend = BACKEND.to_string();
        let snapshot = HvfSnapshot {
            meta,
            vcpus: vec![HvfVcpuState {
                regs: vec![(0, 7), (31, 0xffff_0000_0008_0000)],
                simd: vec![[0xab; 16]; 32],
                sys_regs: vec![(0xc080, 0x30d0_5885)],
                vtimer_masked: true,
            }],
            running: vec![true, false],
            guest_counter: 123_456_789,
            gic: Gic::new(2).state(),
            serial: SerialRegisters::default(),
            vsock: None,
            workspace: None,
            volumes: Vec::new(),
        };
        write_state(&dir, &snapshot).expect("written");
        assert_eq!(read_state(&dir).expect("read back"), snapshot);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_counter_goes_on_from_the_snapshot() {
        // The host's counter has moved on by a million ticks since the guest's was 5000.
        let offset = vtimer_offset_for(1_000_000, 5_000);
        assert_eq!(1_000_000 - offset, 5_000);
        assert_eq!(vtimer_offset_for(10, 5_000), 0, "never a negative offset");
    }

    /// A block-like transport with one queue, its interrupt status as given.
    fn state(interrupt_status: u32) -> MmioDeviceState {
        MmioDeviceState {
            transport: MmioTransportState {
                device_type: 2,
                features_select: 0,
                acked_features_select: 0,
                queue_select: 0,
                device_status: 0xf,
                config_generation: 0,
                interrupt_status,
                acked_features: 1 << 32,
                queues: vec![QueueConfigState {
                    max_size: 256,
                    size: 256,
                    ready: true,
                    desc: 0x8000_0000,
                    avail: 0x8000_1000,
                    used: 0x8000_2000,
                }],
            },
            device: DeviceSnapshot {
                queues: vec![QueuePosition {
                    next_avail: 3,
                    next_used: 3,
                }],
                extra: serde_json::json!({ "capacity_sectors": 2048 }),
            },
        }
    }

    fn snapshot_with_devices() -> HvfSnapshot {
        let mut meta = meta_now(SnapshotKind::Full, 2, 512, Vec::new());
        meta.backend = BACKEND.to_string();
        HvfSnapshot {
            meta,
            vcpus: vec![HvfVcpuState {
                regs: vec![(0, 7)],
                simd: vec![[0xab; 16]; 32],
                sys_regs: vec![(0xc080, 0x30d0_5885)],
                vtimer_masked: false,
            }],
            running: vec![true, false],
            guest_counter: 123_456_789,
            gic: Gic::new(2).state(),
            serial: SerialRegisters::default(),
            vsock: Some(state(0)),
            workspace: Some(state(1)),
            volumes: vec![state(0), state(2)],
        }
    }

    #[test]
    fn the_state_round_trips_with_devices() {
        let dir = std::env::temp_dir().join(format!("apz-hvf-devices-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let snapshot = snapshot_with_devices();
        write_state(&dir, &snapshot).expect("written");
        assert_eq!(read_state(&dir).expect("read back"), snapshot);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_state_without_devices_still_reads() {
        let dir = std::env::temp_dir().join(format!("apz-hvf-old-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("created");
        let mut value = serde_json::to_value(snapshot_with_devices()).expect("serialized");
        let object = value.as_object_mut().expect("an object");
        for key in ["vsock", "workspace", "volumes"] {
            object.remove(key);
        }
        let bytes = serde_json::to_vec(&value).expect("bytes");
        fs::write(dir.join(STATE_FILE), bytes).expect("written");
        let read = read_state(&dir).expect("an older state reads");
        assert_eq!(read.vsock, None);
        assert_eq!(read.workspace, None);
        assert!(read.volumes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_unacknowledged_transports_are_raised_again() {
        assert_eq!(
            unacknowledged_spis([
                (Some(&state(1)), 1),
                (Some(&state(0)), 2),
                (None, 3),
                (Some(&state(2)), 8),
            ]),
            vec![1, 8]
        );
    }
}
