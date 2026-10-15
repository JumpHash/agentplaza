// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/x86_64/vm.rs (trimmed: the
// `msrs_to_save` list and the `xsave2` sizing are dropped, they belong to the
// vCPU state and the vCPU snapshot). The VM snapshot state is the PIT, the KVM
// clock and the in-kernel irqchip (both PICs and the IOAPIC); guest memory and
// the resource allocator are not part of it here.

use std::fmt;
use std::sync::Arc;

use kvm_bindings::{
    KVM_IRQCHIP_IOAPIC, KVM_IRQCHIP_PIC_MASTER, KVM_IRQCHIP_PIC_SLAVE, KVM_PIT_SPEAKER_DUMMY,
    kvm_clock_data, kvm_irqchip, kvm_pit_config, kvm_pit_state2,
};
use zerocopy::IntoBytes;

use crate::arch::x86_64::layout::KVM_TSS_ADDRESS;
use crate::vstate::bus::Bus;
use crate::vstate::kvm::Kvm;
use crate::vstate::memory::u64_to_usize;
use crate::vstate::vm::{VmCommon, VmError};

/// Error type for the x86_64-specific [`KvmVm`] setup and snapshot state.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum KvmVmError {
    /// Failed during KVM_SET_TSS_ADDRESS: {0}
    #[error("Failed during KVM_SET_TSS_ADDRESS: {0}")]
    SetTssAddress(kvm_ioctls::Error),
    /// Failed to create the irq chip: {0}
    #[error("Failed to create the irq chip: {0}")]
    VmSetIrqChip(kvm_ioctls::Error),
    /// Failed to get KVM vm pit state: {0}
    #[error("Failed to get KVM vm pit state: {0}")]
    VmGetPit2(kvm_ioctls::Error),
    /// Failed to get KVM vm clock: {0}
    #[error("Failed to get KVM vm clock: {0}")]
    VmGetClock(kvm_ioctls::Error),
    /// Failed to get KVM vm irqchip: {0}
    #[error("Failed to get KVM vm irqchip: {0}")]
    VmGetIrqChip(kvm_ioctls::Error),
    /// Set PIT2 error: {0}
    #[error("Set PIT2 error: {0}")]
    SetPit2(kvm_ioctls::Error),
    /// Set clock error: {0}
    #[error("Set clock error: {0}")]
    SetClock(kvm_ioctls::Error),
    /// Set IrqChipPicMaster error: {0}
    #[error("Set IrqChipPicMaster error: {0}")]
    SetIrqChipPicMaster(kvm_ioctls::Error),
    /// Set IrqChipPicSlave error: {0}
    #[error("Set IrqChipPicSlave error: {0}")]
    SetIrqChipPicSlave(kvm_ioctls::Error),
    /// Set IrqChipIoAPIC error: {0}
    #[error("Set IrqChipIoAPIC error: {0}")]
    SetIrqChipIoAPIC(kvm_ioctls::Error),
}

/// Structure representing the current architecture's understand of what a "virtual machine" is.
#[derive(Debug)]
pub struct KvmVm {
    /// Architecture independent parts of a vm
    pub common: VmCommon,
    /// Port IO bus
    pub pio_bus: Arc<Bus>,
}

impl KvmVm {
    /// Create a new `KvmVm` struct.
    pub fn new(kvm: Kvm) -> Result<KvmVm, VmError> {
        let common = Self::create_common(kvm)?;

        // `KVM_CAP_XSAVE2` and the snapshot MSR list are vCPU snapshot concerns
        // and are not part of this VM state (trimmed).

        common
            .fd
            .set_tss_address(u64_to_usize(KVM_TSS_ADDRESS))
            .map_err(KvmVmError::SetTssAddress)?;

        let pio_bus = Arc::new(Bus::new());

        Ok(KvmVm { common, pio_bus })
    }

    /// Pre-vCPU creation setup.
    pub fn arch_pre_create_vcpus(&mut self, _: u8) -> Result<(), KvmVmError> {
        // For x86_64 we need to create the interrupt controller before calling `KVM_CREATE_VCPUS`
        self.setup_irqchip()
    }

