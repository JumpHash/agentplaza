// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/x86_64/vm.rs (trimmed: snapshot
// save/restore state, the `msrs_to_save` list and the `xsave2` sizing are
// dropped; the snapshot-oriented error variants are removed).

use std::sync::Arc;

use kvm_bindings::{KVM_PIT_SPEAKER_DUMMY, kvm_pit_config};

use crate::arch::x86_64::layout::KVM_TSS_ADDRESS;
use crate::vstate::bus::Bus;
use crate::vstate::kvm::Kvm;
use crate::vstate::memory::u64_to_usize;
use crate::vstate::vm::{VmCommon, VmError};

/// Error type for the x86_64-specific [`KvmVm`] setup.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum KvmVmError {
    /// Failed during KVM_SET_TSS_ADDRESS: {0}
    #[error("Failed during KVM_SET_TSS_ADDRESS: {0}")]
    SetTssAddress(kvm_ioctls::Error),
    /// Failed to create the irq chip: {0}
    #[error("Failed to create the irq chip: {0}")]
    VmSetIrqChip(kvm_ioctls::Error),
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

        // `KVM_CAP_XSAVE2` and the snapshot MSR list are only needed for
        // snapshotting, which this port does not implement (trimmed).

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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VmState {}

impl KvmVm {
    /// Saves the VM's state. Its vCPUs must be paused.
    pub fn save_state(&self) -> Result<VmState, KvmVmError> {
        todo!("x86_64 VM save")
    }

    /// Restores a state [`KvmVm::save_state`] produced, after the irqchip exists and before any
    /// vCPU runs.
    pub fn restore_state(&mut self, state: &VmState) -> Result<(), KvmVmError> {
        let _ = state;
        todo!("x86_64 VM restore")
    }
}