    /// Post-vCPU creation setup.
    pub fn arch_post_create_vcpus(&mut self, _: u8) -> Result<(), KvmVmError> {
        Ok(())
    }

    /// Creates the irq chip and an in-kernel device model for the PIT.
    pub fn setup_irqchip(&self) -> Result<(), KvmVmError> {
        self.fd()
            .create_irq_chip()
            .map_err(KvmVmError::VmSetIrqChip)?;
        // We need to enable the emulation of a dummy speaker port stub so that writing to port 0x61
        // (i.e. KVM_SPEAKER_BASE_ADDRESS) does not trigger an exit to user space.
        let pit_config = kvm_pit_config {
            flags: KVM_PIT_SPEAKER_DUMMY,
            ..Default::default()
        };
        self.fd()
            .create_pit2(pit_config)
            .map_err(KvmVmError::VmSetIrqChip)
    }
}

/// The state of an x86_64 VM in a snapshot (Firecracker's `VmState`): the PIT, the KVM clock and the
/// in-kernel irqchip (both PICs and the IOAPIC).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct VmState {
    pitstate: kvm_pit_state2,
    clock: kvm_clock_data,
    // TODO: rename this field to adopt inclusive language once Linux updates it, too.
    pic_master: kvm_irqchip,
    // TODO: rename this field to adopt inclusive language once Linux updates it, too.
    pic_slave: kvm_irqchip,
    ioapic: kvm_irqchip,
}

// `kvm_irqchip` is a union, so the irqchips compare and print as their bytes and
// the rest of the state is compared by its own derives.
impl PartialEq for VmState {
    fn eq(&self, other: &Self) -> bool {
        self.pitstate == other.pitstate
            && self.clock == other.clock
            && self.pic_master.as_bytes() == other.pic_master.as_bytes()
            && self.pic_slave.as_bytes() == other.pic_slave.as_bytes()
            && self.ioapic.as_bytes() == other.ioapic.as_bytes()
    }
}

impl fmt::Debug for VmState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VmState")
            .field("pitstate", &self.pitstate)
            .field("clock", &self.clock)
            .field("pic_master", &"?")
            .field("pic_slave", &"?")
            .field("ioapic", &"?")
            .finish()
    }
}

impl KvmVm {
    /// Saves the VM's state. Its vCPUs must be paused.
    pub fn save_state(&self) -> Result<VmState, KvmVmError> {
        let pitstate = self.fd().get_pit2().map_err(KvmVmError::VmGetPit2)?;
        let clock = self.fd().get_clock().map_err(KvmVmError::VmGetClock)?;
        let pic_master = self.get_irqchip(KVM_IRQCHIP_PIC_MASTER)?;
        let pic_slave = self.get_irqchip(KVM_IRQCHIP_PIC_SLAVE)?;
        let ioapic = self.get_irqchip(KVM_IRQCHIP_IOAPIC)?;

        Ok(VmState {
            pitstate,
            clock,
            pic_master,
            pic_slave,
            ioapic,
        })
    }

    /// Restores a state [`KvmVm::save_state`] produced, after the irqchip exists and before any
    /// vCPU runs.
    pub fn restore_state(&mut self, state: &VmState) -> Result<(), KvmVmError> {
        self.fd()
            .set_pit2(&state.pitstate)
            .map_err(KvmVmError::SetPit2)?;

        // `KVM_GET_CLOCK` reports `KVM_CLOCK_TSC_STABLE`, which `KVM_SET_CLOCK` does not take back,
        // so the flags are cleared (Firecracker's non-realtime path). The guest clock then resumes
        // from the saved `clock` value, and the realtime flag is not restored.
        let mut clock = state.clock;
        clock.flags = 0;
        self.fd().set_clock(&clock).map_err(KvmVmError::SetClock)?;

        self.fd()
            .set_irqchip(&state.pic_master)
            .map_err(KvmVmError::SetIrqChipPicMaster)?;
        self.fd()
            .set_irqchip(&state.pic_slave)
            .map_err(KvmVmError::SetIrqChipPicSlave)?;
        self.fd()
            .set_irqchip(&state.ioapic)
            .map_err(KvmVmError::SetIrqChipIoAPIC)?;
        Ok(())
    }

    /// Reads one irqchip (`KVM_IRQCHIP_PIC_MASTER`, `KVM_IRQCHIP_PIC_SLAVE` or `KVM_IRQCHIP_IOAPIC`).
    fn get_irqchip(&self, chip_id: u32) -> Result<kvm_irqchip, KvmVmError> {
        let mut chip = kvm_irqchip {
            chip_id,
            ..Default::default()
        };
        self.fd()
            .get_irqchip(&mut chip)
            .map_err(KvmVmError::VmGetIrqChip)?;
        Ok(chip)
    }
}

#[cfg(test)]
mod tests {
    use kvm_bindings::KVM_NR_IRQCHIPS;

    use super::*;

    /// A fresh VM with its irqchip and PIT created, as `arch_pre_create_vcpus` does.
    fn vm_with_irqchip() -> KvmVm {
        let vm = KvmVm::new(Kvm::new().unwrap()).unwrap();
        vm.setup_irqchip().unwrap();
        vm
    }

    #[test]
    fn test_vm_save_restore_state() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_with_irqchip();
        let state = vm.save_state().unwrap();
        assert_eq!(
            state.pitstate.flags | KVM_PIT_SPEAKER_DUMMY,
            KVM_PIT_SPEAKER_DUMMY
        );
        assert_eq!(state.pic_master.chip_id, KVM_IRQCHIP_PIC_MASTER);
        assert_eq!(state.pic_slave.chip_id, KVM_IRQCHIP_PIC_SLAVE);
        assert_eq!(state.ioapic.chip_id, KVM_IRQCHIP_IOAPIC);

        let mut restored = vm_with_irqchip();
        restored.restore_state(&state).unwrap();
        let resaved = restored.save_state().unwrap();

        // `count_load_time` is a host monotonic timestamp that KVM re-bases on restore, so it
        // cannot match across VMs; the channel programming is compared without it.
        assert_eq!(
            without_load_time(resaved.pitstate),
            without_load_time(state.pitstate)
        );
        assert_eq!(resaved.pic_master.as_bytes(), state.pic_master.as_bytes());
        assert_eq!(resaved.pic_slave.as_bytes(), state.pic_slave.as_bytes());
        assert_eq!(resaved.ioapic.as_bytes(), state.ioapic.as_bytes());
        // The clock value advances between the two VMs; only the flags are compared.
        assert_eq!(resaved.clock.flags, 0);
    }

    fn without_load_time(mut pit: kvm_pit_state2) -> kvm_pit_state2 {
        for channel in &mut pit.channels {
            channel.count_load_time = 0;
        }
        pit
    }

    #[test]
    fn test_vm_restore_state_bad_irqchip() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_with_irqchip();
        let mut state = vm.save_state().unwrap();

        let mut vm = vm_with_irqchip();

        let orig_master_chip_id = state.pic_master.chip_id;
        state.pic_master.chip_id = KVM_NR_IRQCHIPS;
        assert!(matches!(
            vm.restore_state(&state),
            Err(KvmVmError::SetIrqChipPicMaster(_))
        ));
        state.pic_master.chip_id = orig_master_chip_id;

        let orig_slave_chip_id = state.pic_slave.chip_id;
        state.pic_slave.chip_id = KVM_NR_IRQCHIPS;
        assert!(matches!(
            vm.restore_state(&state),
            Err(KvmVmError::SetIrqChipPicSlave(_))
        ));
        state.pic_slave.chip_id = orig_slave_chip_id;

        state.ioapic.chip_id = KVM_NR_IRQCHIPS;
        assert!(matches!(
            vm.restore_state(&state),
            Err(KvmVmError::SetIrqChipIoAPIC(_))
        ));
    }

    #[test]
    fn test_vmstate_serde() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let vm = vm_with_irqchip();
        let state = vm.save_state().unwrap();

        let json = serde_json::to_string(&state).unwrap();
        let back: VmState = serde_json::from_str(&json).unwrap();
        assert!(back == state);

        let mut restored = vm_with_irqchip();
        restored.restore_state(&back).unwrap();
    }
}
